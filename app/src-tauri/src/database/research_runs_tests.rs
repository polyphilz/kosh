use serde_json::{json, Value};
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    passages, research_runs::*, tidbits, Database, DatabasePaths, DeleteTidbitInput,
    EditTidbitInput, TidbitDraft,
};

fn id() -> String {
    Uuid::now_v7().to_string()
}

fn open() -> (TempDir, Database) {
    let root = TempDir::new().expect("temporary database directory");
    let database =
        Database::initialize(DatabasePaths::new(root.path())).expect("initialize database");
    (root, database)
}

fn create_run(database: &Database, run_id: &str, rerun_of_id: Option<String>) {
    database
        .client()
        .create_research_run(CreateResearchRunWrite {
            id: run_id.into(),
            rerun_of_id,
            query: "What should I remember?".into(),
            requested_model: Some("sonnet".into()),
            requested_effort: Some("high".into()),
            now_ms: 100,
        })
        .expect("create research run");
}

fn event(run_id: &str, sequence: u32, kind: &str, fields: Value) -> Value {
    let mut object = fields.as_object().cloned().expect("event fields object");
    object.insert("runId".into(), json!(run_id));
    object.insert("sequence".into(), json!(sequence));
    object.insert("kind".into(), json!(kind));
    Value::Object(object)
}

fn append(database: &Database, run_id: &str, sequence: u32, kind: &str, fields: Value) {
    database
        .client()
        .append_research_event(AppendResearchEventWrite {
            run_id: run_id.into(),
            sequence,
            kind: kind.into(),
            payload: event(run_id, sequence, kind, fields),
            now_ms: 100 + i64::from(sequence),
        })
        .expect("append research event");
}

fn create_evidence(database: &Database) -> (super::Tidbit, super::CitationResolution) {
    let tidbit_id = id();
    let revision_id = id();
    let tidbit = database
        .client()
        .create_tidbit(tidbits::CreateTidbitWrite {
            input: TidbitDraft {
                title: Some("Evidence".into()),
                body_markdown: "The durable fact is forty-two.".into(),
                sources: Vec::new(),
            },
            now_ms: 90,
            tidbit_id,
            revision_id,
            source_ids: Vec::new(),
        })
        .expect("create evidence tidbit");
    let connection = database.open_main_read_only().expect("read-only database");
    let passage_id: String = connection
        .query_row(
            "SELECT id FROM passage WHERE tidbit_revision_id = ?1 ORDER BY ordinal LIMIT 1",
            [&tidbit.current_revision_id],
            |row| row.get(0),
        )
        .expect("evidence passage");
    let evidence = passages::resolve_citation(&connection, &passage_id).expect("resolve evidence");
    (tidbit, evidence)
}

fn grounded_answer(evidence: &super::CitationResolution) -> Value {
    let markdown = "The durable fact is forty-two.【1】";
    let start = markdown.find('【').expect("marker");
    json!({
        "markdown": markdown,
        "citations": [{
            "number": 1,
            "label": "Evidence",
            "evidenceKind": "AUTHORED_TIDBIT",
            "evidence": evidence,
        }],
        "mentions": [{
            "citationNumber": 1,
            "startByte": start,
            "endByte": markdown.len(),
        }],
        "issues": [],
    })
}

#[test]
fn durable_run_preserves_grounded_snapshot_and_reports_newer_revisions() {
    let (_root, database) = open();
    let (tidbit, evidence) = create_evidence(&database);
    let run_id = id();
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    append(
        &database,
        &run_id,
        2,
        "METADATA",
        json!({"model": "claude-sonnet"}),
    );
    append(
        &database,
        &run_id,
        3,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": grounded_answer(&evidence)}),
    );
    append(
        &database,
        &run_id,
        4,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
    );

    let completed = database
        .client()
        .load_research_run(run_id.clone())
        .expect("load completed run");
    assert_eq!(completed.summary.status, ResearchRunStatus::Completed);
    assert_eq!(
        completed.summary.actual_model.as_deref(),
        Some("claude-sonnet")
    );
    assert_eq!(completed.events.len(), 4);
    assert_eq!(completed.final_answer, Some(grounded_answer(&evidence)));
    assert!(!completed.citation_freshness[0].has_newer_revision);

    database
        .client()
        .edit_tidbit(tidbits::EditTidbitWrite {
            input: EditTidbitInput {
                id: tidbit.id.clone(),
                expected_revision_id: tidbit.current_revision_id,
                title: Some("Evidence".into()),
                body_markdown: "The revised durable fact is forty-three.".into(),
                sources: Vec::new(),
            },
            now_ms: 200,
            revision_id: id(),
            source_ids: Vec::new(),
        })
        .expect("edit cited tidbit");
    let historical = database
        .client()
        .load_research_run(run_id)
        .expect("load historical run");
    assert_eq!(historical.final_answer, Some(grounded_answer(&evidence)));
    assert!(historical.citation_freshness[0].has_newer_revision);
    assert!(historical.citation_freshness[0].is_historical);
}

#[test]
fn deleted_citation_is_historical_without_a_newer_revision() {
    let (_root, database) = open();
    let (tidbit, evidence) = create_evidence(&database);
    let run_id = id();
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    append(
        &database,
        &run_id,
        2,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": grounded_answer(&evidence)}),
    );
    append(
        &database,
        &run_id,
        3,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
    );
    database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: tidbit.id,
                expected_revision_id: tidbit.current_revision_id.clone(),
            },
            200,
        )
        .expect("delete cited tidbit");

    let record = database
        .client()
        .load_research_run(run_id)
        .expect("load run with deleted citation");
    let freshness = &record.citation_freshness[0];
    assert_eq!(
        freshness.current_revision_id.as_deref(),
        Some(tidbit.current_revision_id.as_str())
    );
    assert!(!freshness.has_newer_revision);
    assert!(freshness.tidbit_deleted);
    assert!(freshness.is_historical);
}

#[test]
fn restart_interrupts_active_runs_and_reruns_keep_lineage() {
    let (root, database) = open();
    let original = id();
    create_run(&database, &original, None);
    append(&database, &original, 1, "STARTED", json!({}));
    database.shutdown().expect("shutdown database");
    drop(database);

    let reopened = Database::initialize(DatabasePaths::new(root.path()))
        .expect("reopen database after restart");
    assert_eq!(
        reopened
            .client()
            .interrupt_active_research_runs(500)
            .expect("interrupt active runs"),
        1
    );
    let interrupted = reopened
        .client()
        .load_research_run(original.clone())
        .expect("load interrupted run");
    assert_eq!(interrupted.summary.status, ResearchRunStatus::Interrupted);

    let retry = id();
    create_run(&reopened, &retry, Some(original.clone()));
    let retried = reopened
        .client()
        .load_research_run(retry)
        .expect("load rerun");
    assert_eq!(
        retried.summary.rerun_of_id.as_deref(),
        Some(original.as_str())
    );
}

#[test]
fn raw_finals_and_noncontiguous_events_never_partially_persist() {
    let (_root, database) = open();
    let run_id = id();
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    let raw = database
        .client()
        .append_research_event(AppendResearchEventWrite {
            run_id: run_id.clone(),
            sequence: 2,
            kind: "UNTRUSTED_FINAL_OUTPUT".into(),
            payload: event(
                &run_id,
                2,
                "UNTRUSTED_FINAL_OUTPUT",
                json!({"text": "do not persist me"}),
            ),
            now_ms: 102,
        });
    assert!(raw.is_err());
    let gap = database
        .client()
        .append_research_event(AppendResearchEventWrite {
            run_id: run_id.clone(),
            sequence: 3,
            kind: "TOOL_ACTIVITY".into(),
            payload: event(
                &run_id,
                3,
                "TOOL_ACTIVITY",
                json!({"tool": "search", "phase": "STARTED"}),
            ),
            now_ms: 103,
        });
    assert!(gap.is_err());
    let record = database
        .client()
        .load_research_run(run_id)
        .expect("load intact run");
    assert_eq!(record.summary.status, ResearchRunStatus::Running);
    assert_eq!(record.events.len(), 1);
}

#[test]
fn completed_answer_can_be_saved_once_as_a_normal_tidbit() {
    let (_root, database) = open();
    let (_source, evidence) = create_evidence(&database);
    let run_id = id();
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    append(
        &database,
        &run_id,
        2,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": grounded_answer(&evidence)}),
    );
    append(
        &database,
        &run_id,
        3,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
    );
    let saved = database
        .client()
        .save_research_answer_as_tidbit(SaveResearchAnswerWrite {
            run_id: run_id.clone(),
            tidbit_id: id(),
            revision_id: id(),
            now_ms: 200,
        })
        .expect("save answer");
    assert_eq!(saved.body_markdown, "The durable fact is forty-two.【1】");
    let second = database
        .client()
        .save_research_answer_as_tidbit(SaveResearchAnswerWrite {
            run_id: run_id.clone(),
            tidbit_id: id(),
            revision_id: id(),
            now_ms: 201,
        })
        .expect("saving twice is idempotent");
    assert_eq!(second.id, saved.id);
    assert_eq!(
        database
            .client()
            .load_research_run(run_id)
            .expect("load saved run")
            .summary
            .saved_tidbit_id
            .as_deref(),
        Some(saved.id.as_str())
    );
}

#[test]
fn saved_research_answer_neutralizes_every_local_media_capability() {
    let (_root, database) = open();
    let (_source, evidence) = create_evidence(&database);
    let run_id = id();
    let attachment_id = id();
    let mut answer = grounded_answer(&evidence);
    answer["markdown"] = json!(format!(
        "The durable fact is forty-two.【1】\n\n\
         {{{{kosh:image:{attachment_id};width=70%}}}}\n\n\
         {{{{kosh:pdf:{attachment_id}}}}}\n\n\
         {{{{kosh:attachment:{attachment_id}}}}}\n\n\
         ![direct](kosh-media://localhost/attachment/{attachment_id} \"kosh-image:70:\")"
    ));
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    append(
        &database,
        &run_id,
        2,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": answer}),
    );
    append(
        &database,
        &run_id,
        3,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
    );

    let saved = database
        .client()
        .save_research_answer_as_tidbit(SaveResearchAnswerWrite {
            run_id,
            tidbit_id: id(),
            revision_id: id(),
            now_ms: 200,
        })
        .expect("save sanitized research answer");
    assert!(!saved.body_markdown.contains("{{kosh:image:"));
    assert!(!saved.body_markdown.contains("{{kosh:pdf:"));
    assert!(!saved.body_markdown.contains("{{kosh:attachment:"));
    assert!(!saved
        .body_markdown
        .contains("kosh-media://localhost/attachment/"));
    assert!(saved.body_markdown.contains("{{kosh-reference:image:"));
    assert!(saved.body_markdown.contains("{{kosh-reference:pdf:"));
    assert!(saved.body_markdown.contains("{{kosh-reference:attachment:"));
    assert!(saved
        .body_markdown
        .contains("kosh-reference://localhost/attachment/"));
    assert!(super::media::referenced_attachments(&saved.body_markdown).is_empty());
}

#[test]
fn saved_research_answer_rejects_entity_encoded_media_capabilities() {
    let (_root, database) = open();
    let (_source, evidence) = create_evidence(&database);
    let run_id = id();
    let attachment_id = id();
    let mut answer = grounded_answer(&evidence);
    answer["markdown"] = json!(format!(
        "The durable fact is forty-two.【1】\n\n\
         {{{{kosh&#58;image:{attachment_id};width=70%}}}}"
    ));
    create_run(&database, &run_id, None);
    append(&database, &run_id, 1, "STARTED", json!({}));
    append(
        &database,
        &run_id,
        2,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": answer}),
    );
    append(
        &database,
        &run_id,
        3,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
    );

    let error = database
        .client()
        .save_research_answer_as_tidbit(SaveResearchAnswerWrite {
            run_id,
            tidbit_id: id(),
            revision_id: id(),
            now_ms: 200,
        })
        .expect_err("encoded media capability must not enter authored content");
    assert!(error.to_string().contains("encoded local media capability"));
}
