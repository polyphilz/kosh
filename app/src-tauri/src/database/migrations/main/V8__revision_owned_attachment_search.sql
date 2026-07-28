DROP TRIGGER tidbit_revision_attachment_search_after_insert;
DROP VIEW current_attachment_passage;

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
      extractor_config.passage_construction_version
  AND EXISTS (
      SELECT 1
      FROM tidbit_revision_attachment AS durable_membership
      WHERE durable_membership.attachment_id = attachment.id
  );

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

    INSERT OR IGNORE INTO passage_search_document(
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
    WHERE current.attachment_id = new.attachment_id;
END;

DELETE FROM passage_search_document
WHERE passage_id IN (
    SELECT passage.id
    FROM passage
    WHERE passage.owner_kind = 'ATTACHMENT'
)
  AND passage_id NOT IN (
      SELECT passage_id
      FROM current_attachment_passage
  );
