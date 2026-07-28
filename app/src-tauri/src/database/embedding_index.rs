use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::embedding::{self, TextEmbeddingConfig, TextEmbeddingManifest};

use super::{DatabaseError, Result};

pub(crate) const JINA_V1_VEC_TABLE: &str = "passage_embedding_vec_jina_v1";
pub(crate) const RECONCILIATION_BATCH_SIZE: u32 = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingPassageEmbedding {
    pub passage_rowid: i64,
    pub passage_id: String,
    pub content: String,
    pub content_hash: Vec<u8>,
    pub embedding_index_id: String,
    pub index_key: String,
    pub priority_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallEmbeddingDisposition {
    Installed,
    Stale,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PassageEmbeddingIndexState {
    Idle,
    Dirty,
    Running,
    Failed,
}

impl PassageEmbeddingIndexState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "IDLE" => Ok(Self::Idle),
            "DIRTY" => Ok(Self::Dirty),
            "RUNNING" => Ok(Self::Running),
            "FAILED" => Ok(Self::Failed),
            _ => Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("PASSAGE_EMBEDDING has unknown status {value}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PassageEmbeddingIndexProgress {
    pub embedding_index_id: String,
    pub index_key: String,
    pub indexed_passages: i64,
    pub total_passages: i64,
    pub active: bool,
    pub state: PassageEmbeddingIndexState,
    pub error: Option<String>,
}

pub(crate) fn load_reconciliation_batch(
    connection: &mut Connection,
    limit: u32,
) -> Result<Vec<PendingPassageEmbedding>> {
    if !(1..=RECONCILIATION_BATCH_SIZE).contains(&limit) {
        return Err(DatabaseError::InvalidInput(format!(
            "embedding reconciliation limit must be between 1 and {RECONCILIATION_BATCH_SIZE}"
        )));
    }
    let manifest = embedding::jina_v1_manifest();
    let marked = connection.execute(
        "UPDATE index_state
         SET status = 'RUNNING', cursor = NULL, error = NULL
         WHERE name = 'PASSAGE_EMBEDDING' AND version = ?1",
        params![manifest.index_key.as_str()],
    )?;
    if marked != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "PASSAGE_EMBEDDING index state is missing or incompatible".into(),
        });
    }
    // trusted_schema stays OFF, so schema triggers cannot safely invoke a
    // virtual table. Their synchronous metadata deletion makes stale vectors
    // unreachable; reconciliation reaps the now-unowned vector rows here.
    connection.execute(
        "DELETE FROM passage_embedding_vec_jina_v1
         WHERE rowid IN (
             SELECT vector.rowid
             FROM passage_embedding_vec_jina_v1 AS vector
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM passage_search_document AS document
                 JOIN passage
                   ON passage.rowid = document.rowid
                  AND passage.id = document.passage_id
                 JOIN passage_embedding AS metadata
                   ON metadata.passage_id = passage.id
                  AND metadata.embedding_index_id = ?1
                  AND metadata.passage_content_hash = passage.content_hash
                 WHERE document.rowid = vector.rowid
             )
         )",
        params![manifest.id.as_str()],
    )?;

    let mut statement = connection.prepare(
        "SELECT
            document.rowid,
            passage.id,
            passage.content,
            passage.content_hash,
            document.updated_at
         FROM passage_search_document AS document
         JOIN passage
           ON passage.rowid = document.rowid
          AND passage.id = document.passage_id
         LEFT JOIN passage_embedding AS metadata
           ON metadata.passage_id = passage.id
          AND metadata.embedding_index_id = ?1
         WHERE metadata.passage_id IS NULL
            OR metadata.passage_content_hash != passage.content_hash
            OR NOT EXISTS (
                SELECT 1
                FROM passage_embedding_vec_jina_v1 AS vector
                WHERE vector.rowid = document.rowid
            )
         ORDER BY document.updated_at DESC, document.rowid DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![manifest.id.as_str(), limit], |row| {
        Ok(PendingPassageEmbedding {
            passage_rowid: row.get(0)?,
            passage_id: row.get(1)?,
            content: row.get(2)?,
            content_hash: row.get(3)?,
            embedding_index_id: manifest.id.clone(),
            index_key: manifest.index_key.clone(),
            priority_at_ms: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub(crate) fn install_embedding(
    connection: &mut Connection,
    pending: &PendingPassageEmbedding,
    vector: &[f32],
    created_at_ms: i64,
) -> Result<InstallEmbeddingDisposition> {
    if created_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "embedding timestamp must not be negative".into(),
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
    pending: &PendingPassageEmbedding,
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
             FROM passage_search_document AS document
             JOIN passage
               ON passage.rowid = document.rowid
              AND passage.id = document.passage_id
             WHERE document.rowid = ?1
               AND passage.id = ?2
               AND passage.content_hash = ?3",
            params![
                pending.passage_rowid,
                pending.passage_id.as_str(),
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
        "DELETE FROM passage_embedding_vec_jina_v1 WHERE rowid = ?1",
        params![pending.passage_rowid],
    )?;
    transaction.execute(
        "DELETE FROM passage_embedding
         WHERE embedding_index_id = ?1 AND passage_id = ?2",
        params![manifest.id.as_str(), pending.passage_id.as_str()],
    )?;
    let vector_json = serde_json::to_string(vector)?;
    transaction.execute(
        "INSERT INTO passage_embedding_vec_jina_v1(rowid, embedding)
         VALUES(?1, ?2)",
        params![pending.passage_rowid, vector_json],
    )?;
    transaction.execute(
        "INSERT INTO passage_embedding(
            passage_id,
            embedding_index_id,
            passage_content_hash,
            created_at
         ) VALUES(?1, ?2, ?3, ?4)",
        params![
            pending.passage_id.as_str(),
            manifest.id.as_str(),
            pending.content_hash.as_slice(),
            created_at_ms
        ],
    )?;
    Ok(InstallEmbeddingDisposition::Installed)
}

pub(crate) fn progress(connection: &Connection) -> Result<PassageEmbeddingIndexProgress> {
    let manifest = embedding::jina_v1_manifest();
    let total_passages =
        connection.query_row("SELECT count(*) FROM passage_search_document", [], |row| {
            row.get(0)
        })?;
    let vector_table_available = connection
        .query_row(
            "SELECT 1
             FROM sqlite_schema
             WHERE type = 'table' AND name = ?1",
            params![JINA_V1_VEC_TABLE],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let indexed_passages = if vector_table_available {
        connection.query_row(
            "SELECT count(*)
             FROM passage_search_document AS document
             JOIN passage
               ON passage.rowid = document.rowid
              AND passage.id = document.passage_id
             JOIN passage_embedding AS metadata
               ON metadata.passage_id = passage.id
              AND metadata.embedding_index_id = ?1
              AND metadata.passage_content_hash = passage.content_hash
             WHERE EXISTS (
                 SELECT 1
                 FROM passage_embedding_vec_jina_v1 AS vector
                 WHERE vector.rowid = document.rowid
             )",
            params![manifest.id.as_str()],
            |row| row.get(0),
        )?
    } else {
        0
    };
    let active_index_id = connection.query_row(
        "SELECT active_embedding_index_id
         FROM passage_embedding_settings
         WHERE singleton_id = 1",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    let (state, error) = connection.query_row(
        "SELECT status, error
         FROM index_state
         WHERE name = 'PASSAGE_EMBEDDING' AND version = ?1",
        params![manifest.index_key.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    Ok(PassageEmbeddingIndexProgress {
        embedding_index_id: manifest.id.clone(),
        index_key: manifest.index_key,
        indexed_passages,
        total_passages,
        active: active_index_id.as_deref() == Some(manifest.id.as_str()),
        state: PassageEmbeddingIndexState::parse(&state)?,
        error,
    })
}

pub(crate) fn activate_if_complete(
    connection: &mut Connection,
    activated_at_ms: i64,
) -> Result<bool> {
    if activated_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "embedding activation timestamp must not be negative".into(),
        ));
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let progress = progress(&transaction)?;
    if progress.indexed_passages != progress.total_passages {
        transaction.rollback()?;
        return Ok(false);
    }
    let manifest = embedding::jina_v1_manifest();
    transaction.execute(
        "UPDATE passage_embedding_settings
         SET active_embedding_index_id = ?1,
             updated_at = max(updated_at + 1, ?2)
         WHERE singleton_id = 1",
        params![manifest.id.as_str(), activated_at_ms],
    )?;
    transaction.execute(
        "UPDATE index_state
         SET status = 'IDLE', cursor = NULL,
             updated_at = max(updated_at, ?1), error = NULL
         WHERE name = 'PASSAGE_EMBEDDING' AND version = ?2",
        params![activated_at_ms, manifest.index_key.as_str()],
    )?;
    transaction.commit()?;
    Ok(true)
}

pub(crate) fn record_failure(
    connection: &Connection,
    error: &str,
    failed_at_ms: i64,
) -> Result<()> {
    if failed_at_ms < 0 {
        return Err(DatabaseError::InvalidInput(
            "embedding failure timestamp must not be negative".into(),
        ));
    }
    let bounded_error = error.chars().take(1_024).collect::<String>();
    let changed = connection.execute(
        "UPDATE index_state
         SET status = 'FAILED', cursor = NULL,
             updated_at = max(updated_at, ?1), error = ?2
         WHERE name = 'PASSAGE_EMBEDDING'",
        params![failed_at_ms, bounded_error],
    )?;
    if changed != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "PASSAGE_EMBEDDING index state is missing".into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_definition(connection: &Connection) -> Result<()> {
    let manifest = embedding::jina_v1_manifest();
    let stored = connection
        .query_row(
            "SELECT
                id, created_at, index_key, model_name, model_revision,
                lower(hex(model_file_sha256)), dimension, distance_metric,
                normalized, index_schema_version, config_json
             FROM passage_embedding_index
             WHERE index_key = ?1",
            params![manifest.index_key.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::Validation {
            kind: "main",
            reason: "the shipped Jina v1 passage embedding index is missing".into(),
        })?;
    let stored_config: TextEmbeddingConfig = serde_json::from_str(&stored.10)?;
    if stored.0 != manifest.id
        || stored.1 != manifest.created_at
        || stored.2 != manifest.index_key
        || stored.3 != manifest.model_name
        || stored.4 != manifest.model_revision
        || stored.5 != manifest.model_file_sha256
        || stored.6 != manifest.dimension
        || stored.7 != manifest.distance_metric
        || stored.8 != manifest.normalized
        || stored.9 != manifest.index_schema_version
        || stored_config != manifest.config
    {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "the Jina v1 passage embedding index does not match the shipped manifest"
                .into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_embedding(vector: &[f32], dimension: usize) -> Result<()> {
    if vector.len() != dimension {
        return Err(DatabaseError::InvalidInput(format!(
            "embedding has {} dimensions; expected {dimension}",
            vector.len()
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(DatabaseError::InvalidInput(
            "embedding contains a non-finite value".into(),
        ));
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > 0.001 {
        return Err(DatabaseError::InvalidInput(format!(
            "embedding must be L2-normalized; observed norm {norm}"
        )));
    }
    Ok(())
}

pub(crate) fn manifest() -> TextEmbeddingManifest {
    embedding::jina_v1_manifest()
}
