CREATE TABLE offsite_media_upload (
    backup_set_id TEXT NOT NULL
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
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    state TEXT NOT NULL
        CHECK (
            state IN (
                'PENDING',
                'RUNNING',
                'RETRY_WAIT',
                'UPLOADED',
                'FAILED'
            )
        ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at INTEGER CHECK (
        next_attempt_at IS NULL OR next_attempt_at >= 0
    ),
    lease_id TEXT
        CHECK (
            lease_id IS NULL
            OR (
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
            )
        ),
    started_at INTEGER CHECK (started_at IS NULL OR started_at >= 0),
    uploaded_at INTEGER CHECK (uploaded_at IS NULL OR uploaded_at >= 0),
    remote_version TEXT CHECK (
        remote_version IS NULL
        OR (
            length(remote_version) BETWEEN 1 AND 256
            AND instr(remote_version, char(0)) = 0
            AND instr(remote_version, char(10)) = 0
            AND instr(remote_version, char(13)) = 0
        )
    ),
    last_error_code TEXT CHECK (
        last_error_code IS NULL
        OR (
            length(last_error_code) BETWEEN 1 AND 64
            AND last_error_code NOT GLOB '*[^A-Z0-9_]*'
        )
    ),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    PRIMARY KEY (backup_set_id, sha256),
    CHECK (
        (
            state = 'PENDING'
            AND attempt_count = 0
            AND next_attempt_at IS NOT NULL
            AND lease_id IS NULL
            AND started_at IS NULL
            AND uploaded_at IS NULL
            AND remote_version IS NULL
            AND last_error_code IS NULL
        )
        OR (
            state = 'RUNNING'
            AND attempt_count > 0
            AND next_attempt_at IS NULL
            AND lease_id IS NOT NULL
            AND started_at IS NOT NULL
            AND uploaded_at IS NULL
            AND remote_version IS NULL
            AND last_error_code IS NULL
        )
        OR (
            state = 'RETRY_WAIT'
            AND attempt_count > 0
            AND next_attempt_at IS NOT NULL
            AND lease_id IS NULL
            AND started_at IS NULL
            AND uploaded_at IS NULL
            AND remote_version IS NULL
            AND last_error_code IS NOT NULL
        )
        OR (
            state = 'UPLOADED'
            AND attempt_count > 0
            AND next_attempt_at IS NULL
            AND lease_id IS NULL
            AND started_at IS NULL
            AND uploaded_at IS NOT NULL
            AND remote_version IS NOT NULL
            AND last_error_code IS NULL
        )
        OR (
            state = 'FAILED'
            AND attempt_count > 0
            AND next_attempt_at IS NULL
            AND lease_id IS NULL
            AND started_at IS NULL
            AND uploaded_at IS NULL
            AND remote_version IS NULL
            AND last_error_code IS NOT NULL
        )
    )
) STRICT, WITHOUT ROWID;

CREATE INDEX offsite_media_upload_eligible_idx
    ON offsite_media_upload(
        backup_set_id,
        next_attempt_at,
        sha256
    )
    WHERE state IN ('PENDING', 'RETRY_WAIT');

CREATE INDEX offsite_media_upload_running_idx
    ON offsite_media_upload(started_at, backup_set_id, sha256)
    WHERE state = 'RUNNING';

INSERT OR IGNORE INTO offsite_media_upload(
    backup_set_id,
    sha256,
    state,
    attempt_count,
    next_attempt_at,
    created_at,
    updated_at
)
SELECT
    config.backup_set_id,
    referenced.sha256,
    'PENDING',
    0,
    config.updated_at,
    config.updated_at,
    config.updated_at
FROM offsite_backup_config AS config
JOIN (
    SELECT attachment.sha256
    FROM attachment
    WHERE attachment.deleted_at IS NULL
       OR EXISTS (
            SELECT 1
            FROM tidbit_revision_attachment AS membership
            WHERE membership.attachment_id = attachment.id
       )
       OR EXISTS (
            SELECT 1
            FROM research_run_attachment AS research_membership
            WHERE research_membership.attachment_id = attachment.id
       )
    UNION
    SELECT image.preview_sha256
    FROM attachment_image AS image
    JOIN attachment ON attachment.id = image.attachment_id
    WHERE attachment.deleted_at IS NULL
       OR EXISTS (
            SELECT 1
            FROM tidbit_revision_attachment AS membership
            WHERE membership.attachment_id = attachment.id
       )
       OR EXISTS (
            SELECT 1
            FROM research_run_attachment AS research_membership
            WHERE research_membership.attachment_id = attachment.id
       )
) AS referenced
WHERE config.singleton_id = 1
  AND config.enabled = 1;

CREATE TRIGGER offsite_media_upload_attachment_after_insert
AFTER INSERT ON attachment
WHEN new.deleted_at IS NULL
BEGIN
    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        backup_set_id,
        new.sha256,
        'PENDING',
        0,
        new.created_at,
        new.created_at,
        new.created_at
    FROM offsite_backup_config
    WHERE singleton_id = 1
      AND enabled = 1;
END;

CREATE TRIGGER offsite_media_upload_attachment_after_restore
AFTER UPDATE OF deleted_at ON attachment
WHEN new.deleted_at IS NULL
  OR EXISTS (
      SELECT 1
      FROM tidbit_revision_attachment AS membership
      WHERE membership.attachment_id = new.id
  )
  OR EXISTS (
      SELECT 1
      FROM research_run_attachment AS research_membership
      WHERE research_membership.attachment_id = new.id
  )
BEGIN
    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        backup_set_id,
        new.sha256,
        'PENDING',
        0,
        new.updated_at,
        new.updated_at,
        new.updated_at
    FROM offsite_backup_config
    WHERE singleton_id = 1
      AND enabled = 1;

    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        config.backup_set_id,
        image.preview_sha256,
        'PENDING',
        0,
        new.updated_at,
        new.updated_at,
        new.updated_at
    FROM offsite_backup_config AS config
    JOIN attachment_image AS image ON image.attachment_id = new.id
    WHERE config.singleton_id = 1
      AND config.enabled = 1;
END;

CREATE TRIGGER offsite_media_upload_image_after_insert
AFTER INSERT ON attachment_image
BEGIN
    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        backup_set_id,
        new.preview_sha256,
        'PENDING',
        0,
        new.created_at,
        new.created_at,
        new.created_at
    FROM offsite_backup_config
    WHERE singleton_id = 1
      AND enabled = 1;
END;

CREATE TRIGGER offsite_media_upload_revision_membership_after_insert
AFTER INSERT ON tidbit_revision_attachment
BEGIN
    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        config.backup_set_id,
        attachment.sha256,
        'PENDING',
        0,
        attachment.updated_at,
        attachment.updated_at,
        attachment.updated_at
    FROM offsite_backup_config AS config
    JOIN attachment ON attachment.id = new.attachment_id
    WHERE config.singleton_id = 1
      AND config.enabled = 1;

    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        config.backup_set_id,
        image.preview_sha256,
        'PENDING',
        0,
        attachment.updated_at,
        attachment.updated_at,
        attachment.updated_at
    FROM offsite_backup_config AS config
    JOIN attachment ON attachment.id = new.attachment_id
    JOIN attachment_image AS image ON image.attachment_id = attachment.id
    WHERE config.singleton_id = 1
      AND config.enabled = 1;
END;

CREATE TRIGGER offsite_media_upload_research_membership_after_insert
AFTER INSERT ON research_run_attachment
BEGIN
    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        config.backup_set_id,
        attachment.sha256,
        'PENDING',
        0,
        attachment.updated_at,
        attachment.updated_at,
        attachment.updated_at
    FROM offsite_backup_config AS config
    JOIN attachment ON attachment.id = new.attachment_id
    WHERE config.singleton_id = 1
      AND config.enabled = 1;

    INSERT OR IGNORE INTO offsite_media_upload(
        backup_set_id,
        sha256,
        state,
        attempt_count,
        next_attempt_at,
        created_at,
        updated_at
    )
    SELECT
        config.backup_set_id,
        image.preview_sha256,
        'PENDING',
        0,
        attachment.updated_at,
        attachment.updated_at,
        attachment.updated_at
    FROM offsite_backup_config AS config
    JOIN attachment ON attachment.id = new.attachment_id
    JOIN attachment_image AS image ON image.attachment_id = attachment.id
    WHERE config.singleton_id = 1
      AND config.enabled = 1;
END;
