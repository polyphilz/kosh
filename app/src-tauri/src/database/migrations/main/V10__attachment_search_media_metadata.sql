DROP TRIGGER tidbit_revision_attachment_search_after_insert;
DROP TRIGGER attachment_extractor_config_search_after_version_update;
DROP TRIGGER attachment_search_refresh_after_update;

CREATE TRIGGER tidbit_revision_attachment_search_after_insert
AFTER INSERT ON tidbit_revision_attachment
BEGIN
    UPDATE passage_search_document
    SET attachment_names = coalesce(
        (
            SELECT group_concat(
                attachment.display_filename || char(10) || attachment.media_type,
                char(10)
            )
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
        attachment.display_filename || char(10) || attachment.media_type,
        passage.content,
        passage.content_hash,
        attachment.updated_at
    FROM current_attachment_passage AS current
    JOIN passage ON passage.id = current.passage_id
    JOIN attachment ON attachment.id = current.attachment_id
    WHERE current.attachment_id = new.attachment_id;
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
        attachment.display_filename || char(10) || attachment.media_type,
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
            SELECT group_concat(
                current_attachment.display_filename
                    || char(10)
                    || current_attachment.media_type,
                char(10)
            )
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
        new.display_filename || char(10) || new.media_type,
        passage.content,
        passage.content_hash,
        new.updated_at
    FROM current_attachment_passage AS current
    JOIN passage ON passage.id = current.passage_id
    WHERE current.attachment_id = new.id
      AND new.deleted_at IS NULL;
END;

UPDATE passage_search_document
SET attachment_names = coalesce(
    (
        SELECT group_concat(
            attachment.display_filename || char(10) || attachment.media_type,
            char(10)
        )
        FROM tidbit_revision_attachment AS membership
        JOIN attachment ON attachment.id = membership.attachment_id
        JOIN tidbit ON tidbit.current_revision_id = membership.tidbit_revision_id
        WHERE tidbit.id = passage_search_document.tidbit_id
          AND tidbit.deleted_at IS NULL
          AND attachment.deleted_at IS NULL
        ORDER BY membership.sort_order
    ),
    ''
)
WHERE tidbit_id IS NOT NULL;

UPDATE passage_search_document
SET attachment_names = (
    SELECT attachment.display_filename || char(10) || attachment.media_type
    FROM current_attachment_passage AS current
    JOIN attachment ON attachment.id = current.attachment_id
    WHERE current.passage_id = passage_search_document.passage_id
)
WHERE passage_id IN (
    SELECT passage_id
    FROM current_attachment_passage
);
