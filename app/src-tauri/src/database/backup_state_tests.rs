use rusqlite::OptionalExtension;
use tempfile::TempDir;

use crate::backup::domain::{
    BackupProvider, BackupSetId, BackupWriterId, R2AccountId, R2BucketName, R2Jurisdiction,
    R2Target, ReplicaEpochId, FIXED_R2_PREFIX,
};

use super::{
    backup_state::{
        BeginOffsiteBackupConfigIntentInput, CredentialIntentAction, OffsiteBackupTakeoverIntent,
        OffsiteOperationState, SaveOffsiteBackupConfigInput,
    },
    Database, DatabaseError, DatabasePaths,
};

const ACCOUNT_ID: &str = "0123456789abcdef0123456789abcdef";

fn target(jurisdiction: R2Jurisdiction, bucket: &str) -> R2Target {
    R2Target {
        account_id: R2AccountId::parse(ACCOUNT_ID).expect("account"),
        jurisdiction,
        bucket: R2BucketName::parse(bucket).expect("bucket"),
    }
}

#[test]
fn non_secret_backup_state_is_revision_guarded_and_survives_restart() {
    let root = TempDir::new().expect("temporary root");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("database");
    let client = database.client();
    assert_eq!(client.load_offsite_backup_config().expect("load"), None);
    assert_eq!(
        client
            .load_enabled_offsite_backup_config()
            .expect("enabled load"),
        None
    );

    let backup_set_id = BackupSetId::new();
    let replica_epoch_id = ReplicaEpochId::new();
    let saved = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: replica_epoch_id.clone(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 1_000,
        })
        .expect("save");
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.backup_set_id, backup_set_id);
    assert_eq!(saved.replica_epoch_id, replica_epoch_id);
    assert_eq!(saved.provider, BackupProvider::R2);
    assert!(!saved.enabled);
    assert_eq!(saved.created_at_ms, 1_000);
    assert_eq!(saved.updated_at_ms, 1_000);
    assert_eq!(
        saved
            .target
            .keyspace(&saved.backup_set_id)
            .root_prefix()
            .as_str(),
        format!("{FIXED_R2_PREFIX}/{}/", saved.backup_set_id)
    );
    assert_eq!(
        client
            .load_enabled_offsite_backup_config()
            .expect("disabled config"),
        None
    );

    let enabled = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: saved.revision,
            backup_set_id: saved.backup_set_id.clone(),
            replica_epoch_id: saved.replica_epoch_id.clone(),
            enabled: true,
            target: target(R2Jurisdiction::Eu, "kosh-local"),
            now_ms: 2_000,
        })
        .expect("enable");
    assert_eq!(enabled.revision, 2);
    assert!(enabled.enabled);
    assert_eq!(enabled.created_at_ms, 1_000);
    assert_eq!(enabled.updated_at_ms, 2_000);
    assert_eq!(enabled.target.jurisdiction, R2Jurisdiction::Eu);

    assert!(matches!(
        client.save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 1,
            backup_set_id: enabled.backup_set_id.clone(),
            replica_epoch_id: enabled.replica_epoch_id.clone(),
            enabled: false,
            target: enabled.target.clone(),
            now_ms: 3_000,
        }),
        Err(DatabaseError::StaleOffsiteBackupConfig)
    ));
    database.shutdown().expect("shutdown");

    let reopened = Database::initialize(paths).expect("reopened database");
    assert_eq!(
        reopened
            .client()
            .load_enabled_offsite_backup_config()
            .expect("reloaded config"),
        Some(enabled)
    );
}

#[test]
fn replacing_a_backup_set_durably_queues_keychain_cleanup() {
    let root = TempDir::new().expect("temporary root");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
    let client = database.client();
    let first = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 10,
        })
        .expect("first config");
    let second = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: first.revision,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Fedramp, "kosh-local"),
            now_ms: 20,
        })
        .expect("replacement config");

    assert_eq!(
        client
            .load_offsite_credential_cleanup()
            .expect("credential cleanup"),
        vec![first.backup_set_id.clone()]
    );
    assert!(matches!(
        client.save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: second.revision,
            backup_set_id: first.backup_set_id.clone(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 25,
        }),
        Err(DatabaseError::OffsiteBackupSetPendingCredentialCleanup {
            backup_set_id
        }) if backup_set_id == first.backup_set_id.to_string()
    ));
    assert_eq!(
        client
            .load_offsite_backup_config()
            .expect("unchanged configuration"),
        Some(second.clone())
    );
    client
        .complete_offsite_credential_cleanup(first.backup_set_id.clone())
        .expect("complete cleanup");
    assert!(client
        .load_offsite_credential_cleanup()
        .expect("empty cleanup")
        .is_empty());

    let third = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: second.revision,
            backup_set_id: first.backup_set_id.clone(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 30,
        })
        .expect("reuse first set");
    assert_eq!(third.backup_set_id, first.backup_set_id);
    assert_eq!(
        client
            .load_offsite_credential_cleanup()
            .expect("only retired second set"),
        vec![second.backup_set_id]
    );
}

#[test]
fn credential_cleanup_completion_is_idempotent_for_unqueued_sets() {
    let root = TempDir::new().expect("temporary root");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
    let client = database.client();
    let active = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 10,
        })
        .expect("active config");

    client
        .complete_offsite_credential_cleanup(active.backup_set_id.clone())
        .expect("unqueued completion");
    assert_eq!(
        client
            .load_offsite_backup_config()
            .expect("active configuration"),
        Some(active)
    );
}

#[test]
fn recovery_target_intent_is_durable_and_commits_before_being_cleared() {
    let root = TempDir::new().expect("temporary root");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("database");
    let client = database.client();
    let operation_id = uuid::Uuid::now_v7().to_string();
    let backup_set_id = BackupSetId::new();
    let replica_epoch_id = ReplicaEpochId::new();
    client
        .begin_offsite_backup_config_intent(BeginOffsiteBackupConfigIntentInput {
            operation_id: operation_id.clone(),
            proposed: SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id: backup_set_id.clone(),
                replica_epoch_id: replica_epoch_id.clone(),
                enabled: false,
                target: target(R2Jurisdiction::Default, "kosh-local"),
                now_ms: 100,
            },
            credential_action: CredentialIntentAction::Replace,
        })
        .expect("begin intent");
    assert!(matches!(
        client.save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "other-target"),
            now_ms: 101,
        }),
        Err(DatabaseError::OffsiteBackupOperationPending)
    ));
    database.shutdown().expect("shutdown");

    let reopened = Database::initialize(paths).expect("reopened database");
    let client = reopened.client();
    let pending = client
        .load_offsite_backup_config_intent()
        .expect("load intent")
        .expect("durable intent");
    assert_eq!(pending.operation_id, operation_id);
    assert_eq!(pending.state, OffsiteOperationState::Pending);
    assert_eq!(pending.proposed.backup_set_id, backup_set_id);
    assert_eq!(pending.proposed.replica_epoch_id, replica_epoch_id);
    let saved = client
        .commit_offsite_backup_config_intent(operation_id.clone())
        .expect("commit intent");
    assert_eq!(saved.revision, 1);
    assert_eq!(
        client
            .load_offsite_backup_config_intent()
            .expect("committed intent")
            .expect("intent retained")
            .state,
        OffsiteOperationState::Committed
    );
    client
        .complete_offsite_backup_config_intent(operation_id)
        .expect("complete intent");
    assert!(client
        .load_offsite_backup_config_intent()
        .expect("cleared intent")
        .is_none());
    assert_eq!(
        client.load_offsite_backup_config().expect("config"),
        Some(saved)
    );
}

#[test]
fn pending_recovery_target_intent_can_abort_without_mutating_configuration() {
    let root = TempDir::new().expect("temporary root");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
    let client = database.client();
    let operation_id = uuid::Uuid::now_v7().to_string();
    client
        .begin_offsite_backup_config_intent(BeginOffsiteBackupConfigIntentInput {
            operation_id: operation_id.clone(),
            proposed: SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id: BackupSetId::new(),
                replica_epoch_id: ReplicaEpochId::new(),
                enabled: false,
                target: target(R2Jurisdiction::Default, "kosh-local"),
                now_ms: 100,
            },
            credential_action: CredentialIntentAction::Replace,
        })
        .expect("begin intent");
    client
        .abort_offsite_backup_config_intent(operation_id)
        .expect("abort intent");
    assert_eq!(
        client.load_offsite_backup_config().expect("configuration"),
        None
    );
    assert!(client
        .load_offsite_backup_config_intent()
        .expect("intent")
        .is_none());
}

#[test]
fn takeover_intent_atomically_advances_the_local_epoch() {
    let root = TempDir::new().expect("temporary root");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
    let client = database.client();
    let current = client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: BackupSetId::new(),
            replica_epoch_id: ReplicaEpochId::new(),
            enabled: false,
            target: target(R2Jurisdiction::Default, "kosh-local"),
            now_ms: 100,
        })
        .expect("configuration");
    let operation_id = uuid::Uuid::now_v7().to_string();
    let next_replica_epoch_id = ReplicaEpochId::new();
    client
        .begin_offsite_backup_takeover_intent(OffsiteBackupTakeoverIntent {
            operation_id: operation_id.clone(),
            expected_revision: current.revision,
            backup_set_id: current.backup_set_id.clone(),
            previous_replica_epoch_id: current.replica_epoch_id.clone(),
            next_replica_epoch_id: next_replica_epoch_id.clone(),
            expected_owner_replica_epoch_id: ReplicaEpochId::new(),
            expected_owner_writer_id: BackupWriterId::new(),
            expected_owner_version: "owner-version-1".into(),
            next_writer_id: BackupWriterId::new(),
            created_at_ms: 200,
        })
        .expect("begin takeover");
    assert!(client
        .load_offsite_backup_takeover_intent()
        .expect("takeover intent")
        .is_some());
    let updated = client
        .commit_offsite_backup_takeover_intent(operation_id)
        .expect("commit takeover");
    assert_eq!(updated.revision, current.revision + 1);
    assert_eq!(updated.replica_epoch_id, next_replica_epoch_id);
    assert!(client
        .load_offsite_backup_takeover_intent()
        .expect("cleared takeover")
        .is_none());
}

#[test]
fn sqlite_schema_cannot_store_access_or_secret_credentials_or_custom_prefixes() {
    let root = TempDir::new().expect("temporary root");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
    let connection = database.open_main_read_only().expect("read-only database");
    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('offsite_backup_config')")
        .expect("table info")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("column names");
    assert_eq!(
        columns,
        [
            "singleton_id",
            "revision",
            "backup_set_id",
            "replica_epoch_id",
            "enabled",
            "provider",
            "jurisdiction",
            "account_id",
            "bucket",
            "created_at",
            "updated_at",
        ]
    );
    assert!(columns.iter().all(|name| {
        !name.contains("secret") && !name.contains("access") && !name.contains("prefix")
    }));
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'offsite_backup_config'",
            [],
            |row| row.get(0),
        )
        .expect("schema SQL");
    assert!(!sql.to_ascii_lowercase().contains("secret"));
    assert!(!sql.to_ascii_lowercase().contains("access_key"));
    assert!(!sql.to_ascii_lowercase().contains("prefix"));

    let credential_like_table = connection
        .query_row(
            "SELECT name
             FROM sqlite_schema
             WHERE type = 'table'
               AND (
                    lower(sql) LIKE '%secret_access_key%'
                    OR lower(sql) LIKE '%access_key_id%'
               )
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .expect("credential schema scan");
    assert_eq!(credential_like_table, None);
}
