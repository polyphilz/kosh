CREATE TABLE offsite_backup_checkpoint_media (
    checkpoint_id TEXT NOT NULL
        REFERENCES offsite_backup_checkpoint(checkpoint_id)
        ON DELETE CASCADE,
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    byte_length INTEGER NOT NULL CHECK (byte_length > 0),
    PRIMARY KEY (checkpoint_id, sha256)
) STRICT, WITHOUT ROWID;
