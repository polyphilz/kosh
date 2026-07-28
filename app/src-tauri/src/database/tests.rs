use std::sync::{Arc, Barrier};

use refinery::Target;
use rusqlite::{params, Connection};
use tempfile::TempDir;

use super::{
    connection::{self, DatabaseKind, FileState},
    migrations, Database, DatabaseError, DatabasePaths, LexicalSearchMode, SearchPassagesInput,
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
fn initial_migration_checksums_remain_stable() {
    let main_v1 = migrations::main_runner()
        .get_migrations()
        .iter()
        .find(|migration| migration.version() == 1)
        .expect("main V1 migration")
        .checksum();
    let media_v1 = migrations::media_runner()
        .get_migrations()
        .iter()
        .find(|migration| migration.version() == 1)
        .expect("media V1 migration")
        .checksum();
    assert_eq!(main_v1, 1_893_190_742_697_353_014);
    assert_eq!(media_v1, 14_137_568_078_953_250_380);
}

#[test]
fn fresh_pair_reopens_with_durable_pragmas_and_schema() {
    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("fresh pair");
    let diagnostics = database.client().diagnostics().expect("diagnostics");

    assert_eq!(diagnostics.migration_heads, migrations::expected_heads());
    assert_eq!(diagnostics.main_journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(diagnostics.media_journal_mode.to_ascii_lowercase(), "wal");
    assert!(diagnostics.main_foreign_keys);
    assert!(diagnostics.media_foreign_keys);
    drop(database);

    let reopened = Database::initialize(pair.paths.clone()).expect("reopened pair");
    assert_eq!(
        reopened
            .client()
            .diagnostics()
            .expect("reopened diagnostics")
            .migration_heads,
        migrations::expected_heads()
    );
}

#[test]
fn full_integrity_scan_is_an_explicit_maintenance_operation() {
    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("fresh pair");

    database
        .client()
        .full_integrity_check()
        .expect("explicit integrity scan");
}

#[test]
fn database_pair_allows_only_one_active_writer() {
    let pair = TestPair::new();
    let first = Database::initialize(pair.paths.clone()).expect("first writer");

    let error = Database::initialize(pair.paths.clone()).expect_err("second writer refused");
    assert!(matches!(error, DatabaseError::DatabaseInUse { .. }));

    first.shutdown().expect("release first writer");
    drop(Database::initialize(pair.paths.clone()).expect("replacement writer"));
}

#[test]
fn concurrent_shutdown_serializes_writer_join_and_ownership_release() {
    let pair = TestPair::new();
    let database =
        Arc::new(Database::initialize(pair.paths.clone()).expect("exclusive database owner"));
    let barrier = Arc::new(Barrier::new(3));
    let shutdowns = (0..2)
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
    for shutdown in shutdowns {
        shutdown
            .join()
            .expect("shutdown caller did not panic")
            .expect("shutdown succeeded");
    }

    drop(database);
    drop(Database::initialize(pair.paths.clone()).expect("replacement writer"));
}

#[test]
fn pending_migrations_apply_to_an_identified_empty_pair() {
    let pair = TestPair::new();
    std::fs::create_dir_all(pair.paths.root()).expect("pair root");
    drop(
        connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("empty main"),
    );
    drop(
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Fresh)
            .expect("empty media"),
    );

    let database = Database::initialize(pair.paths.clone()).expect("pending migrations");
    assert_eq!(
        database
            .client()
            .diagnostics()
            .expect("diagnostics")
            .migration_heads,
        migrations::expected_heads()
    );
}

#[test]
fn lexical_upgrade_defers_backfill_until_post_start_reconciliation() {
    let pair = TestPair::new();
    let mut main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Fresh)
        .expect("fresh main writer");
    migrations::main_runner()
        .set_target(Target::Version(3))
        .run(&mut main)
        .expect("main schema through passage provenance");
    let mut media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Fresh)
            .expect("fresh media writer");
    migrations::run_media(&mut media).expect("media schema");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000121',
            10, 10, '019f547b-6200-7000-8000-000000000122'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000122',
            '019f547b-6200-7000-8000-000000000121',
            1, 10, 'deferred lexical upgrade evidence', zeroblob(32)
         );
         COMMIT;",
    )
    .expect("pre-lexical authored data");

    migrations::run_main(&mut main).expect("lexical schema migration");
    assert_eq!(
        main.query_row("SELECT count(*) FROM passage_search_document", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("empty derived projection"),
        0
    );
    assert_eq!(
        main.query_row(
            "SELECT status, error FROM index_state WHERE name = 'PASSAGE_FTS'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .expect("deferred lexical state"),
        (
            "DIRTY".into(),
            Some("initial lexical backfill pending".into())
        )
    );
    drop(main);
    drop(media);

    let database = Database::initialize(pair.paths.clone()).expect("authored library opens");
    database
        .client()
        .reconcile_author_passages()
        .expect("post-start passage and search reconciliation");
    let results = database
        .client()
        .search_passages(SearchPassagesInput {
            query: "deferred lexical upgrade".into(),
            mode: LexicalSearchMode::Default,
            limit: 10,
        })
        .expect("search after deferred backfill");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]
            .citation
            .tidbit
            .as_ref()
            .expect("authored citation")
            .id,
        "019f547b-6200-7000-8000-000000000121"
    );
}

#[test]
fn media_lifecycle_upgrade_preserves_attachments_and_allows_shared_blobs() {
    let pair = TestPair::new();
    let mut main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Fresh)
        .expect("fresh main writer");
    migrations::main_runner()
        .set_target(Target::Version(5))
        .run(&mut main)
        .expect("main schema before media lifecycle");
    let sha256 = vec![0x5a_u8; 32];
    main.execute(
        "INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000601',
            10, 10, ?1, 'first.bin', 'application/octet-stream',
            4, 'BINARY', 'NOT_APPLICABLE'
         )",
        params![&sha256],
    )
    .expect("pre-upgrade attachment");
    main.execute(
        "INSERT INTO media_ingest_lease(
            id, sha256, attachment_id, state, created_at, expires_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000603',
            ?1,
            '019f547b-6200-7000-8000-000000000601',
            'COMMITTED',
            10,
            20
         )",
        params![&sha256],
    )
    .expect("pre-upgrade attachment lease");

    migrations::run_main(&mut main).expect("media lifecycle migration");
    assert_eq!(
        main.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
            .expect("foreign key state"),
        1
    );
    let foreign_key_errors = main
        .prepare("PRAGMA foreign_key_check")
        .expect("foreign key check")
        .query_map([], |_| Ok(()))
        .expect("foreign key rows")
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("foreign key results");
    assert!(foreign_key_errors.is_empty());
    assert_eq!(
        main.query_row(
            "SELECT attachment_id
             FROM media_ingest_lease
             WHERE id = '019f547b-6200-7000-8000-000000000603'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("preserved attachment lease"),
        "019f547b-6200-7000-8000-000000000601"
    );
    main.execute(
        "INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000602',
            11, 11, ?1, 'second.bin', 'application/octet-stream',
            4, 'BINARY', 'NOT_APPLICABLE'
         )",
        params![&sha256],
    )
    .expect("shared blob attachment");
    assert_eq!(
        main.query_row(
            "SELECT count(*) FROM attachment WHERE sha256 = ?1",
            params![&sha256],
            |row| row.get::<_, i64>(0),
        )
        .expect("shared attachment count"),
        2
    );
    for (kind, name) in [
        ("view", "current_attachment_passage"),
        ("trigger", "passage_attachment_locator_validate"),
        ("trigger", "attachment_search_refresh_after_update"),
    ] {
        assert_eq!(
            main.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = ?1 AND name = ?2",
                params![kind, name],
                |row| row.get::<_, i64>(0),
            )
            .expect("dependent schema object"),
            1
        );
    }
}

#[test]
fn corrupt_application_id_is_rejected_before_migration() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));

    let connection = Connection::open(&pair.paths.main).expect("main database");
    connection
        .pragma_update(None, "application_id", 0x1234_i32)
        .expect("corrupt application id");
    drop(connection);

    let error = Database::initialize(pair.paths.clone()).expect_err("wrong application id");
    assert!(matches!(
        error,
        DatabaseError::WrongApplicationId { kind: "main", .. }
    ));
}

#[test]
fn divergent_migration_checksum_is_rejected() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));

    let connection = Connection::open(&pair.paths.main).expect("main database");
    connection
        .execute(
            "UPDATE refinery_schema_history SET checksum = '0' WHERE version = 1",
            [],
        )
        .expect("divergent checksum");
    drop(connection);

    let error = Database::initialize(pair.paths.clone()).expect_err("divergent history");
    assert!(matches!(
        error,
        DatabaseError::IncompatibleMigrationHistory { kind: "main", .. }
    ));
}

#[test]
fn unknown_future_migration_is_rejected_as_missing_from_the_binary() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));

    let connection = Connection::open(&pair.paths.main).expect("main database");
    let future_version = migrations::expected_heads()
        .main
        .expect("main migration head")
        + 1;
    connection
        .execute(
            "INSERT INTO refinery_schema_history(version, name, applied_on, checksum)
             SELECT ?1, 'removed_from_binary', applied_on, '0'
             FROM refinery_schema_history
             WHERE version = 1",
            params![future_version],
        )
        .expect("future migration");
    drop(connection);

    let error = Database::initialize(pair.paths.clone()).expect_err("missing migration");
    assert!(matches!(
        error,
        DatabaseError::IncompatibleMigrationHistory { kind: "main", .. }
    ));
}

#[test]
fn interrupted_pristine_pair_creation_resumes_idempotently() {
    let pair = TestPair::new();
    std::fs::create_dir_all(pair.paths.root()).expect("pair root");
    drop(
        connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("interrupted main"),
    );

    let database = Database::initialize(pair.paths.clone()).expect("resumed pair");
    assert!(pair.paths.media.exists());
    assert_eq!(
        database
            .client()
            .diagnostics()
            .expect("diagnostics")
            .migration_heads,
        migrations::expected_heads()
    );
}

#[test]
fn missing_half_of_a_migrated_pair_is_not_silently_recreated() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    std::fs::remove_file(&pair.paths.media).expect("remove migrated media half");

    let error = Database::initialize(pair.paths.clone()).expect_err("incomplete migrated pair");
    assert!(matches!(error, DatabaseError::IncompletePair { .. }));
}

#[test]
fn diagnostic_connections_are_query_only() {
    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("fresh pair");
    let connection = database.open_main_read_only().expect("read-only main");
    let query_only: i64 = connection
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .expect("query_only");
    assert_eq!(query_only, 1);
    assert!(connection
        .execute(
            "UPDATE app_settings SET appearance = 'DARK' WHERE id = 1",
            []
        )
        .is_err());
}

#[test]
fn strict_schema_rejects_non_uuidv7_ids_and_preserves_immutable_rows() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");

    assert!(main
        .execute(
            "INSERT INTO source(id, created_at, label)
             VALUES('zzzzzzzz-zzzz-7zzz-8zzz-zzzzzzzzzzzz', 10, 'invalid')",
            [],
        )
        .is_err());
    let unsafe_url = main
        .execute(
            "INSERT INTO source(id, created_at, normalized_url)
             VALUES(
                '019f547b-6200-7000-8000-000000000200',
                10, 'javascript:alert(1)'
             )",
            [],
        )
        .expect_err("unsafe source URL");
    assert!(unsafe_url.to_string().contains("source_url_safe_scheme"));
    main.execute(
        "INSERT INTO source(id, created_at, label, normalized_url)
         VALUES(
            '019f547b-6200-7000-8000-000000000201',
            10, 'valid', 'https://example.com/evidence'
         )",
        [],
    )
    .expect("valid UUIDv7 source");
    assert!(main
        .execute(
            "UPDATE source SET label = 'mutated'
             WHERE id = '019f547b-6200-7000-8000-000000000201'",
            [],
        )
        .is_err());

    assert!(main
        .execute(
            "UPDATE passage_embedding_index
             SET model_revision = 'mutated'
             WHERE index_key = 'jina_v1'",
            [],
        )
        .is_err());
    main.execute(
        "UPDATE passage_embedding_settings
         SET active_embedding_index_id = (
             SELECT id FROM passage_embedding_index WHERE index_key = 'jina_v1'
         ),
         updated_at = 10
         WHERE singleton_id = 1",
        [],
    )
    .expect("activation remains mutable");
}

#[test]
fn media_schema_enforces_the_product_blob_size_limit() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Existing)
            .expect("media writer");
    let error = media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, x'00', ?2, 10)",
            params![vec![12_u8; 32], connection::MAX_MEDIA_BLOB_BYTES + 1],
        )
        .expect_err("oversized media blob");

    assert!(error.to_string().contains("media_blob_size_limit"));
}

#[test]
fn current_revision_must_belong_to_its_tidbit() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000301',
            10, 10, '019f547b-6200-7000-8000-000000000302'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000302',
            '019f547b-6200-7000-8000-000000000301',
            1, 10, 'first', zeroblob(32)
         );
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000303',
            10, 10, '019f547b-6200-7000-8000-000000000304'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000304',
            '019f547b-6200-7000-8000-000000000303',
            1, 10, 'second', zeroblob(32)
         );
         COMMIT;",
    )
    .expect("two tidbits");

    assert!(main
        .execute(
            "UPDATE tidbit
             SET current_revision_id = '019f547b-6200-7000-8000-000000000304'
             WHERE id = '019f547b-6200-7000-8000-000000000301'",
            [],
        )
        .is_err());

    for locator in ["{}", "{\"start\":0}"] {
        assert!(main
            .execute(
                "INSERT INTO passage(
                    id, tidbit_revision_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000000305',
                    '019f547b-6200-7000-8000-000000000302',
                    'AUTHOR', 0, 'unresolvable', zeroblob(32),
                    'MARKDOWN_BLOCKS', ?1, 10
                 )",
                params![locator],
            )
            .is_err());
    }
}

#[test]
fn attachment_citation_provenance_cannot_silently_retarget() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute_batch(
        "INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000401',
            10, 10, zeroblob(32), 'notes.txt', 'text/plain', 12, 'TEXT', 'PENDING'
         );
         INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, created_at, started_at, completed_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000402',
            '019f547b-6200-7000-8000-000000000401',
            'text', '1', zeroblob(32), 'READY', 10, 10, 10
         );
         INSERT INTO attachment_segment(
            id, extraction_id, ordinal, locator_kind, line_start, line_end,
            content, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000403',
            '019f547b-6200-7000-8000-000000000402',
            0, 'TEXT_LINES', 1, 1, 'evidence', zeroblob(32)
         );",
    )
    .expect("attachment provenance");

    assert!(main
        .execute(
            "INSERT INTO passage(
                id, attachment_segment_id, owner_kind, ordinal, content,
                content_hash, locator_kind, locator_json, created_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000000404',
                '019f547b-6200-7000-8000-000000000403',
                'ATTACHMENT', 0, 'evidence', zeroblob(32),
                'TEXT_LINES', '{\"start\":1,\"end\":2}', 10
             )",
            [],
        )
        .is_err());
    main.execute(
        "INSERT INTO passage(
            id, attachment_segment_id, owner_kind, ordinal, content,
            content_hash, locator_kind, locator_json, created_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000404',
            '019f547b-6200-7000-8000-000000000403',
            'ATTACHMENT', 0, 'evidence', zeroblob(32),
            'TEXT_LINES', '{\"start\":1,\"end\":1}', 10
         )",
        [],
    )
    .expect("passage locator matches stored segment");

    assert!(main
        .execute(
            "UPDATE attachment_extraction
             SET content_hash = randomblob(32)
             WHERE id = '019f547b-6200-7000-8000-000000000402'",
            [],
        )
        .is_err());
    let regression = main
        .execute(
            "UPDATE attachment_extraction
             SET status = 'RUNNING'
             WHERE id = '019f547b-6200-7000-8000-000000000402'",
            [],
        )
        .expect_err("ready extraction cannot regress");
    assert!(regression
        .to_string()
        .contains("ready attachment extractions are terminal"));
    assert!(main
        .execute(
            "UPDATE attachment_segment
             SET content = 'different evidence'
             WHERE id = '019f547b-6200-7000-8000-000000000403'",
            [],
        )
        .is_err());
}

#[test]
fn extractions_require_current_content_and_discard_partial_outputs() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let valid_digest = vec![13_u8; 32];
    let changed_digest = vec![14_u8; 32];

    let media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Existing)
            .expect("media writer");
    for digest in [&valid_digest, &changed_digest] {
        media
            .execute(
                "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
                 VALUES(?1, x'01', 1, 10)",
                params![digest],
            )
            .expect("media blob");
    }
    drop(media);

    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    for (attachment_id, digest) in [
        ("019f547b-6200-7000-8000-000000000801", &valid_digest),
        ("019f547b-6200-7000-8000-000000000803", &changed_digest),
    ] {
        main.execute(
            "INSERT INTO attachment(
                id, created_at, updated_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(?1, 10, 10, ?2, 'scan.png', 'image/png', 1, 'IMAGE', 'PENDING')",
            params![attachment_id, digest],
        )
        .expect("attachment");
    }
    main.execute(
        "INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, created_at, started_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000802',
            '019f547b-6200-7000-8000-000000000801',
            'ocr', 'fixture-v1', ?1, 'RUNNING', 10, 11
         )",
        params![&valid_digest],
    )
    .expect("running extraction");
    let mismatch = main
        .execute(
            "INSERT INTO attachment_extraction(
                id, attachment_id, extractor, extractor_version, content_hash,
                status, created_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000000804',
                '019f547b-6200-7000-8000-000000000803',
                'ocr', 'fixture-v1', ?1, 'READY', 10
             )",
            params![vec![15_u8; 32]],
        )
        .expect_err("extraction hash must match attachment");
    assert!(mismatch
        .to_string()
        .contains("FOREIGN KEY constraint failed"));
    main.execute(
        "INSERT INTO attachment_segment(
            id, extraction_id, ordinal, locator_kind, page_number, content, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000805',
            '019f547b-6200-7000-8000-000000000802',
            0, 'PDF_PAGE', 1, 'partial output', zeroblob(32)
         )",
        [],
    )
    .expect("partial segment");
    assert!(main
        .execute(
            "INSERT INTO passage(
                id, attachment_segment_id, owner_kind, ordinal, content,
                content_hash, locator_kind, locator_json, created_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000000806',
                '019f547b-6200-7000-8000-000000000805',
                'ATTACHMENT', 0, 'partial output', zeroblob(32),
                'PDF_PAGE', '{\"page\":1}', 11
             )",
            [],
        )
        .is_err());
    drop(main);

    let database = Database::initialize(pair.paths.clone()).expect("recovered pair");
    let read_only = database.open_main_read_only().expect("read-only main");
    let valid: (String, Option<i64>, String) = read_only
        .query_row(
            "SELECT status, started_at, extractor_version
             FROM attachment_extraction
             WHERE id = '019f547b-6200-7000-8000-000000000802'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("requeued extraction");
    assert_eq!(valid, ("PENDING".into(), None, "fixture-v1".into()));

    let partial_segments: i64 = read_only
        .query_row(
            "SELECT count(*)
             FROM attachment_segment
             WHERE extraction_id = '019f547b-6200-7000-8000-000000000802'",
            [],
            |row| row.get(0),
        )
        .expect("partial segment count");
    assert_eq!(partial_segments, 0);
}

#[test]
fn passage_embeddings_require_exact_passage_and_index_provenance() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000901',
            10, 10, '019f547b-6200-7000-8000-000000000902'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000902',
            '019f547b-6200-7000-8000-000000000901',
            1, 10, 'semantic evidence', zeroblob(32)
         );
         COMMIT;",
    )
    .expect("embedding provenance fixture");
    let passage_hash = vec![21_u8; 32];
    main.execute(
        "INSERT INTO passage(
            id, tidbit_revision_id, owner_kind, ordinal, content,
            content_hash, locator_kind, locator_json, created_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000903',
            '019f547b-6200-7000-8000-000000000902',
            'AUTHOR', 0, 'semantic evidence', ?1,
            'MARKDOWN_BLOCKS', '{\"start\":0,\"end\":0}', 10
         )",
        params![&passage_hash],
    )
    .expect("passage");

    let mismatch = main
        .execute(
            "INSERT INTO passage_embedding(
                passage_id, embedding_index_id, passage_content_hash, created_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000000903',
                '019f547b-6200-7000-8000-000000000002', ?1, 11
             )",
            params![vec![22_u8; 32]],
        )
        .expect_err("stale passage hash");
    assert!(mismatch
        .to_string()
        .contains("passage embedding provenance mismatch"));
    main.execute(
        "INSERT INTO passage_embedding(
            passage_id, embedding_index_id, passage_content_hash, created_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000903',
            '019f547b-6200-7000-8000-000000000002', ?1, 11
         )",
        params![&passage_hash],
    )
    .expect("matching embedding provenance");

    let invalid_vector = serde_json::to_string(&vec![0.0_f32; 3]).expect("vector JSON");
    assert!(main
        .execute(
            "INSERT INTO passage_embedding_vec_jina_v1(rowid, embedding)
             SELECT rowid, ?1 FROM passage
             WHERE id = '019f547b-6200-7000-8000-000000000903'",
            params![invalid_vector],
        )
        .is_err());
    let mut vector = vec![0.0_f32; 768];
    vector[0] = 1.0;
    let vector_json = serde_json::to_string(&vector).expect("vector JSON");
    main.execute(
        "INSERT INTO passage_embedding_vec_jina_v1(rowid, embedding)
         SELECT rowid, ?1 FROM passage
         WHERE id = '019f547b-6200-7000-8000-000000000903'",
        params![vector_json],
    )
    .expect("matching vector dimension");
}

#[test]
fn revision_provenance_links_cannot_be_retargeted_or_deleted() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000701',
            10, 10, '019f547b-6200-7000-8000-000000000702'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000702',
            '019f547b-6200-7000-8000-000000000701',
            1, 10, 'historical evidence', zeroblob(32)
         );
         INSERT INTO source(id, created_at, label, normalized_url)
         VALUES(
            '019f547b-6200-7000-8000-000000000703',
            10, 'Primary source', 'https://example.com/source'
         );
         INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000704',
            10, 10, randomblob(32), 'evidence.txt', 'text/plain',
            12, 'TEXT', 'PENDING'
         );
         INSERT INTO tidbit_revision_source(tidbit_revision_id, source_id, sort_order)
         VALUES(
            '019f547b-6200-7000-8000-000000000702',
            '019f547b-6200-7000-8000-000000000703',
            0
         );
         INSERT INTO tidbit_revision_attachment(
            tidbit_revision_id, attachment_id, sort_order, display_role
         ) VALUES(
            '019f547b-6200-7000-8000-000000000702',
            '019f547b-6200-7000-8000-000000000704',
            0, 'ATTACHMENT'
         );
         COMMIT;",
    )
    .expect("revision provenance");

    assert!(main
        .execute(
            "UPDATE tidbit_revision_source
             SET sort_order = 1
             WHERE tidbit_revision_id = '019f547b-6200-7000-8000-000000000702'",
            [],
        )
        .is_err());
    assert!(main
        .execute(
            "DELETE FROM tidbit_revision_source
             WHERE tidbit_revision_id = '019f547b-6200-7000-8000-000000000702'",
            [],
        )
        .is_err());
    assert!(main
        .execute(
            "UPDATE tidbit_revision_attachment
             SET display_role = 'INLINE'
             WHERE tidbit_revision_id = '019f547b-6200-7000-8000-000000000702'",
            [],
        )
        .is_err());
    assert!(main
        .execute(
            "DELETE FROM tidbit_revision_attachment
             WHERE tidbit_revision_id = '019f547b-6200-7000-8000-000000000702'",
            [],
        )
        .is_err());
}

#[test]
fn explicit_shutdown_joins_the_only_writer() {
    let pair = TestPair::new();
    let database = Database::initialize(pair.paths.clone()).expect("fresh pair");
    let client = database.client();

    database.shutdown().expect("clean shutdown");
    assert!(matches!(
        client.diagnostics(),
        Err(DatabaseError::WriterUnavailable)
    ));
}

#[test]
fn orphaned_media_stage_is_recoverable_but_missing_committed_bytes_are_not() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));

    let media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Existing)
            .expect("media writer");
    let digest = vec![7_u8; 32];
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, x'010203', 3, 10)",
            params![&digest],
        )
        .expect("orphaned staged blob");
    drop(media);

    drop(Database::initialize(pair.paths.clone()).expect("orphan tolerated"));

    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute(
        "INSERT INTO attachment(
            id, created_at, updated_at, deleted_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000101',
            10, 11, 11, ?1, 'missing.pdf', 'application/pdf', 3, 'PDF', 'PENDING'
         )",
        params![vec![8_u8; 32]],
    )
    .expect("deleted attachment without blob");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000102',
            10, 10, '019f547b-6200-7000-8000-000000000103'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000103',
            '019f547b-6200-7000-8000-000000000102',
            1, 10, 'historical attachment', zeroblob(32)
         );
         INSERT INTO tidbit_revision_attachment(
            tidbit_revision_id, attachment_id, sort_order, display_role
         ) VALUES(
            '019f547b-6200-7000-8000-000000000103',
            '019f547b-6200-7000-8000-000000000101',
            0, 'ATTACHMENT'
         );
         COMMIT;",
    )
    .expect("historical attachment provenance");
    drop(main);

    let error = Database::initialize(pair.paths.clone()).expect_err("missing committed bytes");
    assert!(matches!(
        error,
        DatabaseError::Validation {
            kind: "database pair",
            ..
        }
    ));
}

#[test]
fn stale_fts_is_deferred_until_explicit_post_start_maintenance() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute_batch(
        "BEGIN;
         INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000501',
            10, 10, '019f547b-6200-7000-8000-000000000502'
         );
         INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(
            '019f547b-6200-7000-8000-000000000502',
            '019f547b-6200-7000-8000-000000000501',
            1, 10, 'authored survives', zeroblob(32)
         );
         INSERT INTO passage(
            id, tidbit_revision_id, owner_kind, ordinal, content,
            content_hash, locator_kind, locator_json, created_at,
            construction_version, heading_context_json
         ) VALUES(
            '019f547b-6200-7000-8000-000000000503',
            '019f547b-6200-7000-8000-000000000502',
            'AUTHOR', 0, 'recoverable ﬁle evidence', zeroblob(32),
            'MARKDOWN_BLOCKS', '{\"start\":0,\"end\":0}', 10,
            'markdown-blocks-v1', '[]'
         );
         INSERT INTO active_passage(passage_id, tidbit_id)
         VALUES(
            '019f547b-6200-7000-8000-000000000503',
            '019f547b-6200-7000-8000-000000000501'
         );
         INSERT INTO passage_search_document(
            rowid, passage_id, tidbit_id, title, heading_context, body,
            source_labels, source_domains, attachment_names, extracted_text,
            owner_content_hash, updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            '019f547b-6200-7000-8000-000000000501',
            '', '', passage.content, '', '', '', '',
            zeroblob(32), 10
         FROM passage
         WHERE passage.id = '019f547b-6200-7000-8000-000000000503';
         INSERT INTO passage_fts_word(passage_fts_word) VALUES('delete-all');
         INSERT INTO passage_fts_trigram(passage_fts_trigram) VALUES('delete-all');
         UPDATE index_state
         SET version = 'legacy', status = 'RUNNING', updated_at = 10
         WHERE name = 'PASSAGE_FTS';
         COMMIT;",
    )
    .expect("passage without derived index row");
    drop(main);

    let database = Database::initialize(pair.paths.clone()).expect("authored data opens");
    let read_only = database.open_main_read_only().expect("read-only main");
    let state: (String, String) = read_only
        .query_row(
            "SELECT status, version FROM index_state WHERE name = 'PASSAGE_FTS'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("recovered index state");
    assert_eq!(state, ("DIRTY".into(), "legacy".into()));
    let before: i64 = read_only
        .query_row(
            "SELECT count(*)
             FROM passage_fts_word
             WHERE passage_fts_word MATCH 'file'",
            [],
            |row| row.get(0),
        )
        .expect("stale search remains optional");
    assert_eq!(before, 0);

    assert!(database
        .client()
        .reconcile_fts()
        .expect("explicit FTS maintenance"));
    let matches: i64 = read_only
        .query_row(
            "SELECT count(*)
             FROM passage_fts_word
             WHERE passage_fts_word MATCH 'file'",
            [],
            |row| row.get(0),
        )
        .expect("rebuilt search");
    assert_eq!(matches, 1);
    assert!(database
        .client()
        .reconcile_fts()
        .expect("normalized integrity maintenance"));
    let matches_after_integrity: i64 = read_only
        .query_row(
            "SELECT count(*)
             FROM passage_fts_word
             WHERE passage_fts_word MATCH 'file'",
            [],
            |row| row.get(0),
        )
        .expect("normalized search after integrity check");
    assert_eq!(matches_after_integrity, 1);
    let state: (String, String) = read_only
        .query_row(
            "SELECT status, version FROM index_state WHERE name = 'PASSAGE_FTS'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("maintained index state");
    assert_eq!(state, ("IDLE".into(), "lexical-v1".into()));
}
