use refinery::Target;
use rusqlite::params;
use tempfile::TempDir;

use super::{
    connection::{self, DatabaseKind, FileState},
    embedding_index::{InstallEmbeddingDisposition, PassageEmbeddingIndexState},
    migrations, Database, DatabasePaths, EditTidbitInput, Tidbit, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    paths: DatabasePaths,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary embedding index library");
        let paths = DatabasePaths::new(root.path());
        Self { _root: root, paths }
    }
}

#[test]
fn current_passages_are_indexed_newest_first_and_activated_atomically() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    create_tidbit(&client, "older passage evidence", 10);
    create_tidbit(&client, "newer passage evidence", 20);
    assert!(client
        .passage_embedding_index_needs_reconciliation()
        .expect("dirty state check"));

    let first = client
        .load_embedding_reconciliation_batch(1)
        .expect("first reconciliation batch");
    assert_eq!(first.len(), 1);
    assert!(first[0].content.contains("newer passage"));
    client
        .install_passage_embedding(first[0].clone(), unit_vector(), 30)
        .expect("first embedding");
    assert!(!client
        .activate_passage_embedding_index_if_complete(31)
        .expect("incomplete activation check"));
    assert!(
        !client
            .passage_embedding_index_progress()
            .expect("partial progress")
            .active
    );

    install_all(&client, 40);
    assert!(client
        .activate_passage_embedding_index_if_complete(50)
        .expect("complete activation"));
    let progress = client
        .passage_embedding_index_progress()
        .expect("complete progress");
    assert!(progress.active);
    assert_eq!(progress.indexed_passages, progress.total_passages);
    assert_eq!(progress.state, PassageEmbeddingIndexState::Idle);
    assert!(!client
        .passage_embedding_index_needs_reconciliation()
        .expect("complete state check"));
}

#[test]
fn edits_and_deletes_invalidate_vectors_and_reject_stale_worker_results() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    let original = create_tidbit(&client, "first version", 10);
    let stale = client
        .load_embedding_reconciliation_batch(1)
        .expect("pending original")
        .pop()
        .expect("one pending passage");

    let edited = client
        .edit_tidbit(super::tidbits::EditTidbitWrite {
            input: EditTidbitInput {
                id: original.id.clone(),
                expected_revision_id: original.current_revision_id,
                title: None,
                body_markdown: "second version".into(),
                sources: Vec::new(),
            },
            now_ms: 20,
            revision_id: uuid_v7(),
            source_ids: Vec::new(),
        })
        .expect("edit tidbit");
    assert_eq!(
        client
            .install_passage_embedding(stale, unit_vector(), 21)
            .expect("stale result handled"),
        InstallEmbeddingDisposition::Stale
    );
    let pending = client
        .load_embedding_reconciliation_batch(1)
        .expect("edited passage pending");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].content.contains("second version"));

    client
        .install_passage_embedding(pending[0].clone(), unit_vector(), 22)
        .expect("edited embedding");
    client
        .activate_passage_embedding_index_if_complete(23)
        .expect("activate edited corpus");
    client
        .delete_tidbit(
            super::DeleteTidbitInput {
                id: edited.id,
                expected_revision_id: edited.current_revision_id,
            },
            30,
        )
        .expect("delete tidbit");
    let progress = client
        .passage_embedding_index_progress()
        .expect("deleted progress");
    assert_eq!(progress.indexed_passages, 0);
    assert_eq!(progress.total_passages, 0);
    assert_eq!(progress.state, PassageEmbeddingIndexState::Dirty);
}

#[test]
fn interrupted_and_partial_work_is_requeued_after_restart() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    create_tidbit(&client, "recover this passage", 10);
    let pending = client
        .load_embedding_reconciliation_batch(1)
        .expect("running batch")
        .pop()
        .expect("pending passage");
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    let vector_json = serde_json::to_string(&unit_vector()).expect("vector JSON");
    main.execute(
        "INSERT INTO passage_embedding_vec_jina_v1(rowid, embedding) VALUES(?1, ?2)",
        params![pending.passage_rowid, vector_json],
    )
    .expect("partial vector");
    drop(main);

    let reopened = Database::initialize(library.paths.clone()).expect("reopened database");
    let progress = reopened
        .client()
        .passage_embedding_index_progress()
        .expect("recovered progress");
    assert_eq!(progress.state, PassageEmbeddingIndexState::Dirty);
    assert_eq!(progress.indexed_passages, 0);
    let retried = reopened
        .client()
        .load_embedding_reconciliation_batch(1)
        .expect("requeued batch");
    assert_eq!(retried[0].passage_id, pending.passage_id);
}

#[test]
fn upgrading_from_an_active_legacy_index_waits_for_the_complete_current_corpus() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    create_tidbit(&database.client(), "one passage", 10);
    create_tidbit(&database.client(), "two passage", 20);
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    main.execute(
        "INSERT INTO passage_embedding_index(
            id, created_at, index_key, model_name, model_revision,
            model_file_sha256, dimension, distance_metric, normalized,
            index_schema_version, config_json
         )
         SELECT
            '019f547b-6200-7000-8000-000000000099', 1, 'legacy_v0',
            model_name, model_revision, model_file_sha256, dimension,
            distance_metric, normalized, index_schema_version, config_json
         FROM passage_embedding_index WHERE index_key = 'jina_v1'",
        [],
    )
    .expect("legacy index definition");
    main.execute(
        "UPDATE passage_embedding_settings
         SET active_embedding_index_id = '019f547b-6200-7000-8000-000000000099',
             updated_at = 1",
        [],
    )
    .expect("legacy active pointer");
    drop(main);

    let reopened = Database::initialize(library.paths.clone()).expect("reopened database");
    let client = reopened.client();
    let first = client
        .load_embedding_reconciliation_batch(1)
        .expect("first passage")
        .pop()
        .expect("pending passage");
    client
        .install_passage_embedding(first, unit_vector(), 30)
        .expect("partial new index");
    assert!(!client
        .activate_passage_embedding_index_if_complete(31)
        .expect("partial activation"));
    assert_eq!(
        active_index_id(&reopened),
        "019f547b-6200-7000-8000-000000000099"
    );
    install_all(&client, 40);
    assert!(client
        .activate_passage_embedding_index_if_complete(50)
        .expect("new activation"));
    assert_eq!(
        active_index_id(&reopened),
        "019f547b-6200-7000-8000-000000000002"
    );
}

#[test]
fn v5_required_migration_preserves_authored_data_without_materializing_vec() {
    let library = TestLibrary::new();
    let mut main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("main writer");
    migrations::main_runner()
        .set_target(Target::Version(4))
        .run(&mut main)
        .expect("schema through v4");
    let vec_version: String = main
        .query_row("SELECT vec_version()", [], |row| row.get(0))
        .expect("sqlite-vec version");
    assert_eq!(vec_version, "v0.1.9");
    main.execute(
        "INSERT INTO source(id, created_at, label)
         VALUES('019f547b-6200-7000-8000-000000000090', 10, 'authored source')",
        [],
    )
    .expect("authored v4 data");

    migrations::run_main(&mut main).expect("required v5 migration");
    assert_eq!(
        main.query_row(
            "SELECT label FROM source
             WHERE id = '019f547b-6200-7000-8000-000000000090'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("authored data survives"),
        "authored source"
    );
    assert_eq!(
        main.query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'passage_embedding_vec_jina_v1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("optional table count"),
        0
    );
}

#[test]
fn missing_optional_vector_table_is_recreated_outside_required_migrations() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let original = create_tidbit(&database.client(), "lexical evidence remains", 10);
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    main.execute("DROP TABLE passage_embedding_vec_jina_v1", [])
        .expect("remove optional derived table");
    drop(main);

    let reopened =
        Database::initialize(library.paths.clone()).expect("authored library remains available");
    assert_eq!(
        reopened
            .client()
            .load_tidbit(original.id)
            .expect("load authored tidbit")
            .body_markdown,
        "lexical evidence remains"
    );
    create_tidbit(&reopened.client(), "capture still works", 20);
    assert!(reopened
        .open_main_read_only()
        .expect("read connection")
        .query_row(
            "SELECT 1
             FROM sqlite_schema
             WHERE type = 'table' AND name = 'passage_embedding_vec_jina_v1'",
            [],
            |_| Ok(()),
        )
        .is_ok());
    let progress = reopened
        .client()
        .passage_embedding_index_progress()
        .expect("semantic progress remains observable");
    assert_eq!(progress.state, PassageEmbeddingIndexState::Dirty);
    assert_eq!(progress.indexed_passages, 0);
}

#[test]
fn invalid_optional_vector_table_is_quarantined_without_blocking_capture() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let original = create_tidbit(&database.client(), "authored evidence survives", 10);
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    main.execute("DROP TABLE passage_embedding_vec_jina_v1", [])
        .expect("remove vector table");
    main.execute_batch(
        "CREATE TABLE passage_embedding_vec_jina_v1(
            rowid INTEGER PRIMARY KEY,
            embedding BLOB NOT NULL
         ) STRICT;",
    )
    .expect("install incompatible derived table");
    drop(main);

    let reopened =
        Database::initialize(library.paths.clone()).expect("authored library remains available");
    assert_eq!(
        reopened
            .client()
            .load_tidbit(original.id)
            .expect("load authored tidbit")
            .body_markdown,
        "authored evidence survives"
    );
    create_tidbit(&reopened.client(), "capture remains available", 20);
    let progress = reopened
        .client()
        .passage_embedding_index_progress()
        .expect("quarantined progress");
    assert_eq!(progress.state, PassageEmbeddingIndexState::Failed);
    assert_eq!(progress.indexed_passages, 0);
    assert!(!reopened
        .client()
        .passage_embedding_index_needs_reconciliation()
        .expect("quarantined state check"));

    reopened.shutdown().expect("shutdown quarantined database");
    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("repair writer");
    main.execute("DROP TABLE passage_embedding_vec_jina_v1", [])
        .expect("remove incompatible vector table");
    drop(main);
    let repaired = Database::initialize(library.paths.clone()).expect("revalidated repair");
    assert!(repaired
        .client()
        .passage_embedding_index_needs_reconciliation()
        .expect("repair leaves index dirty"));
}

#[test]
fn orphan_vector_reaping_is_bounded_and_keeps_work_state_dirty() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    let tidbits = (0..40)
        .map(|ordinal| create_tidbit(&client, &format!("passage {ordinal}"), 10 + ordinal))
        .collect::<Vec<_>>();
    install_all(&client, 100);
    assert!(client
        .activate_passage_embedding_index_if_complete(101)
        .expect("activate indexed corpus"));

    for (ordinal, tidbit) in tidbits.into_iter().enumerate() {
        client
            .delete_tidbit(
                super::DeleteTidbitInput {
                    id: tidbit.id,
                    expected_revision_id: tidbit.current_revision_id,
                },
                200 + ordinal as i64,
            )
            .expect("delete indexed tidbit");
    }
    assert!(client
        .passage_embedding_index_needs_reconciliation()
        .expect("queued work state"));
    assert!(client
        .load_embedding_reconciliation_batch(32)
        .expect("first bounded reap")
        .is_empty());
    assert_eq!(
        derived_row_count(&database, "passage_embedding_reap_queue"),
        8
    );
    assert_eq!(
        derived_row_count(&database, "passage_embedding_vec_jina_v1"),
        8
    );
    assert!(!client
        .activate_passage_embedding_index_if_complete(300)
        .expect("activation with remaining reap work"));
    assert!(client
        .passage_embedding_index_needs_reconciliation()
        .expect("remaining queue keeps work active"));

    assert!(client
        .load_embedding_reconciliation_batch(32)
        .expect("second bounded reap")
        .is_empty());
    assert_eq!(
        derived_row_count(&database, "passage_embedding_reap_queue"),
        0
    );
    assert_eq!(
        derived_row_count(&database, "passage_embedding_vec_jina_v1"),
        0
    );
    client
        .activate_passage_embedding_index_if_complete(301)
        .expect("final activation");
    assert!(!client
        .passage_embedding_index_needs_reconciliation()
        .expect("no remaining work"));
}

fn create_tidbit(client: &super::DatabaseClient, body: &str, now_ms: i64) -> Tidbit {
    client
        .create_tidbit_with_ids(
            TidbitDraft {
                title: None,
                body_markdown: body.into(),
                sources: Vec::new(),
            },
            now_ms,
            uuid_v7(),
            uuid_v7(),
            Vec::new(),
        )
        .expect("create tidbit")
}

fn install_all(client: &super::DatabaseClient, created_at_ms: i64) {
    loop {
        let pending = client
            .load_embedding_reconciliation_batch(32)
            .expect("reconciliation batch");
        if pending.is_empty() {
            return;
        }
        for passage in pending {
            client
                .install_passage_embedding(passage, unit_vector(), created_at_ms)
                .expect("install embedding");
        }
    }
}

fn unit_vector() -> Vec<f32> {
    let mut vector = vec![0.0; 768];
    vector[0] = 1.0;
    vector
}

fn active_index_id(database: &Database) -> String {
    database
        .open_main_read_only()
        .expect("read connection")
        .query_row(
            "SELECT active_embedding_index_id
             FROM passage_embedding_settings WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .expect("active index")
}

fn derived_row_count(database: &Database, table: &str) -> i64 {
    assert!(matches!(
        table,
        "passage_embedding_reap_queue" | "passage_embedding_vec_jina_v1"
    ));
    database
        .open_main_read_only()
        .expect("read connection")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("derived row count")
}

fn uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}
