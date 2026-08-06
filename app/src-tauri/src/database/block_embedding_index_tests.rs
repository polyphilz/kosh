use rusqlite::params;
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    block_embedding_index::JINA_V1_VEC_TABLE,
    embedding_index::InstallEmbeddingDisposition,
    tidbits::CreateTidbitWrite,
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    Database, DatabasePaths, MediaLimits, Tidbit, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    paths: DatabasePaths,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary block embedding library");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("block embedding database");
        Self {
            _root: root,
            paths,
            database,
        }
    }

    fn create(&self, document_json: String, markdown: String, now_ms: i64) -> Tidbit {
        self.database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    document_json,
                    body_markdown: markdown,
                    sources: Vec::new(),
                },
                now_ms,
                tidbit_id: Uuid::now_v7().to_string(),
                revision_id: Uuid::now_v7().to_string(),
                source_ids: Vec::new(),
            })
            .expect("create note")
    }
}

#[test]
fn one_guarded_vector_is_installed_for_each_nonempty_block() {
    let library = TestLibrary::new();
    let long_code = "x".repeat(40_000);
    let note = library.create(
        serde_json::json!({
            "schemaVersion": 1,
            "blocks": [
                block("heading", "heading", "Embedding context", serde_json::json!({"level": 1})),
                block("empty", "paragraph", "", serde_json::json!({})),
                block("code", "codeBlock", &long_code, serde_json::json!({"language": "text"})),
            ],
        })
        .to_string(),
        format!("# Embedding context\n\n```text\n{long_code}\n```"),
        10,
    );
    let client = library.database.client();
    let pending = client
        .load_block_embedding_reconciliation_batch(32)
        .expect("pending blocks");
    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending
            .iter()
            .map(|block| block.block_id.as_str())
            .collect::<Vec<_>>(),
        ["code", "heading"]
    );
    let code = pending
        .iter()
        .find(|block| block.block_id == "code")
        .expect("code block");
    assert!(code.content.starts_with("Embedding context\n"));
    assert!(code.content.len() <= 24 * 1024);

    for block in pending {
        assert_eq!(
            client
                .install_block_embedding(block, unit_vector(), 20)
                .expect("install block embedding"),
            InstallEmbeddingDisposition::Installed
        );
    }
    assert!(client
        .activate_block_embedding_index_if_complete(21)
        .expect("activate block index"));

    let main = library.database.open_main_read_only().expect("main reader");
    let (metadata, vectors): (i64, i64) = main
        .query_row(
            "SELECT
                (SELECT count(*) FROM block_embedding WHERE tidbit_id = ?1),
                (SELECT count(*) FROM block_embedding_vec_jina_v1)",
            params![note.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("block vector counts");
    assert_eq!((metadata, vectors), (2, 2));
}

#[test]
fn edits_empty_blocks_and_deletion_reject_stale_results_and_remove_vectors() {
    let library = TestLibrary::new();
    let first = library.create(
        document("stable", "first version"),
        "first version".into(),
        10,
    );
    let client = library.database.client();
    let stale = client
        .load_block_embedding_reconciliation_batch(1)
        .expect("first pending block")
        .pop()
        .expect("pending block");

    let second = checkpoint(
        &client,
        &first,
        2,
        document("stable", "second version"),
        "second version",
        20,
    );
    assert_eq!(
        client
            .install_block_embedding(stale, unit_vector(), 22)
            .expect("stale result handled"),
        InstallEmbeddingDisposition::Stale
    );
    let current = client
        .load_block_embedding_reconciliation_batch(1)
        .expect("edited block pending")
        .pop()
        .expect("edited block");
    assert_eq!(current.block_id, "stable");
    assert_eq!(current.content, "second version");
    client
        .install_block_embedding(current, unit_vector(), 23)
        .expect("install edited block");
    assert!(client
        .activate_block_embedding_index_if_complete(24)
        .expect("activate edited index"));

    let empty = checkpoint(&client, &second, 3, document("stable", ""), "", 30);
    assert!(client
        .load_block_embedding_reconciliation_batch(1)
        .expect("empty block reaps old vector")
        .is_empty());
    assert!(client
        .activate_block_embedding_index_if_complete(31)
        .expect("activate empty corpus"));
    assert_eq!(derived_count(&library.database, "block_embedding"), 0);
    assert_eq!(derived_count(&library.database, JINA_V1_VEC_TABLE), 0);

    client
        .delete_tidbit(
            super::DeleteTidbitInput {
                id: empty.id,
                expected_revision_id: empty.current_revision_id,
            },
            40,
        )
        .expect("delete empty note");
    assert!(client
        .load_block_embedding_reconciliation_batch(1)
        .expect("deleted note has no work")
        .is_empty());
}

#[test]
fn missing_block_vector_table_is_recreated_without_blocking_authored_notes() {
    let library = TestLibrary::new();
    let note = library.create(
        document("stable", "durable note"),
        "durable note".into(),
        10,
    );
    library.database.shutdown().expect("shutdown database");

    let main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("maintenance writer");
    main.execute(&format!("DROP TABLE {JINA_V1_VEC_TABLE}"), [])
        .expect("drop derived block vectors");
    drop(main);

    let reopened = Database::initialize(library.paths.clone()).expect("reopen authored library");
    assert_eq!(
        reopened
            .client()
            .load_tidbit(note.id)
            .expect("load authored note")
            .body_markdown,
        "durable note"
    );
    assert!(reopened
        .client()
        .block_embedding_index_needs_reconciliation()
        .expect("recreated block index is dirty"));
}

#[test]
fn corrupt_block_vector_table_is_quarantined_without_blocking_authored_notes() {
    let library = TestLibrary::new();
    let note = library.create(
        document("stable", "durable note"),
        "durable note".into(),
        10,
    );
    library.database.shutdown().expect("shutdown database");

    let main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("maintenance writer");
    main.execute(&format!("DROP TABLE {JINA_V1_VEC_TABLE}"), [])
        .expect("drop derived block vectors");
    main.execute(
        &format!("CREATE TABLE {JINA_V1_VEC_TABLE}(embedding TEXT) STRICT"),
        [],
    )
    .expect("install corrupt derived block vectors");
    drop(main);

    let reopened = Database::initialize(library.paths.clone()).expect("reopen authored library");
    assert_eq!(
        reopened
            .client()
            .load_tidbit(note.id)
            .expect("load authored note")
            .body_markdown,
        "durable note"
    );
    assert!(!reopened
        .client()
        .block_embedding_index_needs_reconciliation()
        .expect("quarantined block index stays optional"));
    let main = reopened.open_main_read_only().expect("main reader");
    let (status, error): (String, Option<String>) = main
        .query_row(
            "SELECT status, error FROM index_state WHERE name = 'BLOCK_EMBEDDING'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("block embedding quarantine state");
    assert_eq!(status, "FAILED");
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("repair is required")));
}

fn checkpoint(
    client: &super::DatabaseClient,
    note: &Tidbit,
    generation: i64,
    document_json: String,
    markdown: &str,
    now_ms: i64,
) -> Tidbit {
    client
        .save_working_copy(SaveWorkingCopyWrite {
            input: super::SaveWorkingCopyInput {
                note_id: note.id.clone(),
                base_revision_id: Some(note.current_revision_id.clone()),
                edit_generation: generation,
                document_json,
                body_markdown: markdown.into(),
                sources: Vec::new(),
            },
            now_ms,
            media_limits: MediaLimits::default(),
            allow_empty_ephemeral: false,
        })
        .expect("save edit");
    client
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: super::CheckpointWorkingCopyInput {
                note_id: note.id.clone(),
                expected_edit_generation: generation,
            },
            now_ms: now_ms + 1,
            revision_id: Uuid::now_v7().to_string(),
            source_ids: Vec::new(),
        })
        .expect("checkpoint edit")
        .note
        .expect("edited note")
}

fn document(id: &str, text: &str) -> String {
    serde_json::json!({
        "schemaVersion": 1,
        "blocks": [block(id, "paragraph", text, serde_json::json!({}))],
    })
    .to_string()
}

fn block(id: &str, block_type: &str, text: &str, props: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": block_type,
        "props": props,
        "content": if text.is_empty() {
            serde_json::Value::Array(Vec::new())
        } else {
            serde_json::json!([{"type": "text", "text": text, "styles": {}}])
        },
        "children": [],
    })
}

fn derived_count(database: &Database, table: &str) -> i64 {
    database
        .open_main_read_only()
        .expect("main reader")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("derived count")
}

fn unit_vector() -> Vec<f32> {
    let mut vector = vec![0.0; 768];
    vector[0] = 1.0;
    vector
}
