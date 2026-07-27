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
        4_593_326_547_640_045_059
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
fn interrupted_pair_creation_is_reported_without_touching_the_survivor() {
    let pair = TestPair::new();
    std::fs::create_dir_all(pair.paths.root()).expect("pair root");
    drop(
        connection::open_writer(&pair.paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("interrupted main"),
    );
    let before = std::fs::metadata(&pair.paths.main)
        .expect("main metadata")
        .len();

    let error = Database::initialize(pair.paths.clone()).expect_err("incomplete pair");
    assert!(matches!(error, DatabaseError::IncompletePair { .. }));
    assert!(!pair.paths.media.exists());
    assert_eq!(
        std::fs::metadata(&pair.paths.main)
            .expect("main metadata after")
            .len(),
        before
    );
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
fn media_blob_deletion_requires_explicit_reap_authorization() {
    let pair = TestPair::new();
    drop(Database::initialize(pair.paths.clone()).expect("fresh pair"));
    let media =
        connection::open_writer(&pair.paths.media, DatabaseKind::Media, FileState::Existing)
            .expect("media writer");
    let digest = vec![9_u8; 32];
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, x'CAFE', 2, 10)",
            params![&digest],
        )
        .expect("media blob");
    assert!(media
        .execute("DELETE FROM media_blob WHERE sha256 = ?1", params![&digest])
        .is_err());
    media
        .execute(
            "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
             VALUES(?1, 20, 'test reconciliation')",
            params![&digest],
        )
        .expect("reap authorization");
    assert_eq!(
        media
            .execute("DELETE FROM media_blob WHERE sha256 = ?1", params![&digest])
            .expect("authorized reap"),
        1
    );
}
