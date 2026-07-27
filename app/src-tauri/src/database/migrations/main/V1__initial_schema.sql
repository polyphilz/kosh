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
    title TEXT,
    body_markdown TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    UNIQUE (tidbit_id, revision_number),
    UNIQUE (tidbit_id, id),
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (title IS NULL OR length(title) > 0)
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

CREATE TABLE attachment (
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
    sha256 BLOB NOT NULL UNIQUE CHECK (length(sha256) = 32),
    display_filename TEXT NOT NULL CHECK (length(display_filename) > 0),
    media_type TEXT NOT NULL CHECK (length(media_type) > 0),
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('IMAGE', 'PDF', 'TEXT', 'BINARY')),
    extraction_state TEXT NOT NULL
        CHECK (extraction_state IN ('PENDING', 'READY', 'FAILED', 'NOT_APPLICABLE'))
) STRICT;

CREATE TABLE tidbit_revision_attachment (
    tidbit_revision_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    display_role TEXT NOT NULL CHECK (display_role IN ('INLINE', 'ATTACHMENT')),
    PRIMARY KEY (tidbit_revision_id, attachment_id),
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
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
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
    locator_kind TEXT NOT NULL CHECK (locator_kind IN ('PDF_PAGE', 'OCR_REGION', 'TEXT_LINES')),
    page_number INTEGER CHECK (page_number IS NULL OR page_number > 0),
    line_start INTEGER CHECK (line_start IS NULL OR line_start > 0),
    line_end INTEGER CHECK (line_end IS NULL OR line_end >= line_start),
    region_json TEXT CHECK (region_json IS NULL OR json_valid(region_json)),
    content TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    UNIQUE (extraction_id, ordinal),
    FOREIGN KEY (extraction_id) REFERENCES attachment_extraction(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (
            locator_kind = 'PDF_PAGE'
            AND page_number IS NOT NULL
            AND line_start IS NULL
            AND line_end IS NULL
            AND region_json IS NULL
        )
        OR (
            locator_kind = 'OCR_REGION'
            AND region_json IS NOT NULL
            AND json_type(region_json) = 'object'
            AND line_start IS NULL
            AND line_end IS NULL
        )
        OR (
            locator_kind = 'TEXT_LINES'
            AND page_number IS NULL
            AND line_start IS NOT NULL
            AND line_end IS NOT NULL
            AND region_json IS NULL
        )
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
        CHECK (locator_kind IN ('MARKDOWN_BLOCKS', 'PDF_PAGE', 'OCR_REGION', 'TEXT_LINES')),
    locator_json TEXT NOT NULL CHECK (json_valid(locator_json)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
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
                (
                    locator_kind = 'PDF_PAGE'
                    AND json_type(locator_json, '$.page') IS 'integer'
                    AND json_extract(locator_json, '$.page') > 0
                )
                OR (
                    locator_kind = 'OCR_REGION'
                    AND json_type(locator_json, '$.region') IS 'object'
                    AND (
                        json_type(locator_json, '$.page') IS NULL
                        OR (
                            json_type(locator_json, '$.page') IS 'integer'
                            AND json_extract(locator_json, '$.page') > 0
                        )
                    )
                )
                OR (
                    locator_kind = 'TEXT_LINES'
                    AND json_type(locator_json, '$.start') IS 'integer'
                    AND json_extract(locator_json, '$.start') > 0
                    AND json_type(locator_json, '$.end') IS 'integer'
                    AND json_extract(locator_json, '$.end')
                        >= json_extract(locator_json, '$.start')
                )
            )
        )
    )
) STRICT;

CREATE UNIQUE INDEX passage_author_ordinal_uq
    ON passage(tidbit_revision_id, ordinal)
    WHERE owner_kind = 'AUTHOR';

CREATE UNIQUE INDEX passage_attachment_ordinal_uq
    ON passage(attachment_segment_id, ordinal)
    WHERE owner_kind = 'ATTACHMENT';

CREATE VIRTUAL TABLE passage_fts_word USING fts5(
    content,
    content = 'passage',
    content_rowid = 'rowid',
    tokenize = 'unicode61'
);

CREATE VIRTUAL TABLE passage_fts_trigram USING fts5(
    content,
    content = 'passage',
    content_rowid = 'rowid',
    tokenize = 'trigram'
);

CREATE TABLE embedding_index (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    model_id TEXT NOT NULL CHECK (length(model_id) > 0),
    model_version TEXT NOT NULL CHECK (length(model_version) > 0),
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    distance_metric TEXT NOT NULL CHECK (distance_metric IN ('COSINE')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0, 1)),
    UNIQUE (model_id, model_version, dimension, distance_metric)
) STRICT;

CREATE UNIQUE INDEX embedding_index_single_active_uq
    ON embedding_index(active)
    WHERE active = 1;

CREATE TABLE passage_embedding (
    passage_id TEXT NOT NULL,
    embedding_index_id TEXT NOT NULL,
    passage_content_hash BLOB NOT NULL CHECK (length(passage_content_hash) = 32),
    vector_bytes BLOB NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (passage_id, embedding_index_id),
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (embedding_index_id) REFERENCES embedding_index(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

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
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    title TEXT,
    body_markdown TEXT NOT NULL DEFAULT '',
    CHECK (title IS NULL OR length(title) > 0)
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

CREATE TABLE research_run (
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
    query TEXT NOT NULL CHECK (length(query) > 0),
    status TEXT NOT NULL
        CHECK (status IN ('PENDING', 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELED')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    started_at INTEGER,
    completed_at INTEGER,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    answer_markdown TEXT,
    error TEXT,
    CHECK (started_at IS NULL OR started_at >= created_at),
    CHECK (completed_at IS NULL OR completed_at >= coalesce(started_at, created_at))
) STRICT;

CREATE TABLE research_event (
    research_run_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    kind TEXT NOT NULL CHECK (length(kind) > 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    PRIMARY KEY (research_run_id, ordinal),
    FOREIGN KEY (research_run_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE TABLE research_citation (
    research_run_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    passage_id TEXT NOT NULL,
    cited_text TEXT NOT NULL CHECK (length(cited_text) > 0),
    PRIMARY KEY (research_run_id, ordinal),
    FOREIGN KEY (research_run_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE TABLE app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    appearance TEXT NOT NULL CHECK (appearance IN ('SYSTEM', 'LIGHT', 'DARK')),
    search_mode TEXT NOT NULL CHECK (search_mode IN ('HYBRID', 'EXACT')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

INSERT INTO app_settings (
    id,
    appearance,
    search_mode,
    created_at,
    updated_at
) VALUES (1, 'SYSTEM', 'HYBRID', 0, 0);

CREATE TABLE index_state (
    name TEXT PRIMARY KEY CHECK (length(name) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    status TEXT NOT NULL CHECK (status IN ('IDLE', 'DIRTY', 'RUNNING', 'FAILED')),
    cursor TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    error TEXT
) STRICT;

INSERT INTO index_state(name, version, status, cursor, updated_at, error)
VALUES('PASSAGE_FTS', '1', 'IDLE', NULL, 0, NULL);

CREATE TRIGGER tidbit_revision_prevent_update
BEFORE UPDATE ON tidbit_revision
BEGIN
    SELECT RAISE(ABORT, 'tidbit revisions are immutable');
END;

CREATE TRIGGER tidbit_revision_prevent_delete
BEFORE DELETE ON tidbit_revision
BEGIN
    SELECT RAISE(ABORT, 'tidbit revisions are retained');
END;

CREATE TRIGGER source_prevent_update
BEFORE UPDATE ON source
BEGIN
    SELECT RAISE(ABORT, 'sources are immutable');
END;

CREATE TRIGGER source_prevent_delete
BEFORE DELETE ON source
BEGIN
    SELECT RAISE(ABORT, 'sources are retained');
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

CREATE TRIGGER attachment_identity_prevent_update
BEFORE UPDATE OF created_at, sha256, byte_length, kind ON attachment
BEGIN
    SELECT RAISE(ABORT, 'attachment identity is immutable');
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

CREATE TRIGGER embedding_index_identity_prevent_update
BEFORE UPDATE OF id, model_id, model_version, dimension, distance_metric, created_at
ON embedding_index
BEGIN
    SELECT RAISE(ABORT, 'embedding index identity is immutable');
END;

CREATE TRIGGER passage_embedding_prevent_update
BEFORE UPDATE ON passage_embedding
BEGIN
    SELECT RAISE(ABORT, 'passage embeddings are immutable');
END;

CREATE TRIGGER passage_embedding_prevent_delete
BEFORE DELETE ON passage_embedding
BEGIN
    SELECT RAISE(ABORT, 'passage embeddings are retained');
END;

CREATE TRIGGER passage_embedding_provenance_validate
BEFORE INSERT ON passage_embedding
BEGIN
    SELECT RAISE(ABORT, 'passage embedding provenance mismatch')
    WHERE NOT EXISTS (
        SELECT 1
        FROM passage
        JOIN embedding_index
          ON embedding_index.id = new.embedding_index_id
        WHERE passage.id = new.passage_id
          AND passage.content_hash = new.passage_content_hash
          AND length(new.vector_bytes) = embedding_index.dimension * 4
    );
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

CREATE TRIGGER passage_prevent_update
BEFORE UPDATE ON passage
BEGIN
    SELECT RAISE(ABORT, 'passages are immutable');
END;

CREATE TRIGGER passage_prevent_delete
BEFORE DELETE ON passage
BEGIN
    SELECT RAISE(ABORT, 'passages are retained');
END;

CREATE TRIGGER research_event_prevent_update
BEFORE UPDATE ON research_event
BEGIN
    SELECT RAISE(ABORT, 'research events are immutable');
END;

CREATE TRIGGER research_event_prevent_delete
BEFORE DELETE ON research_event
BEGIN
    SELECT RAISE(ABORT, 'research events are retained');
END;

CREATE TRIGGER research_citation_prevent_update
BEFORE UPDATE ON research_citation
BEGIN
    SELECT RAISE(ABORT, 'research citations are immutable');
END;

CREATE TRIGGER research_citation_prevent_delete
BEFORE DELETE ON research_citation
BEGIN
    SELECT RAISE(ABORT, 'research citations are retained');
END;
