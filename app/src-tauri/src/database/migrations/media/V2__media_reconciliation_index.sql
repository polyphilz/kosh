CREATE INDEX media_blob_lease_hash_expiry_idx
    ON media_blob_lease(sha256, expires_at);
