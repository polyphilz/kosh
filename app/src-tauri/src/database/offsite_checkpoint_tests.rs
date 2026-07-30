use std::{
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
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
    DatabaseError, DatabasePaths, LocalCheckpointSync, OffsiteBackupConfig,
    PrepareOffsiteCheckpointInput,
};

struct ImmediateSync;

impl LocalCheckpointSync for ImmediateSync {
    fn sync_local(&self) -> Result<LitestreamTxid, CheckpointErrorCode> {
        Ok(LitestreamTxid::from_local(42))
    }
}

struct FailingSync;

impl LocalCheckpointSync for FailingSync {
    fn sync_local(&self) -> Result<LitestreamTxid, CheckpointErrorCode> {
        Err(CheckpointErrorCode::FenceTimeout)
    }
}

struct InspectingSync {
    main_path: PathBuf,
    checkpoint_id: CheckpointId,
    entered: mpsc::SyncSender<()>,
    release: Mutex<Receiver<()>>,
}

impl LocalCheckpointSync for InspectingSync {
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
    let prepared = client
        .prepare_offsite_checkpoint(
            input(checkpoint_id.clone(), config, created_at_ms),
            Arc::new(ImmediateSync),
        )
        .expect("prepare checkpoint");
    client
        .mark_offsite_checkpoint_fenced(checkpoint_id.clone(), prepared.litestream_txid)
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
fn writer_fence_commits_prepared_row_then_blocks_later_messages_until_sync() {
    let (_root, database, config) = enabled_database();
    let checkpoint_id = CheckpointId::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let sync = Arc::new(InspectingSync {
        main_path: database.paths().main.clone(),
        checkpoint_id: checkpoint_id.clone(),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let prepare_client = database.client();
    let prepare_config = config.clone();
    let prepare = thread::spawn(move || {
        prepare_client.prepare_offsite_checkpoint(input(checkpoint_id, &prepare_config, 2), sync)
    });
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer entered local sync");

    let diagnostics_client = database.client();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let diagnostics = thread::spawn(move || {
        let _ = done_tx.send(diagnostics_client.diagnostics());
    });
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    release_tx.send(()).expect("release fence");
    prepare.join().expect("prepare thread").expect("checkpoint");
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("later writer message unblocked")
        .expect("diagnostics");
    diagnostics.join().expect("diagnostics thread");
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
    assert!(matches!(
        database
            .client()
            .prepare_offsite_checkpoint(input(failed.clone(), &config, 2), Arc::new(FailingSync),),
        Err(DatabaseError::OffsiteCheckpointFence(
            CheckpointErrorCode::FenceTimeout
        ))
    ));
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
fn startup_reclassifies_interrupted_checkpoint_attempts_as_failed() {
    let (_root, database, config) = enabled_database();
    let checkpoint_id = CheckpointId::new();
    assert!(matches!(
        database.client().prepare_offsite_checkpoint(
            input(checkpoint_id.clone(), &config, 4),
            Arc::new(FailingSync),
        ),
        Err(DatabaseError::OffsiteCheckpointFence(_))
    ));
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
    let prepared = client
        .prepare_offsite_checkpoint(
            input(checkpoint_id.clone(), &config, 7),
            Arc::new(ImmediateSync),
        )
        .expect("prepare");
    client
        .mark_offsite_checkpoint_fenced(checkpoint_id.clone(), prepared.litestream_txid)
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
