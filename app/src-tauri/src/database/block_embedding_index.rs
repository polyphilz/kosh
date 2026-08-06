use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::embedding;

use super::{DatabaseError, Result};

pub(crate) const JINA_V1_VEC_TABLE: &str = "block_embedding_vec_jina_v1";
pub(crate) const RECONCILIATION_BATCH_SIZE: u32 = 32;
const MAX_BLOCK_EMBEDDING_BYTES: usize = 24 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallEmbeddingDisposition {
    Installed,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BlockEmbeddingIndexState {
    Dirty,
    Running,
    Idle,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BlockEmbeddingIndexProgress {
    pub state: BlockEmbeddingIndexState,
    pub embedding_index_id: String,
    pub index_key: String,
    pub indexed_blocks: i64,
    pub total_blocks: i64,
    pub active: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingBlockEmbedding {
    pub block_rowid: i64,
    pub tidbit_id: String,
    pub block_id: String,
    pub content: String,
    pub content_hash: Vec<u8>,
    pub embedding_index_id: String,
    pub index_key: String,
    pub priority_at_ms: i64,
}

pub(crate) fn ensure_vector_table(connection: &Connection) -> Result<()> {
    super::connection::register_sqlite_vec()?;
    if connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![JINA_V1_VEC_TABLE],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return validate_vector_table_definition(connection);
    }
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE VIRTUAL TABLE block_embedding_vec_jina_v1 USING vec0(
            embedding float[768] distance_metric=cosine
         );
         UPDATE index_state
         SET status = 'DIRTY', cursor = NULL, error = NULL
         WHERE name = 'BLOCK_EMBEDDING';
         COMMIT;",
    )?;
    Ok(())
}

pub(crate) fn load_reconciliation_batch(
    connection: &mut Connection,
    limit: u32,
) -> Result<Vec<PendingBlockEmbedding>> {
    if !(1..=RECONCILIATION_BATCH_SIZE).contains(&limit) {
        return Err(DatabaseError::InvalidInput(format!(
            "block embedding reconciliation limit must be between 1 and {RECONCILIATION_BATCH_SIZE}"
        )));
    }
    let manifest = embedding::jina_v1_manifest();
    let marked = connection.execute(
        "UPDATE index_state
         SET status = 'RUNNING', cursor = NULL, error = NULL
         WHERE name = 'BLOCK_EMBEDDING' AND version = ?1",
        params![manifest.index_key.as_str()],
    )?;
    if marked != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_EMBEDDING index state is missing or incompatible".into(),
        });
    }
    reap_invalidated_vectors(connection)?;

    let mut statement = connection.prepare(
        "SELECT
            document.rowid,
            document.tidbit_id,
            document.block_id,
            document.heading_context,
            document.body,
            document.attachment_names,
            document.extracted_text,
            document.content_hash,
            document.updated_at
         FROM block_search_document AS document
         LEFT JOIN block_embedding AS metadata
           ON metadata.tidbit_id = document.tidbit_id
          AND metadata.block_id = document.block_id
          AND metadata.embedding_index_id = ?1
         WHERE metadata.block_id IS NULL
            OR metadata.block_content_hash != document.content_hash
            OR NOT EXISTS (
                SELECT 1
                FROM block_embedding_vec_jina_v1 AS vector
                WHERE vector.rowid = document.rowid
            )
         ORDER BY document.updated_at DESC, document.rowid DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![manifest.id.as_str(), limit], |row| {
        let content = embedding_content(
            row.get::<_, String>(3)?.as_str(),
            row.get::<_, String>(4)?.as_str(),
            row.get::<_, String>(5)?.as_str(),
            row.get::<_, String>(6)?.as_str(),
        );
        Ok(PendingBlockEmbedding {
            block_rowid: row.get(0)?,
            tidbit_id: row.get(1)?,
            block_id: row.get(2)?,
            content,
            content_hash: row.get(7)?,
            embedding_index_id: manifest.id.clone(),
            index_key: manifest.index_key.clone(),
            priority_at_ms: row.get(8)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn embedding_content(
    heading_context: &str,
    body: &str,
    attachment_names: &str,
    extracted_text: &str,
) -> String {
    let joined = [heading_context, body, attachment_names, extracted_text]
        .into_iter()
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_utf8(joined, MAX_BLOCK_EMBEDDING_BYTES)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn reap_invalidated_vectors(connection: &mut Connection) -> Result<()> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rowids = {
        let mut statement = transaction.prepare(
            "SELECT block_rowid
             FROM block_embedding_reap_queue
             ORDER BY block_rowid
             LIMIT ?1",
        )?;
        let rows = statement
            .query_map(params![RECONCILIATION_BATCH_SIZE], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        rows
    };
    for rowid in rowids {
        transaction.execute(
            "DELETE FROM block_embedding_vec_jina_v1 WHERE rowid = ?1",
            params![rowid],
        )?;
        transaction.execute(
            "DELETE FROM block_embedding_reap_queue WHERE block_rowid = ?1",
            params![rowid],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub(crate) fn needs_reconciliation(connection: &Connection) -> Result<bool> {
    let manifest = embedding::jina_v1_manifest();
    let (version, status, active_index_id) = connection.query_row(
        "SELECT state.version, state.status, settings.active_embedding_index_id
         FROM index_state AS state
         CROSS JOIN block_embedding_settings AS settings
         WHERE state.name = 'BLOCK_EMBEDDING' AND settings.singleton_id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    if status == "FAILED" {
        return Ok(false);
    }
    if version != manifest.index_key {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_EMBEDDING index state version is incompatible".into(),
        });
    }
    if status != "IDLE" || active_index_id.as_deref() != Some(manifest.id.as_str()) {
        return Ok(true);
    }
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM block_embedding_reap_queue)",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(crate) fn install_embedding(
    connection: &mut Connection,
    pending: &PendingBlockEmbedding,
    vector: &[f32],
    created_at_ms: i64,
) -> Result<InstallEmbeddingDisposition> {
    if created_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "block embedding timestamp must not be negative".into(),
        ));
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let disposition =
        install_embedding_in_transaction(&transaction, pending, vector, created_at_ms)?;
    transaction.commit()?;
    Ok(disposition)
}

fn install_embedding_in_transaction(
    transaction: &Transaction<'_>,
    pending: &PendingBlockEmbedding,
    vector: &[f32],
    created_at_ms: i64,
) -> Result<InstallEmbeddingDisposition> {
    let manifest = embedding::jina_v1_manifest();
    validate_embedding(vector, manifest.dimension as usize)?;
    if pending.embedding_index_id != manifest.id || pending.index_key != manifest.index_key {
        return Ok(InstallEmbeddingDisposition::Stale);
    }
    let still_current = transaction
        .query_row(
            "SELECT 1
             FROM block_search_document
             WHERE rowid = ?1
               AND tidbit_id = ?2
               AND block_id = ?3
               AND content_hash = ?4",
            params![
                pending.block_rowid,
                pending.tidbit_id.as_str(),
                pending.block_id.as_str(),
                pending.content_hash.as_slice()
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !still_current {
        return Ok(InstallEmbeddingDisposition::Stale);
    }

    transaction.execute(
        "DELETE FROM block_embedding_reap_queue WHERE block_rowid = ?1",
        params![pending.block_rowid],
    )?;
    transaction.execute(
        "DELETE FROM block_embedding_vec_jina_v1 WHERE rowid = ?1",
        params![pending.block_rowid],
    )?;
    transaction.execute(
        "DELETE FROM block_embedding
         WHERE embedding_index_id = ?1 AND tidbit_id = ?2 AND block_id = ?3",
        params![
            manifest.id.as_str(),
            pending.tidbit_id.as_str(),
            pending.block_id.as_str()
        ],
    )?;
    let vector_json = serde_json::to_string(vector)?;
    transaction.execute(
        "INSERT INTO block_embedding_vec_jina_v1(rowid, embedding) VALUES(?1, ?2)",
        params![pending.block_rowid, vector_json],
    )?;
    transaction.execute(
        "INSERT INTO block_embedding(
            tidbit_id, block_id, embedding_index_id, block_content_hash, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![
            pending.tidbit_id.as_str(),
            pending.block_id.as_str(),
            manifest.id.as_str(),
            pending.content_hash.as_slice(),
            created_at_ms
        ],
    )?;
    Ok(InstallEmbeddingDisposition::Installed)
}

pub(crate) fn progress(connection: &Connection) -> Result<BlockEmbeddingIndexProgress> {
    let manifest = embedding::jina_v1_manifest();
    let (status, active_index_id, error) = connection.query_row(
        "SELECT state.status, settings.active_embedding_index_id, state.error
         FROM index_state AS state
         CROSS JOIN block_embedding_settings AS settings
         WHERE state.name = 'BLOCK_EMBEDDING' AND settings.singleton_id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let state = match status.as_str() {
        "DIRTY" => BlockEmbeddingIndexState::Dirty,
        "RUNNING" => BlockEmbeddingIndexState::Running,
        "IDLE" => BlockEmbeddingIndexState::Idle,
        "FAILED" => BlockEmbeddingIndexState::Failed,
        _ => {
            return Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("BLOCK_EMBEDDING has unknown status {status}"),
            });
        }
    };
    Ok(BlockEmbeddingIndexProgress {
        state,
        embedding_index_id: manifest.id.clone(),
        index_key: manifest.index_key.clone(),
        indexed_blocks: indexed_count(connection)?,
        total_blocks: document_count(connection)?,
        active: active_index_id.as_deref() == Some(manifest.id.as_str()),
        error,
    })
}

pub(crate) fn validate_embedding(embedding: &[f32], expected_dimension: usize) -> Result<()> {
    if embedding.len() != expected_dimension {
        return Err(DatabaseError::InvalidInput(format!(
            "embedding must contain {expected_dimension} dimensions"
        )));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(DatabaseError::InvalidInput(
            "embedding values must be finite".into(),
        ));
    }
    Ok(())
}

pub(crate) fn activate_if_complete(
    connection: &mut Connection,
    activated_at_ms: i64,
) -> Result<bool> {
    if activated_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "block embedding activation timestamp must not be negative".into(),
        ));
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let queued_reaps: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM block_embedding_reap_queue)",
        [],
        |row| row.get(0),
    )?;
    if queued_reaps || indexed_count(&transaction)? != document_count(&transaction)? {
        transaction.rollback()?;
        return Ok(false);
    }
    let manifest = embedding::jina_v1_manifest();
    transaction.execute(
        "UPDATE block_embedding_settings
         SET active_embedding_index_id = ?1, updated_at = max(updated_at + 1, ?2)
         WHERE singleton_id = 1",
        params![manifest.id.as_str(), activated_at_ms],
    )?;
    transaction.execute(
        "UPDATE index_state
         SET status = 'IDLE', cursor = NULL,
             updated_at = max(updated_at, ?1), error = NULL
         WHERE name = 'BLOCK_EMBEDDING' AND version = ?2",
        params![activated_at_ms, manifest.index_key.as_str()],
    )?;
    transaction.commit()?;
    Ok(true)
}

fn document_count(connection: &Connection) -> Result<i64> {
    connection
        .query_row("SELECT count(*) FROM block_search_document", [], |row| {
            row.get(0)
        })
        .map_err(Into::into)
}

fn indexed_count(connection: &Connection) -> Result<i64> {
    let manifest = embedding::jina_v1_manifest();
    connection
        .query_row(
            "SELECT count(*)
             FROM block_search_document AS document
             JOIN block_embedding AS metadata
               ON metadata.tidbit_id = document.tidbit_id
              AND metadata.block_id = document.block_id
              AND metadata.embedding_index_id = ?1
              AND metadata.block_content_hash = document.content_hash
             WHERE EXISTS (
                 SELECT 1 FROM block_embedding_vec_jina_v1 AS vector
                 WHERE vector.rowid = document.rowid
             )",
            params![manifest.id.as_str()],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(crate) fn record_retryable_failure(
    connection: &Connection,
    error: &str,
    failed_at_ms: i64,
) -> Result<()> {
    record_failure_with_state(connection, error, failed_at_ms, "DIRTY")
}

pub(crate) fn quarantine(connection: &Connection, error: &str, failed_at_ms: i64) -> Result<()> {
    record_failure_with_state(connection, error, failed_at_ms, "FAILED")
}

pub(crate) fn release_quarantine(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE index_state
         SET status = 'DIRTY', cursor = NULL, error = NULL
         WHERE name = 'BLOCK_EMBEDDING' AND status = 'FAILED'",
        [],
    )?;
    Ok(())
}

fn record_failure_with_state(
    connection: &Connection,
    error: &str,
    failed_at_ms: i64,
    state: &str,
) -> Result<()> {
    if failed_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "block embedding failure timestamp must not be negative".into(),
        ));
    }
    let bounded_error = error.chars().take(1_024).collect::<String>();
    let changed = connection.execute(
        "UPDATE index_state
         SET status = ?3, cursor = NULL,
             updated_at = max(updated_at, ?1), error = ?2
         WHERE name = 'BLOCK_EMBEDDING'",
        params![failed_at_ms, bounded_error, state],
    )?;
    if changed != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_EMBEDDING index state is missing".into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_definition(connection: &Connection) -> Result<()> {
    let manifest = embedding::jina_v1_manifest();
    let state_version: String = connection.query_row(
        "SELECT version FROM index_state WHERE name = 'BLOCK_EMBEDDING'",
        [],
        |row| row.get(0),
    )?;
    if state_version != manifest.index_key {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_EMBEDDING state does not match the shipped manifest".into(),
        });
    }
    validate_vector_table_definition(connection)
}

fn validate_vector_table_definition(connection: &Connection) -> Result<()> {
    let manifest = embedding::jina_v1_manifest();
    let actual_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![JINA_V1_VEC_TABLE],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| DatabaseError::Validation {
            kind: "main",
            reason: "the Jina v1 block vector table is missing".into(),
        })?;
    let expected_sql = format!(
        "CREATE VIRTUAL TABLE {JINA_V1_VEC_TABLE} USING vec0(
            embedding float[{}] distance_metric={}
         )",
        manifest.dimension, manifest.distance_metric
    );
    if canonical_schema_sql(&actual_sql) != canonical_schema_sql(&expected_sql) {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "the Jina v1 block vector table does not match the shipped schema".into(),
        });
    }
    Ok(())
}

fn canonical_schema_sql(sql: &str) -> String {
    sql.trim_end_matches(';')
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{embedding_content, MAX_BLOCK_EMBEDDING_BYTES};

    #[test]
    fn embedding_content_is_ordered_nonempty_and_utf8_bounded() {
        assert_eq!(
            embedding_content("Heading", "Body", "file.png", "OCR"),
            "Heading\nBody\nfile.png\nOCR"
        );
        let oversized = format!("{}é", "x".repeat(MAX_BLOCK_EMBEDDING_BYTES));
        let truncated = embedding_content("", &oversized, "", "");
        assert_eq!(truncated.len(), MAX_BLOCK_EMBEDDING_BYTES);
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
