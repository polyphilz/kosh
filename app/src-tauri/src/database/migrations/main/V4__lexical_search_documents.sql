DROP TABLE passage_fts_trigram;
DROP TABLE passage_fts_word;

CREATE TABLE attachment_extractor_config (
    extractor TEXT PRIMARY KEY CHECK (length(extractor) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    passage_construction_version TEXT NOT NULL
        CHECK (length(passage_construction_version) > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;

INSERT INTO attachment_extractor_config(
    extractor,
    version,
    passage_construction_version,
    updated_at
)
VALUES
    ('ocr', '1', 'ocr-region-v1', 0),
    ('pdf-text', '1', 'pdf-page-v1', 0),
    ('text', '1', 'text-lines-v1', 0);

CREATE TRIGGER attachment_extractor_config_identity_prevent_update
BEFORE UPDATE OF extractor ON attachment_extractor_config
BEGIN
    SELECT RAISE(ABORT, 'extractor configuration identity is immutable');
END;

CREATE TRIGGER attachment_extractor_config_prevent_delete
BEFORE DELETE ON attachment_extractor_config
BEGIN
    SELECT RAISE(ABORT, 'extractor configuration is retained');
END;

CREATE TABLE passage_search_document (
    rowid INTEGER PRIMARY KEY,
    passage_id TEXT NOT NULL UNIQUE,
    tidbit_id TEXT,
    title TEXT NOT NULL,
    heading_context TEXT NOT NULL,
    body TEXT NOT NULL,
    source_labels TEXT NOT NULL,
    source_domains TEXT NOT NULL,
    attachment_names TEXT NOT NULL,
    extracted_text TEXT NOT NULL,
    owner_content_hash BLOB NOT NULL CHECK (length(owner_content_hash) = 32),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX passage_search_document_tidbit_idx
    ON passage_search_document(tidbit_id);

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

CREATE VIRTUAL TABLE passage_fts_word USING fts5(
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = 'passage_search_document',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_'''
);

CREATE VIRTUAL TABLE passage_fts_trigram USING fts5(
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = 'passage_search_document',
    content_rowid = 'rowid',
    tokenize = 'trigram'
);

CREATE TRIGGER passage_search_document_fts_after_insert
AFTER INSERT ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.title),
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.title),
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
END;

CREATE TRIGGER passage_search_document_fts_after_delete
AFTER DELETE ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        passage_fts_word, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.title),
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.title),
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
END;

CREATE TRIGGER passage_search_document_fts_after_update
AFTER UPDATE OF
    rowid,
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text
ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        passage_fts_word, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.title),
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_word(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.title),
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.title),
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.title),
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
END;

CREATE TRIGGER passage_attachment_evidence_validate
BEFORE INSERT ON passage
WHEN new.owner_kind = 'ATTACHMENT'
BEGIN
    SELECT RAISE(ABORT, 'attachment passage content does not match its immutable segment')
    WHERE NOT EXISTS (
        SELECT 1
        FROM attachment_segment AS segment
        WHERE segment.id = new.attachment_segment_id
          AND segment.content = new.content
          AND segment.content_hash = new.content_hash
    );
END;

CREATE TRIGGER passage_attachment_search_after_insert
AFTER INSERT ON passage
WHEN new.owner_kind = 'ATTACHMENT'
BEGIN
    DELETE FROM passage_search_document
    WHERE passage_id IN (
        SELECT candidate.id
        FROM passage AS candidate
        JOIN attachment_segment AS candidate_segment
          ON candidate_segment.id = candidate.attachment_segment_id
        JOIN attachment_extraction AS candidate_extraction
          ON candidate_extraction.id = candidate_segment.extraction_id
        WHERE candidate.owner_kind = 'ATTACHMENT'
          AND candidate_extraction.attachment_id = (
              SELECT extraction.attachment_id
              FROM attachment_segment AS segment
              JOIN attachment_extraction AS extraction
                ON extraction.id = segment.extraction_id
              WHERE segment.id = new.attachment_segment_id
          )
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
    WHERE current.attachment_id = (
        SELECT extraction.attachment_id
        FROM attachment_segment AS segment
        JOIN attachment_extraction AS extraction
          ON extraction.id = segment.extraction_id
        WHERE segment.id = new.attachment_segment_id
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
    active.tidbit_id,
    coalesce(revision.title, ''),
    coalesce(
        (
            SELECT group_concat(value, char(10))
            FROM json_each(passage.heading_context_json)
        ),
        ''
    ),
    passage.content,
    coalesce(
        (
            SELECT group_concat(coalesce(source.label, ''), char(10))
            FROM tidbit_revision_source AS membership
            JOIN source ON source.id = membership.source_id
            WHERE membership.tidbit_revision_id = revision.id
            ORDER BY membership.sort_order
        ),
        ''
    ),
    coalesce(
        (
            SELECT group_concat(coalesce(source.normalized_url, ''), char(10))
            FROM tidbit_revision_source AS membership
            JOIN source ON source.id = membership.source_id
            WHERE membership.tidbit_revision_id = revision.id
            ORDER BY membership.sort_order
        ),
        ''
    ),
    coalesce(
        (
            SELECT group_concat(attachment.display_filename, char(10))
            FROM tidbit_revision_attachment AS membership
            JOIN attachment ON attachment.id = membership.attachment_id
            WHERE membership.tidbit_revision_id = revision.id
              AND attachment.deleted_at IS NULL
            ORDER BY membership.sort_order
        ),
        ''
    ),
    '',
    revision.content_hash,
    tidbit.updated_at
FROM active_passage AS active
JOIN passage ON passage.id = active.passage_id
JOIN tidbit ON tidbit.id = active.tidbit_id
JOIN tidbit_revision AS revision
  ON revision.id = passage.tidbit_revision_id
 AND revision.id = tidbit.current_revision_id
 AND revision.tidbit_id = tidbit.id
WHERE tidbit.deleted_at IS NULL
  AND passage.owner_kind = 'AUTHOR';

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
JOIN attachment ON attachment.id = current.attachment_id;

UPDATE index_state
SET version = 'lexical-v1',
    status = 'IDLE',
    cursor = NULL,
    error = NULL
WHERE name = 'PASSAGE_FTS';
