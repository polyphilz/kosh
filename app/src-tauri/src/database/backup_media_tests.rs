use std::io::Cursor;

use refinery::Target;
use rusqlite::params;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::backup::domain::{
    BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target, ReplicaEpochId,
};

use super::{
    backup_media::{self, OffsiteMediaUploadFailureCode},
    backup_state::SaveOffsiteBackupConfigInput,
    connection::{self, DatabaseKind, FileState},
    drafts::SaveDraftWrite,
    media::{CanonicalImage, IngestAttachmentMetadata, IngestImageWrite, StagedAttachment},
    migrations, Database, DatabasePaths, MediaLimits, SaveDraftInput,
};

const ACCOUNT_ID: &str = "0123456789abcdef0123456789abcdef";
const DRAFT_ID: &str = "019f547b-6200-7000-8000-000000008001";

struct TestLibrary {
    _root: TempDir,
    paths: DatabasePaths,
    staging: std::path::PathBuf,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary backup media library");
        let paths = DatabasePaths::new(root.path());
        let staging = root.path().join("staging");
        let database = Database::initialize(paths.clone()).expect("database");
        database
            .client()
            .save_draft(SaveDraftWrite {
                input: SaveDraftInput {
                    context_key: "capture".into(),
                    tidbit_id: None,
                    base_revision_id: None,
                    title: None,
                    body_markdown: String::new(),
                    sources: Vec::new(),
                },
                now_ms: 10,
                draft_id: DRAFT_ID.into(),
                media_limits: MediaLimits::default(),
            })
            .expect("capture draft");
        Self {
            _root: root,
            paths,
            staging,
            database,
        }
    }

    fn save_config(
        &self,
        expected_revision: i64,
        backup_set_id: BackupSetId,
        enabled: bool,
        bucket: &str,
        now_ms: i64,
    ) -> super::backup_state::OffsiteBackupConfig {
        self.database
            .client()
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision,
                backup_set_id,
                replica_epoch_id: ReplicaEpochId::new(),
                enabled,
                target: target(bucket),
                now_ms,
            })
            .expect("save backup config")
    }

    fn ingest(&self, suffix: u64, bytes: &[u8], now_ms: i64) -> super::AttachmentRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(bytes),
            &self.staging,
            &id(suffix + 2),
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage attachment");
        self.database
            .client()
            .ingest_attachment(staged.write(IngestAttachmentMetadata {
                attachment_id: id(suffix),
                ingest_lease_id: id(suffix + 1),
                draft_id: DRAFT_ID.into(),
                display_filename: format!("attachment-{suffix}.bin"),
                media_type: "application/octet-stream".into(),
                now_ms,
                limits: MediaLimits::default(),
            }))
            .expect("ingest attachment")
    }

    fn ingest_image(
        &self,
        suffix: u64,
        original: &[u8],
        preview: &[u8],
        now_ms: i64,
    ) -> super::ImageRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(original),
            &self.staging,
            &id(suffix + 2),
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage image");
        self.database
            .client()
            .ingest_image(IngestImageWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(suffix),
                    ingest_lease_id: id(suffix + 1),
                    draft_id: DRAFT_ID.into(),
                    display_filename: "knowledge.png".into(),
                    media_type: "image/png".into(),
                    now_ms,
                    limits: MediaLimits::default(),
                }),
                extraction_id: id(suffix + 3),
                preview: CanonicalImage {
                    bytes: preview.to_vec(),
                    natural_width: 1_200,
                    natural_height: 800,
                },
            })
            .expect("ingest image")
    }
}

fn target(bucket: &str) -> R2Target {
    R2Target {
        account_id: R2AccountId::parse(ACCOUNT_ID).expect("account"),
        jurisdiction: R2Jurisdiction::Default,
        bucket: R2BucketName::parse(bucket).expect("bucket"),
    }
}

fn id(suffix: u64) -> String {
    format!("019f547b-6200-7000-8000-{suffix:012x}")
}

#[test]
fn enabling_and_reference_transactions_seed_source_and_preview_hashes() {
    let library = TestLibrary::new();
    library.ingest_image(100, b"original-one", b"preview-one", 20);
    assert_eq!(
        library
            .database
            .open_main_read_only()
            .expect("main reader")
            .query_row("SELECT count(*) FROM offsite_media_upload", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("queue count"),
        0
    );

    let backup_set_id = BackupSetId::new();
    let enabled = library.save_config(0, backup_set_id.clone(), true, "kosh-local", 30);
    let progress = library
        .database
        .client()
        .offsite_media_upload_progress()
        .expect("seed progress");
    assert_eq!(progress.referenced, 2);
    assert_eq!(progress.pending, 2);
    assert_eq!(progress.untracked, 0);

    library.ingest(200, b"new reference", 40);
    let progress = library
        .database
        .client()
        .offsite_media_upload_progress()
        .expect("transactional progress");
    assert_eq!(progress.referenced, 3);
    assert_eq!(progress.pending, 3);

    let revoked = library
        .database
        .client()
        .claim_next_offsite_media_upload(45, id(380))
        .expect("claim before disable")
        .expect("queued upload");
    let disabled = library.save_config(
        enabled.revision,
        backup_set_id.clone(),
        false,
        "kosh-local",
        50,
    );
    assert!(!library
        .database
        .client()
        .complete_offsite_media_upload(revoked, "\"late\"".into(), 51)
        .expect("revoked completion"));
    library.ingest(300, b"created while disabled", 60);
    let progress = library
        .database
        .client()
        .offsite_media_upload_progress()
        .expect("disabled progress");
    assert_eq!(progress.referenced, 4);
    assert_eq!(progress.pending, 2);
    assert_eq!(progress.retry_wait, 1);
    assert_eq!(progress.untracked, 1);
    assert!(library
        .database
        .client()
        .claim_next_offsite_media_upload(65, id(381))
        .expect("disabled claim")
        .is_none());

    let reenabled = library.save_config(
        disabled.revision,
        backup_set_id.clone(),
        true,
        "kosh-local",
        70,
    );
    let progress = library
        .database
        .client()
        .offsite_media_upload_progress()
        .expect("reenabled progress");
    assert_eq!(progress.referenced, 4);
    assert_eq!(progress.pending, 3);
    assert_eq!(progress.retry_wait, 1);
    assert_eq!(progress.untracked, 0);

    let attempted = library
        .database
        .client()
        .claim_next_offsite_media_upload(75, id(390))
        .expect("claim before retarget")
        .expect("queued upload");
    assert!(library
        .database
        .client()
        .fail_offsite_media_upload(
            attempted,
            OffsiteMediaUploadFailureCode::RemoteObjectMismatch,
            None,
            76,
        )
        .expect("record failed destination"));
    let retargeted = library.save_config(
        reenabled.revision,
        backup_set_id.clone(),
        true,
        "kosh-other",
        80,
    );
    let attempts: i64 = library
        .database
        .open_main_read_only()
        .expect("main reader")
        .query_row(
            "SELECT coalesce(sum(attempt_count), 0)
             FROM offsite_media_upload
             WHERE backup_set_id = ?1",
            [backup_set_id.as_str()],
            |row| row.get(0),
        )
        .expect("reset attempts");
    assert_eq!(attempts, 0);

    let replacement = BackupSetId::new();
    library.save_config(
        retargeted.revision,
        replacement.clone(),
        true,
        "kosh-other",
        90,
    );
    let sets = library
        .database
        .open_main_read_only()
        .expect("main reader")
        .prepare("SELECT DISTINCT backup_set_id FROM offsite_media_upload")
        .expect("prepare sets")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query sets")
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("collect sets");
    assert_eq!(sets, vec![replacement.to_string()]);
}

#[test]
fn running_leases_recover_after_restart_and_stale_workers_cannot_mutate_them() {
    let library = TestLibrary::new();
    let backup_set_id = BackupSetId::new();
    library.save_config(0, backup_set_id, true, "kosh-local", 20);
    library.ingest(400, b"restartable payload", 30);

    let first = library
        .database
        .client()
        .claim_next_offsite_media_upload(40, id(410))
        .expect("first claim")
        .expect("pending upload");
    assert_eq!(first.attempt_count, 1);
    let TestLibrary {
        _root,
        paths,
        database,
        ..
    } = library;
    database.shutdown().expect("shutdown");
    drop(database);

    let reopened = Database::initialize(paths).expect("reopened database");
    let client = reopened.client();
    assert_eq!(
        client
            .recover_interrupted_offsite_media_uploads(50)
            .expect("recover"),
        1
    );
    assert!(!client
        .complete_offsite_media_upload(first.clone(), "\"stale\"".into(), 51)
        .expect("stale completion"));
    assert!(client
        .claim_next_offsite_media_upload(49, id(411))
        .expect("early claim")
        .is_none());

    let second = client
        .claim_next_offsite_media_upload(50, id(412))
        .expect("second claim")
        .expect("recovered upload");
    assert_eq!(second.sha256, first.sha256);
    assert_eq!(second.attempt_count, 2);
    assert!(client
        .fail_offsite_media_upload(
            second.clone(),
            OffsiteMediaUploadFailureCode::RemoteNetwork,
            Some(70),
            60,
        )
        .expect("schedule retry"));
    assert!(client
        .claim_next_offsite_media_upload(69, id(413))
        .expect("pre-retry claim")
        .is_none());
    let third = client
        .claim_next_offsite_media_upload(70, id(414))
        .expect("third claim")
        .expect("due retry");
    assert_eq!(third.attempt_count, 3);
    assert!(client
        .complete_offsite_media_upload(third, "\"verified\"".into(), 80)
        .expect("complete"));
    let progress = client
        .offsite_media_upload_progress()
        .expect("completed progress");
    assert_eq!(progress.uploaded, 1);
    assert_eq!(progress.running + progress.retry_wait + progress.failed, 0);
}

#[test]
fn remote_write_authorization_cancels_a_claim_after_its_last_reference_is_removed() {
    let root = tempfile::tempdir().expect("temporary retention fence root");
    let paths = DatabasePaths::new(root.path());
    let mut connection = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Fresh)
        .expect("main writer");
    migrations::main_runner()
        .run(&mut connection)
        .expect("migrate main database");
    let backup_set_id = BackupSetId::new();
    connection
        .execute(
            "INSERT INTO offsite_backup_config(
                singleton_id, revision, backup_set_id, replica_epoch_id, enabled,
                provider, jurisdiction, account_id, bucket, created_at, updated_at
             ) VALUES(1, 1, ?1, ?2, 1, 'R2', 'DEFAULT', ?3, 'kosh-local', 10, 10)",
            params![
                backup_set_id.as_str(),
                ReplicaEpochId::new().as_str(),
                ACCOUNT_ID,
            ],
        )
        .expect("backup config");
    let attachment_id = id(450);
    let hash = Sha256::digest(b"retained media").to_vec();
    connection
        .execute(
            "INSERT INTO attachment(
                id, created_at, updated_at, deleted_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(?1, 20, 20, NULL, ?2, 'retained.bin',
                      'application/octet-stream', 14, 'BINARY', 'NOT_APPLICABLE')",
            params![&attachment_id, &hash],
        )
        .expect("retained attachment");
    let claim = backup_media::claim_next(&mut connection, 30, id(451))
        .expect("claim query")
        .expect("retained claim");
    assert!(backup_media::authorize_remote_write(&connection, &claim)
        .expect("authorize retained claim"));

    connection
        .execute(
            "UPDATE attachment
             SET deleted_at = 40, updated_at = 40
             WHERE id = ?1",
            [&attachment_id],
        )
        .expect("remove final live reference");
    assert!(!backup_media::authorize_remote_write(&connection, &claim)
        .expect("reject unretained claim"));
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM offsite_media_upload", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("canceled queue count"),
        0
    );
}

#[test]
fn v19_migration_seeds_media_referenced_by_an_enabled_v18_config() {
    let root = tempfile::tempdir().expect("temporary V18 root");
    let paths = DatabasePaths::new(root.path());
    let mut connection = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Fresh)
        .expect("V18 writer");
    migrations::main_runner()
        .set_target(Target::Version(18))
        .run(&mut connection)
        .expect("migrate through V18");
    let attachment_id = id(500);
    let hash = Sha256::digest(b"preexisting media").to_vec();
    connection
        .execute(
            "INSERT INTO attachment(
                id, created_at, updated_at, deleted_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(?1, 10, 10, NULL, ?2, 'legacy.bin',
                      'application/octet-stream', 17, 'BINARY', 'NOT_APPLICABLE')",
            params![&attachment_id, &hash],
        )
        .expect("legacy attachment");
    let backup_set_id = BackupSetId::new();
    connection
        .execute(
            "INSERT INTO offsite_backup_config(
                singleton_id, revision, backup_set_id, replica_epoch_id, enabled,
                provider, jurisdiction, account_id, bucket, created_at, updated_at
             ) VALUES(1, 1, ?1, ?2, 1, 'R2', 'DEFAULT', ?3, 'kosh-local', 20, 20)",
            params![
                backup_set_id.as_str(),
                ReplicaEpochId::new().as_str(),
                ACCOUNT_ID,
            ],
        )
        .expect("legacy config");

    migrations::main_runner()
        .run(&mut connection)
        .expect("migrate through V19");
    let queued: (String, Vec<u8>, String) = connection
        .query_row(
            "SELECT backup_set_id, sha256, state FROM offsite_media_upload",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("seeded upload");
    assert_eq!(queued.0, backup_set_id.to_string());
    assert_eq!(queued.1, hash);
    assert_eq!(queued.2, "PENDING");

    let rolled_back_hash = Sha256::digest(b"rolled back media").to_vec();
    let transaction = connection.transaction().expect("reference transaction");
    transaction
        .execute(
            "INSERT INTO attachment(
                id, created_at, updated_at, deleted_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(?1, 30, 30, NULL, ?2, 'rollback.bin',
                      'application/octet-stream', 17, 'BINARY', 'NOT_APPLICABLE')",
            params![id(501), &rolled_back_hash],
        )
        .expect("transactional attachment");
    assert_eq!(
        transaction
            .query_row("SELECT count(*) FROM offsite_media_upload", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("in-transaction queue count"),
        2
    );
    transaction.rollback().expect("rollback reference");
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM offsite_media_upload", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("post-rollback queue count"),
        1
    );
}
