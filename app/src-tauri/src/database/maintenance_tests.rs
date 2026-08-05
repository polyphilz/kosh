use tempfile::TempDir;

use super::{
    connection::{self, DatabaseKind, FileState},
    maintenance, Database, DatabasePaths, LexicalSearchMode, SearchPassagesInput, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary maintenance library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("test database");
        Self {
            _root: root,
            database,
        }
    }
}

#[test]
fn diagnostics_and_rebuilds_preserve_authored_history_and_citations() {
    let library = TestLibrary::new();
    let client = library.database.client();
    let original = client
        .create_tidbit_with_ids(
            TidbitDraft {
                document_json: super::document::single_paragraph(
                    "Original exact citation evidence.",
                ),
                body_markdown: "Original exact citation evidence.".into(),
                sources: Vec::new(),
            },
            10,
            uuid_v7(),
            uuid_v7(),
            Vec::new(),
        )
        .expect("create tidbit");
    let original_passage = client
        .search_passages(SearchPassagesInput {
            query: "\"Original exact citation evidence\"".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("original search")
        .first()
        .expect("original result")
        .passage_id
        .clone();
    client
        .save_working_copy_for_test(
            original.id.clone(),
            Some(original.current_revision_id.clone()),
            1,
            "Updated searchable citation evidence.".into(),
            Vec::new(),
            20,
        )
        .expect("save edited working copy");
    let edited = client
        .checkpoint_working_copy_for_test(original.id.clone(), 1, 21, uuid_v7(), Vec::new())
        .expect("checkpoint edit")
        .note
        .expect("edited note");
    install_all_embeddings(&client, 30);
    client
        .activate_passage_embedding_index_if_complete(31)
        .expect("activate embeddings");

    let before = client.maintenance_snapshot().expect("before snapshot");
    assert_eq!(before.active_tidbits, 1);
    assert_eq!(before.revisions, 2);
    assert!(before.authored_passages >= 2);
    assert_eq!(before.search_documents, 1);

    assert_eq!(client.rebuild_search().expect("first search rebuild"), 1);
    assert_eq!(
        client.rebuild_search().expect("idempotent search rebuild"),
        1
    );
    let after_search = client
        .maintenance_snapshot()
        .expect("after search snapshot");
    assert_eq!(after_search.revisions, before.revisions);
    assert_eq!(after_search.authored_passages, before.authored_passages);
    assert_eq!(
        client
            .resolve_citation(original_passage)
            .expect("historical citation")
            .tidbit
            .expect("citation tidbit")
            .revision_id,
        original.current_revision_id
    );
    assert_eq!(
        client
            .search_passages(SearchPassagesInput {
                query: "\"Updated searchable citation evidence\"".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("rebuilt search")
            .first()
            .and_then(|result| result.citation.tidbit.as_ref())
            .map(|tidbit| tidbit.revision_id.as_str()),
        Some(edited.current_revision_id.as_str())
    );

    install_all_embeddings(&client, 35);
    client
        .activate_passage_embedding_index_if_complete(36)
        .expect("reactivate embeddings after search rebuild");
    assert!(
        client
            .rebuild_embeddings(40)
            .expect("first embedding rebuild")
            > 0
    );
    assert_eq!(
        client
            .rebuild_embeddings(41)
            .expect("idempotent embedding rebuild"),
        0
    );
    let embedding = client
        .passage_embedding_index_progress()
        .expect("embedding progress");
    assert!(!embedding.active);
    assert_eq!(embedding.indexed_passages, 0);
    assert_eq!(
        client
            .maintenance_snapshot()
            .expect("final snapshot")
            .revisions,
        before.revisions
    );
}

#[test]
fn empty_extraction_retry_is_idempotent() {
    let library = TestLibrary::new();
    let client = library.database.client();

    assert_eq!(
        client.retry_failed_extractions(10).expect("first retry"),
        Default::default()
    );
    assert_eq!(
        client.retry_failed_extractions(11).expect("second retry"),
        Default::default()
    );
}

#[test]
fn only_current_ocr_failures_are_reported_and_retried() {
    let root = tempfile::tempdir().expect("temporary extraction library");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("database");
    database.shutdown().expect("stop writer");
    drop(database);
    let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("maintenance writer");
    main.execute_batch(
        "BEGIN;
         INSERT INTO attachment(
            id, created_at, updated_at, deleted_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES
            (
              '019f547b-6200-7000-8000-000000009101',
              1, 1, NULL, zeroblob(32), 'scan.png', 'image/png', 10, 'IMAGE', 'FAILED'
            ),
            (
              '019f547b-6200-7000-8000-000000009301',
              1, 1, NULL, randomblob(32), 'obsolete.png', 'image/png', 10, 'IMAGE', 'FAILED'
            ),
            (
              '019f547b-6200-7000-8000-000000009401',
              1, 1, NULL, randomblob(32), 'retired.png', 'image/png', 10, 'IMAGE', 'FAILED'
            );
         INSERT INTO attachment_image(
            attachment_id, preview_sha256, preview_media_type,
            preview_byte_length, natural_width, natural_height, created_at
         ) VALUES
            (
              '019f547b-6200-7000-8000-000000009101',
              randomblob(32), 'image/webp', 5, 10, 10, 1
            ),
            (
              '019f547b-6200-7000-8000-000000009301',
              randomblob(32), 'image/webp', 5, 10, 10, 1
            ),
            (
              '019f547b-6200-7000-8000-000000009401',
              randomblob(32), 'image/webp', 5, 10, 10, 1
            );
         INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, error, created_at, completed_at
         )
         SELECT
            '019f547b-6200-7000-8000-000000009302',
            attachment.id, 'ocr', config.version, attachment.sha256,
            'FAILED', 'obsolete OCR failure', 1, 2
         FROM attachment
         JOIN attachment_extractor_config AS config ON config.extractor = 'ocr'
         WHERE attachment.id = '019f547b-6200-7000-8000-000000009301';
         INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, error, created_at, completed_at
         )
         SELECT
            '019f547b-6200-7000-8000-000000009402',
            attachment.id, 'ocr', config.version, attachment.sha256,
            'FAILED', 'retired OCR failure', 1, 2
         FROM attachment
         JOIN attachment_extractor_config AS config ON config.extractor = 'ocr'
         WHERE attachment.id = '019f547b-6200-7000-8000-000000009401';
         INSERT INTO image_ocr_queue(
            extraction_id, state, attempt_count, next_attempt_at,
            started_at, last_error, updated_at
         ) VALUES
            (
              '019f547b-6200-7000-8000-000000009302',
              'FAILED', 3, NULL, NULL, 'obsolete OCR failure', 2
            ),
            (
              '019f547b-6200-7000-8000-000000009402',
              'FAILED', 3, NULL, NULL, 'retired OCR failure', 2
            );
         UPDATE attachment
         SET deleted_at = 2, updated_at = 2
         WHERE id = '019f547b-6200-7000-8000-000000009401';
         UPDATE attachment_extractor_config
         SET version = '2', updated_at = 2
         WHERE extractor = 'ocr';
         INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, error, created_at, completed_at
         )
         SELECT
            '019f547b-6200-7000-8000-000000009102',
            attachment.id, 'ocr', config.version, attachment.sha256,
            'FAILED', 'controlled OCR failure', 3, 4
         FROM attachment
         JOIN attachment_extractor_config AS config ON config.extractor = 'ocr'
         WHERE attachment.id = '019f547b-6200-7000-8000-000000009101';
         INSERT INTO image_ocr_queue(
            extraction_id, state, attempt_count, next_attempt_at,
            started_at, last_error, updated_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000009102',
            'FAILED', 3, NULL, NULL, 'controlled OCR failure', 4
         );
         COMMIT;",
    )
    .expect("failed OCR fixture");

    assert_eq!(
        maintenance::snapshot(&main)
            .expect("queue snapshot")
            .image_ocr
            .failed,
        1
    );
    let report = maintenance::retry_failed_extractions(&mut main, 10).expect("retry OCR");
    assert_eq!(report.image_ocr_queued, 1);
    let (state, attempts, next_attempt_at): (String, i64, Option<i64>) = main
        .query_row(
            "SELECT state, attempt_count, next_attempt_at
             FROM image_ocr_queue WHERE extraction_id = ?1",
            ["019f547b-6200-7000-8000-000000009102"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("retried OCR queue");
    assert_eq!(
        (state.as_str(), attempts, next_attempt_at),
        ("PENDING", 0, Some(10))
    );
    assert_eq!(
        maintenance::retry_failed_extractions(&mut main, 11).expect("idempotent retry"),
        Default::default()
    );
    assert_eq!(
        maintenance::snapshot(&main)
            .expect("post-retry queue snapshot")
            .image_ocr
            .failed,
        0
    );
    for extraction_id in [
        "019f547b-6200-7000-8000-000000009302",
        "019f547b-6200-7000-8000-000000009402",
    ] {
        let (queue_state, extraction_state): (String, String) = main
            .query_row(
                "SELECT queue.state, extraction.status
                 FROM image_ocr_queue AS queue
                 JOIN attachment_extraction AS extraction
                   ON extraction.id = queue.extraction_id
                 WHERE extraction.id = ?1",
                [extraction_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("excluded OCR failure");
        assert_eq!(
            (queue_state.as_str(), extraction_state.as_str()),
            ("FAILED", "FAILED")
        );
    }
}

fn install_all_embeddings(client: &super::DatabaseClient, created_at_ms: i64) {
    loop {
        let pending = client
            .load_embedding_reconciliation_batch(32)
            .expect("embedding batch");
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

fn uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}
