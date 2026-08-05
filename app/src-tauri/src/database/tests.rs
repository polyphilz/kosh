use std::sync::{Arc, Barrier};

use rusqlite::{Connection, OptionalExtension};
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    migrations, Database, DatabaseError, DatabasePaths, DeleteTidbitInput, LexicalSearchMode,
    RestoreTidbitInput, SearchBlocksInput, SourceDraft,
};

struct TestPair {
    _root: TempDir,
    paths: DatabasePaths,
}

impl TestPair {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary database root");
        let paths = DatabasePaths::new(root.path());
        Self { _root: root, paths }
    }
}

#[test]
fn fresh_schema_has_one_cutover_migration_and_no_retired_surfaces() {
    let main_migrations = migrations::main_runner().get_migrations().to_vec();
    let media_migrations = migrations::media_runner().get_migrations().to_vec();
    assert_eq!(main_migrations.len(), 1);
    assert_eq!(main_migrations[0].version(), 1);
    assert_eq!(main_migrations[0].name(), "note_first_schema");
    assert_eq!(media_migrations.len(), 1);
    assert_eq!(media_migrations[0].version(), 1);
    assert_eq!(media_migrations[0].name(), "media_schema");
    assert_eq!(migrations::expected_heads().main, Some(1));
    assert_eq!(migrations::expected_heads().media, Some(1));

    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("fresh database pair");
    let main = database.open_main_read_only().expect("main database");
    for retired in [
        "draft_context",
        "research_run",
        "research_citation",
        "research_run_attachment",
        "purge_authorization",
    ] {
        assert!(!table_exists(&main, retired), "retired table {retired}");
    }
    assert!(!column_exists(&main, "tidbit_revision", "title"));
    assert_eq!(
        table_columns(&main, "draft"),
        [
            "id",
            "base_revision_id",
            "edit_generation",
            "media_reservation",
            "created_at",
            "updated_at",
            "document_json",
            "body_markdown",
        ]
    );
    for guard in [
        "tidbit_revision_attachment_prevent_delete",
        "tidbit_revision_source_prevent_delete",
    ] {
        assert!(trigger_exists(&main, guard), "missing delete guard {guard}");
    }
    drop(main);
    database.shutdown().expect("close fresh database pair");
}

#[test]
fn note_lifecycle_search_delete_restore_and_restart() {
    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("database");
    let client = database.client();
    let note_id = Uuid::now_v7().to_string();
    let first_revision_id = Uuid::now_v7().to_string();
    client
        .save_working_copy_for_test(
            note_id.clone(),
            None,
            1,
            "# Arrays\n\nExact citrine evidence lives here.".into(),
            vec![SourceDraft {
                label: Some("Reference".into()),
                url: Some("https://example.com/reference".into()),
            }],
            10,
        )
        .expect("save new note");
    let created = client
        .checkpoint_working_copy_for_test(
            note_id.clone(),
            1,
            11,
            first_revision_id.clone(),
            vec![Uuid::now_v7().to_string()],
        )
        .expect("checkpoint new note")
        .note
        .expect("created note");
    assert_eq!(created.display_title, "Arrays");

    let first_result = client
        .search_blocks(SearchBlocksInput {
            query: "citrine".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search first revision")
        .pop()
        .expect("first search result");
    assert_eq!(first_result.note_id, created.id);
    assert_eq!(first_result.block_id, "native-fixture-block");

    client
        .save_working_copy_for_test(
            note_id.clone(),
            Some(first_revision_id.clone()),
            2,
            "# Arrays\n\nExact amber evidence replaced the earlier wording.".into(),
            Vec::new(),
            20,
        )
        .expect("save edited note");
    let edited = client
        .checkpoint_working_copy_for_test(
            note_id.clone(),
            2,
            21,
            Uuid::now_v7().to_string(),
            Vec::new(),
        )
        .expect("checkpoint edited note")
        .note
        .expect("edited note");
    assert!(client
        .search_blocks(SearchBlocksInput {
            query: "citrine".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search removed wording")
        .is_empty());
    assert!(client
        .search_blocks(SearchBlocksInput {
            query: "amber".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search current revision")
        .iter()
        .any(|result| result.note_id == note_id));

    let deleted = client
        .delete_tidbit(
            DeleteTidbitInput {
                id: note_id.clone(),
                expected_revision_id: edited.current_revision_id.clone(),
            },
            30,
        )
        .expect("delete note");
    assert!(deleted.deleted_at_ms.is_some());
    assert!(client
        .search_blocks(SearchBlocksInput {
            query: "amber".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search deleted note")
        .is_empty());
    client
        .restore_tidbit(
            RestoreTidbitInput {
                id: note_id.clone(),
                expected_revision_id: edited.current_revision_id,
            },
            31,
        )
        .expect("restore note");
    database.shutdown().expect("close database");

    let reopened = Database::initialize(pair.paths.clone()).expect("reopen database");
    assert_eq!(
        reopened
            .client()
            .load_tidbit(note_id)
            .expect("restored note after restart")
            .body_markdown,
        "# Arrays\n\nExact amber evidence replaced the earlier wording."
    );
    reopened
        .client()
        .full_integrity_check()
        .expect("integrity after restart");
}

#[test]
fn incompatible_migration_history_is_rejected_without_resetting_the_profile() {
    let pair = TestPair::new();
    Database::initialize(pair.paths.clone())
        .expect("database")
        .shutdown()
        .expect("close database");
    let connection = Connection::open(&pair.paths.main).expect("tamper migration history");
    connection
        .execute(
            "UPDATE refinery_schema_history SET checksum = '0' WHERE version = 1",
            [],
        )
        .expect("install divergent checksum");
    drop(connection);

    let error = Database::initialize(pair.paths.clone()).expect_err("divergent history refused");
    assert!(matches!(
        error,
        DatabaseError::IncompatibleMigrationHistory { kind: "main", .. }
    ));
    let connection = Connection::open(&pair.paths.main).expect("inspect refused profile");
    assert_eq!(
        connection
            .query_row(
                "SELECT checksum FROM refinery_schema_history WHERE version = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("retained divergent checksum"),
        "0"
    );
    assert!(table_exists(&connection, "tidbit"));
}

#[test]
fn database_pair_allows_only_one_active_writer() {
    let pair = TestPair::new();
    let first = Database::initialize(pair.paths.clone()).expect("first writer");
    let error = Database::initialize(pair.paths.clone()).expect_err("second writer refused");
    assert!(matches!(error, DatabaseError::DatabaseInUse { .. }));
    first.shutdown().expect("release writer");
    drop(Database::initialize(pair.paths.clone()).expect("replacement writer"));
}

#[test]
fn concurrent_shutdown_is_idempotent() {
    let pair = TestPair::new();
    let database = Arc::new(Database::initialize(pair.paths.clone()).expect("database"));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let database = Arc::clone(&database);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                database.shutdown()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        worker.join().expect("shutdown thread").expect("shutdown");
    }
    drop(database);
    drop(Database::initialize(pair.paths.clone()).expect("replacement writer"));
}

fn table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .expect("inspect table")
        .is_some()
}

fn trigger_exists(connection: &Connection, trigger: &str) -> bool {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
            [trigger],
            |_| Ok(()),
        )
        .optional()
        .expect("inspect trigger")
        .is_some()
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> bool {
    table_columns(connection, table)
        .iter()
        .any(|candidate| candidate == column)
}

fn table_columns(connection: &Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare table columns");
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect table columns")
}
