use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, MAIN_DB};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    connection::{self, DatabaseKind},
    validation, DatabaseError, DatabasePaths, Result,
};

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const SNAPSHOT_DIRECTORY: &str = "safety-snapshots";
const MAX_SNAPSHOTS: usize = 3;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SafetySnapshotReason {
    Migration,
    MediaReclaim,
}

impl SafetySnapshotReason {
    fn label(self) -> &'static str {
        match self {
            Self::Migration => "migration",
            Self::MediaReclaim => "media-reclaim",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafetySnapshotReport {
    pub id: String,
    pub reason: SafetySnapshotReason,
    pub created_at_ms: i64,
    pub directory: PathBuf,
    pub main_bytes: u64,
    pub media_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SafetySnapshotManifest {
    schema_version: u32,
    id: String,
    reason: SafetySnapshotReason,
    created_at_ms: i64,
    main: SnapshotFile,
    media: SnapshotFile,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotFile {
    filename: String,
    bytes: u64,
    sha256: String,
}

pub(super) fn create(
    main: &mut Connection,
    media: &mut Connection,
    paths: &DatabasePaths,
    reason: SafetySnapshotReason,
) -> Result<SafetySnapshotReport> {
    verify_pair_connections(main, media)?;
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DatabaseError::InvalidInput("system time predates the Unix epoch".into()))?
        .as_millis()
        .try_into()
        .map_err(|_| DatabaseError::InvalidInput("system time exceeds SQLite's range".into()))?;
    let id = format!(
        "{}-{created_at_ms}-{}",
        reason.label(),
        Uuid::now_v7().simple()
    );
    let snapshot_root = paths.root.join(SNAPSHOT_DIRECTORY);
    create_private_directory(&snapshot_root)?;
    cleanup_interrupted_staging(&snapshot_root)?;
    let staging = snapshot_root.join(format!(".{id}.incomplete"));
    let published = snapshot_root.join(&id);
    create_private_directory(&staging)?;

    let result = create_in_staging(
        main,
        media,
        &staging,
        &id,
        reason,
        created_at_ms,
        &published,
    );
    if result.is_err() {
        remove_owned_staging(&snapshot_root, &staging);
    }
    let report = result?;
    prune_published_snapshots(&snapshot_root, &report.id)?;
    Ok(report)
}

fn create_in_staging(
    main: &mut Connection,
    media: &mut Connection,
    staging: &Path,
    id: &str,
    reason: SafetySnapshotReason,
    created_at_ms: i64,
    published: &Path,
) -> Result<SafetySnapshotReport> {
    let snapshot_paths = DatabasePaths::new(staging);
    vacuum_into(main, &snapshot_paths.main)?;
    vacuum_into(media, &snapshot_paths.media)?;
    verify_pair(&snapshot_paths)?;

    let main_file = inspect_snapshot_file(&snapshot_paths.main, "kosh.sqlite3")?;
    let media_file = inspect_snapshot_file(&snapshot_paths.media, "media.sqlite3")?;
    let manifest = SafetySnapshotManifest {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        id: id.to_owned(),
        reason,
        created_at_ms,
        main: main_file,
        media: media_file,
    };
    write_manifest(staging, &manifest)?;
    fs::rename(staging, published)?;
    sync_directory(
        published
            .parent()
            .ok_or_else(|| DatabaseError::InvalidInput("snapshot has no parent".into()))?,
    )?;

    Ok(SafetySnapshotReport {
        id: id.to_owned(),
        reason,
        created_at_ms,
        directory: published.to_owned(),
        main_bytes: manifest.main.bytes,
        media_bytes: manifest.media.bytes,
    })
}

fn vacuum_into(connection: &mut Connection, destination: &Path) -> Result<()> {
    if destination.exists() {
        return Err(DatabaseError::InvalidInput(format!(
            "snapshot destination already exists: {}",
            destination.display()
        )));
    }
    let destination = destination
        .to_str()
        .ok_or_else(|| DatabaseError::InvalidInput("snapshot path is not valid Unicode".into()))?;
    connection.execute("VACUUM INTO ?1", params![destination])?;
    Ok(())
}

fn verify_pair(paths: &DatabasePaths) -> Result<()> {
    let main = connection::open_read_only(&paths.main, DatabaseKind::Main)?;
    let media = connection::open_read_only(&paths.media, DatabaseKind::Media)?;
    verify_pair_connections(&main, &media)
}

fn verify_pair_connections(main: &Connection, media: &Connection) -> Result<()> {
    validation::full_integrity_check_pair(main, media)?;
    validation::validate_foreign_keys(main, DatabaseKind::Main)?;
    validation::validate_foreign_keys(media, DatabaseKind::Media)?;
    validate_attachment_blob_relationship(main, media)
}

fn validate_attachment_blob_relationship(main: &Connection, media: &Connection) -> Result<()> {
    if !table_exists(main, "attachment")? || !table_exists(media, "media_blob")? {
        return Ok(());
    }
    let mut retained = vec!["attachment.deleted_at IS NULL"];
    if table_exists(main, "tidbit_revision_attachment")? {
        retained.push(
            "EXISTS (
                SELECT 1 FROM tidbit_revision_attachment AS membership
                WHERE membership.attachment_id = attachment.id
            )",
        );
    }
    if table_exists(main, "research_run_attachment")? {
        retained.push(
            "EXISTS (
                SELECT 1 FROM research_run_attachment AS membership
                WHERE membership.attachment_id = attachment.id
            )",
        );
    }
    let sql = format!(
        "SELECT attachment.id, attachment.sha256, attachment.byte_length
         FROM attachment
         WHERE {}",
        retained.join(" OR ")
    );
    let mut attachments = main.prepare(&sql)?;
    let rows = attachments.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut blob = media.prepare("SELECT rowid, byte_length FROM media_blob WHERE sha256 = ?1")?;
    let mut verified_digests = std::collections::HashSet::new();
    for row in rows {
        let (attachment_id, sha256, expected_bytes) = row?;
        let (rowid, actual_bytes) = blob
            .query_row(params![&sha256], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .optional()?
            .ok_or_else(|| DatabaseError::Validation {
                kind: "safety snapshot",
                reason: format!("attachment {attachment_id} has no retained media blob"),
            })?;
        if actual_bytes != expected_bytes {
            return Err(DatabaseError::Validation {
                kind: "safety snapshot",
                reason: format!(
                    "attachment {attachment_id} expects {expected_bytes} bytes, media has {actual_bytes}"
                ),
            });
        }
        if verified_digests.insert(sha256.clone()) {
            let actual_sha256 = hash_media_blob(media, rowid)?;
            if actual_sha256.as_slice() != sha256.as_slice() {
                return Err(DatabaseError::Validation {
                    kind: "safety snapshot",
                    reason: format!(
                        "attachment {attachment_id} references a media blob whose SHA-256 is corrupt"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn hash_media_blob(media: &Connection, rowid: i64) -> Result<[u8; 32]> {
    let mut blob = media.blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = blob.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn inspect_snapshot_file(path: &Path, filename: &str) -> Result<SnapshotFile> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(DatabaseError::InvalidInput(format!(
            "snapshot file is not a non-empty regular file: {}",
            path.display()
        )));
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(SnapshotFile {
        filename: filename.to_owned(),
        bytes: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn write_manifest(directory: &Path, manifest: &SafetySnapshotManifest) -> Result<()> {
    let temporary = directory.join("manifest.json.tmp");
    let final_path = directory.join("manifest.json");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, final_path)?;
    sync_directory(directory)
}

fn create_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(DatabaseError::InvalidInput(format!(
                "safety snapshot directory is not a real directory: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)?,
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn prune_published_snapshots(snapshot_root: &Path, current_id: &str) -> Result<()> {
    let mut owned = fs::read_dir(snapshot_root)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let name = entry.file_name().into_string().ok()?;
            if !file_type.is_dir()
                || name.starts_with('.')
                || !(name.starts_with("migration-") || name.starts_with("media-reclaim-"))
            {
                return None;
            }
            let manifest = read_owned_manifest(&entry.path(), &name).ok()?;
            Some((manifest.created_at_ms, name, entry.path()))
        })
        .collect::<Vec<_>>();
    owned.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    let remove_count = owned.len().saturating_sub(MAX_SNAPSHOTS);
    for (_, _, path) in owned
        .into_iter()
        .filter(|(_, name, _)| name != current_id)
        .take(remove_count)
    {
        fs::remove_dir_all(path)?;
    }
    sync_directory(snapshot_root)
}

fn cleanup_interrupted_staging(snapshot_root: &Path) -> Result<()> {
    let mut removed = false;
    for entry in fs::read_dir(snapshot_root)? {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(name) if is_owned_staging_name(&name) => name,
            _ => continue,
        };
        if !entry.file_type()?.is_dir() || !staging_contents_are_owned(&entry.path())? {
            continue;
        }
        log::warn!("removing interrupted safety snapshot staging directory {name}");
        fs::remove_dir_all(entry.path())?;
        removed = true;
    }
    if removed {
        sync_directory(snapshot_root)?;
    }
    Ok(())
}

fn staging_contents_are_owned(directory: &Path) -> Result<bool> {
    const OWNED_FILES: [&str; 4] = [
        "kosh.sqlite3",
        "media.sqlite3",
        "manifest.json",
        "manifest.json.tmp",
    ];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Ok(false);
        };
        if !entry.file_type()?.is_file() || !OWNED_FILES.contains(&name.as_str()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn is_owned_staging_name(name: &str) -> bool {
    let Some(id) = name
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".incomplete"))
    else {
        return false;
    };
    let tail = id
        .strip_prefix("migration-")
        .or_else(|| id.strip_prefix("media-reclaim-"));
    let Some((created_at_ms, uuid)) = tail.and_then(|tail| tail.split_once('-')) else {
        return false;
    };
    created_at_ms.parse::<i64>().is_ok_and(|value| value >= 0)
        && uuid.len() == 32
        && uuid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && uuid.as_bytes()[12] == b'7'
        && matches!(uuid.as_bytes()[16], b'8' | b'9' | b'a' | b'b')
}

fn read_owned_manifest(directory: &Path, expected_id: &str) -> Result<SafetySnapshotManifest> {
    let manifest_path = directory.join("manifest.json");
    let metadata = fs::symlink_metadata(&manifest_path)?;
    if !metadata.file_type().is_file() {
        return Err(DatabaseError::InvalidInput(
            "snapshot manifest is not a regular file".into(),
        ));
    }
    let manifest: SafetySnapshotManifest =
        serde_json::from_reader(BufReader::new(File::open(manifest_path)?))?;
    if manifest.schema_version != SNAPSHOT_SCHEMA_VERSION || manifest.id != expected_id {
        return Err(DatabaseError::InvalidInput(
            "snapshot manifest ownership does not match its directory".into(),
        ));
    }
    Ok(manifest)
}

fn remove_owned_staging(snapshot_root: &Path, staging: &Path) {
    let is_direct_child = staging.parent() == Some(snapshot_root);
    let has_owned_name = staging
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_owned_staging_name);
    if is_direct_child && has_owned_name {
        let _ = fs::remove_dir_all(staging);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{
        connection::FileState,
        drafts::{SaveDraftInput, SaveDraftWrite},
        migrations,
        tidbits::{self, CreateTidbitWrite},
        AttachmentIngestInput, LexicalSearchMode, MediaLimits, SearchPassagesInput, SourceDraft,
        TidbitDraft,
    };
    use refinery::Target;
    use std::io::{Cursor, Seek, SeekFrom};

    #[test]
    fn verified_snapshot_pair_reopens_with_search_and_citation_provenance() {
        let root = tempfile::tempdir().expect("snapshot source");
        let paths = DatabasePaths::new(root.path());
        let database = crate::database::Database::initialize(paths).expect("database");
        let client = database.client();
        let tidbit = client
            .create_tidbit_with_ids(
                TidbitDraft {
                    title: Some("Recovery drill".into()),
                    body_markdown: "Exact safety snapshot evidence.".into(),
                    sources: vec![SourceDraft {
                        label: Some("Snapshot source".into()),
                        url: Some("https://example.invalid/snapshot".into()),
                    }],
                },
                10,
                Uuid::now_v7().to_string(),
                Uuid::now_v7().to_string(),
                vec![Uuid::now_v7().to_string()],
            )
            .expect("create evidence");
        let expected_passage = client
            .search_passages(SearchPassagesInput {
                query: "\"Exact safety snapshot evidence\"".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("search evidence")[0]
            .passage_id
            .clone();
        let (report, _) = client
            .maintain_media_with_safety_snapshot(20, crate::database::MediaLimits::default())
            .expect("create snapshot");
        assert!(report.main_bytes > 0);
        assert!(report.media_bytes > 0);
        database.shutdown().expect("shutdown source");
        drop(database);

        let restored = crate::database::Database::initialize(DatabasePaths::new(&report.directory))
            .expect("reopen snapshot pair");
        let restored_client = restored.client();
        let result = restored_client
            .search_passages(SearchPassagesInput {
                query: "\"Exact safety snapshot evidence\"".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("search restored evidence");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].passage_id, expected_passage);
        let citation = restored_client
            .resolve_citation(expected_passage)
            .expect("resolve restored citation");
        assert_eq!(
            citation.tidbit.expect("authored citation").revision_id,
            tidbit.current_revision_id
        );
        assert_eq!(
            citation.sources[0].url.as_deref(),
            Some("https://example.invalid/snapshot")
        );
        restored_client
            .full_integrity_check()
            .expect("restored integrity");
    }

    #[test]
    fn retention_ignores_unowned_directories_and_keeps_three_verified_pairs() {
        let root = tempfile::tempdir().expect("snapshot retention");
        let paths = DatabasePaths::new(root.path());
        let database = crate::database::Database::initialize(paths.clone()).expect("database");
        let client = database.client();
        let unowned = paths
            .root
            .join(SNAPSHOT_DIRECTORY)
            .join("migration-user-folder");
        fs::create_dir_all(&unowned).expect("unowned directory");
        fs::write(unowned.join("keep.txt"), "do not delete").expect("unowned marker");

        for _ in 0..4 {
            client
                .maintain_media_with_safety_snapshot(10, crate::database::MediaLimits::default())
                .expect("snapshot");
        }

        let published = fs::read_dir(paths.root.join(SNAPSHOT_DIRECTORY))
            .expect("snapshot directory")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().join("manifest.json").is_file())
            .count();
        assert_eq!(published, MAX_SNAPSHOTS);
        assert_eq!(
            fs::read_to_string(unowned.join("keep.txt")).expect("unowned marker survives"),
            "do not delete"
        );
    }

    #[test]
    fn snapshot_creation_removes_only_validated_interrupted_staging_directories() {
        let root = tempfile::tempdir().expect("snapshot staging recovery");
        let paths = DatabasePaths::new(root.path());
        let database = crate::database::Database::initialize(paths.clone()).expect("database");
        let snapshot_root = paths.root.join(SNAPSHOT_DIRECTORY);
        let interrupted = snapshot_root.join(format!(
            ".migration-10-{}.incomplete",
            Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&interrupted).expect("interrupted directory");
        fs::write(interrupted.join("kosh.sqlite3"), b"partial snapshot")
            .expect("interrupted snapshot file");
        let unowned = snapshot_root.join(".user-data.incomplete");
        fs::create_dir_all(&unowned).expect("unowned directory");
        fs::write(unowned.join("keep.txt"), b"user data").expect("unowned file");

        database
            .client()
            .maintain_media_with_safety_snapshot(10, crate::database::MediaLimits::default())
            .expect("replacement snapshot");

        assert!(!interrupted.exists());
        assert_eq!(
            fs::read(unowned.join("keep.txt")).expect("unowned file survives"),
            b"user data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_creation_rejects_a_symlinked_snapshot_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("snapshot root");
        let external = tempfile::tempdir().expect("external target");
        let paths = DatabasePaths::new(root.path());
        let database = crate::database::Database::initialize(paths.clone()).expect("database");
        symlink(external.path(), paths.root.join(SNAPSHOT_DIRECTORY)).expect("snapshot symlink");

        let error = database
            .client()
            .maintain_media_with_safety_snapshot(10, MediaLimits::default())
            .expect_err("symlinked snapshot root must fail");

        assert!(matches!(error, DatabaseError::InvalidInput(_)));
        assert_eq!(
            fs::read_dir(external.path())
                .expect("external directory")
                .count(),
            0
        );
    }

    #[test]
    fn snapshot_creation_rejects_a_retained_blob_with_a_false_digest() {
        let root = tempfile::tempdir().expect("corrupt snapshot pair");
        let paths = DatabasePaths::new(root.path());
        let database = crate::database::Database::initialize(paths.clone()).expect("database");
        let draft_id = Uuid::now_v7().to_string();
        database
            .client()
            .save_draft(SaveDraftWrite {
                input: SaveDraftInput {
                    context_key: "capture".into(),
                    tidbit_id: None,
                    base_revision_id: None,
                    title: None,
                    body_markdown: String::new(),
                    sources: Vec::new(),
                },
                now_ms: 1,
                draft_id: draft_id.clone(),
                media_limits: MediaLimits::default(),
            })
            .expect("attachment draft");
        let bytes = b"content-addressed evidence";
        database
            .ingest_attachment(
                AttachmentIngestInput {
                    draft_id,
                    display_filename: "evidence.bin".into(),
                    media_type: "application/octet-stream".into(),
                    now_ms: 2,
                    limits: MediaLimits::default(),
                },
                Cursor::new(bytes),
            )
            .expect("retained attachment");
        let expected_sha256 = Sha256::digest(bytes).to_vec();
        let media = Connection::open(&paths.media).expect("media writer");
        let rowid = media
            .query_row(
                "SELECT rowid FROM media_blob WHERE sha256 = ?1",
                params![expected_sha256],
                |row| row.get::<_, i64>(0),
            )
            .expect("media rowid");
        let mut blob = media
            .blob_open(MAIN_DB, "media_blob", "bytes", rowid, false)
            .expect("writable media blob");
        blob.seek(SeekFrom::Start(0)).expect("seek media blob");
        blob.write_all(b"X").expect("corrupt media blob");
        drop(blob);
        drop(media);

        let error = database
            .client()
            .maintain_media_with_safety_snapshot(10, MediaLimits::default())
            .expect_err("corrupt media must fail snapshot verification");

        assert!(matches!(error, DatabaseError::Validation { .. }));
        let snapshot_root = paths.root.join(SNAPSHOT_DIRECTORY);
        let published = snapshot_root.exists()
            && fs::read_dir(snapshot_root)
                .expect("snapshot root")
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().join("manifest.json").is_file());
        assert!(!published);
    }

    #[test]
    fn pending_migration_publishes_verified_pre_migration_pair_before_changes() {
        let root = tempfile::tempdir().expect("pre-migration snapshot");
        let paths = DatabasePaths::new(root.path());
        let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("old main");
        let mut media =
            connection::open_writer(&paths.media, DatabaseKind::Media, FileState::Fresh)
                .expect("old media");
        migrations::main_runner()
            .set_target(Target::Version(15))
            .run(&mut main)
            .expect("main at V15");
        migrations::media_runner()
            .set_target(Target::Version(1))
            .run(&mut media)
            .expect("media at V1");
        tidbits::create_tidbit(
            &mut main,
            CreateTidbitWrite {
                input: TidbitDraft {
                    title: Some("Before migration".into()),
                    body_markdown: "Pre-migration authored evidence.".into(),
                    sources: Vec::new(),
                },
                now_ms: 10,
                tidbit_id: Uuid::now_v7().to_string(),
                revision_id: Uuid::now_v7().to_string(),
                source_ids: Vec::new(),
            },
        )
        .expect("old authored note");
        drop(main);
        drop(media);

        let upgraded =
            crate::database::Database::initialize(paths.clone()).expect("upgrade database pair");
        upgraded
            .client()
            .full_integrity_check()
            .expect("upgraded integrity");

        let snapshot_root = paths.root.join(SNAPSHOT_DIRECTORY);
        let snapshot = fs::read_dir(snapshot_root)
            .expect("migration snapshots")
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("migration-"))
            })
            .expect("pre-migration snapshot")
            .path();
        let snapshot_paths = DatabasePaths::new(snapshot);
        let old_main =
            connection::open_read_only(&snapshot_paths.main, DatabaseKind::Main).expect("snapshot");
        let head: i32 = old_main
            .query_row(
                "SELECT max(version) FROM refinery_schema_history",
                [],
                |row| row.get(0),
            )
            .expect("snapshot migration head");
        let body: String = old_main
            .query_row("SELECT body_markdown FROM tidbit_revision", [], |row| {
                row.get(0)
            })
            .expect("snapshot authored note");
        assert_eq!(head, 15);
        assert_eq!(body, "Pre-migration authored evidence.");
    }
}
