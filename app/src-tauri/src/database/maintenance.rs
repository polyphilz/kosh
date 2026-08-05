use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;

use super::{block_embedding_index, block_search, DatabaseError, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueCounts {
    pub pending: u64,
    pub running: u64,
    pub retry_wait: u64,
    pub ready: u64,
    pub failed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDiagnostic {
    pub name: String,
    pub version: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceDatabaseSnapshot {
    pub active_tidbits: u64,
    pub trashed_tidbits: u64,
    pub current_notes: u64,
    pub searchable_blocks: u64,
    pub attachments: u64,
    pub attachment_bytes: u64,
    pub image_ocr: QueueCounts,
    pub indexes: Vec<IndexDiagnostic>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionRetryReport {
    pub image_ocr_queued: u64,
}

pub(super) fn snapshot(connection: &Connection) -> Result<MaintenanceDatabaseSnapshot> {
    let (active_tidbits, trashed_tidbits) = connection.query_row(
        "SELECT
            count(*) FILTER (WHERE deleted_at IS NULL),
            count(*) FILTER (WHERE deleted_at IS NOT NULL)
         FROM tidbit",
        [],
        |row| Ok((nonnegative(row.get(0)?)?, nonnegative(row.get(1)?)?)),
    )?;
    let (attachments, attachment_bytes) = connection.query_row(
        "SELECT count(*), coalesce(sum(byte_length), 0)
         FROM attachment
         WHERE deleted_at IS NULL",
        [],
        |row| Ok((nonnegative(row.get(0)?)?, nonnegative(row.get(1)?)?)),
    )?;
    let mut indexes = connection
        .prepare(
            "SELECT name, version, status, error
             FROM index_state
             ORDER BY name",
        )?
        .query_map([], |row| {
            Ok(IndexDiagnostic {
                name: row.get(0)?,
                version: row.get(1)?,
                status: row.get(2)?,
                error: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    indexes.shrink_to_fit();

    Ok(MaintenanceDatabaseSnapshot {
        active_tidbits,
        trashed_tidbits,
        current_notes: count(connection, "tidbit")?,
        searchable_blocks: count(connection, "block_search_document")?,
        attachments,
        attachment_bytes,
        image_ocr: queue_counts(connection, "image_ocr_queue", "ocr")?,
        indexes,
    })
}

pub(super) fn rebuild_search(connection: &mut Connection) -> Result<u64> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    block_search::rebuild_documents(&transaction)?;
    let documents =
        transaction.query_row("SELECT count(*) FROM block_search_document", [], |row| {
            nonnegative(row.get(0)?)
        })?;
    transaction.commit()?;
    Ok(documents)
}

pub(super) fn rebuild_embeddings(connection: &mut Connection, now_ms: i64) -> Result<u64> {
    if now_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "embedding rebuild timestamp must not be negative".into(),
        ));
    }
    block_embedding_index::ensure_vector_table(connection)?;
    let manifest = crate::embedding::jina_v1_manifest();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let queued_block_vectors = transaction.execute(
        "INSERT INTO block_embedding_reap_queue(block_rowid)
         SELECT rowid FROM block_embedding_vec_jina_v1 WHERE rowid > 0
         ON CONFLICT(block_rowid) DO NOTHING",
        [],
    )? as u64;
    let queued_block_documents = transaction.execute(
        "INSERT INTO block_embedding_reap_queue(block_rowid)
         SELECT rowid FROM block_search_document WHERE true
         ON CONFLICT(block_rowid) DO NOTHING",
        [],
    )? as u64;
    let invalidated_blocks = transaction.execute(
        "DELETE FROM block_embedding WHERE embedding_index_id = ?1",
        params![manifest.id.as_str()],
    )? as u64;
    transaction.execute(
        "UPDATE block_embedding_settings
         SET active_embedding_index_id = NULL,
             updated_at = max(updated_at + 1, ?1)
         WHERE singleton_id = 1",
        params![now_ms],
    )?;
    let block_changed = transaction.execute(
        "UPDATE index_state
         SET status = 'DIRTY', cursor = NULL,
             updated_at = max(updated_at, ?1), error = NULL
         WHERE name = 'BLOCK_EMBEDDING' AND version = ?2",
        params![now_ms, manifest.index_key.as_str()],
    )?;
    if block_changed != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_EMBEDDING index state is missing or incompatible".into(),
        });
    }
    transaction.commit()?;
    Ok(invalidated_blocks.max(queued_block_vectors.saturating_add(queued_block_documents)))
}

pub(super) fn retry_failed_extractions(
    connection: &mut Connection,
    now_ms: i64,
) -> Result<ExtractionRetryReport> {
    if now_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "extraction retry timestamp must not be negative".into(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let image_ids = current_failed_extraction_ids(&transaction, "image_ocr_queue", "ocr")?;
    reset_extractions(&transaction, "image_ocr_queue", &image_ids, now_ms)?;
    transaction.commit()?;
    Ok(ExtractionRetryReport {
        image_ocr_queued: image_ids.len() as u64,
    })
}

fn current_failed_extraction_ids(
    connection: &Connection,
    queue: &str,
    extractor: &str,
) -> Result<Vec<String>> {
    let sql = format!(
        "SELECT extraction.id
         FROM {queue} AS queue
         JOIN attachment_extraction AS extraction ON extraction.id = queue.extraction_id
         JOIN attachment_extractor_config AS config
           ON config.extractor = extraction.extractor
          AND config.version = extraction.extractor_version
         JOIN attachment
           ON attachment.id = extraction.attachment_id
          AND attachment.sha256 = extraction.content_hash
          AND attachment.deleted_at IS NULL
         WHERE queue.state = 'FAILED'
           AND extraction.extractor = ?1
         ORDER BY extraction.id"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params![extractor], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn reset_extractions(
    transaction: &rusqlite::Transaction<'_>,
    queue: &str,
    extraction_ids: &[String],
    now_ms: i64,
) -> Result<()> {
    for extraction_id in extraction_ids {
        transaction.execute(
            "UPDATE attachment_extraction
             SET status = 'PENDING', error = NULL,
                 started_at = NULL, completed_at = NULL
             WHERE id = ?1",
            params![extraction_id],
        )?;
        transaction.execute(
            &format!(
                "UPDATE {queue}
                 SET state = 'PENDING', attempt_count = 0,
                     next_attempt_at = ?1, started_at = NULL,
                     last_error = NULL, updated_at = ?1
                 WHERE extraction_id = ?2"
            ),
            params![now_ms, extraction_id],
        )?;
        transaction.execute(
            "UPDATE attachment
             SET extraction_state = 'PENDING', updated_at = max(updated_at, ?1)
             WHERE id = (
                SELECT attachment_id
                FROM attachment_extraction
                WHERE id = ?2
             )",
            params![now_ms, extraction_id],
        )?;
    }
    Ok(())
}

fn queue_counts(connection: &Connection, table: &str, extractor: &str) -> Result<QueueCounts> {
    let sql = format!(
        "SELECT
            count(*) FILTER (WHERE queue.state = 'PENDING'),
            count(*) FILTER (WHERE queue.state = 'RUNNING'),
            count(*) FILTER (WHERE queue.state = 'RETRY_WAIT'),
            count(*) FILTER (WHERE queue.state = 'READY'),
            count(*) FILTER (WHERE queue.state = 'FAILED')
         FROM {table} AS queue
         JOIN attachment_extraction AS extraction ON extraction.id = queue.extraction_id
         JOIN attachment_extractor_config AS config
           ON config.extractor = extraction.extractor
          AND config.version = extraction.extractor_version
         JOIN attachment
           ON attachment.id = extraction.attachment_id
          AND attachment.sha256 = extraction.content_hash
          AND attachment.deleted_at IS NULL
         WHERE extraction.extractor = ?1"
    );
    connection
        .query_row(&sql, params![extractor], |row| {
            Ok(QueueCounts {
                pending: nonnegative(row.get(0)?)?,
                running: nonnegative(row.get(1)?)?,
                retry_wait: nonnegative(row.get(2)?)?,
                ready: nonnegative(row.get(3)?)?,
                failed: nonnegative(row.get(4)?)?,
            })
        })
        .map_err(Into::into)
}

fn count(connection: &Connection, table: &str) -> Result<u64> {
    count_where(connection, table, "1")
}

fn count_where(connection: &Connection, table: &str, predicate: &str) -> Result<u64> {
    let sql = format!("SELECT count(*) FROM {table} WHERE {predicate}");
    connection
        .query_row(&sql, [], |row| nonnegative(row.get(0)?))
        .map_err(Into::into)
}

fn nonnegative(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
