use std::{
    fs::{self, File},
    os::fd::AsRawFd,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use rusqlite::{functions::FunctionFlags, limits::Limit, Connection, OpenFlags};

use super::{
    error::{DatabaseError, Result},
    legacy_research, media, search,
};

pub const MAIN_APPLICATION_ID: i32 = i32::from_be_bytes(*b"KOSH");
pub const MEDIA_APPLICATION_ID: i32 = i32::from_be_bytes(*b"KMED");
pub const MAX_MEDIA_BLOB_BYTES: i64 = 256 * 1024 * 1024;
const MEDIA_CONNECTION_LENGTH_LIMIT: i32 = MAX_MEDIA_BLOB_BYTES as i32 + 64 * 1024;
static SQLITE_VEC_REGISTRATION: OnceLock<std::result::Result<(), i32>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseKind {
    Main,
    Media,
}

impl DatabaseKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Media => "media",
        }
    }

    pub const fn application_id(self) -> i32 {
        match self {
            Self::Main => MAIN_APPLICATION_ID,
            Self::Media => MEDIA_APPLICATION_ID,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileState {
    Fresh,
    Existing,
}

impl FileState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Existing => "existing",
        }
    }
}

pub fn register_sqlite_vec() -> Result<()> {
    let result = SQLITE_VEC_REGISTRATION.get_or_init(|| {
        type ExtensionEntry = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::ffi::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::ffi::c_int;

        // sqlite-vec exposes SQLite's C extension entry point. SQLite's
        // auto-extension API requires the erased callback signature.
        let code = unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                ExtensionEntry,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )))
        };
        if code == rusqlite::ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(code)
        }
    });
    result.map_err(DatabaseError::VecRegistration)
}

pub fn inspect_file(path: &Path) -> Result<FileState> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(DatabaseError::InvalidInput(
            format!("database path is not a regular file: {}", path.display()),
        )),
        Ok(metadata) if metadata.len() > 0 => Ok(FileState::Existing),
        Ok(_) => Ok(FileState::Fresh),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileState::Fresh),
        Err(error) => Err(error.into()),
    }
}

pub fn open_writer(path: &Path, kind: DatabaseKind, state: FileState) -> Result<Connection> {
    if let Err(error) = register_sqlite_vec() {
        log::warn!("sqlite-vec is unavailable; semantic search is disabled: {error}");
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(path, flags)?;
    if state == FileState::Fresh {
        // The application ID must be the first persistent write. That leaves
        // an interrupted first launch recognizable and safely resumable.
        connection.pragma_update(None, "application_id", kind.application_id())?;
    } else {
        verify_application_id(&connection, path, kind)?;
    }
    configure_writer(&connection, kind, "WAL")?;
    verify_application_id(&connection, path, kind)?;
    Ok(connection)
}

pub fn open_bound_writer(file: &File, kind: DatabaseKind, state: FileState) -> Result<Connection> {
    if let Err(error) = register_sqlite_vec() {
        log::warn!("sqlite-vec is unavailable; semantic search is disabled: {error}");
    }
    normalize_bound_journal_header(file, kind, state)?;
    let path = bound_descriptor_path(file, kind)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(&path, flags)?;
    set_journal_mode(&connection, "MEMORY")?;
    if state == FileState::Fresh {
        connection.pragma_update(None, "application_id", kind.application_id())?;
    } else {
        verify_application_id(&connection, &path, kind)?;
    }
    // A descriptor path has no stable sibling pathname for WAL or rollback
    // files. MEMORY journaling keeps every write attached to the already-open
    // database inode while synchronous=FULL and the caller's final fsync keep
    // the staged database durable before publication.
    configure_writer(&connection, kind, "MEMORY")?;
    verify_application_id(&connection, &path, kind)?;
    Ok(connection)
}

pub fn open_read_only(path: &Path, kind: DatabaseKind) -> Result<Connection> {
    if let Err(error) = register_sqlite_vec() {
        log::warn!("sqlite-vec is unavailable on read connection: {error}");
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(path, flags)?;
    configure_length_limit(&connection, kind)?;
    connection.busy_timeout(Duration::from_millis(5_000))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "query_only", "ON")?;
    verify_application_id(&connection, path, kind)?;
    Ok(connection)
}

pub fn open_bound_read_only(file: &File, kind: DatabaseKind) -> Result<Connection> {
    if let Err(error) = register_sqlite_vec() {
        log::warn!("sqlite-vec is unavailable on read connection: {error}");
    }
    let path = bound_descriptor_path(file, kind)?;
    let uri = format!("file:{}?immutable=1", path.to_string_lossy());
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let connection = Connection::open_with_flags(&uri, flags)?;
    configure_length_limit(&connection, kind)?;
    connection.busy_timeout(Duration::from_millis(5_000))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "query_only", "ON")?;
    verify_application_id(&connection, &path, kind)?;
    Ok(connection)
}

pub fn is_pristine_identified(path: &Path, kind: DatabaseKind) -> Result<bool> {
    let connection = open_read_only(path, kind)?;
    let owned_schema_objects: i64 = connection.query_row(
        "SELECT count(*)
         FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(owned_schema_objects == 0)
}

pub fn verify_application_id(
    connection: &Connection,
    path: &Path,
    kind: DatabaseKind,
) -> Result<()> {
    let actual: i32 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let expected = kind.application_id();
    if actual != expected {
        return Err(DatabaseError::WrongApplicationId {
            kind: kind.label(),
            path: path.to_owned(),
            expected,
            actual,
        });
    }
    Ok(())
}

fn configure_writer(
    connection: &Connection,
    kind: DatabaseKind,
    journal_mode: &'static str,
) -> Result<()> {
    configure_length_limit(connection, kind)?;
    if kind == DatabaseKind::Main {
        connection.create_scalar_function(
            "kosh_search_normalize",
            1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                let value = context.get::<String>(0)?;
                Ok(search::normalize_for_search(&value))
            },
        )?;
        connection.create_scalar_function(
            "kosh_search_short_grams",
            1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                let value = context.get::<String>(0)?;
                Ok(search::short_grams_for_search(&value))
            },
        )?;
        connection.create_scalar_function(
            "kosh_markdown_references_attachment",
            2,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                let markdown = context.get::<String>(0)?;
                let attachment_id = context.get::<String>(1)?;
                Ok(media::markdown_references_attachment(
                    &markdown,
                    &attachment_id,
                ))
            },
        )?;
        connection.create_scalar_function(
            "kosh_research_citation_mentions",
            2,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                let markdown = context.get::<String>(0)?;
                let citation_count = context.get::<i64>(1)?;
                let citation_count = usize::try_from(citation_count).map_err(|_| {
                    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "citation count is outside the supported range",
                    )))
                })?;
                let mentions = legacy_research::citation_mentions(&markdown, citation_count);
                serde_json::to_string(&mentions)
                    .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
            },
        )?;
    }
    connection.busy_timeout(Duration::from_millis(5_000))?;
    set_journal_mode(connection, journal_mode)?;

    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "cache_size", -20_000_i64)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;

    verify_pragma(connection, "synchronous", 2)?;
    verify_pragma(connection, "foreign_keys", 1)?;
    verify_pragma(connection, "trusted_schema", 0)?;
    verify_pragma(connection, "cache_size", -20_000)?;
    verify_pragma(connection, "temp_store", 2)?;
    Ok(())
}

fn set_journal_mode(connection: &Connection, expected: &'static str) -> Result<()> {
    let actual: String =
        connection.query_row(&format!("PRAGMA journal_mode = {expected}"), [], |row| {
            row.get(0)
        })?;
    if !actual.eq_ignore_ascii_case(expected) {
        return invalid(format!("journal_mode is {actual}, expected {expected}"));
    }
    Ok(())
}

fn bound_descriptor_path(file: &File, kind: DatabaseKind) -> Result<PathBuf> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(DatabaseError::InvalidInput(format!(
            "bound {} database descriptor is not a regular file",
            kind.label()
        )));
    }
    Ok(PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd())))
}

fn normalize_bound_journal_header(file: &File, kind: DatabaseKind, state: FileState) -> Result<()> {
    if state == FileState::Fresh {
        if file.metadata()?.len() != 0 {
            return Err(DatabaseError::InvalidInput(format!(
                "fresh bound {} database is not empty",
                kind.label()
            )));
        }
        return Ok(());
    }
    let mut header = [0_u8; 100];
    file.read_exact_at(&mut header, 0)?;
    if &header[..16] != b"SQLite format 3\0"
        || !matches!(header[18], 1 | 2)
        || !matches!(header[19], 1 | 2)
    {
        return Err(DatabaseError::InvalidInput(format!(
            "bound {} database has an invalid SQLite header",
            kind.label()
        )));
    }
    if header[18] == 2 || header[19] == 2 {
        // Litestream restores a self-contained, integrity-checked database
        // file, never a live WAL sidecar. Reset SQLite's persistent WAL header
        // bytes before descriptor-path writes so SQLite does not attempt to
        // discover siblings through `/dev/fd`; the bound writer immediately
        // selects MEMORY journaling for staged migration and validation.
        file.write_all_at(&[1, 1], 18)?;
        file.sync_all()?;
    }
    Ok(())
}

fn configure_length_limit(connection: &Connection, kind: DatabaseKind) -> Result<()> {
    if kind == DatabaseKind::Media {
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MEDIA_CONNECTION_LENGTH_LIMIT)?;
    }
    Ok(())
}

fn verify_pragma(connection: &Connection, pragma: &'static str, expected: i64) -> Result<()> {
    let actual: i64 = connection.query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))?;
    if actual != expected {
        return invalid(format!("PRAGMA {pragma} is {actual}, expected {expected}"));
    }
    Ok(())
}

fn invalid<T>(reason: String) -> Result<T> {
    Err(DatabaseError::Validation {
        kind: "connection",
        reason,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::OpenOptions;
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn inspection_rejects_symlinks_without_following_the_target() {
        let directory = tempfile::tempdir().expect("database inspection root");
        let target = directory.path().join("outside.sqlite3");
        let linked = directory.path().join("kosh.sqlite3");
        fs::write(&target, b"outside database").expect("target fixture");
        symlink(&target, &linked).expect("database symlink");

        let error = inspect_file(&linked).expect_err("symlink must be rejected");

        assert!(matches!(error, DatabaseError::InvalidInput(_)));
        assert_eq!(
            fs::read(&target).expect("unchanged target"),
            b"outside database"
        );
    }

    #[test]
    fn bound_connections_reopen_the_exact_database_file_without_sidecars() {
        let directory = tempfile::tempdir().expect("bound database root");
        let path = directory.path().join("media.sqlite3");
        let created = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("bound database file");
        let writer = open_bound_writer(&created, DatabaseKind::Media, FileState::Fresh)
            .expect("bound writer");
        writer
            .execute("CREATE TABLE probe(value TEXT)", [])
            .expect("bound schema");
        writer
            .execute("INSERT INTO probe VALUES('ok')", [])
            .expect("bound write");
        drop(writer);
        created.sync_all().expect("bound database sync");
        let reader = open_bound_read_only(&created, DatabaseKind::Media).expect("bound reader");
        assert_eq!(
            reader
                .query_row("SELECT value FROM probe", [], |row| row.get::<_, String>(0))
                .expect("bound read"),
            "ok"
        );
        assert!(!directory.path().join("media.sqlite3-wal").exists());
        assert!(!directory.path().join("media.sqlite3-shm").exists());
        assert!(!directory.path().join("media.sqlite3-journal").exists());
    }
}
