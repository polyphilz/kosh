CREATE TABLE offsite_backup_config_intent (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    operation_id TEXT NOT NULL UNIQUE
        CHECK (
            length(operation_id) = 36
            AND operation_id = lower(operation_id)
        ),
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    backup_set_id TEXT NOT NULL
        CHECK (
            length(backup_set_id) = 36
            AND backup_set_id = lower(backup_set_id)
        ),
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND replica_epoch_id = lower(replica_epoch_id)
        ),
    provider TEXT NOT NULL CHECK (provider = 'R2'),
    jurisdiction TEXT NOT NULL CHECK (jurisdiction IN ('DEFAULT', 'EU', 'FEDRAMP')),
    account_id TEXT NOT NULL
        CHECK (
            length(account_id) = 32
            AND account_id = lower(account_id)
            AND account_id NOT GLOB '*[^0-9a-f]*'
        ),
    bucket TEXT NOT NULL CHECK (length(bucket) BETWEEN 3 AND 63),
    credential_action TEXT NOT NULL CHECK (credential_action IN ('REUSE', 'REPLACE')),
    state TEXT NOT NULL CHECK (state IN ('PENDING', 'COMMITTED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE offsite_backup_takeover_intent (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    operation_id TEXT NOT NULL UNIQUE
        CHECK (
            length(operation_id) = 36
            AND operation_id = lower(operation_id)
        ),
    expected_revision INTEGER NOT NULL CHECK (expected_revision > 0),
    backup_set_id TEXT NOT NULL
        CHECK (
            length(backup_set_id) = 36
            AND backup_set_id = lower(backup_set_id)
        ),
    previous_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(previous_replica_epoch_id) = 36
            AND previous_replica_epoch_id = lower(previous_replica_epoch_id)
        ),
    next_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(next_replica_epoch_id) = 36
            AND next_replica_epoch_id = lower(next_replica_epoch_id)
        ),
    expected_owner_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(expected_owner_replica_epoch_id) = 36
            AND expected_owner_replica_epoch_id = lower(expected_owner_replica_epoch_id)
        ),
    expected_owner_writer_id TEXT NOT NULL
        CHECK (
            length(expected_owner_writer_id) = 64
            AND expected_owner_writer_id = lower(expected_owner_writer_id)
            AND expected_owner_writer_id NOT GLOB '*[^0-9a-f]*'
        ),
    expected_owner_version TEXT NOT NULL
        CHECK (length(expected_owner_version) BETWEEN 1 AND 256),
    next_writer_id TEXT NOT NULL
        CHECK (
            length(next_writer_id) = 64
            AND next_writer_id = lower(next_writer_id)
            AND next_writer_id NOT GLOB '*[^0-9a-f]*'
        ),
    state TEXT NOT NULL CHECK (state IN ('PENDING', 'COMMITTED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK (previous_replica_epoch_id <> next_replica_epoch_id)
) STRICT;
