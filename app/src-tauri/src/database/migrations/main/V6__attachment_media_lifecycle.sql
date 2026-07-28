DROP TRIGGER attachment_extractor_config_search_after_version_update;
DROP TRIGGER passage_attachment_locator_validate;
DROP TRIGGER tidbit_revision_attachment_search_after_insert;
DROP TRIGGER attachment_search_refresh_after_update;
DROP VIEW current_attachment_passage;

CREATE TABLE attachment_v6 (
    id TEXT PRIMARY KEY
        CHECK (
            length(id) = 36
            AND lower(id) = id
            AND substr(id, 9, 1) = '-'
            AND substr(id, 14, 1) = '-'
            AND substr(id, 15, 1) = '7'
            AND substr(id, 19, 1) = '-'
            AND substr(id, 20, 1) GLOB '[89ab]'
            AND substr(id, 24, 1) = '-'
            AND length(replace(id, '-', '')) = 32
            AND replace(id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    deleted_at INTEGER CHECK (deleted_at IS NULL OR deleted_at >= created_at),
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    display_filename TEXT NOT NULL CHECK (length(display_filename) > 0),
    media_type TEXT NOT NULL CHECK (length(media_type) > 0),
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('IMAGE', 'PDF', 'TEXT', 'BINARY')),
    extraction_state TEXT NOT NULL
        CHECK (extraction_state IN ('PENDING', 'READY', 'FAILED', 'NOT_APPLICABLE')),
    UNIQUE (id, sha256)
) STRICT;

INSERT INTO attachment_v6(
    id,
    created_at,
    updated_at,
    deleted_at,
    sha256,
    display_filename,
    media_type,
    byte_length,
    kind,
    extraction_state
)
SELECT
    id,
    created_at,
    updated_at,
    deleted_at,
    sha256,
    display_filename,
    media_type,
    byte_length,
    kind,
    extraction_state
FROM attachment;

DROP TABLE attachment;
ALTER TABLE attachment_v6 RENAME TO attachment;

CREATE INDEX attachment_sha256_idx
    ON attachment(sha256, id);

CREATE INDEX media_ingest_lease_attachment_idx
    ON media_ingest_lease(attachment_id, state, expires_at);

CREATE TABLE media_blob_reap_candidate (
    sha256 BLOB PRIMARY KEY CHECK (length(sha256) = 32),
    orphaned_at INTEGER NOT NULL CHECK (orphaned_at >= 0),
    reason TEXT NOT NULL CHECK (length(reason) > 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX media_blob_reap_candidate_age_idx
    ON media_blob_reap_candidate(orphaned_at, sha256);

CREATE TRIGGER attachment_identity_prevent_update
BEFORE UPDATE OF created_at, sha256, byte_length, kind ON attachment
BEGIN
    SELECT RAISE(ABORT, 'attachment identity is immutable');
END;

CREATE VIEW current_attachment_passage AS
SELECT
    passage.id AS passage_id,
    attachment.id AS attachment_id,
    extraction.id AS extraction_id
FROM passage
JOIN attachment_segment AS segment
  ON segment.id = passage.attachment_segment_id
JOIN attachment_extraction AS extraction
  ON extraction.id = segment.extraction_id
 AND extraction.status = 'READY'
JOIN attachment_extractor_config AS extractor_config
  ON extractor_config.extractor = extraction.extractor
 AND extractor_config.version = extraction.extractor_version
JOIN attachment
  ON attachment.id = extraction.attachment_id
 AND attachment.sha256 = extraction.content_hash
 AND attachment.deleted_at IS NULL
WHERE passage.owner_kind = 'ATTACHMENT'
  AND passage.content = segment.content
  AND passage.content_hash = segment.content_hash
  AND passage.construction_version =
      extractor_config.passage_construction_version;

CREATE TRIGGER passage_attachment_locator_validate
BEFORE INSERT ON passage
WHEN new.owner_kind = 'ATTACHMENT'
BEGIN
    SELECT RAISE(ABORT, 'passage locator does not match attachment segment')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment_segment AS segment
        JOIN attachment_extraction AS extraction
          ON extraction.id = segment.extraction_id
        JOIN attachment
          ON attachment.id = extraction.attachment_id
         AND attachment.sha256 = extraction.content_hash
        WHERE segment.id = new.attachment_segment_id
          AND extraction.status = 'READY'
          AND segment.locator_kind = new.locator_kind
          AND (
              (
                  segment.locator_kind = 'PDF_PAGE'
                  AND segment.page_number = json_extract(new.locator_json, '$.page')
              )
              OR (
                  segment.locator_kind = 'OCR_REGION'
                  AND coalesce(segment.page_number, -1)
                      = coalesce(json_extract(new.locator_json, '$.page'), -1)
                  AND json(segment.region_json)
                      = json(json_extract(new.locator_json, '$.region'))
              )
              OR (
                  segment.locator_kind = 'TEXT_LINES'
                  AND segment.line_start = json_extract(new.locator_json, '$.start')
                  AND segment.line_end = json_extract(new.locator_json, '$.end')
              )
          )
    );
END;

CREATE TRIGGER tidbit_revision_attachment_search_after_insert
AFTER INSERT ON tidbit_revision_attachment
BEGIN
    UPDATE passage_search_document
    SET attachment_names = coalesce(
        (
            SELECT group_concat(attachment.display_filename, char(10))
            FROM tidbit_revision_attachment AS membership
            JOIN attachment ON attachment.id = membership.attachment_id
            WHERE membership.tidbit_revision_id = new.tidbit_revision_id
              AND attachment.deleted_at IS NULL
            ORDER BY membership.sort_order
        ),
        ''
    )
    WHERE tidbit_id = (
        SELECT tidbit.id
        FROM tidbit
        WHERE tidbit.current_revision_id = new.tidbit_revision_id
          AND tidbit.deleted_at IS NULL
    )
      AND passage_id IN (
          SELECT passage.id
          FROM passage
          WHERE passage.tidbit_revision_id = new.tidbit_revision_id
            AND passage.owner_kind = 'AUTHOR'
      );
END;

CREATE TRIGGER attachment_extractor_config_search_after_version_update
AFTER UPDATE OF version, passage_construction_version
ON attachment_extractor_config
BEGIN
    DELETE FROM passage_search_document
    WHERE passage_id IN (
        SELECT passage.id
        FROM passage
        JOIN attachment_segment AS segment
          ON segment.id = passage.attachment_segment_id
        JOIN attachment_extraction AS extraction
          ON extraction.id = segment.extraction_id
        WHERE passage.owner_kind = 'ATTACHMENT'
          AND extraction.extractor = new.extractor
    );

    INSERT INTO passage_search_document(
        rowid,
        passage_id,
        tidbit_id,
        title,
        heading_context,
        body,
        source_labels,
        source_domains,
        attachment_names,
        extracted_text,
        owner_content_hash,
        updated_at
    )
    SELECT
        passage.rowid,
        passage.id,
        NULL,
        '',
        coalesce(
            (
                SELECT group_concat(value, char(10))
                FROM json_each(passage.heading_context_json)
            ),
            ''
        ),
        '',
        '',
        '',
        attachment.display_filename,
        passage.content,
        passage.content_hash,
        attachment.updated_at
    FROM current_attachment_passage AS current
    JOIN passage ON passage.id = current.passage_id
    JOIN attachment ON attachment.id = current.attachment_id
    JOIN attachment_extraction AS extraction
      ON extraction.id = current.extraction_id
    WHERE extraction.extractor = new.extractor;
END;

CREATE TRIGGER attachment_search_refresh_after_update
AFTER UPDATE OF display_filename, deleted_at, updated_at ON attachment
BEGIN
    UPDATE passage_search_document
    SET attachment_names = coalesce(
        (
            SELECT group_concat(current_attachment.display_filename, char(10))
            FROM tidbit
            JOIN tidbit_revision_attachment AS current_membership
              ON current_membership.tidbit_revision_id = tidbit.current_revision_id
            JOIN attachment AS current_attachment
              ON current_attachment.id = current_membership.attachment_id
             AND current_attachment.deleted_at IS NULL
            WHERE tidbit.id = passage_search_document.tidbit_id
              AND tidbit.deleted_at IS NULL
            ORDER BY current_membership.sort_order
        ),
        ''
    )
    WHERE EXISTS (
        SELECT 1
        FROM passage
        JOIN tidbit
          ON tidbit.id = passage_search_document.tidbit_id
         AND tidbit.current_revision_id = passage.tidbit_revision_id
         AND tidbit.deleted_at IS NULL
        JOIN tidbit_revision_attachment AS changed_membership
          ON changed_membership.tidbit_revision_id = tidbit.current_revision_id
         AND changed_membership.attachment_id = new.id
        WHERE passage.id = passage_search_document.passage_id
          AND passage.owner_kind = 'AUTHOR'
    );

    DELETE FROM passage_search_document
    WHERE passage_id IN (
        SELECT passage.id
        FROM passage
        JOIN attachment_segment AS segment
          ON segment.id = passage.attachment_segment_id
        JOIN attachment_extraction AS extraction
          ON extraction.id = segment.extraction_id
        WHERE passage.owner_kind = 'ATTACHMENT'
          AND extraction.attachment_id = new.id
    );

    INSERT INTO passage_search_document(
        rowid,
        passage_id,
        tidbit_id,
        title,
        heading_context,
        body,
        source_labels,
        source_domains,
        attachment_names,
        extracted_text,
        owner_content_hash,
        updated_at
    )
    SELECT
        passage.rowid,
        passage.id,
        NULL,
        '',
        coalesce(
            (
                SELECT group_concat(value, char(10))
                FROM json_each(passage.heading_context_json)
            ),
            ''
        ),
        '',
        '',
        '',
        new.display_filename,
        passage.content,
        passage.content_hash,
        new.updated_at
    FROM current_attachment_passage AS current
    JOIN passage ON passage.id = current.passage_id
    WHERE current.attachment_id = new.id
      AND new.deleted_at IS NULL;
END;
