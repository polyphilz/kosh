use std::{
    io::Cursor,
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    thread,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

use crate::backup::{
    domain::{
        BackupSetId, CheckpointErrorCode, CheckpointId, CheckpointPhase, R2AccountId, R2BucketName,
        R2Jurisdiction, R2Target, ReplicaEpochId,
    },
    litestream::LitestreamTxid,
};

use super::{
    backup_state::SaveOffsiteBackupConfigInput, settings::SetShortcutSettingsInput, Database,
    DatabaseError, DatabasePaths, MediaLimits, OffsiteBackupConfig, PrepareOffsiteCheckpointInput,
};
use super::{
    media::{IngestAttachmentMetadata, StagedAttachment},
    offsite_checkpoint::FAILED_CHECKPOINT_RETENTION,
};

struct InspectingSync {
    main_path: PathBuf,
    checkpoint_id: CheckpointId,
    entered: mpsc::SyncSender<()>,
    release: Mutex<Receiver<()>>,
}

impl InspectingSync {
    fn sync_local(&self) -> Result<LitestreamTxid, CheckpointErrorCode> {
        let connection = Connection::open_with_flags(
            &self.main_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        let phase = connection
            .query_row(
                "SELECT phase FROM offsite_backup_checkpoint WHERE checkpoint_id = ?1",
                [self.checkpoint_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        if phase != CheckpointPhase::Prepared.as_db_str() {
            return Err(CheckpointErrorCode::InvalidConfiguration);
        }
        self.entered
            .send(())
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        self.release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        Ok(LitestreamTxid::from_local(42))
    }
}

fn enabled_database() -> (TempDir, Database, OffsiteBackupConfig) {
    let root = tempfile::tempdir().expect("temporary database");
    let database =
        Database::initialize(DatabasePaths::new(root.path())).expect("initialize database");
    let config = database
        .client()
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: true,
            target: R2Target {
                account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef")
                    .expect("account"),
                jurisdiction: R2Jurisdiction::Default,
                bucket: R2BucketName::parse("kosh-checkpoint-test").expect("bucket"),
            },
            now_ms: 1,
        })
        .expect("enable backup");
    (root, database, config)
}

fn input(
    checkpoint_id: CheckpointId,
    config: &OffsiteBackupConfig,
    created_at_ms: i64,
) -> PrepareOffsiteCheckpointInput {
    PrepareOffsiteCheckpointInput {
        checkpoint_id,
        backup_set_id: config.backup_set_id.clone(),
        replica_epoch_id: config.replica_epoch_id.clone(),
        created_at_ms,
        kosh_version: "test".into(),
    }
}

fn publish(database: &Database, config: &OffsiteBackupConfig, created_at_ms: i64) -> CheckpointId {
    let client = database.client();
    let checkpoint_id = CheckpointId::new();
    let _prepared = client
        .prepare_offsite_checkpoint(input(checkpoint_id.clone(), config, created_at_ms))
        .expect("prepare checkpoint");
    let txid = LitestreamTxid::from_local(42);
    client
        .mark_offsite_checkpoint_fenced(checkpoint_id.clone(), txid)
        .expect("mark fenced");
    client
        .mark_offsite_checkpoint_replicated(checkpoint_id.clone())
        .expect("mark replicated");
    client
        .mark_offsite_checkpoint_published(
            checkpoint_id.clone(),
            format!("kosh/v1/test/{checkpoint_id}.json"),
        )
        .expect("mark published");
    checkpoint_id
}

#[test]
fn litestream_wait_leaves_the_writer_available_and_later_content_stales_the_fence() {
    let (_root, database, config) = enabled_database();
    let checkpoint_id = CheckpointId::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let sync = InspectingSync {
        main_path: database.paths().main.clone(),
        checkpoint_id: checkpoint_id.clone(),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    };
    let prepare_client = database.client();
    let prepare_config = config.clone();
    let prepare_checkpoint_id = checkpoint_id.clone();
    let prepare = thread::spawn(move || {
        let prepared = prepare_client
            .prepare_offsite_checkpoint(input(prepare_checkpoint_id, &prepare_config, 2))
            .expect("prepare checkpoint");
        let txid = sync.sync_local().expect("local sync");
        (prepared, txid)
    });
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("checkpoint worker entered local sync");

    let diagnostics_client = database.client();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let diagnostics = thread::spawn(move || {
        let _ = done_tx.send(diagnostics_client.diagnostics());
    });
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer remains available during local sync")
        .expect("diagnostics");
    diagnostics.join().expect("diagnostics thread");

    let shortcuts = database
        .client()
        .load_shortcut_settings()
        .expect("load shortcuts");
    database
        .client()
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: shortcuts.revision,
            keyboard_bindings: shortcuts.keyboard_bindings,
        })
        .expect("write authored state during local sync");

    release_tx.send(()).expect("release fence");
    let (_prepared, txid) = prepare.join().expect("prepare thread");
    assert!(matches!(
        database
            .client()
            .mark_offsite_checkpoint_fenced(checkpoint_id, txid),
        Err(DatabaseError::StaleOffsiteCheckpoint)
    ));
}

#[test]
fn checkpoint_state_machine_is_monotonic_and_publication_sequence_ignores_clock_rollback() {
    let (_root, database, config) = enabled_database();
    let first = publish(&database, &config, 200);
    let second = publish(&database, &config, 100);
    assert_ne!(first, second);
    let state = database
        .client()
        .load_offsite_checkpoint_schedule_state()
        .expect("schedule state");
    assert_eq!(
        state
            .last_published
            .expect("published checkpoint")
            .checkpoint_id,
        second
    );
    assert!(matches!(
        database.client().mark_offsite_checkpoint_replicated(first),
        Err(DatabaseError::StaleOffsiteCheckpoint)
    ));
}

#[test]
fn failed_fence_is_durable_without_replacing_the_last_published_checkpoint() {
    let (_root, database, config) = enabled_database();
    let published = publish(&database, &config, 1);
    let failed = CheckpointId::new();
    database
        .client()
        .prepare_offsite_checkpoint(input(failed.clone(), &config, 2))
        .expect("prepare failed fence");
    database
        .client()
        .mark_offsite_checkpoint_failed(failed, CheckpointErrorCode::FenceTimeout)
        .expect("record failure");
    assert_eq!(
        database
            .client()
            .load_offsite_checkpoint_schedule_state()
            .expect("schedule state")
            .last_published
            .expect("last published")
            .checkpoint_id,
        published
    );
}

#[test]
fn terminal_checkpoint_cleanup_bounds_headers_without_pruning_publications() {
    let (_root, database, config) = enabled_database();
    let published = publish(&database, &config, 1);
    let mut newest_failed = None;
    for created_at_ms in 2..=i64::from(FAILED_CHECKPOINT_RETENTION) + 8 {
        let checkpoint_id = CheckpointId::new();
        database
            .client()
            .prepare_offsite_checkpoint(input(checkpoint_id.clone(), &config, created_at_ms))
            .expect("prepare failed checkpoint");
        database
            .client()
            .mark_offsite_checkpoint_failed(
                checkpoint_id.clone(),
                CheckpointErrorCode::NetworkTimeout,
            )
            .expect("record failed checkpoint");
        newest_failed = Some(checkpoint_id);
    }
    for created_at_ms in 100..104 {
        let checkpoint_id = CheckpointId::new();
        database
            .client()
            .prepare_offsite_checkpoint(input(checkpoint_id.clone(), &config, created_at_ms))
            .expect("prepare interrupted checkpoint");
        newest_failed = Some(checkpoint_id);
    }
    assert_eq!(
        database
            .client()
            .fail_incomplete_offsite_checkpoints(CheckpointErrorCode::WorkerUnavailable)
            .expect("recover interrupted checkpoints"),
        4
    );

    let connection = Connection::open_with_flags(
        &database.paths().main,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open main database");
    let failed_count = connection
        .query_row(
            "SELECT count(*) FROM offsite_backup_checkpoint WHERE phase = ?1",
            [CheckpointPhase::Failed.as_db_str()],
            |row| row.get::<_, u32>(0),
        )
        .expect("count failed checkpoints");
    assert_eq!(failed_count, FAILED_CHECKPOINT_RETENTION);
    let newest_phase = connection
        .query_row(
            "SELECT phase FROM offsite_backup_checkpoint WHERE checkpoint_id = ?1",
            [newest_failed.expect("newest failed").as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load newest failed checkpoint");
    assert_eq!(newest_phase, CheckpointPhase::Failed.as_db_str());
    let published_phase = connection
        .query_row(
            "SELECT phase FROM offsite_backup_checkpoint WHERE checkpoint_id = ?1",
            [published.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load published checkpoint");
    assert_eq!(published_phase, CheckpointPhase::Published.as_db_str());
}

#[test]
fn authored_mutations_advance_content_clock_but_checkpoint_bookkeeping_does_not() {
    let (_root, database, config) = enabled_database();
    let client = database.client();
    let initial = client
        .load_offsite_checkpoint_schedule_state()
        .expect("initial clock")
        .content_revision;
    let shortcuts = client.load_shortcut_settings().expect("shortcuts");
    client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: shortcuts.revision,
            keyboard_bindings: shortcuts.keyboard_bindings,
        })
        .expect("authored settings update");
    let authored = client
        .load_offsite_checkpoint_schedule_state()
        .expect("authored clock")
        .content_revision;
    assert!(authored > initial);

    publish(&database, &config, 3);
    assert_eq!(
        client
            .load_offsite_checkpoint_schedule_state()
            .expect("bookkeeping clock")
            .content_revision,
        authored
    );
}

#[test]
fn checkpoint_media_snapshot_is_persisted_and_read_in_bounded_keyset_pages() {
    let (root, database, config) = enabled_database();
    let client = database.client();
    let draft_id = "019f547b-6200-7000-8000-000000008001";
    client
        .save_working_copy_for_test(draft_id.into(), None, 1, String::new(), Vec::new(), 2, true)
        .expect("save working copy");

    let staging = root.path().join("staging");
    for index in 0_u64..19 {
        let staging_id = format!("019f547b-6200-7000-8000-{:012x}", 0x100 + index);
        let staged = StagedAttachment::from_reader(
            Cursor::new(format!("paged checkpoint media {index}").into_bytes()),
            &staging,
            &staging_id,
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage attachment");
        client
            .ingest_attachment(staged.write(IngestAttachmentMetadata {
                attachment_id: format!("019f547b-6200-7000-8000-{:012x}", 0x200 + index),
                ingest_lease_id: format!("019f547b-6200-7000-8000-{:012x}", 0x300 + index),
                draft_id: draft_id.into(),
                display_filename: format!("page-{index}.bin"),
                media_type: "application/octet-stream".into(),
                now_ms: 3 + index as i64,
                limits: MediaLimits::default(),
            }))
            .expect("ingest attachment");
        let upload = client
            .claim_next_offsite_media_upload(
                100 + index as i64,
                format!("019f547b-6200-7000-8000-{:012x}", 0x400 + index),
            )
            .expect("claim upload")
            .expect("pending upload");
        assert!(client
            .complete_offsite_media_upload(
                upload,
                format!("\"remote-version-{index}\""),
                200 + index as i64,
            )
            .expect("complete upload"));
    }

    let checkpoint_id = CheckpointId::new();
    let prepared = client
        .prepare_offsite_checkpoint(input(checkpoint_id.clone(), &config, 500))
        .expect("prepare checkpoint");
    assert_eq!(prepared.referenced_hash_count, 19);

    let mut cursor = None;
    let mut page_sizes = Vec::new();
    let mut captured = Vec::new();
    loop {
        let page = client
            .load_offsite_checkpoint_media_page(checkpoint_id.clone(), cursor, 8)
            .expect("load media page");
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 8);
        page_sizes.push(page.len());
        cursor = page.last().map(|reference| reference.sha256);
        captured.extend(page);
    }
    assert_eq!(captured.len(), 19);
    assert_eq!(page_sizes, [8, 8, 3]);
    assert!(captured
        .windows(2)
        .all(|pair| pair[0].sha256.as_bytes() < pair[1].sha256.as_bytes()));
    assert!(matches!(
        client.load_offsite_checkpoint_media_page(checkpoint_id.clone(), None, 257),
        Err(DatabaseError::InvalidOffsiteCheckpoint(_))
    ));
    client
        .mark_offsite_checkpoint_failed(
            checkpoint_id.clone(),
            CheckpointErrorCode::WorkerUnavailable,
        )
        .expect("fail checkpoint");
    assert!(client
        .load_offsite_checkpoint_media_page(checkpoint_id.clone(), None, 8)
        .expect("load reclaimed page")
        .is_empty());
    assert_eq!(
        database
            .open_main_read_only()
            .expect("read checkpoint header")
            .query_row(
                "SELECT referenced_hash_count
                 FROM offsite_backup_checkpoint
                 WHERE checkpoint_id = ?1",
                [checkpoint_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .expect("retained aggregate evidence"),
        19
    );
}

#[test]
fn startup_reclassifies_interrupted_checkpoint_attempts_as_failed() {
    let (_root, database, config) = enabled_database();
    let checkpoint_id = CheckpointId::new();
    database
        .client()
        .prepare_offsite_checkpoint(input(checkpoint_id.clone(), &config, 4))
        .expect("prepare interrupted checkpoint");
    assert_eq!(
        database
            .client()
            .fail_incomplete_offsite_checkpoints(CheckpointErrorCode::WorkerUnavailable)
            .expect("fail incomplete"),
        1
    );
    assert_eq!(
        database
            .open_main_read_only()
            .expect("read-only main")
            .query_row(
                "SELECT phase FROM offsite_backup_checkpoint WHERE checkpoint_id = ?1",
                [checkpoint_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .expect("phase"),
        CheckpointPhase::Failed.as_db_str()
    );
}

#[test]
fn configuration_changes_wait_for_each_checkpoint_remote_operation_and_revoke_old_lineage() {
    let (_root, database, config) = enabled_database();
    let client = database.client();
    let remote_client = client.clone();
    let remote_config = config.clone();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let remote = thread::spawn(move || {
        remote_client.with_current_offsite_checkpoint(&remote_config, || {
            entered_tx.send(()).expect("entered remote operation");
            release_rx.recv().expect("release remote operation");
            7
        })
    });
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("remote operation started");

    let save_client = client.clone();
    let save_config = config.clone();
    let (saved_tx, saved_rx) = mpsc::sync_channel(1);
    let save = thread::spawn(move || {
        let result = save_client.save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: save_config.revision,
            backup_set_id: save_config.backup_set_id,
            replica_epoch_id: save_config.replica_epoch_id,
            enabled: false,
            target: save_config.target,
            now_ms: 5,
        });
        saved_tx.send(result).expect("send save result");
    });
    assert!(matches!(
        saved_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    release_tx.send(()).expect("release remote");
    assert_eq!(
        remote
            .join()
            .expect("remote thread")
            .expect("remote authorization"),
        Some(7)
    );
    saved_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("save unblocked")
        .expect("disable backup");
    save.join().expect("save thread");

    let mut called = false;
    assert_eq!(
        client
            .with_current_offsite_checkpoint(&config, || {
                called = true;
            })
            .expect("stale authorization"),
        None
    );
    assert!(!called);
}

#[test]
fn publication_requires_the_original_configuration_revision_to_remain_enabled() {
    let (_root, database, config) = enabled_database();
    let client = database.client();
    let checkpoint_id = CheckpointId::new();
    let _prepared = client
        .prepare_offsite_checkpoint(input(checkpoint_id.clone(), &config, 7))
        .expect("prepare");
    let txid = LitestreamTxid::from_local(42);
    client
        .mark_offsite_checkpoint_fenced(checkpoint_id.clone(), txid)
        .expect("fenced");
    client
        .mark_offsite_checkpoint_replicated(checkpoint_id.clone())
        .expect("replicated");
    client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: config.revision,
            backup_set_id: config.backup_set_id,
            replica_epoch_id: config.replica_epoch_id,
            enabled: false,
            target: config.target,
            now_ms: 8,
        })
        .expect("disable backup");

    assert!(matches!(
        client.mark_offsite_checkpoint_published(checkpoint_id, "kosh/v1/test/revoked.json".into()),
        Err(DatabaseError::StaleOffsiteCheckpoint)
    ));
}

#[test]
fn retired_upload_rows_do_not_invalidate_historical_checkpoint_facts() {
    let (root, database, config) = enabled_database();
    let client = database.client();
    let draft_id = "019f547b-6200-7000-8000-000000009001";
    client
        .save_working_copy_for_test(draft_id.into(), None, 1, String::new(), Vec::new(), 2, true)
        .expect("save working copy");
    let staging = root.path().join("staging");
    let staged = StagedAttachment::from_reader(
        Cursor::new(b"historical checkpoint media"),
        &staging,
        "019f547b-6200-7000-8000-000000009002",
        MediaLimits::default().max_attachment_bytes,
    )
    .expect("stage attachment");
    client
        .ingest_attachment(staged.write(IngestAttachmentMetadata {
            attachment_id: "019f547b-6200-7000-8000-000000009003".into(),
            ingest_lease_id: "019f547b-6200-7000-8000-000000009004".into(),
            draft_id: draft_id.into(),
            display_filename: "history.bin".into(),
            media_type: "application/octet-stream".into(),
            now_ms: 3,
            limits: MediaLimits::default(),
        }))
        .expect("ingest attachment");
    let upload = client
        .claim_next_offsite_media_upload(4, "019f547b-6200-7000-8000-000000009005".into())
        .expect("claim upload")
        .expect("pending upload");
    assert!(client
        .complete_offsite_media_upload(upload, "\"remote-version\"".into(), 5)
        .expect("complete upload"));
    let published = publish(&database, &config, 6);
    assert!(client
        .load_offsite_checkpoint_media_page(published.clone(), None, 8)
        .expect("published snapshot is reclaimed")
        .is_empty());

    client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: config.revision,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: true,
            target: config.target,
            now_ms: 7,
        })
        .expect("replace backup set");

    assert_eq!(
        client
            .load_offsite_checkpoint_schedule_state()
            .expect("historical checkpoint remains readable")
            .last_published
            .expect("published checkpoint")
            .checkpoint_id,
        published
    );
}
