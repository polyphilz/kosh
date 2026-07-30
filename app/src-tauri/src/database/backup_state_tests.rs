use rusqlite::OptionalExtension;
use tempfile::TempDir;

use crate::backup::domain::{
    BackupProvider, BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target,
    ReplicaEpochId, FIXED_R2_PREFIX,
};

use super::{backup_state::SaveOffsiteBackupConfigInput, Database, DatabaseError, DatabasePaths};

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
