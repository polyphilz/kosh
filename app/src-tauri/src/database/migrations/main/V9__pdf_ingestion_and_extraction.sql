CREATE TABLE attachment_pdf (
    attachment_id TEXT PRIMARY KEY,
    page_count INTEGER NOT NULL CHECK (page_count > 0 AND page_count <= 2000),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE TRIGGER attachment_pdf_validate_insert
BEFORE INSERT ON attachment_pdf
BEGIN
    SELECT RAISE(ABORT, 'PDF metadata requires an active PDF attachment')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment
        WHERE attachment.id = new.attachment_id
          AND attachment.kind = 'PDF'
          AND attachment.media_type = 'application/pdf'
          AND attachment.deleted_at IS NULL
    );
END;

CREATE TRIGGER attachment_pdf_prevent_update
BEFORE UPDATE ON attachment_pdf
BEGIN
    SELECT RAISE(ABORT, 'canonical PDF metadata is immutable');
END;

CREATE TABLE pdf_extraction_queue (
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

CREATE INDEX pdf_extraction_queue_eligible_idx
    ON pdf_extraction_queue(next_attempt_at, extraction_id)
    WHERE state IN ('PENDING', 'RETRY_WAIT');

CREATE INDEX pdf_extraction_queue_running_idx
    ON pdf_extraction_queue(started_at, extraction_id)
    WHERE state = 'RUNNING';

CREATE TRIGGER pdf_extraction_queue_validate_insert
BEFORE INSERT ON pdf_extraction_queue
BEGIN
    SELECT RAISE(ABORT, 'PDF queue entries require current PDF extraction provenance')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment_extraction AS extraction
        JOIN attachment_extractor_config AS config
          ON config.extractor = extraction.extractor
         AND config.version = extraction.extractor_version
        JOIN attachment
          ON attachment.id = extraction.attachment_id
         AND attachment.sha256 = extraction.content_hash
         AND attachment.kind = 'PDF'
         AND attachment.deleted_at IS NULL
        JOIN attachment_pdf AS pdf
          ON pdf.attachment_id = attachment.id
        WHERE extraction.id = new.extraction_id
          AND extraction.extractor = 'pdf-text'
    );
END;

CREATE TRIGGER pdf_extraction_queue_identity_prevent_update
BEFORE UPDATE OF extraction_id ON pdf_extraction_queue
BEGIN
    SELECT RAISE(ABORT, 'PDF extraction queue identity is immutable');
END;

CREATE TABLE pdf_page_extraction (
    extraction_id TEXT NOT NULL,
    page_number INTEGER NOT NULL CHECK (page_number > 0),
    source TEXT NOT NULL CHECK (source IN ('NATIVE_TEXT', 'OCR', 'UNAVAILABLE')),
    segment_id TEXT,
    error TEXT,
    PRIMARY KEY (extraction_id, page_number),
    FOREIGN KEY (extraction_id) REFERENCES attachment_extraction(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (segment_id) REFERENCES attachment_segment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (source IN ('NATIVE_TEXT', 'OCR') AND segment_id IS NOT NULL AND error IS NULL)
        OR (source = 'UNAVAILABLE' AND segment_id IS NULL AND error IS NOT NULL)
    )
) STRICT, WITHOUT ROWID;

CREATE TRIGGER pdf_page_extraction_validate_insert
BEFORE INSERT ON pdf_page_extraction
BEGIN
    SELECT RAISE(ABORT, 'PDF page extraction does not match its immutable provenance')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment_extraction AS extraction
        JOIN attachment_pdf AS pdf
          ON pdf.attachment_id = extraction.attachment_id
        WHERE extraction.id = new.extraction_id
          AND extraction.extractor = 'pdf-text'
          AND extraction.status = 'READY'
          AND new.page_number <= pdf.page_count
          AND (
              (
                  new.source IN ('NATIVE_TEXT', 'OCR')
                  AND EXISTS (
                      SELECT 1
                      FROM attachment_segment AS segment
                      WHERE segment.id = new.segment_id
                        AND segment.extraction_id = new.extraction_id
                        AND segment.locator_kind = 'PDF_PAGE'
                        AND segment.page_number = new.page_number
                  )
              )
              OR new.source = 'UNAVAILABLE'
          )
    );
END;

CREATE TRIGGER pdf_page_extraction_prevent_update
BEFORE UPDATE ON pdf_page_extraction
BEGIN
    SELECT RAISE(ABORT, 'PDF page extraction outcomes are immutable');
END;

CREATE TRIGGER pdf_page_extraction_prevent_delete
BEFORE DELETE ON pdf_page_extraction
BEGIN
    SELECT RAISE(ABORT, 'PDF page extraction outcomes are retained');
END;

CREATE TABLE attachment_passage_revision (
    passage_id TEXT PRIMARY KEY,
    tidbit_revision_id TEXT NOT NULL,
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE TRIGGER attachment_passage_revision_validate_insert
BEFORE INSERT ON attachment_passage_revision
BEGIN
    SELECT RAISE(ABORT, 'attachment passage revision does not own the cited attachment')
    WHERE NOT EXISTS (
        SELECT 1
        FROM passage
        JOIN attachment_segment AS segment
          ON segment.id = passage.attachment_segment_id
        JOIN attachment_extraction AS extraction
          ON extraction.id = segment.extraction_id
        JOIN tidbit_revision_attachment AS membership
          ON membership.attachment_id = extraction.attachment_id
         AND membership.tidbit_revision_id = new.tidbit_revision_id
        WHERE passage.id = new.passage_id
          AND passage.owner_kind = 'ATTACHMENT'
    );
END;

CREATE TRIGGER attachment_passage_revision_prevent_update
BEFORE UPDATE ON attachment_passage_revision
BEGIN
    SELECT RAISE(ABORT, 'attachment passage revision provenance is immutable');
END;

CREATE TRIGGER attachment_passage_revision_prevent_delete
BEFORE DELETE ON attachment_passage_revision
BEGIN
    SELECT RAISE(ABORT, 'attachment passage revision provenance is retained');
END;

CREATE TRIGGER attachment_passage_revision_after_membership
AFTER INSERT ON tidbit_revision_attachment
BEGIN
    INSERT OR IGNORE INTO attachment_passage_revision(passage_id, tidbit_revision_id)
    SELECT passage.id, new.tidbit_revision_id
    FROM passage
    JOIN attachment_segment AS segment
      ON segment.id = passage.attachment_segment_id
    JOIN attachment_extraction AS extraction
      ON extraction.id = segment.extraction_id
    WHERE passage.owner_kind = 'ATTACHMENT'
      AND extraction.attachment_id = new.attachment_id;
END;

CREATE TRIGGER attachment_passage_revision_after_passage
AFTER INSERT ON passage
WHEN new.owner_kind = 'ATTACHMENT'
BEGIN
    INSERT OR IGNORE INTO attachment_passage_revision(passage_id, tidbit_revision_id)
    SELECT new.id, membership.tidbit_revision_id
    FROM attachment_segment AS segment
    JOIN attachment_extraction AS extraction
      ON extraction.id = segment.extraction_id
    JOIN tidbit_revision_attachment AS membership
      ON membership.attachment_id = extraction.attachment_id
    JOIN tidbit_revision AS revision
      ON revision.id = membership.tidbit_revision_id
    WHERE segment.id = new.attachment_segment_id
    ORDER BY revision.created_at, revision.id
    LIMIT 1;
END;

INSERT INTO attachment_passage_revision(passage_id, tidbit_revision_id)
SELECT
    passage.id,
    (
        SELECT membership.tidbit_revision_id
        FROM tidbit_revision_attachment AS membership
        JOIN tidbit_revision AS revision
          ON revision.id = membership.tidbit_revision_id
        WHERE membership.attachment_id = extraction.attachment_id
        ORDER BY revision.created_at, revision.id
        LIMIT 1
    )
FROM passage
JOIN attachment_segment AS segment
  ON segment.id = passage.attachment_segment_id
JOIN attachment_extraction AS extraction
  ON extraction.id = segment.extraction_id
WHERE passage.owner_kind = 'ATTACHMENT'
  AND EXISTS (
      SELECT 1
      FROM tidbit_revision_attachment AS membership
      WHERE membership.attachment_id = extraction.attachment_id
  );
