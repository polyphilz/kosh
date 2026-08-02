CREATE TABLE media_blob (
    sha256 BLOB PRIMARY KEY CHECK (length(sha256) = 32),
    bytes BLOB NOT NULL,
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    CONSTRAINT media_blob_size_limit CHECK (byte_length <= 268435456),
    CHECK (byte_length = length(bytes))
) STRICT;

CREATE TABLE media_blob_lease (
    lease_id TEXT NOT NULL
        CHECK (
            length(lease_id) = 36
            AND lower(lease_id) = lease_id
            AND substr(lease_id, 9, 1) = '-'
            AND substr(lease_id, 14, 1) = '-'
            AND substr(lease_id, 15, 1) = '7'
            AND substr(lease_id, 19, 1) = '-'
            AND substr(lease_id, 20, 1) GLOB '[89ab]'
            AND substr(lease_id, 24, 1) = '-'
            AND length(replace(lease_id, '-', '')) = 32
            AND replace(lease_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at),
    PRIMARY KEY (lease_id, sha256),
    FOREIGN KEY (sha256) REFERENCES media_blob(sha256)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE INDEX media_blob_lease_expiry_idx
    ON media_blob_lease(expires_at, lease_id);

CREATE INDEX media_blob_lease_hash_expiry_idx
    ON media_blob_lease(sha256, expires_at);

CREATE TABLE media_blob_reap_authorization (
    sha256 BLOB PRIMARY KEY CHECK (length(sha256) = 32),
    authorized_at INTEGER NOT NULL CHECK (authorized_at >= 0),
    reason TEXT NOT NULL CHECK (length(reason) > 0),
    FOREIGN KEY (sha256) REFERENCES media_blob(sha256)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE TRIGGER media_blob_prevent_update
BEFORE UPDATE ON media_blob
BEGIN
    SELECT RAISE(ABORT, 'media blobs are immutable');
END;

CREATE TRIGGER media_blob_guard_delete
BEFORE DELETE ON media_blob
WHEN NOT EXISTS (
    SELECT 1
    FROM media_blob_reap_authorization
    WHERE sha256 = old.sha256
)
BEGIN
    SELECT RAISE(ABORT, 'media blob deletion requires authorization');
END;
