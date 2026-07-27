use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::{
    connection::{self, DatabaseKind},
    error::{DatabaseError, Result},
    migrations,
};

const MAIN_TABLES: &[&str] = &[
    "app_settings",
    "attachment",
    "attachment_extraction",
    "attachment_segment",
    "draft",
    "draft_media_lease",
    "embedding_index",
    "index_state",
    "media_ingest_lease",
    "passage",
    "passage_embedding",
    "passage_fts_trigram",
    "passage_fts_word",
    "research_citation",
    "research_event",
    "research_run",
    "source",
    "tidbit",
    "tidbit_revision",
    "tidbit_revision_attachment",
    "tidbit_revision_source",
];

const MEDIA_TABLES: &[&str] = &[
    "media_blob",
    "media_blob_lease",
    "media_blob_reap_authorization",
];

pub fn validate_migrated_pair(
    main: &mut Connection,
    media: &mut Connection,
    main_path: &Path,
    media_path: &Path,
) -> Result<()> {
    connection::verify_application_id(main, main_path, DatabaseKind::Main)?;
    connection::verify_application_id(media, media_path, DatabaseKind::Media)?;
    validate_expected_heads(main, media)?;
    validate_required_features(main)?;
    validate_tables(main, DatabaseKind::Main, MAIN_TABLES)?;
    validate_tables(media, DatabaseKind::Media, MEDIA_TABLES)?;
    validate_strict_tables(
        main,
        DatabaseKind::Main,
        MAIN_TABLES
            .iter()
            .copied()
            .filter(|table| !table.starts_with("passage_fts_")),
    )?;
    validate_strict_tables(media, DatabaseKind::Media, MEDIA_TABLES.iter().copied())?;
    recover_interrupted_derived_work(main)?;
    validate_foreign_keys(main, DatabaseKind::Main)?;
    validate_foreign_keys(media, DatabaseKind::Media)?;
    validate_media_relationship(main, media)?;
    Ok(())
}

pub(super) fn full_integrity_check_pair(main: &Connection, media: &Connection) -> Result<()> {
    full_integrity_check(main, DatabaseKind::Main)?;
    full_integrity_check(media, DatabaseKind::Media)
}

fn full_integrity_check(connection: &Connection, kind: DatabaseKind) -> Result<()> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let result: String = row.get(0)?;
        if result != "ok" {
            return invalid(kind, format!("integrity_check returned {result}"));
        }
    }
    Ok(())
}

pub fn validate_foreign_keys(connection: &Connection, kind: DatabaseKind) -> Result<()> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    if let Some(row) = rows.next()? {
        let table: String = row.get(0)?;
        let rowid: Option<i64> = row.get(1)?;
        return invalid(
            kind,
            format!("foreign_key_check failed for {table} row {rowid:?}"),
        );
    }
    Ok(())
}

fn validate_expected_heads(main: &mut Connection, media: &mut Connection) -> Result<()> {
    let actual = migrations::current_heads(main, media)?;
    let expected = migrations::expected_heads();
    if actual != expected {
        return Err(DatabaseError::Validation {
            kind: "migration",
            reason: format!("heads are {actual:?}, expected {expected:?}"),
        });
    }
    Ok(())
}

fn validate_required_features(connection: &Connection) -> Result<()> {
    let fts5: i64 = connection.query_row(
        "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
        [],
        |row| row.get(0),
    )?;
    if fts5 != 1 {
        return invalid(DatabaseKind::Main, "bundled SQLite lacks FTS5".into());
    }
    let json: i64 =
        connection.query_row("SELECT json_valid('{\"ok\":true}')", [], |row| row.get(0))?;
    if json != 1 {
        return invalid(DatabaseKind::Main, "bundled SQLite lacks JSON".into());
    }
    Ok(())
}

fn validate_tables(connection: &Connection, kind: DatabaseKind, required: &[&str]) -> Result<()> {
    for table in required {
        if !table_exists(connection, table)? {
            return invalid(kind, format!("required table {table} is missing"));
        }
    }
    Ok(())
}

fn validate_strict_tables<'a>(
    connection: &Connection,
    kind: DatabaseKind,
    tables: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    for table in tables {
        let strict: i64 = connection.query_row(
            "SELECT strict
             FROM pragma_table_list
             WHERE schema = 'main' AND name = ?1",
            params![table],
            |row| row.get(0),
        )?;
        if strict != 1 {
            return invalid(kind, format!("required table {table} is not STRICT"));
        }
    }
    Ok(())
}

fn recover_interrupted_derived_work(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE index_state
         SET status = 'DIRTY',
             cursor = NULL,
             updated_at = 0,
             error = 'maintenance was interrupted'
         WHERE status = 'RUNNING'",
        [],
    )?;
    connection.execute(
        "UPDATE attachment_extraction
         SET status = 'PENDING',
             error = NULL,
             started_at = NULL,
             completed_at = NULL
         WHERE status = 'RUNNING'
           AND EXISTS (
               SELECT 1
               FROM attachment
               WHERE attachment.id = attachment_extraction.attachment_id
                 AND attachment.sha256 = attachment_extraction.content_hash
           )",
        [],
    )?;
    connection.execute(
        "UPDATE attachment_extraction
         SET status = 'FAILED',
             error = 'attachment content hash changed before recovery',
             completed_at = coalesce(started_at, created_at)
         WHERE status = 'RUNNING'",
        [],
    )?;
    Ok(())
}

pub(super) fn reconcile_fts(connection: &Connection) -> Result<bool> {
    const INDEXES: &[&str] = &["passage_fts_word", "passage_fts_trigram"];
    let mut failed = Vec::new();

    for index in INDEXES {
        let integrity = format!("INSERT INTO {index}({index}, rank) VALUES('integrity-check', 1)");
        if connection.execute(&integrity, []).is_err() {
            let rebuild = format!("INSERT INTO {index}({index}) VALUES('rebuild')");
            if connection.execute(&rebuild, []).is_err() {
                failed.push(*index);
            }
        }
    }

    let (status, error) = if failed.is_empty() {
        ("IDLE", None)
    } else {
        (
            "DIRTY",
            Some(format!("could not rebuild {}", failed.join(", "))),
        )
    };
    connection.execute(
        "INSERT INTO index_state(name, version, status, cursor, updated_at, error)
         VALUES('PASSAGE_FTS', '1', ?1, NULL, 0, ?2)
         ON CONFLICT(name) DO UPDATE SET
            version = excluded.version,
            status = excluded.status,
            cursor = NULL,
            updated_at = excluded.updated_at,
            error = excluded.error",
        params![status, error],
    )?;
    Ok(failed.is_empty())
}

fn validate_media_relationship(main: &Connection, media: &Connection) -> Result<()> {
    let mut attachments = main.prepare(
        "SELECT id, sha256, byte_length
         FROM attachment
         ORDER BY id",
    )?;
    let rows = attachments.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut blobs =
        media.prepare("SELECT byte_length FROM media_blob WHERE sha256 = ?1 LIMIT 1")?;

    for row in rows {
        let (attachment_id, sha256, expected_length) = row?;
        let actual_length = blobs
            .query_row(params![sha256], |row| row.get::<_, i64>(0))
            .optional()?
            .ok_or_else(|| DatabaseError::Validation {
                kind: "database pair",
                reason: format!("attachment {attachment_id} has no media blob"),
            })?;
        if actual_length != expected_length {
            return Err(DatabaseError::Validation {
                kind: "database pair",
                reason: format!(
                    "attachment {attachment_id} expects {expected_length} bytes, media has {actual_length}"
                ),
            });
        }
    }
    Ok(())
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn invalid<T>(kind: DatabaseKind, reason: String) -> Result<T> {
    Err(DatabaseError::Validation {
        kind: kind.label(),
        reason,
    })
}
