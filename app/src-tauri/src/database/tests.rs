use rusqlite::{params, Connection};
use tempfile::TempDir;

use super::{
    connection::{self, DatabaseKind, FileState},
    migrations, Database, DatabaseError, DatabasePaths,
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
    assert_eq!(
        migrations::main_runner().get_migrations()[0].checksum(),
        11_151_929_077_668_977_415
    );
    assert_eq!(
        migrations::media_runner().get_migrations()[0].checksum(),
        11_141_227_704_927_312_419
    );
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
    connection
        .execute(
            "INSERT INTO refinery_schema_history(version, name, applied_on, checksum)
             SELECT 2, 'removed_from_binary', applied_on, '0'
             FROM refinery_schema_history
             WHERE version = 1",
            [],
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
    main.execute(
        "INSERT INTO source(id, created_at, label)
         VALUES('019f547b-6200-7000-8000-000000000201', 10, 'valid')",
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
            status, created_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000402',
            '019f547b-6200-7000-8000-000000000401',
            'text', '1', zeroblob(32), 'PENDING', 10
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
            "UPDATE attachment_extraction
             SET content_hash = randomblob(32)
             WHERE id = '019f547b-6200-7000-8000-000000000402'",
            [],
        )
        .is_err());
    main.execute(
        "UPDATE attachment_extraction
         SET status = 'RUNNING', started_at = 11
         WHERE id = '019f547b-6200-7000-8000-000000000402'",
        [],
    )
    .expect("extraction lifecycle may advance");
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
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000101',
            10, 10, ?1, 'missing.pdf', 'application/pdf', 3, 'PDF', 'PENDING'
         )",
        params![vec![8_u8; 32]],
    )
    .expect("attachment without blob");
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
fn stale_fts_is_rebuilt_without_blocking_authored_content() {
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
            content_hash, locator_kind, locator_json, created_at
         ) VALUES(
            '019f547b-6200-7000-8000-000000000503',
            '019f547b-6200-7000-8000-000000000502',
            'AUTHOR', 0, 'recoverable lexical evidence', zeroblob(32),
            'MARKDOWN_BLOCKS', '{\"start\":0,\"end\":0}', 10
         );
         COMMIT;",
    )
    .expect("passage without derived index row");
    drop(main);

    let database = Database::initialize(pair.paths.clone()).expect("FTS reconciliation");
    let read_only = database.open_main_read_only().expect("read-only main");
    let matches: i64 = read_only
        .query_row(
            "SELECT count(*)
             FROM passage_fts_word
             WHERE passage_fts_word MATCH 'lexical'",
            [],
            |row| row.get(0),
        )
        .expect("rebuilt search");
    assert_eq!(matches, 1);
}

#[test]
fn media_reaper_checks_live_references_and_consumes_authorization_atomically() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Existing)
            .expect("media writer");
    let orphan_digest = vec![9_u8; 32];
    let referenced_digest = vec![10_u8; 32];
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, x'CAFE', 2, 10)",
            params![&orphan_digest],
        )
        .expect("orphan media blob");
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, x'BEEF', 2, 10)",
            params![&referenced_digest],
        )
        .expect("referenced media blob");
    assert!(media
        .execute(
            "DELETE FROM media_blob WHERE sha256 = ?1",
            params![&orphan_digest]
        )
        .is_err());
    media
        .execute(
            "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
             VALUES(?1, 19, 'stale capability')",
            params![&orphan_digest],
        )
        .expect("stale authorization");
    drop(media);

    let main = connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    main.execute(
        "INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(
            '019f547b-6200-7000-8000-000000000601',
            10, 10, ?1, 'kept.bin', 'application/octet-stream',
            2, 'BINARY', 'NOT_APPLICABLE'
         )",
        params![&referenced_digest],
    )
    .expect("live attachment");
    drop(main);

    let database = Database::initialize(pair.paths.clone()).expect("reconciled pair");
    let client = database.client();
    assert!(client
        .reap_media_blob(orphan_digest.clone(), 20, "orphaned stage".into())
        .expect("orphan reap"));
    assert!(!client
        .reap_media_blob(orphan_digest, 21, "idempotent retry".into())
        .expect("missing blob is idempotent"));
    assert!(matches!(
        client.reap_media_blob(referenced_digest.clone(), 22, "unsafe reap".into()),
        Err(DatabaseError::MediaInUse { references: 1 })
    ));

    let media = database.open_media_read_only().expect("read-only media");
    let kept: i64 = media
        .query_row(
            "SELECT count(*) FROM media_blob WHERE sha256 = ?1",
            params![referenced_digest],
            |row| row.get(0),
        )
        .expect("kept blob");
    assert_eq!(kept, 1);
    let capabilities: i64 = media
        .query_row(
            "SELECT count(*) FROM media_blob_reap_authorization",
            [],
            |row| row.get(0),
        )
        .expect("authorization rows");
    assert_eq!(capabilities, 0);
}
