use rusqlite::params;
use tempfile::TempDir;

use super::{
    connection::{self, DatabaseKind, FileState},
    embedding_index::{
        self, InstallEmbeddingDisposition, PassageEmbeddingIndexState, JINA_V1_VEC_TABLE,
    },
    Database, DatabasePaths, LexicalSearchMode, SearchExecutionMode, SearchPassagesInput,
    SemanticSearchReadiness, Tidbit, TidbitDraft,
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
fn semantic_retrieval_falls_back_when_fresh_content_invalidates_the_active_index() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    let semantic_tidbit = create_tidbit(&client, "Automobile service intervals", 10);
    create_tidbit(&client, "Sourdough fermentation notes", 20);
    let pending = client
        .load_embedding_reconciliation_batch(32)
        .expect("pending passages");
    for passage in pending {
        let axis = usize::from(passage.content.contains("Sourdough"));
        client
            .install_passage_embedding(passage, axis_vector(axis), 30)
            .expect("install embedding");
    }
    assert!(client
        .activate_passage_embedding_index_if_complete(31)
        .expect("activate complete index"));
    assert_eq!(
        client
            .passage_embedding_search_readiness()
            .expect("ready search state"),
        SemanticSearchReadiness::Ready
    );

    let input = SearchPassagesInput {
        query: "car upkeep schedule".into(),
        mode: LexicalSearchMode::Default,
        limit: 10,
    };
    let hybrid = client
        .search_passages_with_semantics(
            input.clone(),
            Some(axis_vector(0)),
            SemanticSearchReadiness::Ready,
        )
        .expect("hybrid search");
    assert_eq!(hybrid.execution_mode, SearchExecutionMode::Hybrid);
    assert_eq!(hybrid.semantic_readiness, SemanticSearchReadiness::Ready);
    assert_eq!(
        hybrid.results[0]
            .citation
            .tidbit
            .as_ref()
            .map(|tidbit| tidbit.id.as_str()),
        Some(semantic_tidbit.id.as_str())
    );
    let punctuation = client
        .search_passages_with_semantics(
            SearchPassagesInput {
                query: "*".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
            Some(axis_vector(0)),
            SemanticSearchReadiness::Ready,
        )
        .expect("punctuation-only search");
    assert_eq!(punctuation.execution_mode, SearchExecutionMode::LexicalOnly);
    assert!(punctuation.results.is_empty());

    client
        .save_working_copy_for_test(
            semantic_tidbit.id.clone(),
            Some(semantic_tidbit.current_revision_id),
            1,
            "Updated automobile service intervals".into(),
            40,
        )
        .expect("save edit that invalidates active vectors");
    client
        .checkpoint_working_copy_for_test(semantic_tidbit.id, 1, 41, uuid_v7())
        .expect("checkpoint edit that invalidates active vectors");
    assert_eq!(
        client
            .passage_embedding_search_readiness()
            .expect("fresh search state"),
        SemanticSearchReadiness::Indexing
    );
    let fallback = client
        .search_passages_with_semantics(input, Some(axis_vector(0)), SemanticSearchReadiness::Ready)
        .expect("freshness fallback");
    assert_eq!(fallback.execution_mode, SearchExecutionMode::LexicalOnly);
    assert_eq!(
        fallback.semantic_readiness,
        SemanticSearchReadiness::Indexing
    );
    assert!(fallback.results.is_empty());
}

#[test]
fn structural_semantic_query_failure_is_quarantined_after_lexical_fallback() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    create_tidbit(&client, "Automobile service intervals", 10);
    install_all(&client, 20);
    assert!(client
        .activate_passage_embedding_index_if_complete(21)
        .expect("activate complete index"));
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    main.execute("DROP TABLE passage_embedding_vec_jina_v1", [])
        .expect("drop vector table");
    main.execute_batch(
        "CREATE TABLE passage_embedding_vec_jina_v1(
            rowid INTEGER PRIMARY KEY,
            embedding BLOB NOT NULL
         ) STRICT;",
    )
    .expect("install incompatible derived table");
    let response = super::search::search_passages_with_semantics(
        &main,
        SearchPassagesInput {
            query: "automobile".into(),
            mode: LexicalSearchMode::Default,
            limit: 10,
        },
        Some(&axis_vector(0)),
        SemanticSearchReadiness::Ready,
    )
    .expect("lexical fallback survives vector query failure");
    assert_eq!(response.execution_mode, SearchExecutionMode::LexicalOnly);
    assert_eq!(response.semantic_readiness, SemanticSearchReadiness::Failed);
    assert_eq!(response.results.len(), 1);
    assert_eq!(
        main.query_row(
            "SELECT status FROM index_state WHERE name = 'PASSAGE_EMBEDDING'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("quarantined state"),
        "FAILED"
    );
}

#[test]
fn hybrid_results_collapse_overlapping_windows_but_preserve_distinct_tidbit_sections() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    let long_prose = (0..120)
        .map(|ordinal| {
            format!(
                "Passage sentence {ordinal} explains a bounded semantic window with stable context."
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let overlapping = create_tidbit(&client, &format!("# Long note\n\n{long_prose}"), 10);
    let distinct = create_tidbit(
        &client,
        "# First\n\nThis first distinct passage belongs to its own section.\n\n# Second\n\nThis second distinct passage belongs to another section.",
        20,
    );
    let other = create_tidbit(&client, "A separate semantic passage candidate", 30);
    let pending = client
        .load_embedding_reconciliation_batch(32)
        .expect("pending passages");
    assert!(
        pending
            .iter()
            .filter(|passage| passage.content.contains("Passage sentence"))
            .count()
            >= 2
    );
    assert!(
        pending
            .iter()
            .filter(|passage| passage.content.contains("distinct passage"))
            .count()
            >= 2
    );
    for passage in pending {
        client
            .install_passage_embedding(passage, axis_vector(0), 40)
            .expect("install tied embedding");
    }
    assert!(client
        .activate_passage_embedding_index_if_complete(41)
        .expect("activate tied index"));
    let input = SearchPassagesInput {
        query: "passage".into(),
        mode: LexicalSearchMode::Default,
        limit: 10,
    };
    let first = client
        .search_passages_with_semantics(
            input.clone(),
            Some(axis_vector(0)),
            SemanticSearchReadiness::Ready,
        )
        .expect("first hybrid search");
    let second = client
        .search_passages_with_semantics(input, Some(axis_vector(0)), SemanticSearchReadiness::Ready)
        .expect("second hybrid search");
    assert_eq!(
        first
            .results
            .iter()
            .map(|result| result.passage_id.as_str())
            .collect::<Vec<_>>(),
        second
            .results
            .iter()
            .map(|result| result.passage_id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(first.results.len(), 4);
    assert_eq!(
        first
            .results
            .iter()
            .filter_map(|result| result.citation.tidbit.as_ref())
            .filter(|tidbit| tidbit.id == overlapping.id)
            .count(),
        1
    );
    assert_eq!(
        first
            .results
            .iter()
            .filter_map(|result| result.citation.tidbit.as_ref())
            .filter(|tidbit| tidbit.id == distinct.id)
            .count(),
        2
    );
    assert!(first.results.iter().any(|result| result
        .citation
        .tidbit
        .as_ref()
        .is_some_and(|tidbit| tidbit.id == other.id)));
    assert!(first
        .results
        .iter()
        .all(|result| result.passage_id == result.citation.passage_id));
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

    client
        .save_working_copy_for_test(
            original.id.clone(),
            Some(original.current_revision_id),
            1,
            "second version".into(),
            20,
        )
        .expect("save edit");
    let edited = client
        .checkpoint_working_copy_for_test(original.id, 1, 21, uuid_v7())
        .expect("checkpoint edit")
        .note
        .expect("edited note");
    assert_eq!(
        client
            .install_passage_embedding(stale, unit_vector(), 22)
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
fn missing_optional_vector_table_is_recreated_outside_required_migrations() {
    let library = TestLibrary::new();
    let database = Database::initialize(library.paths.clone()).expect("database");
    let client = database.client();
    let original = create_tidbit(&client, "lexical evidence remains", 10);
    install_all(&client, 11);
    assert!(client
        .activate_passage_embedding_index_if_complete(12)
        .expect("activate complete index"));
    database.shutdown().expect("shutdown database");

    let main =
        connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
            .expect("main writer");
    main.execute("DROP TABLE passage_embedding_vec_jina_v1", [])
        .expect("remove optional derived table");
    drop(main);

    let reopened =
        Database::initialize(library.paths.clone()).expect("authored library remains available");
    let recreated_progress = reopened
        .client()
        .passage_embedding_index_progress()
        .expect("recreated index progress");
    assert_eq!(recreated_progress.state, PassageEmbeddingIndexState::Dirty);
    assert_eq!(recreated_progress.indexed_passages, 0);
    assert!(reopened
        .client()
        .passage_embedding_index_needs_reconciliation()
        .expect("recreated table requires reconciliation"));
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
    let quarantine_error = reopened
        .client()
        .passage_embedding_index_progress()
        .expect("initial quarantine status")
        .error
        .expect("quarantine diagnosis");
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
    assert_eq!(progress.error.as_deref(), Some(quarantine_error.as_str()));
    assert_eq!(progress.indexed_passages, 0);
    assert_eq!(
        reopened
            .client()
            .passage_embedding_search_readiness()
            .expect("failed search state"),
        SemanticSearchReadiness::Failed
    );
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
fn incompatible_vector_table_contracts_are_quarantined() {
    for definition in [
        "embedding float[384] distance_metric=cosine",
        "embedding float[768] distance_metric=l2",
    ] {
        let library = TestLibrary::new();
        let database = Database::initialize(library.paths.clone()).expect("database");
        database.shutdown().expect("shutdown database");

        let main =
            connection::open_writer(&library.paths.main, DatabaseKind::Main, FileState::Existing)
                .expect("main writer");
        main.execute(&format!("DROP TABLE {JINA_V1_VEC_TABLE}"), [])
            .expect("drop shipped table");
        main.execute_batch(&format!(
            "CREATE VIRTUAL TABLE {JINA_V1_VEC_TABLE} USING vec0({definition});"
        ))
        .expect("install incompatible vector table");
        assert!(embedding_index::validate_definition(&main).is_err());
        drop(main);

        let reopened =
            Database::initialize(library.paths.clone()).expect("authored database remains usable");
        assert_eq!(
            reopened
                .client()
                .passage_embedding_index_progress()
                .expect("quarantined progress")
                .state,
            PassageEmbeddingIndexState::Failed
        );
    }
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
                body_markdown: body.into(),
            },
            now_ms,
            uuid_v7(),
            uuid_v7(),
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
    axis_vector(0)
}

fn axis_vector(axis: usize) -> Vec<f32> {
    let mut vector = vec![0.0; 768];
    vector[axis] = 1.0;
    vector
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
