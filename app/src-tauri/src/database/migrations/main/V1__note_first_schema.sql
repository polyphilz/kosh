-- Kosh hard-cutover schema. No pre-release profile is migrated.
PRAGMA foreign_keys = ON;
CREATE TABLE tidbit (
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
    current_revision_id TEXT NOT NULL,
    FOREIGN KEY (id, current_revision_id) REFERENCES tidbit_revision(tidbit_id, id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE tidbit_revision (
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
    tidbit_id TEXT NOT NULL,
    revision_number INTEGER NOT NULL CHECK (revision_number > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    document_json TEXT NOT NULL
        CHECK (
            json_valid(document_json)
            AND json_type(document_json) = 'object'
            AND json_extract(document_json, '$.schemaVersion') = 1
            AND json_type(document_json, '$.blocks') = 'array'
            AND json_array_length(document_json, '$.blocks') > 0
        ),
    body_markdown TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    UNIQUE (tidbit_id, revision_number),
    UNIQUE (tidbit_id, id),
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX tidbit_active_updated_idx
    ON tidbit(updated_at DESC, id)
    WHERE deleted_at IS NULL;
CREATE TABLE source (
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
    label TEXT,
    normalized_url TEXT,
    CHECK (label IS NULL OR length(label) > 0),
    CHECK (normalized_url IS NULL OR length(normalized_url) > 0),
    CONSTRAINT source_url_safe_scheme CHECK (
        normalized_url IS NULL
        OR substr(normalized_url, 1, 7) = 'http://'
        OR substr(normalized_url, 1, 8) = 'https://'
    ),
    CHECK (label IS NOT NULL OR normalized_url IS NOT NULL)
) STRICT;
CREATE TABLE tidbit_revision_source (
    tidbit_revision_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (tidbit_revision_id, source_id),
    UNIQUE (tidbit_revision_id, sort_order),
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (source_id) REFERENCES source(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE tidbit_revision_attachment (
    tidbit_revision_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    block_id TEXT NOT NULL CHECK (length(block_id) BETWEEN 1 AND 256),
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    display_role TEXT NOT NULL CHECK (display_role IN ('INLINE', 'ATTACHMENT')),
    PRIMARY KEY (tidbit_revision_id, attachment_id),
    UNIQUE (tidbit_revision_id, block_id),
    UNIQUE (tidbit_revision_id, sort_order),
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE media_ingest_lease (
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
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    attachment_id TEXT,
    state TEXT NOT NULL CHECK (state IN ('STAGED', 'COMMITTED', 'ABANDONED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at),
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (state = 'STAGED' AND attachment_id IS NULL)
        OR (state IN ('COMMITTED', 'ABANDONED'))
    )
) STRICT;
CREATE INDEX media_ingest_lease_reconciliation_idx
    ON media_ingest_lease(state, expires_at, id);
CREATE TABLE attachment_extraction (
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
    attachment_id TEXT NOT NULL,
    extractor TEXT NOT NULL CHECK (length(extractor) > 0),
    extractor_version TEXT NOT NULL CHECK (length(extractor_version) > 0),
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    status TEXT NOT NULL CHECK (status IN ('PENDING', 'RUNNING', 'READY', 'FAILED')),
    error TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    started_at INTEGER,
    completed_at INTEGER,
    UNIQUE (attachment_id, extractor, extractor_version, content_hash),
    FOREIGN KEY (attachment_id, content_hash) REFERENCES attachment(id, sha256)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (started_at IS NULL OR started_at >= created_at),
    CHECK (completed_at IS NULL OR completed_at >= coalesce(started_at, created_at)),
    CHECK ((status = 'FAILED' AND error IS NOT NULL) OR status != 'FAILED')
) STRICT;
CREATE TABLE attachment_segment (
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
    extraction_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    locator_kind TEXT NOT NULL CHECK (locator_kind = 'OCR_REGION'),
    region_json TEXT CHECK (region_json IS NULL OR json_valid(region_json)),
    content TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    UNIQUE (extraction_id, ordinal),
    FOREIGN KEY (extraction_id) REFERENCES attachment_extraction(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        locator_kind = 'OCR_REGION'
        AND region_json IS NOT NULL
        AND json_type(region_json) = 'object'
    )
) STRICT;
CREATE TABLE passage (
    rowid INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE
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
    tidbit_revision_id TEXT,
    attachment_segment_id TEXT,
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('AUTHOR', 'ATTACHMENT')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    content TEXT NOT NULL CHECK (length(content) > 0),
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    locator_kind TEXT NOT NULL
        CHECK (locator_kind IN ('MARKDOWN_BLOCKS', 'OCR_REGION')),
    locator_json TEXT NOT NULL CHECK (json_valid(locator_json)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0), construction_version TEXT NOT NULL
        CHECK (length(construction_version) > 0), heading_context_json TEXT NOT NULL DEFAULT '[]'
        CHECK (
            json_valid(heading_context_json)
            AND json_type(heading_context_json) = 'array'
        ),
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attachment_segment_id) REFERENCES attachment_segment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (owner_kind = 'AUTHOR' AND tidbit_revision_id IS NOT NULL AND attachment_segment_id IS NULL)
        OR (
            owner_kind = 'ATTACHMENT'
            AND tidbit_revision_id IS NULL
            AND attachment_segment_id IS NOT NULL
        )
    ),
    CONSTRAINT passage_locator_shape CHECK (
        (
            owner_kind = 'AUTHOR'
            AND locator_kind = 'MARKDOWN_BLOCKS'
            AND json_type(locator_json, '$.start') IS 'integer'
            AND json_extract(locator_json, '$.start') >= 0
            AND json_type(locator_json, '$.end') IS 'integer'
            AND json_extract(locator_json, '$.end') >= json_extract(locator_json, '$.start')
        )
        OR (
            owner_kind = 'ATTACHMENT'
            AND (
                locator_kind = 'OCR_REGION'
                AND json_type(locator_json, '$.region') IS 'object'
            )
        )
    )
) STRICT;
CREATE TABLE draft (
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
    base_revision_id TEXT,
    edit_generation INTEGER NOT NULL
        CHECK (edit_generation > 0 AND edit_generation <= 9007199254740991),
    media_reservation INTEGER NOT NULL DEFAULT 0
        CHECK (media_reservation IN (0, 1)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    document_json TEXT NOT NULL
        CHECK (
            json_valid(document_json)
            AND json_type(document_json) = 'object'
            AND json_extract(document_json, '$.schemaVersion') = 1
            AND json_type(document_json, '$.blocks') = 'array'
            AND json_array_length(document_json, '$.blocks') > 0
        ),
    body_markdown TEXT NOT NULL DEFAULT '',
    FOREIGN KEY (id, base_revision_id) REFERENCES tidbit_revision(tidbit_id, id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE draft_media_lease (
    draft_id TEXT NOT NULL,
    media_ingest_lease_id TEXT NOT NULL,
    PRIMARY KEY (draft_id, media_ingest_lease_id),
    FOREIGN KEY (draft_id) REFERENCES draft(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (media_ingest_lease_id) REFERENCES media_ingest_lease(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE index_state (
    name TEXT PRIMARY KEY CHECK (length(name) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    status TEXT NOT NULL CHECK (status IN ('IDLE', 'DIRTY', 'RUNNING', 'FAILED')),
    cursor TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    error TEXT
) STRICT;
CREATE TRIGGER tidbit_revision_prevent_update
BEFORE UPDATE ON tidbit_revision
BEGIN
    SELECT RAISE(ABORT, 'tidbit revisions are immutable');
END;
CREATE TRIGGER source_prevent_update
BEFORE UPDATE ON source
BEGIN
    SELECT RAISE(ABORT, 'sources are immutable');
END;
CREATE TRIGGER tidbit_revision_source_prevent_update
BEFORE UPDATE ON tidbit_revision_source
BEGIN
    SELECT RAISE(ABORT, 'revision source links are immutable');
END;
CREATE TRIGGER tidbit_revision_source_prevent_delete
BEFORE DELETE ON tidbit_revision_source
BEGIN
    SELECT RAISE(ABORT, 'revision source links are retained');
END;
CREATE TRIGGER tidbit_revision_attachment_prevent_update
BEFORE UPDATE ON tidbit_revision_attachment
BEGIN
    SELECT RAISE(ABORT, 'revision attachment links are immutable');
END;
CREATE TRIGGER tidbit_revision_attachment_prevent_delete
BEFORE DELETE ON tidbit_revision_attachment
BEGIN
    SELECT RAISE(ABORT, 'revision attachment links are retained');
END;
CREATE TRIGGER attachment_extraction_identity_prevent_update
BEFORE UPDATE OF attachment_id, extractor, extractor_version, content_hash, created_at
ON attachment_extraction
BEGIN
    SELECT RAISE(ABORT, 'attachment extraction identity is immutable');
END;
CREATE TRIGGER attachment_extraction_prevent_delete
BEFORE DELETE ON attachment_extraction
BEGIN
    SELECT RAISE(ABORT, 'attachment extractions are retained');
END;
CREATE TRIGGER attachment_extraction_ready_prevent_regression
BEFORE UPDATE OF status ON attachment_extraction
WHEN old.status = 'READY' AND new.status != 'READY'
BEGIN
    SELECT RAISE(ABORT, 'ready attachment extractions are terminal');
END;
CREATE TRIGGER attachment_segment_prevent_update
BEFORE UPDATE ON attachment_segment
BEGIN
    SELECT RAISE(ABORT, 'attachment segments are immutable');
END;
CREATE TRIGGER attachment_segment_prevent_delete
BEFORE DELETE ON attachment_segment
WHEN NOT EXISTS (
    SELECT 1
    FROM attachment_extraction
    WHERE attachment_extraction.id = old.extraction_id
      AND attachment_extraction.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'attachment segments are retained');
END;
CREATE TRIGGER passage_prevent_update
BEFORE UPDATE ON passage
BEGIN
    SELECT RAISE(ABORT, 'passages are immutable');
END;
CREATE TABLE draft_source (
    draft_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    label TEXT,
    url TEXT,
    PRIMARY KEY (draft_id, position),
    FOREIGN KEY (draft_id) REFERENCES draft(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX passage_author_version_ordinal_uq
    ON passage(tidbit_revision_id, construction_version, ordinal)
    WHERE owner_kind = 'AUTHOR';
CREATE UNIQUE INDEX passage_attachment_version_ordinal_uq
    ON passage(attachment_segment_id, construction_version, ordinal)
    WHERE owner_kind = 'ATTACHMENT';
CREATE TABLE active_passage (
    passage_id TEXT PRIMARY KEY,
    tidbit_id TEXT NOT NULL,
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE INDEX active_passage_tidbit_idx ON active_passage(tidbit_id);
CREATE TRIGGER active_passage_validate
BEFORE INSERT ON active_passage
BEGIN
    SELECT RAISE(ABORT, 'active passage is not current authored content')
    WHERE NOT EXISTS (
        SELECT 1
        FROM passage
        JOIN tidbit
          ON tidbit.id = new.tidbit_id
         AND tidbit.current_revision_id = passage.tidbit_revision_id
         AND tidbit.deleted_at IS NULL
        WHERE passage.id = new.passage_id
          AND passage.owner_kind = 'AUTHOR'
    );
END;
CREATE TRIGGER active_passage_prevent_update
BEFORE UPDATE ON active_passage
BEGIN
    SELECT RAISE(ABORT, 'active passage mappings are replaced, never updated');
END;
CREATE TABLE attachment_extractor_config (
    extractor TEXT PRIMARY KEY CHECK (length(extractor) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    passage_construction_version TEXT NOT NULL
        CHECK (length(passage_construction_version) > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;
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
CREATE TABLE block_search_document (
    rowid INTEGER PRIMARY KEY,
    tidbit_id TEXT NOT NULL,
    tidbit_revision_id TEXT NOT NULL,
    block_id TEXT NOT NULL CHECK (length(block_id) BETWEEN 1 AND 256),
    block_ordinal INTEGER NOT NULL CHECK (block_ordinal >= 0),
    block_type TEXT NOT NULL CHECK (length(block_type) > 0),
    heading_context TEXT NOT NULL,
    body TEXT NOT NULL,
    attachment_names TEXT NOT NULL,
    extracted_text TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    UNIQUE (tidbit_id, block_id),
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id, tidbit_revision_id) REFERENCES tidbit_revision(tidbit_id, id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (length(trim(body || attachment_names || extracted_text)) > 0)
) STRICT;
CREATE INDEX block_search_document_revision_idx
    ON block_search_document(tidbit_revision_id, block_ordinal);
CREATE TRIGGER block_search_document_validate_current_insert
BEFORE INSERT ON block_search_document
BEGIN
    SELECT RAISE(ABORT, 'block search documents must belong to the current note revision')
    WHERE NOT EXISTS (
        SELECT 1
        FROM tidbit
        WHERE tidbit.id = new.tidbit_id
          AND tidbit.current_revision_id = new.tidbit_revision_id
          AND tidbit.deleted_at IS NULL
    );
END;
CREATE TRIGGER block_search_document_prevent_update
BEFORE UPDATE ON block_search_document
BEGIN
    SELECT RAISE(ABORT, 'block search documents are replaced, never updated');
END;
CREATE VIRTUAL TABLE block_fts_word USING fts5(
    heading_context,
    body,
    attachment_names,
    extracted_text,
    content = 'block_search_document',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_'''
);
CREATE VIRTUAL TABLE block_fts_trigram USING fts5(
    heading_context,
    body,
    attachment_names,
    extracted_text,
    content = 'block_search_document',
    content_rowid = 'rowid',
    tokenize = 'trigram'
);
CREATE VIRTUAL TABLE block_fts_short USING fts5(
    heading_context,
    body,
    attachment_names,
    extracted_text,
    content = 'block_search_document',
    content_rowid = 'rowid',
    tokenize = 'unicode61'
);
CREATE TRIGGER block_search_document_fts_after_insert
AFTER INSERT ON block_search_document
BEGIN
    INSERT INTO block_fts_word(
        rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO block_fts_trigram(
        rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO block_fts_short(
        rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_short_grams(new.heading_context),
        kosh_search_short_grams(new.body),
        kosh_search_short_grams(new.attachment_names),
        kosh_search_short_grams(new.extracted_text)
    );
END;
CREATE TRIGGER block_search_document_fts_after_delete
AFTER DELETE ON block_search_document
BEGIN
    INSERT INTO block_fts_word(
        block_fts_word, rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO block_fts_trigram(
        block_fts_trigram, rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO block_fts_short(
        block_fts_short, rowid, heading_context, body, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_short_grams(old.heading_context),
        kosh_search_short_grams(old.body),
        kosh_search_short_grams(old.attachment_names),
        kosh_search_short_grams(old.extracted_text)
    );
END;
CREATE TABLE passage_search_document (
    rowid INTEGER PRIMARY KEY,
    passage_id TEXT NOT NULL UNIQUE,
    tidbit_id TEXT,
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
CREATE VIRTUAL TABLE passage_fts_short USING fts5(
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = 'passage_search_document',
    content_rowid = 'rowid',
    tokenize = 'unicode61'
);
CREATE TRIGGER passage_search_document_fts_after_insert
AFTER INSERT ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_short(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_short_grams(new.heading_context),
        kosh_search_short_grams(new.body),
        kosh_search_short_grams(new.source_labels),
        kosh_search_short_grams(new.source_domains),
        kosh_search_short_grams(new.attachment_names),
        kosh_search_short_grams(new.extracted_text)
    );
END;
CREATE TRIGGER passage_search_document_fts_after_delete
AFTER DELETE ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        passage_fts_word, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_short(
        passage_fts_short, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_short_grams(old.heading_context),
        kosh_search_short_grams(old.body),
        kosh_search_short_grams(old.source_labels),
        kosh_search_short_grams(old.source_domains),
        kosh_search_short_grams(old.attachment_names),
        kosh_search_short_grams(old.extracted_text)
    );
END;
CREATE TRIGGER passage_search_document_fts_after_update
AFTER UPDATE OF
    rowid,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text
ON passage_search_document
BEGIN
    INSERT INTO passage_fts_word(
        passage_fts_word, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_word(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        passage_fts_trigram, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_normalize(old.heading_context),
        kosh_search_normalize(old.body),
        kosh_search_normalize(old.source_labels),
        kosh_search_normalize(old.source_domains),
        kosh_search_normalize(old.attachment_names),
        kosh_search_normalize(old.extracted_text)
    );
    INSERT INTO passage_fts_trigram(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_normalize(new.heading_context),
        kosh_search_normalize(new.body),
        kosh_search_normalize(new.source_labels),
        kosh_search_normalize(new.source_domains),
        kosh_search_normalize(new.attachment_names),
        kosh_search_normalize(new.extracted_text)
    );
    INSERT INTO passage_fts_short(
        passage_fts_short, rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_short_grams(old.heading_context),
        kosh_search_short_grams(old.body),
        kosh_search_short_grams(old.source_labels),
        kosh_search_short_grams(old.source_domains),
        kosh_search_short_grams(old.attachment_names),
        kosh_search_short_grams(old.extracted_text)
    );
    INSERT INTO passage_fts_short(
        rowid, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_short_grams(new.heading_context),
        kosh_search_short_grams(new.body),
        kosh_search_short_grams(new.source_labels),
        kosh_search_short_grams(new.source_domains),
        kosh_search_short_grams(new.attachment_names),
        kosh_search_short_grams(new.extracted_text)
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
CREATE TABLE passage_embedding_index (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    index_key TEXT NOT NULL UNIQUE CHECK (length(index_key) > 0),
    model_name TEXT NOT NULL CHECK (length(model_name) > 0),
    model_revision TEXT NOT NULL CHECK (length(model_revision) > 0),
    model_file_sha256 BLOB NOT NULL CHECK (length(model_file_sha256) = 32),
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    distance_metric TEXT NOT NULL CHECK (distance_metric = 'COSINE'),
    normalized INTEGER NOT NULL CHECK (normalized IN (0, 1)),
    index_schema_version INTEGER NOT NULL CHECK (index_schema_version > 0),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
    )
) STRICT;
INSERT INTO passage_embedding_index (
    id,
    created_at,
    index_key,
    model_name,
    model_revision,
    model_file_sha256,
    dimension,
    distance_metric,
    normalized,
    index_schema_version,
    config_json
) VALUES (
    '019f547b-6200-7000-8000-000000000002',
    1783828800000,
    'jina_v1',
    'jinaai/jina-embeddings-v5-text-nano-retrieval-GGUF',
    '59cfaceeeb7d738c404659435af4c0da74d06c96',
    X'86b6e6279e9b9e71389f02a082764a2ac2b15a50e37482c26f98d69092f12442',
    768,
    'COSINE',
    1,
    1,
    '{"schemaVersion":1,"modelFile":"v5-nano-retrieval-Q8_0.gguf","modelFileSize":232883776,"quantization":"Q8_0","pooling":"last","normalization":"L2","queryPrefix":"Query: ","documentPrefix":"Document: ","documentConstructionVersion":1}'
);
CREATE TRIGGER passage_embedding_index_prevent_update
BEFORE UPDATE ON passage_embedding_index
BEGIN
    SELECT RAISE(ABORT, 'passage embedding index definitions are immutable');
END;
CREATE TRIGGER passage_embedding_index_prevent_delete
BEFORE DELETE ON passage_embedding_index
BEGIN
    SELECT RAISE(ABORT, 'passage embedding index definitions are retained');
END;
CREATE TABLE passage_embedding (
    passage_id TEXT NOT NULL,
    embedding_index_id TEXT NOT NULL,
    passage_content_hash BLOB NOT NULL CHECK (length(passage_content_hash) = 32),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (embedding_index_id, passage_id),
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (embedding_index_id) REFERENCES passage_embedding_index(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TRIGGER passage_embedding_provenance_validate
BEFORE INSERT ON passage_embedding
BEGIN
    SELECT RAISE(ABORT, 'passage embedding provenance mismatch')
    WHERE NOT EXISTS (
        SELECT 1
        FROM passage
        JOIN passage_embedding_index
          ON passage_embedding_index.id = new.embedding_index_id
        WHERE passage.id = new.passage_id
          AND passage.content_hash = new.passage_content_hash
    );
END;
CREATE TRIGGER passage_embedding_prevent_update
BEFORE UPDATE ON passage_embedding
BEGIN
    SELECT RAISE(ABORT, 'passage embeddings are immutable');
END;
CREATE TABLE passage_embedding_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    active_embedding_index_id TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    FOREIGN KEY (active_embedding_index_id) REFERENCES passage_embedding_index(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE passage_embedding_reap_queue (
    passage_rowid INTEGER PRIMARY KEY CHECK (passage_rowid > 0)
) STRICT;
CREATE TRIGGER passage_embedding_settings_prevent_delete
BEFORE DELETE ON passage_embedding_settings
BEGIN
    SELECT RAISE(ABORT, 'passage embedding settings singleton cannot be deleted');
END;
CREATE TRIGGER passage_embedding_invalidate_after_search_delete
AFTER DELETE ON passage_search_document
BEGIN
    INSERT INTO passage_embedding_reap_queue(passage_rowid)
    VALUES(old.rowid)
    ON CONFLICT(passage_rowid) DO NOTHING;
    DELETE FROM passage_embedding WHERE passage_id = old.passage_id;
    UPDATE index_state
    SET status = CASE
            WHEN status IN ('RUNNING', 'FAILED') THEN status
            ELSE 'DIRTY'
        END,
        cursor = NULL,
        error = CASE WHEN status = 'FAILED' THEN error ELSE NULL END
    WHERE name = 'PASSAGE_EMBEDDING';
END;
CREATE TRIGGER passage_embedding_dirty_after_search_insert
AFTER INSERT ON passage_search_document
BEGIN
    UPDATE index_state
    SET status = CASE
            WHEN status IN ('RUNNING', 'FAILED') THEN status
            ELSE 'DIRTY'
        END,
        cursor = NULL,
        error = CASE WHEN status = 'FAILED' THEN error ELSE NULL END
    WHERE name = 'PASSAGE_EMBEDDING';
END;
CREATE TRIGGER passage_embedding_invalidate_after_search_update
AFTER UPDATE OF passage_id, body, extracted_text, owner_content_hash
ON passage_search_document
BEGIN
    INSERT INTO passage_embedding_reap_queue(passage_rowid)
    VALUES(old.rowid)
    ON CONFLICT(passage_rowid) DO NOTHING;
    DELETE FROM passage_embedding WHERE passage_id = old.passage_id;
    UPDATE index_state
    SET status = CASE
            WHEN status IN ('RUNNING', 'FAILED') THEN status
            ELSE 'DIRTY'
        END,
        cursor = NULL,
        error = CASE WHEN status = 'FAILED' THEN error ELSE NULL END
    WHERE name = 'PASSAGE_EMBEDDING';
END;
CREATE TABLE "attachment" (
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
    kind TEXT NOT NULL CHECK (kind IN ('IMAGE', 'FILE')),
    owner_note_id TEXT
        CHECK (
            owner_note_id IS NULL
            OR (
                length(owner_note_id) = 36
                AND lower(owner_note_id) = owner_note_id
                AND substr(owner_note_id, 9, 1) = '-'
                AND substr(owner_note_id, 14, 1) = '-'
                AND substr(owner_note_id, 15, 1) = '7'
                AND substr(owner_note_id, 19, 1) = '-'
                AND substr(owner_note_id, 20, 1) GLOB '[89ab]'
                AND substr(owner_note_id, 24, 1) = '-'
                AND length(replace(owner_note_id, '-', '')) = 32
                AND replace(owner_note_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    owner_block_id TEXT CHECK (owner_block_id IS NULL OR length(owner_block_id) BETWEEN 1 AND 256),
    extraction_state TEXT NOT NULL
        CHECK (extraction_state IN ('PENDING', 'READY', 'FAILED', 'NOT_APPLICABLE')),
    UNIQUE (id, sha256),
    UNIQUE (owner_note_id, owner_block_id),
    CHECK ((owner_note_id IS NULL) = (owner_block_id IS NULL))
) STRICT;
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
CREATE TRIGGER attachment_owner_prevent_change
BEFORE UPDATE OF owner_note_id, owner_block_id ON attachment
WHEN old.owner_note_id IS NOT NULL OR old.owner_block_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'attachment block ownership is immutable');
END;
CREATE TRIGGER tidbit_revision_attachment_owner_validate
BEFORE INSERT ON tidbit_revision_attachment
BEGIN
    SELECT RAISE(ABORT, 'revision attachment does not match its note block owner')
    WHERE NOT EXISTS (
        SELECT 1
        FROM tidbit_revision AS revision
        JOIN attachment ON attachment.id = new.attachment_id
        WHERE revision.id = new.tidbit_revision_id
          AND attachment.owner_note_id = revision.tidbit_id
          AND attachment.owner_block_id = new.block_id
          AND attachment.deleted_at IS NULL
    );
END;
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
          AND segment.locator_kind = 'OCR_REGION'
          AND json(segment.region_json) = json(json_extract(new.locator_json, '$.region'))
    );
END;
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
CREATE TRIGGER tidbit_revision_attachment_search_after_insert
AFTER INSERT ON tidbit_revision_attachment
BEGIN
    UPDATE passage_search_document
    SET attachment_names = coalesce(
        (
            SELECT group_concat(
                attachment.display_filename,
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
            SELECT group_concat(
                current_attachment.display_filename,
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
CREATE TABLE shortcut_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0)
, automatic_update_checks_enabled INTEGER NOT NULL DEFAULT 1
    CHECK (automatic_update_checks_enabled IN (0, 1))) STRICT;
CREATE TABLE keyboard_binding (
    command TEXT PRIMARY KEY CHECK (command IN ('QUICK_ADD', 'MAIN_WINDOW')),
    accelerator TEXT NOT NULL CHECK (length(accelerator) BETWEEN 3 AND 96)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER shortcut_settings_prevent_delete
BEFORE DELETE ON shortcut_settings
BEGIN
    SELECT RAISE(ABORT, 'shortcut settings singleton cannot be deleted');
END;
CREATE TRIGGER keyboard_binding_prevent_delete
BEFORE DELETE ON keyboard_binding
BEGIN
    SELECT RAISE(ABORT, 'keyboard bindings cannot be deleted');
END;
CREATE INDEX tidbit_deleted_updated_idx
    ON tidbit(updated_at DESC, id DESC)
    WHERE deleted_at IS NOT NULL;
CREATE TABLE offsite_backup_config (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    backup_set_id TEXT NOT NULL UNIQUE
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
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND lower(replica_epoch_id) = replica_epoch_id
            AND substr(replica_epoch_id, 9, 1) = '-'
            AND substr(replica_epoch_id, 14, 1) = '-'
            AND substr(replica_epoch_id, 15, 1) = '7'
            AND substr(replica_epoch_id, 19, 1) = '-'
            AND substr(replica_epoch_id, 20, 1) GLOB '[89ab]'
            AND substr(replica_epoch_id, 24, 1) = '-'
            AND length(replace(replica_epoch_id, '-', '')) = 32
            AND replace(replica_epoch_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    provider TEXT NOT NULL CHECK (provider = 'R2'),
    jurisdiction TEXT NOT NULL
        CHECK (jurisdiction IN ('DEFAULT', 'EU', 'FEDRAMP')),
    account_id TEXT NOT NULL
        CHECK (
            length(account_id) = 32
            AND lower(account_id) = account_id
            AND account_id NOT GLOB '*[^0-9a-f]*'
        ),
    bucket TEXT NOT NULL
        CHECK (
            length(bucket) BETWEEN 3 AND 63
            AND lower(bucket) = bucket
            AND bucket NOT GLOB '*[^a-z0-9-]*'
            AND substr(bucket, 1, 1) GLOB '[a-z0-9]'
            AND substr(bucket, -1, 1) GLOB '[a-z0-9]'
        ),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
CREATE TABLE offsite_credential_cleanup (
    backup_set_id TEXT PRIMARY KEY
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
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
) STRICT;
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
CREATE TABLE offsite_backup_checkpoint (
    checkpoint_id TEXT PRIMARY KEY
        CHECK (
            length(checkpoint_id) = 36
            AND lower(checkpoint_id) = checkpoint_id
            AND substr(checkpoint_id, 9, 1) = '-'
            AND substr(checkpoint_id, 14, 1) = '-'
            AND substr(checkpoint_id, 15, 1) = '7'
            AND substr(checkpoint_id, 19, 1) = '-'
            AND substr(checkpoint_id, 20, 1) GLOB '[89ab]'
            AND substr(checkpoint_id, 24, 1) = '-'
            AND length(replace(checkpoint_id, '-', '')) = 32
            AND replace(checkpoint_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
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
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND lower(replica_epoch_id) = replica_epoch_id
            AND substr(replica_epoch_id, 9, 1) = '-'
            AND substr(replica_epoch_id, 14, 1) = '-'
            AND substr(replica_epoch_id, 15, 1) = '7'
            AND substr(replica_epoch_id, 19, 1) = '-'
            AND substr(replica_epoch_id, 20, 1) GLOB '[89ab]'
            AND substr(replica_epoch_id, 24, 1) = '-'
            AND length(replace(replica_epoch_id, '-', '')) = 32
            AND replace(replica_epoch_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    phase TEXT NOT NULL
        CHECK (phase IN ('PREPARED', 'FENCED', 'REPLICATED', 'PUBLISHED', 'FAILED')),
    config_revision INTEGER NOT NULL CHECK (config_revision > 0),
    content_revision INTEGER NOT NULL CHECK (content_revision >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    kosh_version TEXT NOT NULL
        CHECK (
            length(CAST(kosh_version AS BLOB)) BETWEEN 1 AND 64
            AND instr(kosh_version, char(0)) = 0
            AND instr(kosh_version, char(10)) = 0
            AND instr(kosh_version, char(13)) = 0
        ),
    main_migration_head INTEGER NOT NULL CHECK (main_migration_head > 0),
    media_migration_head INTEGER NOT NULL CHECK (media_migration_head > 0),
    referenced_hash_count INTEGER NOT NULL CHECK (referenced_hash_count >= 0),
    referenced_total_bytes INTEGER NOT NULL CHECK (referenced_total_bytes >= 0),
    referenced_hash_set_sha256 BLOB NOT NULL
        CHECK (length(referenced_hash_set_sha256) = 32),
    litestream_txid TEXT
        CHECK (
            litestream_txid IS NULL
            OR (
                length(litestream_txid) = 16
                AND lower(litestream_txid) = litestream_txid
                AND litestream_txid NOT GLOB '*[^0-9a-f]*'
            )
        ),
    manifest_object_key TEXT
        CHECK (
            manifest_object_key IS NULL
            OR length(CAST(manifest_object_key AS BLOB)) BETWEEN 1 AND 1024
        ),
    publication_sequence INTEGER
        CHECK (publication_sequence IS NULL OR publication_sequence > 0),
    last_error_code TEXT
        CHECK (
            last_error_code IS NULL
            OR (
                length(last_error_code) BETWEEN 1 AND 64
                AND last_error_code NOT GLOB '*[^A-Z0-9_]*'
            )
        ),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK (
        phase IN ('PREPARED', 'FAILED')
        OR litestream_txid IS NOT NULL
    ),
    CHECK (
        phase = 'PUBLISHED'
        OR (manifest_object_key IS NULL AND publication_sequence IS NULL)
    ),
    CHECK (
        phase <> 'PUBLISHED'
        OR (
            litestream_txid IS NOT NULL
            AND manifest_object_key IS NOT NULL
            AND publication_sequence IS NOT NULL
            AND last_error_code IS NULL
        )
    ),
    CHECK (phase <> 'FAILED' OR last_error_code IS NOT NULL),
    CHECK (phase = 'FAILED' OR last_error_code IS NULL)
) STRICT;
CREATE UNIQUE INDEX offsite_backup_checkpoint_publication_sequence_idx
    ON offsite_backup_checkpoint(publication_sequence)
    WHERE publication_sequence IS NOT NULL;
CREATE INDEX offsite_backup_checkpoint_lineage_idx
    ON offsite_backup_checkpoint(
        backup_set_id,
        replica_epoch_id,
        publication_sequence DESC
    );
CREATE TABLE offsite_backup_content_clock (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;
CREATE TRIGGER offsite_backup_content_clock_prevent_delete
BEFORE DELETE ON offsite_backup_content_clock
BEGIN
    SELECT RAISE(ABORT, 'off-site backup content clock cannot be deleted');
END;
CREATE TRIGGER offsite_clock_active_passage_insert AFTER INSERT ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_active_passage_update AFTER UPDATE ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_active_passage_delete AFTER DELETE ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_insert AFTER INSERT ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_update AFTER UPDATE ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_delete AFTER DELETE ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_insert AFTER INSERT ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_update AFTER UPDATE ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_delete AFTER DELETE ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_insert AFTER INSERT ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_update AFTER UPDATE ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_delete AFTER DELETE ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_insert AFTER INSERT ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_update AFTER UPDATE ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_delete AFTER DELETE ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_insert AFTER INSERT ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_update AFTER UPDATE ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_delete AFTER DELETE ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_insert AFTER INSERT ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_update AFTER UPDATE ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_delete AFTER DELETE ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_insert AFTER INSERT ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_update AFTER UPDATE ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_delete AFTER DELETE ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_insert AFTER INSERT ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_update AFTER UPDATE ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_delete AFTER DELETE ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_insert AFTER INSERT ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_update AFTER UPDATE ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_delete AFTER DELETE ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_insert AFTER INSERT ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_update AFTER UPDATE ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_delete AFTER DELETE ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_insert AFTER INSERT ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_update AFTER UPDATE ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_delete AFTER DELETE ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_insert AFTER INSERT ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_update AFTER UPDATE ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_delete AFTER DELETE ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_insert AFTER INSERT ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_update AFTER UPDATE ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_delete AFTER DELETE ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_insert AFTER INSERT ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_update AFTER UPDATE ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_delete AFTER DELETE ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_block_search_document_insert AFTER INSERT ON block_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_block_search_document_delete AFTER DELETE ON block_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_insert AFTER INSERT ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_update AFTER UPDATE ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_delete AFTER DELETE ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_insert AFTER INSERT ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_update AFTER UPDATE ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_delete AFTER DELETE ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_insert AFTER INSERT ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_update AFTER UPDATE ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_delete AFTER DELETE ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_insert AFTER INSERT ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_update AFTER UPDATE ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_delete AFTER DELETE ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_insert AFTER INSERT ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_update AFTER UPDATE ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_delete AFTER DELETE ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_insert AFTER INSERT ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_update AFTER UPDATE ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_delete AFTER DELETE ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_insert AFTER INSERT ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_update AFTER UPDATE ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_delete AFTER DELETE ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_insert AFTER INSERT ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_update AFTER UPDATE ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_delete AFTER DELETE ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_insert AFTER INSERT ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_update AFTER UPDATE ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_delete AFTER DELETE ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_insert AFTER INSERT ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_update AFTER UPDATE ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_delete AFTER DELETE ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_insert AFTER INSERT ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_update AFTER UPDATE ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_delete AFTER DELETE ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_insert AFTER INSERT ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_update AFTER UPDATE ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_delete AFTER DELETE ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TABLE offsite_backup_checkpoint_media (
    checkpoint_id TEXT NOT NULL
        REFERENCES offsite_backup_checkpoint(checkpoint_id)
        ON DELETE CASCADE,
    sha256 BLOB NOT NULL CHECK (length(sha256) = 32),
    byte_length INTEGER NOT NULL CHECK (byte_length > 0),
    PRIMARY KEY (checkpoint_id, sha256)
) STRICT, WITHOUT ROWID;
CREATE TABLE offsite_backup_config_intent (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    operation_id TEXT NOT NULL UNIQUE
        CHECK (
            length(operation_id) = 36
            AND operation_id = lower(operation_id)
        ),
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    backup_set_id TEXT NOT NULL
        CHECK (
            length(backup_set_id) = 36
            AND backup_set_id = lower(backup_set_id)
        ),
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND replica_epoch_id = lower(replica_epoch_id)
        ),
    provider TEXT NOT NULL CHECK (provider = 'R2'),
    jurisdiction TEXT NOT NULL CHECK (jurisdiction IN ('DEFAULT', 'EU', 'FEDRAMP')),
    account_id TEXT NOT NULL
        CHECK (
            length(account_id) = 32
            AND account_id = lower(account_id)
            AND account_id NOT GLOB '*[^0-9a-f]*'
        ),
    bucket TEXT NOT NULL CHECK (length(bucket) BETWEEN 3 AND 63),
    credential_action TEXT NOT NULL CHECK (credential_action IN ('REUSE', 'REPLACE')),
    state TEXT NOT NULL CHECK (state IN ('PENDING', 'COMMITTED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
CREATE TABLE offsite_backup_takeover_intent (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    operation_id TEXT NOT NULL UNIQUE
        CHECK (
            length(operation_id) = 36
            AND operation_id = lower(operation_id)
        ),
    expected_revision INTEGER NOT NULL CHECK (expected_revision > 0),
    backup_set_id TEXT NOT NULL
        CHECK (
            length(backup_set_id) = 36
            AND backup_set_id = lower(backup_set_id)
        ),
    previous_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(previous_replica_epoch_id) = 36
            AND previous_replica_epoch_id = lower(previous_replica_epoch_id)
        ),
    next_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(next_replica_epoch_id) = 36
            AND next_replica_epoch_id = lower(next_replica_epoch_id)
        ),
    expected_owner_replica_epoch_id TEXT NOT NULL
        CHECK (
            length(expected_owner_replica_epoch_id) = 36
            AND expected_owner_replica_epoch_id = lower(expected_owner_replica_epoch_id)
        ),
    expected_owner_writer_id TEXT NOT NULL
        CHECK (
            length(expected_owner_writer_id) = 64
            AND expected_owner_writer_id = lower(expected_owner_writer_id)
            AND expected_owner_writer_id NOT GLOB '*[^0-9a-f]*'
        ),
    expected_owner_version TEXT NOT NULL
        CHECK (length(expected_owner_version) BETWEEN 1 AND 256),
    next_writer_id TEXT NOT NULL
        CHECK (
            length(next_writer_id) = 64
            AND next_writer_id = lower(next_writer_id)
            AND next_writer_id NOT GLOB '*[^0-9a-f]*'
        ),
    state TEXT NOT NULL CHECK (state IN ('PENDING', 'COMMITTED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK (previous_replica_epoch_id <> next_replica_epoch_id)
) STRICT;
CREATE TRIGGER tidbit_revision_prevent_delete BEFORE DELETE ON tidbit_revision BEGIN SELECT RAISE(ABORT, 'tidbit revisions are retained'); END;
CREATE TRIGGER source_prevent_delete BEFORE DELETE ON source BEGIN SELECT RAISE(ABORT, 'sources are retained'); END;
CREATE TRIGGER passage_prevent_delete BEFORE DELETE ON passage BEGIN SELECT RAISE(ABORT, 'passages are retained'); END;
INSERT INTO index_state(name, version, status, cursor, updated_at, error) VALUES('PASSAGE_FTS', 'lexical-v4', 'IDLE', NULL, 0, NULL);
INSERT INTO index_state(name, version, status, cursor, updated_at, error) VALUES('BLOCK_FTS', 'block-lexical-v1', 'IDLE', NULL, 0, NULL);
INSERT INTO index_state(name, version, status, cursor, updated_at, error) VALUES('PASSAGE_EMBEDDING', 'jina_v1', 'DIRTY', NULL, 0, NULL);
INSERT INTO attachment_extractor_config(extractor, version, passage_construction_version, updated_at) VALUES('ocr', '1', 'ocr-region-v1', 0);
INSERT INTO passage_embedding_settings(singleton_id, active_embedding_index_id, updated_at) VALUES(1, NULL, 0);
INSERT INTO shortcut_settings(singleton_id, revision, automatic_update_checks_enabled) VALUES(1, 1, 1);
INSERT INTO keyboard_binding(command, accelerator) VALUES('MAIN_WINDOW', 'control+alt+super+KeyO'), ('QUICK_ADD', 'control+alt+super+KeyK');
INSERT INTO offsite_backup_content_clock(singleton_id, revision) VALUES(1, 0);
