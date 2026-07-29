use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::research::GroundedResearchAnswer;

use super::{
    tidbits::{self, CreateTidbitWrite},
    DatabaseError, Result, Tidbit, TidbitDraft,
};

const MAX_LIST_LIMIT: u32 = 100;
const MAX_EVENT_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 2_048;
const MAX_TITLE_CHARS: usize = 96;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchRunStatus {
    Queued,
    Running,
    Completed,
    Canceled,
    Failed,
    Interrupted,
}

impl ResearchRunStatus {
    fn database_value(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Completed => "COMPLETED",
            Self::Canceled => "CANCELED",
            Self::Failed => "FAILED",
            Self::Interrupted => "INTERRUPTED",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "QUEUED" => Ok(Self::Queued),
            "RUNNING" => Ok(Self::Running),
            "COMPLETED" => Ok(Self::Completed),
            "CANCELED" => Ok(Self::Canceled),
            "FAILED" => Ok(Self::Failed),
            "INTERRUPTED" => Ok(Self::Interrupted),
            _ => Err(rusqlite::Error::InvalidColumnType(
                0,
                "status".into(),
                rusqlite::types::Type::Text,
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchRunCursor {
    pub updated_at_ms: i64,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListResearchRunsInput {
    pub limit: u32,
    pub cursor: Option<ResearchRunCursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchRunSummary {
    pub id: String,
    pub rerun_of_id: Option<String>,
    pub query: String,
    pub status: ResearchRunStatus,
    pub requested_model: Option<String>,
    pub requested_effort: Option<String>,
    pub actual_model: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub error: Option<String>,
    pub stderr_truncated: bool,
    pub saved_tidbit_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchRunRecord {
    #[serde(flatten)]
    pub summary: ResearchRunSummary,
    pub events: Vec<Value>,
    pub final_answer: Option<Value>,
    pub citation_freshness: Vec<ResearchCitationFreshness>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchCitationFreshness {
    pub citation_number: u32,
    pub cited_revision_id: Option<String>,
    pub current_revision_id: Option<String>,
    pub has_newer_revision: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchRunPage {
    pub items: Vec<ResearchRunSummary>,
    pub next_cursor: Option<ResearchRunCursor>,
}

pub(crate) struct CreateResearchRunWrite {
    pub id: String,
    pub rerun_of_id: Option<String>,
    pub query: String,
    pub requested_model: Option<String>,
    pub requested_effort: Option<String>,
    pub now_ms: i64,
}

pub(crate) struct AppendResearchEventWrite {
    pub run_id: String,
    pub sequence: u32,
    pub kind: String,
    pub payload: Value,
    pub now_ms: i64,
}

pub(crate) struct SaveResearchAnswerWrite {
    pub run_id: String,
    pub tidbit_id: String,
    pub revision_id: String,
    pub now_ms: i64,
}

pub(super) fn create(
    connection: &mut Connection,
    write: CreateResearchRunWrite,
) -> Result<ResearchRunRecord> {
    validate_uuid_v7(&write.id, "runId")?;
    if let Some(parent) = write.rerun_of_id.as_deref() {
        validate_uuid_v7(parent, "rerunOfId")?;
        let exists = connection
            .query_row(
                "SELECT 1 FROM research_run WHERE id = ?1",
                params![parent],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(DatabaseError::NotFound {
                entity: "research run",
                id: parent.into(),
            });
        }
    }
    validate_timestamp(write.now_ms, "nowMs")?;
    let query = write.query.trim();
    if query.is_empty() || write.query.len() > 65_536 {
        return Err(DatabaseError::InvalidInput(
            "research query must contain between 1 and 65536 bytes".into(),
        ));
    }
    validate_model(write.requested_model.as_deref())?;
    validate_effort(write.requested_effort.as_deref())?;
    connection.execute(
        "INSERT INTO research_run(
            id, rerun_of_id, query, status, requested_model, requested_effort,
            created_at, updated_at
         ) VALUES(?1, ?2, ?3, 'QUEUED', ?4, ?5, ?6, ?6)",
        params![
            write.id,
            write.rerun_of_id,
            write.query,
            write.requested_model,
            write.requested_effort,
            write.now_ms
        ],
    )?;
    load(connection, &write.id)
}

pub(super) fn append_event(
    connection: &mut Connection,
    write: AppendResearchEventWrite,
) -> Result<()> {
    validate_uuid_v7(&write.run_id, "runId")?;
    validate_timestamp(write.now_ms, "nowMs")?;
    if write.sequence == 0 {
        return Err(DatabaseError::InvalidInput(
            "research event sequence must be positive".into(),
        ));
    }
    let payload_json = serde_json::to_string(&write.payload)
        .map_err(|error| DatabaseError::InvalidInput(error.to_string()))?;
    if payload_json.len() > MAX_EVENT_JSON_BYTES {
        return Err(DatabaseError::InvalidInput(
            "research event exceeded its durable byte limit".into(),
        ));
    }
    validate_event_envelope(&write)?;
    let transaction = connection.transaction()?;
    let (status, last_sequence, updated_at) = transaction
        .query_row(
            "SELECT status, last_event_sequence, updated_at
             FROM research_run
             WHERE id = ?1",
            params![write.run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "research run",
            id: write.run_id.clone(),
        })?;
    if !matches!(status.as_str(), "QUEUED" | "RUNNING") {
        return Err(DatabaseError::InvalidInput(
            "terminal research runs cannot accept more events".into(),
        ));
    }
    if write.kind != "STARTED" && status != "RUNNING" {
        return Err(DatabaseError::InvalidInput(
            "research events must begin with STARTED".into(),
        ));
    }
    if i64::from(write.sequence) != last_sequence.saturating_add(1) {
        return Err(DatabaseError::InvalidInput(
            "research event sequence is not contiguous".into(),
        ));
    }
    let event_at = write.now_ms.max(updated_at);
    transaction.execute(
        "INSERT INTO research_run_event(run_id, sequence, created_at, kind, payload_json)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![
            write.run_id,
            write.sequence,
            event_at,
            write.kind,
            payload_json
        ],
    )?;
    apply_event(&transaction, &write, &status, event_at)?;
    transaction.execute(
        "UPDATE research_run
         SET last_event_sequence = ?1, updated_at = ?2
         WHERE id = ?3",
        params![write.sequence, event_at, write.run_id],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn fail_start(
    connection: &Connection,
    run_id: &str,
    error: &str,
    now_ms: i64,
) -> Result<()> {
    validate_uuid_v7(run_id, "runId")?;
    validate_timestamp(now_ms, "nowMs")?;
    let changed = connection.execute(
        "UPDATE research_run
         SET status = 'FAILED',
             completed_at = max(created_at, ?1),
             updated_at = max(updated_at, ?1),
             error = ?2
         WHERE id = ?3 AND status IN ('QUEUED', 'RUNNING')",
        params![now_ms, truncate(error, MAX_ERROR_BYTES), run_id],
    )?;
    if changed == 0 {
        let exists = connection
            .query_row(
                "SELECT 1 FROM research_run
                 WHERE id = ?1 AND status IN ('COMPLETED', 'CANCELED', 'FAILED', 'INTERRUPTED')",
                params![run_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Ok(());
        }
        return Err(DatabaseError::InvalidInput(
            "research run could not record its launch failure".into(),
        ));
    }
    Ok(())
}

pub(super) fn interrupt_active(connection: &Connection, now_ms: i64) -> Result<u64> {
    validate_timestamp(now_ms, "nowMs")?;
    Ok(connection.execute(
        "UPDATE research_run
         SET status = 'INTERRUPTED',
             completed_at = max(created_at, ?1),
             updated_at = max(updated_at, ?1),
             error = 'Kosh restarted before this research run completed.'
         WHERE status IN ('QUEUED', 'RUNNING')",
        params![now_ms],
    )? as u64)
}

pub(super) fn list(
    connection: &Connection,
    input: ListResearchRunsInput,
) -> Result<ResearchRunPage> {
    if input.limit == 0 || input.limit > MAX_LIST_LIMIT {
        return Err(DatabaseError::InvalidInput(format!(
            "limit must be between 1 and {MAX_LIST_LIMIT}"
        )));
    }
    if let Some(cursor) = &input.cursor {
        validate_timestamp(cursor.updated_at_ms, "cursor.updatedAtMs")?;
        validate_uuid_v7(&cursor.id, "cursor.id")?;
    }
    let fetch_limit = i64::from(input.limit) + 1;
    let (cursor_time, cursor_id) = input.cursor.as_ref().map_or((None, None), |cursor| {
        (Some(cursor.updated_at_ms), Some(cursor.id.as_str()))
    });
    let mut statement = connection.prepare(
        "SELECT
            id, rerun_of_id, query, status, requested_model, requested_effort,
            actual_model, created_at, started_at, completed_at, updated_at,
            error, stderr_truncated, saved_tidbit_id
         FROM research_run
         WHERE (
            ?1 IS NULL
            OR updated_at < ?1
            OR (updated_at = ?1 AND id < ?2)
         )
         ORDER BY updated_at DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![cursor_time, cursor_id, fetch_limit],
        summary_from_row,
    )?;
    let mut items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let has_more = items.len() > input.limit as usize;
    if has_more {
        items.pop();
    }
    let next_cursor = has_more.then(|| {
        let last = items.last().expect("a paginated page has a final run");
        ResearchRunCursor {
            updated_at_ms: last.updated_at_ms,
            id: last.id.clone(),
        }
    });
    Ok(ResearchRunPage { items, next_cursor })
}

pub(super) fn load(connection: &Connection, id: &str) -> Result<ResearchRunRecord> {
    validate_uuid_v7(id, "runId")?;
    let summary = connection
        .query_row(
            "SELECT
                id, rerun_of_id, query, status, requested_model, requested_effort,
                actual_model, created_at, started_at, completed_at, updated_at,
                error, stderr_truncated, saved_tidbit_id
             FROM research_run
             WHERE id = ?1",
            params![id],
            summary_from_row,
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "research run",
            id: id.into(),
        })?;
    let final_answer_json = connection.query_row(
        "SELECT final_answer_json FROM research_run WHERE id = ?1",
        params![id],
        |row| row.get::<_, Option<String>>(0),
    )?;
    let final_answer = final_answer_json
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| DatabaseError::InvalidInput(error.to_string()))
        })
        .transpose()?;
    let mut statement = connection.prepare(
        "SELECT payload_json
         FROM research_run_event
         WHERE run_id = ?1
         ORDER BY sequence",
    )?;
    let events = statement
        .query_map(params![id], |row| row.get::<_, String>(0))?
        .map(|json| {
            json.map_err(DatabaseError::from).and_then(|json| {
                serde_json::from_str(&json)
                    .map_err(|error| DatabaseError::InvalidInput(error.to_string()))
            })
        })
        .collect::<Result<Vec<Value>>>()?;
    let citation_freshness = final_answer
        .as_ref()
        .map(|answer| citation_freshness(connection, answer))
        .transpose()?
        .unwrap_or_default();
    Ok(ResearchRunRecord {
        summary,
        events,
        final_answer,
        citation_freshness,
    })
}

pub(super) fn save_answer_as_tidbit(
    connection: &mut Connection,
    write: SaveResearchAnswerWrite,
) -> Result<Tidbit> {
    validate_uuid_v7(&write.run_id, "runId")?;
    validate_uuid_v7(&write.tidbit_id, "tidbitId")?;
    validate_uuid_v7(&write.revision_id, "revisionId")?;
    validate_timestamp(write.now_ms, "nowMs")?;
    let (query, final_answer_json, saved_tidbit_id) = connection
        .query_row(
            "SELECT query, final_answer_json, saved_tidbit_id
             FROM research_run
             WHERE id = ?1 AND status = 'COMPLETED'",
            params![write.run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            DatabaseError::InvalidInput("only completed research answers can become tidbits".into())
        })?;
    if let Some(tidbit_id) = saved_tidbit_id {
        return tidbits::load_tidbit(connection, &tidbit_id);
    }
    let answer: Value = serde_json::from_str(&final_answer_json)
        .map_err(|error| DatabaseError::InvalidInput(error.to_string()))?;
    let body_markdown = answer
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|markdown| !markdown.trim().is_empty())
        .ok_or_else(|| DatabaseError::InvalidInput("research answer has no Markdown".into()))?;
    let body_markdown = super::media::neutralize_untrusted_media_references(body_markdown);
    let title = format!(
        "Research: {}",
        truncate_chars(query.trim(), MAX_TITLE_CHARS - 10)
    );
    tidbits::create_tidbit_from_research(
        connection,
        CreateTidbitWrite {
            input: TidbitDraft {
                title: Some(title),
                body_markdown,
                sources: Vec::new(),
            },
            now_ms: write.now_ms,
            tidbit_id: write.tidbit_id,
            revision_id: write.revision_id,
            source_ids: Vec::new(),
        },
        &write.run_id,
    )
}

pub(super) fn link_saved_tidbit(
    transaction: &Transaction<'_>,
    run_id: &str,
    tidbit_id: &str,
    now_ms: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE research_run
         SET saved_tidbit_id = ?1,
             updated_at = max(updated_at, ?2)
         WHERE id = ?3
           AND status = 'COMPLETED'
           AND saved_tidbit_id IS NULL",
        params![tidbit_id, now_ms, run_id],
    )?;
    if changed != 1 {
        return Err(DatabaseError::InvalidInput(
            "research answer was already saved or is unavailable".into(),
        ));
    }
    Ok(())
}

fn apply_event(
    transaction: &Transaction<'_>,
    write: &AppendResearchEventWrite,
    status: &str,
    event_at: i64,
) -> Result<()> {
    match write.kind.as_str() {
        "STARTED" => {
            if status != "QUEUED" {
                return Err(DatabaseError::InvalidInput(
                    "research STARTED event requires a queued run".into(),
                ));
            }
            transaction.execute(
                "UPDATE research_run
                 SET status = 'RUNNING', started_at = max(created_at, ?1)
                 WHERE id = ?2",
                params![event_at, write.run_id],
            )?;
        }
        "METADATA" => {
            let model = write
                .payload
                .get("model")
                .and_then(Value::as_str)
                .map(|model| truncate(model, 128));
            transaction.execute(
                "UPDATE research_run SET actual_model = ?1 WHERE id = ?2",
                params![model, write.run_id],
            )?;
        }
        "GROUNDED_FINAL_OUTPUT" => {
            let answer = write.payload.get("answer").ok_or_else(|| {
                DatabaseError::InvalidInput("grounded event has no answer".into())
            })?;
            validate_final_answer(answer)?;
            let json = serde_json::to_string(answer)
                .map_err(|error| DatabaseError::InvalidInput(error.to_string()))?;
            let changed = transaction.execute(
                "UPDATE research_run
                 SET final_answer_json = ?1
                 WHERE id = ?2 AND final_answer_json IS NULL",
                params![json, write.run_id],
            )?;
            if changed != 1 {
                return Err(DatabaseError::InvalidInput(
                    "research run already has a grounded answer".into(),
                ));
            }
        }
        "FINISHED" => {
            let outcome = write
                .payload
                .get("outcome")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    DatabaseError::InvalidInput("finished event has no outcome".into())
                })?;
            let terminal = match outcome {
                "SUCCEEDED" => ResearchRunStatus::Completed,
                "CANCELED" | "REPLACED" => ResearchRunStatus::Canceled,
                "SHUTDOWN" => ResearchRunStatus::Interrupted,
                "FAILED" | "TIMED_OUT" => ResearchRunStatus::Failed,
                _ => {
                    return Err(DatabaseError::InvalidInput(
                        "finished event has an unknown outcome".into(),
                    ))
                }
            };
            if terminal == ResearchRunStatus::Completed {
                let has_answer = transaction.query_row(
                    "SELECT final_answer_json IS NOT NULL FROM research_run WHERE id = ?1",
                    params![write.run_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !has_answer {
                    return Err(DatabaseError::InvalidInput(
                        "successful research run has no grounded answer".into(),
                    ));
                }
            }
            let error = write
                .payload
                .get("error")
                .and_then(Value::as_str)
                .map(|error| truncate(error, MAX_ERROR_BYTES));
            let stderr_truncated = write
                .payload
                .get("stderrTruncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            transaction.execute(
                "UPDATE research_run
                 SET status = ?1,
                     completed_at = max(created_at, ?2),
                     error = ?3,
                     stderr_truncated = ?4
                 WHERE id = ?5",
                params![
                    terminal.database_value(),
                    event_at,
                    error,
                    stderr_truncated,
                    write.run_id
                ],
            )?;
        }
        "UNTRUSTED_TEXT_DELTA" => {
            let text = write
                .payload
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| DatabaseError::InvalidInput("text event has no plaintext".into()))?;
            if text.len() > 1024 * 1024 {
                return Err(DatabaseError::InvalidInput(
                    "text event exceeded its visible limit".into(),
                ));
            }
        }
        "TOOL_ACTIVITY" => {
            let tool = write
                .payload
                .get("tool")
                .and_then(Value::as_str)
                .filter(|tool| !tool.is_empty() && tool.len() <= 256)
                .ok_or_else(|| {
                    DatabaseError::InvalidInput("tool event has an invalid tool".into())
                })?;
            let phase = write.payload.get("phase").and_then(Value::as_str);
            if !matches!(phase, Some("STARTED" | "FINISHED")) || tool.contains(char::is_whitespace)
            {
                return Err(DatabaseError::InvalidInput(
                    "tool event has an invalid phase or name".into(),
                ));
            }
        }
        "UNTRUSTED_FINAL_OUTPUT" => {
            return Err(DatabaseError::InvalidInput(
                "raw final research output cannot be persisted".into(),
            ))
        }
        _ => {
            return Err(DatabaseError::InvalidInput(
                "research event kind is not durable".into(),
            ))
        }
    }
    Ok(())
}

fn validate_event_envelope(write: &AppendResearchEventWrite) -> Result<()> {
    if write.payload.get("runId").and_then(Value::as_str) != Some(write.run_id.as_str())
        || write.payload.get("sequence").and_then(Value::as_u64) != Some(u64::from(write.sequence))
        || write.payload.get("kind").and_then(Value::as_str) != Some(write.kind.as_str())
    {
        return Err(DatabaseError::InvalidInput(
            "research event envelope does not match its durable identity".into(),
        ));
    }
    Ok(())
}

fn validate_final_answer(answer: &Value) -> Result<()> {
    let answer: GroundedResearchAnswer =
        serde_json::from_value(answer.clone()).map_err(|error| {
            DatabaseError::InvalidInput(format!("grounded answer is invalid: {error}"))
        })?;
    let markdown = answer.markdown.as_str();
    if markdown.len() > 1024 * 1024 {
        return Err(DatabaseError::InvalidInput(
            "grounded answer exceeded its Markdown limit".into(),
        ));
    }
    if answer.citations.len() > 256
        || answer
            .citations
            .iter()
            .enumerate()
            .any(|(index, citation)| citation.number != index as u32 + 1)
    {
        return Err(DatabaseError::InvalidInput(
            "grounded answer citation registry is invalid".into(),
        ));
    }
    if answer.mentions.len() > 2_048 || answer.issues.len() > 256 {
        return Err(DatabaseError::InvalidInput(
            "grounded answer metadata exceeded its limit".into(),
        ));
    }
    let mut previous_end = 0;
    for mention in &answer.mentions {
        let expected = format!("【{}】", mention.citation_number);
        if mention.citation_number == 0
            || mention.citation_number as usize > answer.citations.len()
            || mention.start_byte >= mention.end_byte
            || mention.end_byte > markdown.len()
            || mention.start_byte < previous_end
            || !markdown.is_char_boundary(mention.start_byte)
            || !markdown.is_char_boundary(mention.end_byte)
            || markdown.get(mention.start_byte..mention.end_byte) != Some(expected.as_str())
        {
            return Err(DatabaseError::InvalidInput(
                "grounded answer citation mention is invalid".into(),
            ));
        }
        previous_end = mention.end_byte;
    }
    Ok(())
}

fn citation_freshness(
    connection: &Connection,
    answer: &Value,
) -> Result<Vec<ResearchCitationFreshness>> {
    let Some(citations) = answer.get("citations").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    citations
        .iter()
        .map(|citation| {
            let number = citation
                .get("number")
                .and_then(Value::as_u64)
                .and_then(|number| u32::try_from(number).ok())
                .ok_or_else(|| {
                    DatabaseError::InvalidInput("stored citation number is invalid".into())
                })?;
            let tidbit = citation
                .get("evidence")
                .and_then(|value| value.get("tidbit"));
            let cited_revision_id = tidbit
                .and_then(|value| value.get("revisionId"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let tidbit_id = tidbit
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str);
            let current_revision_id = tidbit_id
                .map(|tidbit_id| {
                    connection
                        .query_row(
                            "SELECT current_revision_id FROM tidbit WHERE id = ?1",
                            params![tidbit_id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                })
                .transpose()?
                .flatten();
            Ok(ResearchCitationFreshness {
                citation_number: number,
                has_newer_revision: cited_revision_id.is_some()
                    && current_revision_id != cited_revision_id,
                cited_revision_id,
                current_revision_id,
            })
        })
        .collect()
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<ResearchRunSummary> {
    Ok(ResearchRunSummary {
        id: row.get(0)?,
        rerun_of_id: row.get(1)?,
        query: row.get(2)?,
        status: ResearchRunStatus::parse(&row.get::<_, String>(3)?)?,
        requested_model: row.get(4)?,
        requested_effort: row.get(5)?,
        actual_model: row.get(6)?,
        created_at_ms: row.get(7)?,
        started_at_ms: row.get(8)?,
        completed_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
        error: row.get(11)?,
        stderr_truncated: row.get(12)?,
        saved_tidbit_id: row.get(13)?,
    })
}

fn validate_uuid_v7(value: &str, field: &str) -> Result<()> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| DatabaseError::InvalidInput(format!("{field} must be a UUID")))?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a canonical UUIDv7"
        )));
    }
    Ok(())
}

fn validate_timestamp(value: i64, field: &str) -> Result<()> {
    if value < 0 {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be non-negative"
        )));
    }
    Ok(())
}

fn validate_model(value: Option<&str>) -> Result<()> {
    if value.is_some_and(|model| {
        model.is_empty()
            || model.len() > 128
            || model.starts_with('-')
            || !model
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._[]".contains(character))
    }) {
        return Err(DatabaseError::InvalidInput(
            "requested model is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_effort(value: Option<&str>) -> Result<()> {
    if value.is_some_and(|effort| !matches!(effort, "low" | "medium" | "high" | "xhigh" | "max")) {
        return Err(DatabaseError::InvalidInput(
            "requested effort is invalid".into(),
        ));
    }
    Ok(())
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
