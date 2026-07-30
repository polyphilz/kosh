CREATE TABLE offsite_backup_config (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    backup_set_id TEXT NOT NULL UNIQUE
        CHECK (
            length(backup_set_id) = 36
            AND lower(backup_set_id) = backup_set_id
            AND substr(backup_set_id, 9, 1) = '-'
            AND substr(backup_set_id, 14, 1) = '-'
            AND substr(backup_set_id, 15, 1) = '7'
            AND substr(backup_set_id, 19, 1) = '-'
            AND substr(backup_set_id, 20, 1) GLOB '[89ab]'
            AND substr(backup_set_id, 24, 1) = '-'
            AND length(replace(backup_set_id, '-', '')) = 32
            AND replace(backup_set_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND lower(replica_epoch_id) = replica_epoch_id
            AND substr(replica_epoch_id, 9, 1) = '-'
            AND substr(replica_epoch_id, 14, 1) = '-'
            AND substr(replica_epoch_id, 15, 1) = '7'
            AND substr(replica_epoch_id, 19, 1) = '-'
            AND substr(replica_epoch_id, 20, 1) GLOB '[89ab]'
            AND substr(replica_epoch_id, 24, 1) = '-'
            AND length(replace(replica_epoch_id, '-', '')) = 32
            AND replace(replica_epoch_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    provider TEXT NOT NULL CHECK (provider = 'R2'),
    jurisdiction TEXT NOT NULL
        CHECK (jurisdiction IN ('DEFAULT', 'EU', 'FEDRAMP')),
    account_id TEXT NOT NULL
        CHECK (
            length(account_id) = 32
            AND lower(account_id) = account_id
            AND account_id NOT GLOB '*[^0-9a-f]*'
        ),
    bucket TEXT NOT NULL
        CHECK (
            length(bucket) BETWEEN 3 AND 63
            AND lower(bucket) = bucket
            AND bucket NOT GLOB '*[^a-z0-9-]*'
            AND substr(bucket, 1, 1) GLOB '[a-z0-9]'
            AND substr(bucket, -1, 1) GLOB '[a-z0-9]'
        ),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE offsite_credential_cleanup (
    backup_set_id TEXT PRIMARY KEY
        CHECK (
            length(backup_set_id) = 36
            AND lower(backup_set_id) = backup_set_id
            AND substr(backup_set_id, 9, 1) = '-'
            AND substr(backup_set_id, 14, 1) = '-'
            AND substr(backup_set_id, 15, 1) = '7'
            AND substr(backup_set_id, 19, 1) = '-'
            AND substr(backup_set_id, 20, 1) GLOB '[89ab]'
            AND substr(backup_set_id, 24, 1) = '-'
            AND length(replace(backup_set_id, '-', '')) = 32
            AND replace(backup_set_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
) STRICT;
