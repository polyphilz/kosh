DROP TABLE passage_fts_trigram;
DROP TABLE passage_fts_word;

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
        new.rowid, new.title, new.heading_context, new.body, new.source_labels,
        new.source_domains, new.attachment_names, new.extracted_text
    );
    INSERT INTO passage_fts_trigram(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid, new.title, new.heading_context, new.body, new.source_labels,
        new.source_domains, new.attachment_names, new.extracted_text
    );
END;

CREATE TRIGGER passage_search_document_fts_after_delete
AFTER DELETE ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        passage_fts_word, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid, old.title, old.heading_context, old.body,
        old.source_labels, old.source_domains, old.attachment_names,
        old.extracted_text
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid, old.title, old.heading_context, old.body,
        old.source_labels, old.source_domains, old.attachment_names,
        old.extracted_text
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
        'delete', old.rowid, old.title, old.heading_context, old.body,
        old.source_labels, old.source_domains, old.attachment_names,
        old.extracted_text
    );
    INSERT INTO passage_fts_word(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid, new.title, new.heading_context, new.body, new.source_labels,
        new.source_domains, new.attachment_names, new.extracted_text
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid, old.title, old.heading_context, old.body,
        old.source_labels, old.source_domains, old.attachment_names,
        old.extracted_text
    );
    INSERT INTO passage_fts_trigram(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid, new.title, new.heading_context, new.body, new.source_labels,
        new.source_domains, new.attachment_names, new.extracted_text
    );
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

UPDATE index_state
SET version = 'lexical-v1',
    status = 'IDLE',
    cursor = NULL,
    error = NULL
WHERE name = 'PASSAGE_FTS';
