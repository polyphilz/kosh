use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::{
    connection::{self, DatabaseKind},
    embedding_index,
    error::{DatabaseError, Result},
    migrations,
};

const MAIN_TABLES: &[&str] = &[
    "active_passage",
    "app_settings",
    "attachment",
    "attachment_extraction",
    "attachment_extractor_config",
    "attachment_segment",
    "draft",
    "draft_context",
    "draft_media_lease",
    "draft_source",
    "index_state",
    "media_ingest_lease",
    "passage",
    "passage_embedding",
    "passage_embedding_index",
    "passage_embedding_reap_queue",
    "passage_embedding_settings",
    "passage_fts_short",
    "passage_fts_trigram",
    "passage_fts_word",
    "passage_search_document",
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
        MAIN_TABLES.iter().copied().filter(|table| {
            !table.starts_with("passage_fts_") && *table != embedding_index::JINA_V1_VEC_TABLE
        }),
    )?;
    validate_strict_tables(media, DatabaseKind::Media, MEDIA_TABLES.iter().copied())?;
    recover_interrupted_derived_work(main)?;
    validate_optional_embedding_index(main);
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

fn validate_optional_embedding_index(connection: &mut Connection) {
    let result = (|| {
        let vec_version: String =
            connection.query_row("SELECT vec_version()", [], |row| row.get(0))?;
        if vec_version != "v0.1.9" {
            return invalid(
                DatabaseKind::Main,
                format!("sqlite-vec version is {vec_version}, expected v0.1.9"),
            );
        }
        if !table_exists(connection, embedding_index::JINA_V1_VEC_TABLE)? {
            return invalid(
                DatabaseKind::Main,
                "the Jina v1 vector table is missing".into(),
            );
        }
        embedding_index::validate_definition(connection)?;
        validate_vec_smoke_test(connection)
    })();
    match result {
        Ok(()) => {
            let _ = embedding_index::release_quarantine(connection);
        }
        Err(error) => {
            log::warn!("semantic passage index is unavailable at startup: {error}");
            let _ = embedding_index::quarantine(
                connection,
                "semantic passage index is unavailable; repair is required",
                0,
            );
        }
    }
}

fn validate_vec_smoke_test(connection: &mut Connection) -> Result<()> {
    let manifest = embedding_index::manifest();
    let mut vector = vec![0.0_f32; manifest.dimension as usize];
    vector[0] = 1.0;
    let vector_json = serde_json::to_string(&vector)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM passage_embedding_vec_jina_v1 WHERE rowid = -1",
        [],
    )?;
    transaction.execute(
        "INSERT INTO passage_embedding_vec_jina_v1(rowid, embedding) VALUES(-1, ?1)",
        params![vector_json.as_str()],
    )?;
    let rowid: i64 = transaction.query_row(
        "SELECT rowid
         FROM passage_embedding_vec_jina_v1
         WHERE embedding MATCH ?1 AND k = 1",
        params![vector_json.as_str()],
        |row| row.get(0),
    )?;
    transaction.rollback()?;
    if rowid != -1 {
        return invalid(
            DatabaseKind::Main,
            "sqlite-vec smoke test returned an unexpected row".into(),
        );
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

fn recover_interrupted_derived_work(connection: &mut Connection) -> Result<()> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "UPDATE index_state
         SET status = 'DIRTY',
             cursor = NULL,
             updated_at = 0,
             error = 'maintenance was interrupted'
         WHERE status = 'RUNNING'",
        [],
    )?;
    // Segments are not installed evidence until their extraction becomes
    // READY. Remove partial output while the attempt is still RUNNING so the
    // same immutable ordinals can be produced safely on retry.
    transaction.execute(
        "DELETE FROM attachment_segment
         WHERE EXISTS (
             SELECT 1
             FROM attachment_extraction
             WHERE attachment_extraction.id = attachment_segment.extraction_id
               AND attachment_extraction.status = 'RUNNING'
         )",
        [],
    )?;
    transaction.execute(
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
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'FAILED',
             error = 'attachment content hash changed before recovery',
             completed_at = coalesce(started_at, created_at)
         WHERE status = 'RUNNING'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn reconcile_fts(connection: &mut Connection) -> Result<bool> {
    const INDEXES: &[&str] = &[
        "passage_fts_word",
        "passage_fts_trigram",
        "passage_fts_short",
    ];
    const VERSION: &str = "lexical-v1";
    let existing_version = connection
        .query_row(
            "SELECT version FROM index_state WHERE name = 'PASSAGE_FTS'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let force_rebuild = existing_version.as_deref() != Some(VERSION);
    // Commit the in-progress marker separately. A crash during the following
    // atomic rebuild leaves RUNNING for startup to recover to DIRTY.
    connection.execute(
        "INSERT INTO index_state(name, version, status, cursor, updated_at, error)
         VALUES('PASSAGE_FTS', ?1, 'RUNNING', NULL, 0, NULL)
         ON CONFLICT(name) DO UPDATE SET
            status = 'RUNNING',
            cursor = NULL,
            updated_at = 0,
            error = NULL",
        params![existing_version.as_deref().unwrap_or(VERSION)],
    )?;

    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut failed = Vec::new();

    for index in INDEXES {
        let integrity = format!("INSERT INTO {index}({index}, rank) VALUES('integrity-check', 0)");
        let needs_rebuild = force_rebuild || transaction.execute(&integrity, []).is_err();
        if needs_rebuild
            && (rebuild_normalized_fts(&transaction, index).is_err()
                || transaction.execute(&integrity, []).is_err())
        {
            failed.push(*index);
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
    let stored_version = if failed.is_empty() {
        VERSION
    } else {
        existing_version.as_deref().unwrap_or(VERSION)
    };
    transaction.execute(
        "INSERT INTO index_state(name, version, status, cursor, updated_at, error)
         VALUES('PASSAGE_FTS', ?1, ?2, NULL, 0, ?3)
         ON CONFLICT(name) DO UPDATE SET
            version = excluded.version,
            status = excluded.status,
            cursor = NULL,
            updated_at = excluded.updated_at,
            error = excluded.error",
        params![stored_version, status, error],
    )?;
    transaction.commit()?;
    Ok(failed.is_empty())
}

fn rebuild_normalized_fts(
    transaction: &rusqlite::Transaction<'_>,
    index: &str,
) -> rusqlite::Result<()> {
    transaction.execute(
        &format!("INSERT INTO {index}({index}) VALUES('delete-all')"),
        [],
    )?;
    let projection = if index == "passage_fts_short" {
        "kosh_search_short_grams"
    } else {
        "kosh_search_normalize"
    };
    transaction.execute(
        &format!(
            "INSERT INTO {index}(
                rowid, title, heading_context, body, source_labels,
                source_domains, attachment_names, extracted_text
             )
             SELECT
                rowid,
                {projection}(title),
                {projection}(heading_context),
                {projection}(body),
                {projection}(source_labels),
                {projection}(source_domains),
                {projection}(attachment_names),
                {projection}(extracted_text)
             FROM passage_search_document
             ORDER BY rowid"
        ),
        [],
    )?;
    Ok(())
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
