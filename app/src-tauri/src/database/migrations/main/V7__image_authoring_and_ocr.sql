CREATE TABLE attachment_image (
    attachment_id TEXT PRIMARY KEY,
    preview_sha256 BLOB NOT NULL CHECK (length(preview_sha256) = 32),
    preview_media_type TEXT NOT NULL CHECK (preview_media_type = 'image/webp'),
    preview_byte_length INTEGER NOT NULL CHECK (preview_byte_length > 0),
    natural_width INTEGER NOT NULL CHECK (natural_width > 0),
    natural_height INTEGER NOT NULL CHECK (natural_height > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE INDEX attachment_image_preview_sha256_idx
    ON attachment_image(preview_sha256, attachment_id);

CREATE TRIGGER attachment_image_validate_insert
BEFORE INSERT ON attachment_image
BEGIN
    SELECT RAISE(ABORT, 'image metadata requires an active image attachment')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment
        WHERE attachment.id = new.attachment_id
          AND attachment.kind = 'IMAGE'
          AND attachment.deleted_at IS NULL
    );
END;

CREATE TRIGGER attachment_image_prevent_update
BEFORE UPDATE ON attachment_image
BEGIN
    SELECT RAISE(ABORT, 'canonical image metadata is immutable');
END;

CREATE TABLE image_ocr_queue (
    extraction_id TEXT PRIMARY KEY,
    state TEXT NOT NULL
        CHECK (state IN ('PENDING', 'RUNNING', 'RETRY_WAIT', 'READY', 'FAILED')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at INTEGER CHECK (next_attempt_at IS NULL OR next_attempt_at >= 0),
    started_at INTEGER CHECK (started_at IS NULL OR started_at >= 0),
    last_error TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    FOREIGN KEY (extraction_id) REFERENCES attachment_extraction(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (
            state = 'PENDING'
            AND attempt_count = 0
            AND next_attempt_at IS NOT NULL
            AND started_at IS NULL
            AND last_error IS NULL
        )
        OR (
            state = 'RUNNING'
            AND attempt_count > 0
            AND next_attempt_at IS NULL
            AND started_at IS NOT NULL
        )
        OR (
            state = 'RETRY_WAIT'
            AND attempt_count > 0
            AND next_attempt_at IS NOT NULL
            AND started_at IS NULL
            AND last_error IS NOT NULL
        )
        OR (
            state = 'READY'
            AND next_attempt_at IS NULL
            AND started_at IS NULL
            AND last_error IS NULL
        )
        OR (
            state = 'FAILED'
            AND attempt_count > 0
            AND next_attempt_at IS NULL
            AND started_at IS NULL
            AND last_error IS NOT NULL
        )
    )
) STRICT, WITHOUT ROWID;

CREATE INDEX image_ocr_queue_eligible_idx
    ON image_ocr_queue(next_attempt_at, extraction_id)
    WHERE state IN ('PENDING', 'RETRY_WAIT');

CREATE INDEX image_ocr_queue_running_idx
    ON image_ocr_queue(started_at, extraction_id)
    WHERE state = 'RUNNING';

CREATE TRIGGER image_ocr_queue_validate_insert
BEFORE INSERT ON image_ocr_queue
BEGIN
    SELECT RAISE(ABORT, 'OCR queue entries require current image extraction provenance')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment_extraction AS extraction
        JOIN attachment_extractor_config AS config
          ON config.extractor = extraction.extractor
         AND config.version = extraction.extractor_version
        JOIN attachment
          ON attachment.id = extraction.attachment_id
         AND attachment.sha256 = extraction.content_hash
         AND attachment.kind = 'IMAGE'
         AND attachment.deleted_at IS NULL
        JOIN attachment_image AS image
          ON image.attachment_id = attachment.id
        WHERE extraction.id = new.extraction_id
          AND extraction.extractor = 'ocr'
    );
END;

CREATE TRIGGER image_ocr_queue_identity_prevent_update
BEFORE UPDATE OF extraction_id ON image_ocr_queue
BEGIN
    SELECT RAISE(ABORT, 'OCR queue identity is immutable');
END;
