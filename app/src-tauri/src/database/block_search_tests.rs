use rusqlite::{params, Connection};
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    tidbits::CreateTidbitWrite,
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    Database, DatabasePaths, DeleteTidbitInput, MediaLimits, RestoreTidbitInput, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary block-search library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("block-search database");
        Self {
            _root: root,
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
    let mut statement = connection
        .prepare(
            "SELECT document.tidbit_id, document.block_id
             FROM block_fts_word
             JOIN block_search_document AS document
               ON document.rowid = block_fts_word.rowid
             WHERE block_fts_word MATCH ?1
             ORDER BY document.tidbit_id, document.block_ordinal",
        )
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
