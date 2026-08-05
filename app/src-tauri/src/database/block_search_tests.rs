use rusqlite::{params, Connection};
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    block_query, block_search,
    connection::{self, DatabaseKind, FileState},
    tidbits::CreateTidbitWrite,
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    Database, DatabasePaths, DeleteTidbitInput, LexicalBenchmarkAttachmentWrite, MediaLimits,
    RestoreTidbitInput, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    paths: DatabasePaths,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary block-search library");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("block-search database");
        Self {
            _root: root,
            paths,
            database,
        }
    }

    fn connection(&self) -> Connection {
        self.database
            .open_main_read_only()
            .expect("block-search connection")
    }
}

#[test]
fn per_note_refresh_preserves_global_block_fts_rebuild_state() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let edited_note_id = Uuid::now_v7().to_string();
    let edited_revision_id = Uuid::now_v7().to_string();
    let edited = client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: document("amber"),
                body_markdown: "# Vectors\n\nAn amber invariant.".into(),
                sources: Vec::new(),
            },
            now_ms: 10,
            tidbit_id: edited_note_id.clone(),
            revision_id: edited_revision_id,
            source_ids: Vec::new(),
        })
        .expect("create edited note");
    let missing_note_id = Uuid::now_v7().to_string();
    client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: document("beryl"),
                body_markdown: "# Vectors\n\nA beryl invariant.".into(),
                sources: Vec::new(),
            },
            now_ms: 11,
            tidbit_id: missing_note_id.clone(),
            revision_id: Uuid::now_v7().to_string(),
            source_ids: Vec::new(),
        })
        .expect("create missing-index note");

    let writer =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("open block-search writer");
    writer
        .execute(
            "INSERT INTO block_fts_word(
                block_fts_word, rowid, heading_context, body, attachment_names, extracted_text
             )
             SELECT
                'delete', rowid,
                kosh_search_normalize(heading_context),
                kosh_search_normalize(body),
                kosh_search_normalize(attachment_names),
                kosh_search_normalize(extracted_text)
             FROM block_search_document
             WHERE tidbit_id = ?1",
            params![&missing_note_id],
        )
        .expect("remove one note's block word index rows");
    writer
        .execute(
            "UPDATE index_state
             SET version = 'block-lexical-v0', status = 'DIRTY', error = 'interrupted rebuild'
             WHERE name = 'BLOCK_FTS'",
            [],
        )
        .expect("mark block FTS stale");
    drop(writer);

    client
        .save_working_copy(SaveWorkingCopyWrite {
            input: super::SaveWorkingCopyInput {
                note_id: edited_note_id.clone(),
                base_revision_id: Some(edited.current_revision_id),
                edit_generation: 2,
                document_json: document("copper"),
                body_markdown: "# Vectors\n\nA copper invariant.".into(),
                sources: Vec::new(),
            },
            now_ms: 20,
            media_limits: MediaLimits::default(),
            allow_empty_ephemeral: false,
        })
        .expect("save routine note edit");
    client
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: super::CheckpointWorkingCopyInput {
                note_id: edited_note_id.clone(),
                expected_edit_generation: 2,
            },
            now_ms: 21,
            revision_id: Uuid::now_v7().to_string(),
            source_ids: Vec::new(),
        })
        .expect("checkpoint routine note edit");

    assert_eq!(
        block_fts_state(&library.connection()),
        (
            "block-lexical-v0".into(),
            "DIRTY".into(),
            Some("interrupted rebuild".into()),
        )
    );
    assert!(search_blocks(&library.connection(), "beryl").is_empty());
    assert_eq!(
        search_blocks(&library.connection(), "copper"),
        [(edited_note_id, "fact".into())]
    );

    assert!(client.reconcile_fts().expect("reconcile stale block FTS"));
    assert_eq!(
        block_fts_state(&library.connection()),
        (block_search::FTS_VERSION.into(), "IDLE".into(), None)
    );
    assert_eq!(
        search_blocks(&library.connection(), "beryl"),
        [(missing_note_id, "fact".into())]
    );
}

#[test]
fn current_block_fts_replaces_edits_and_follows_note_lifecycle() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let note_id = Uuid::now_v7().to_string();
    let first_revision_id = Uuid::now_v7().to_string();
    let first_document = document("citrine");
    let created = client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: first_document,
                body_markdown: "# Vectors\n\nA citrine invariant.".into(),
                sources: Vec::new(),
            },
            now_ms: 10,
            tidbit_id: note_id.clone(),
            revision_id: first_revision_id.clone(),
            source_ids: Vec::new(),
        })
        .expect("create note");

    let first_hash = block_hash(&library.connection(), &note_id, "fact");
    assert_eq!(
        search_blocks(&library.connection(), "citrine"),
        [(note_id.clone(), "fact".into())]
    );
    assert_eq!(document_count(&library.connection(), &note_id), 2);
    assert_eq!(
        search_blocks_in(&library.connection(), "block_fts_trigram", "trine"),
        [(note_id.clone(), "fact".into())]
    );
    assert_eq!(
        search_blocks_in(
            &library.connection(),
            "block_fts_short",
            &block_query::short_grams("ci"),
        ),
        [(note_id.clone(), "fact".into())]
    );

    client
        .save_working_copy(SaveWorkingCopyWrite {
            input: super::SaveWorkingCopyInput {
                note_id: note_id.clone(),
                base_revision_id: Some(created.current_revision_id.clone()),
                edit_generation: 2,
                document_json: document("amber"),
                body_markdown: "# Vectors\n\nAn amber invariant.".into(),
                sources: Vec::new(),
            },
            now_ms: 20,
            media_limits: MediaLimits::default(),
            allow_empty_ephemeral: false,
        })
        .expect("save edit");
    let second_revision_id = Uuid::now_v7().to_string();
    let edited = client
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: super::CheckpointWorkingCopyInput {
                note_id: note_id.clone(),
                expected_edit_generation: 2,
            },
            now_ms: 21,
            revision_id: second_revision_id,
            source_ids: Vec::new(),
        })
        .expect("checkpoint edit")
        .note
        .expect("edited note");

    assert!(search_blocks(&library.connection(), "citrine").is_empty());
    assert_eq!(
        search_blocks(&library.connection(), "amber"),
        [(note_id.clone(), "fact".into())]
    );
    assert_ne!(
        first_hash,
        block_hash(&library.connection(), &note_id, "fact")
    );

    client
        .delete_tidbit(
            DeleteTidbitInput {
                id: note_id.clone(),
                expected_revision_id: edited.current_revision_id.clone(),
            },
            30,
        )
        .expect("delete note");
    assert!(search_blocks(&library.connection(), "amber").is_empty());
    assert_eq!(document_count(&library.connection(), &note_id), 0);

    client
        .restore_tidbit(
            RestoreTidbitInput {
                id: note_id.clone(),
                expected_revision_id: edited.current_revision_id,
            },
            40,
        )
        .expect("restore note");
    assert_eq!(
        search_blocks(&library.connection(), "amber"),
        [(note_id, "fact".into())]
    );
}

#[test]
fn strict_refresh_rejects_drifted_attachment_ownership_but_rebuild_skips_it() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let valid_note_id = Uuid::now_v7().to_string();
    client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: document("healthy"),
                body_markdown: "A healthy note.".into(),
                sources: Vec::new(),
            },
            now_ms: 10,
            tidbit_id: valid_note_id.clone(),
            revision_id: Uuid::now_v7().to_string(),
            source_ids: Vec::new(),
        })
        .expect("create valid note");
    let drifted_note_id = Uuid::now_v7().to_string();
    let drifted_revision_id = Uuid::now_v7().to_string();
    client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: document("before-drift"),
                body_markdown: "A note before drift.".into(),
                sources: Vec::new(),
            },
            now_ms: 11,
            tidbit_id: drifted_note_id.clone(),
            revision_id: drifted_revision_id.clone(),
            source_ids: Vec::new(),
        })
        .expect("create note that will drift");

    let mut writer =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("open block-search writer");
    writer
        .execute("DROP TRIGGER tidbit_revision_prevent_update", [])
        .expect("allow corrupt restore fixture");
    let missing_attachment_id = Uuid::now_v7().to_string();
    let drifted_document = serde_json::json!({
        "schemaVersion": 1,
        "blocks": [{
            "id": "missing-file",
            "type": "koshFileAttachment",
            "props": {"attachmentId": missing_attachment_id, "caption": "missing evidence"},
            "children": [],
        }],
    })
    .to_string();
    writer
        .execute(
            "UPDATE tidbit_revision SET document_json = ?1 WHERE id = ?2",
            params![drifted_document, drifted_revision_id],
        )
        .expect("install drifted document fixture");

    let transaction = writer.transaction().expect("strict refresh transaction");
    let error = block_search::replace_tidbit_documents(&transaction, &drifted_note_id)
        .expect_err("routine refresh must reject drifted ownership");
    assert!(error.to_string().contains("does not own attachment"));
    transaction.rollback().expect("roll back strict refresh");

    let transaction = writer.transaction().expect("rebuild transaction");
    block_search::rebuild_documents(&transaction).expect("rebuild skips only drifted block");
    transaction.commit().expect("commit tolerant rebuild");
    assert_eq!(
        search_blocks(&library.connection(), "healthy"),
        [(valid_note_id, "fact".into())]
    );
    assert_eq!(document_count(&library.connection(), &drifted_note_id), 0);
}

#[test]
fn failed_block_fts_reconciliation_stays_dirty_and_reports_the_index() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let writer =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("open block-search writer");
    writer
        .execute("DROP TABLE block_fts_short", [])
        .expect("remove one block FTS index");
    writer
        .execute(
            "UPDATE index_state SET status = 'DIRTY' WHERE name = 'BLOCK_FTS'",
            [],
        )
        .expect("mark block FTS dirty");
    drop(writer);

    assert!(!client
        .reconcile_fts()
        .expect("failed reconciliation report"));
    let (version, status, error) = block_fts_state(&library.connection());
    assert_eq!(version, block_search::FTS_VERSION);
    assert_eq!(status, "DIRTY");
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("block_fts_short")));
}

#[test]
fn lexical_benchmark_attachments_are_present_in_block_search() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let note_id = Uuid::now_v7().to_string();
    let revision_id = Uuid::now_v7().to_string();
    client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                document_json: document("benchmark"),
                body_markdown: "A benchmark note.".into(),
                sources: Vec::new(),
            },
            now_ms: 10,
            tidbit_id: note_id.clone(),
            revision_id: revision_id.clone(),
            source_ids: Vec::new(),
        })
        .expect("create benchmark note");
    client
        .install_lexical_benchmark_attachments(vec![LexicalBenchmarkAttachmentWrite {
            revision_id,
            attachment_id: Uuid::now_v7().to_string(),
            created_at_ms: 11,
            display_filename: "benchmark-evidence.bin".into(),
            media_type: "application/octet-stream".into(),
            byte_length: 128,
        }])
        .expect("install benchmark attachment");

    let hits = search_blocks(&library.connection(), "evidence");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, note_id);
    assert!(hits[0].1.starts_with("benchmark-attachment-"));
}

fn document(word: &str) -> String {
    serde_json::json!({
        "schemaVersion": 1,
        "blocks": [
            {
                "id": "heading",
                "type": "heading",
                "props": {"level": 1},
                "content": [{"type": "text", "text": "Vectors", "styles": {}}],
                "children": [],
            },
            {
                "id": "fact",
                "type": "paragraph",
                "props": {},
                "content": [{"type": "text", "text": format!("A {word} invariant."), "styles": {}}],
                "children": [],
            },
            {
                "id": "empty",
                "type": "paragraph",
                "props": {},
                "content": [],
                "children": [],
            },
        ],
    })
    .to_string()
}

fn search_blocks(connection: &Connection, query: &str) -> Vec<(String, String)> {
    search_blocks_in(connection, "block_fts_word", query)
}

fn search_blocks_in(connection: &Connection, index: &str, query: &str) -> Vec<(String, String)> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT document.tidbit_id, document.block_id
             FROM {index}
             JOIN block_search_document AS document
               ON document.rowid = {index}.rowid
             WHERE {index} MATCH ?1
             ORDER BY document.tidbit_id, document.block_ordinal"
        ))
        .expect("block FTS query");
    statement
        .query_map(params![query], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("block FTS rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("block FTS results")
}

fn block_hash(connection: &Connection, note_id: &str, block_id: &str) -> Vec<u8> {
    connection
        .query_row(
            "SELECT content_hash
             FROM block_search_document
             WHERE tidbit_id = ?1 AND block_id = ?2",
            params![note_id, block_id],
            |row| row.get(0),
        )
        .expect("block content hash")
}

fn document_count(connection: &Connection, note_id: &str) -> i64 {
    connection
        .query_row(
            "SELECT count(*) FROM block_search_document WHERE tidbit_id = ?1",
            params![note_id],
            |row| row.get(0),
        )
        .expect("block document count")
}

fn block_fts_state(connection: &Connection) -> (String, String, Option<String>) {
    connection
        .query_row(
            "SELECT version, status, error FROM index_state WHERE name = 'BLOCK_FTS'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("BLOCK_FTS state")
}
