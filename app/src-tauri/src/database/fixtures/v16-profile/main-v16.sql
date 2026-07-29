PRAGMA foreign_keys=OFF;
PRAGMA application_id=1263489864;
BEGIN TRANSACTION;
CREATE TABLE refinery_schema_history(
             version int4 PRIMARY KEY,
             name VARCHAR(255),
             applied_on VARCHAR(255),
             checksum VARCHAR(255));
INSERT INTO refinery_schema_history VALUES(1,'initial_schema','2026-07-29T00:00:00Z','1893190742697353014');
INSERT INTO refinery_schema_history VALUES(2,'draft_context_and_sources','2026-07-29T00:00:00Z','16801361238832560671');
INSERT INTO refinery_schema_history VALUES(3,'passage_provenance_and_activation','2026-07-29T00:00:00Z','8288572353083456604');
INSERT INTO refinery_schema_history VALUES(4,'lexical_search_documents','2026-07-29T00:00:00Z','10437088144070821696');
INSERT INTO refinery_schema_history VALUES(5,'passage_embedding_index','2026-07-29T00:00:00Z','14385599993913504301');
INSERT INTO refinery_schema_history VALUES(6,'attachment_media_lifecycle','2026-07-29T00:00:00Z','11825815564124719727');
INSERT INTO refinery_schema_history VALUES(7,'image_authoring_and_ocr','2026-07-29T00:00:00Z','2916902560853186141');
INSERT INTO refinery_schema_history VALUES(8,'revision_owned_attachment_search','2026-07-29T00:00:00Z','16727045578399992609');
INSERT INTO refinery_schema_history VALUES(9,'pdf_ingestion_and_extraction','2026-07-29T00:00:00Z','10945939301669115061');
INSERT INTO refinery_schema_history VALUES(10,'attachment_search_media_metadata','2026-07-29T00:00:00Z','3134851775050899026');
INSERT INTO refinery_schema_history VALUES(11,'global_shortcuts','2026-07-29T00:00:00Z','3327239181938582502');
INSERT INTO refinery_schema_history VALUES(12,'durable_research_history','2026-07-29T00:00:00Z','6472079739740222394');
INSERT INTO refinery_schema_history VALUES(13,'preserve_legacy_research_citations','2026-07-29T00:00:00Z','14547370235723156979');
INSERT INTO refinery_schema_history VALUES(14,'restore_legacy_research_mentions','2026-07-29T00:00:00Z','3208702526766386453');
INSERT INTO refinery_schema_history VALUES(15,'retain_research_citation_media','2026-07-29T00:00:00Z','5230327161746649084');
INSERT INTO refinery_schema_history VALUES(16,'library_trash_and_purge','2026-07-29T00:00:00Z','12337096388385527022');
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
INSERT INTO tidbit VALUES('019f547b-6200-7000-8000-000000000901',1785201600000,1785201600000,NULL,'019f547b-6200-7000-8000-000000000902');
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
INSERT INTO tidbit_revision VALUES('019f547b-6200-7000-8000-000000000902','019f547b-6200-7000-8000-000000000901',1,1785201600000,'Checked migration evidence','The checked migration profile preserves exact amber evidence.',X'6d4a8c55f25d2b1de5eaf1ec9d1d5f3123ec130e5c9fc32d1721cc78ee931b97');
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
INSERT INTO source VALUES('019f547b-6200-7000-8000-000000000903',1785201600000,'Migration notebook','https://example.com/migration-v16');
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
INSERT INTO tidbit_revision_source VALUES('019f547b-6200-7000-8000-000000000902','019f547b-6200-7000-8000-000000000903',0);
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
    created_at INTEGER NOT NULL CHECK (created_at >= 0), construction_version TEXT NOT NULL DEFAULT 'legacy'
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
INSERT INTO passage VALUES(1,'019f547b-6200-7000-8000-000000000904','019f547b-6200-7000-8000-000000000902',NULL,'AUTHOR',0,'The checked migration profile preserves exact amber evidence.',X'6d4a8c55f25d2b1de5eaf1ec9d1d5f3123ec130e5c9fc32d1721cc78ee931b97','MARKDOWN_BLOCKS','{"start":0,"end":0}',1785201600000,'markdown-blocks-v2','[]');
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
CREATE TABLE app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    appearance TEXT NOT NULL CHECK (appearance IN ('SYSTEM', 'LIGHT', 'DARK')),
    search_mode TEXT NOT NULL CHECK (search_mode IN ('HYBRID', 'EXACT')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
INSERT INTO app_settings VALUES(1,'SYSTEM','HYBRID',0,0);
CREATE TABLE index_state (
    name TEXT PRIMARY KEY CHECK (length(name) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    status TEXT NOT NULL CHECK (status IN ('IDLE', 'DIRTY', 'RUNNING', 'FAILED')),
    cursor TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    error TEXT
) STRICT;
INSERT INTO index_state VALUES('PASSAGE_FTS','lexical-v1','IDLE',NULL,0,NULL);
INSERT INTO index_state VALUES('PASSAGE_BUILD','markdown-blocks-v1','DIRTY',NULL,0,NULL);
INSERT INTO index_state VALUES('PASSAGE_EMBEDDING','jina_v1','IDLE',NULL,0,NULL);
CREATE TABLE draft_source (
    draft_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    label TEXT,
    url TEXT,
    PRIMARY KEY (draft_id, position),
    FOREIGN KEY (draft_id) REFERENCES draft(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE active_passage (
    passage_id TEXT PRIMARY KEY,
    tidbit_id TEXT NOT NULL,
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
INSERT INTO active_passage VALUES('019f547b-6200-7000-8000-000000000904','019f547b-6200-7000-8000-000000000901');
CREATE TABLE attachment_extractor_config (
    extractor TEXT PRIMARY KEY CHECK (length(extractor) > 0),
    version TEXT NOT NULL CHECK (length(version) > 0),
    passage_construction_version TEXT NOT NULL
        CHECK (length(passage_construction_version) > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;
INSERT INTO attachment_extractor_config VALUES('ocr','1','ocr-region-v1',0);
INSERT INTO attachment_extractor_config VALUES('pdf-text','1','pdf-page-v1',0);
INSERT INTO attachment_extractor_config VALUES('text','1','text-lines-v1',0);
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
CREATE TABLE IF NOT EXISTS 'passage_fts_word_data'(id INTEGER PRIMARY KEY, block BLOB);
INSERT INTO passage_fts_word_data VALUES(1,X'');
INSERT INTO passage_fts_word_data VALUES(10,X'00000000000000');
CREATE TABLE IF NOT EXISTS 'passage_fts_word_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'passage_fts_word_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'passage_fts_word_config'(k PRIMARY KEY, v) WITHOUT ROWID;
INSERT INTO passage_fts_word_config VALUES('version',4);
CREATE TABLE IF NOT EXISTS 'passage_fts_trigram_data'(id INTEGER PRIMARY KEY, block BLOB);
INSERT INTO passage_fts_trigram_data VALUES(1,X'');
INSERT INTO passage_fts_trigram_data VALUES(10,X'00000000000000');
CREATE TABLE IF NOT EXISTS 'passage_fts_trigram_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'passage_fts_trigram_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'passage_fts_trigram_config'(k PRIMARY KEY, v) WITHOUT ROWID;
INSERT INTO passage_fts_trigram_config VALUES('version',4);
CREATE TABLE IF NOT EXISTS 'passage_fts_short_data'(id INTEGER PRIMARY KEY, block BLOB);
INSERT INTO passage_fts_short_data VALUES(1,X'');
INSERT INTO passage_fts_short_data VALUES(10,X'00000000000000');
CREATE TABLE IF NOT EXISTS 'passage_fts_short_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'passage_fts_short_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'passage_fts_short_config'(k PRIMARY KEY, v) WITHOUT ROWID;
INSERT INTO passage_fts_short_config VALUES('version',4);
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
INSERT INTO passage_embedding_index VALUES('019f547b-6200-7000-8000-000000000002',1783828800000,'jina_v1','jinaai/jina-embeddings-v5-text-nano-retrieval-GGUF','59cfaceeeb7d738c404659435af4c0da74d06c96',X'86b6e6279e9b9e71389f02a082764a2ac2b15a50e37482c26f98d69092f12442',768,'COSINE',1,1,'{"schemaVersion":1,"modelFile":"v5-nano-retrieval-Q8_0.gguf","modelFileSize":232883776,"quantization":"Q8_0","pooling":"last","normalization":"L2","queryPrefix":"Query: ","documentPrefix":"Document: ","documentConstructionVersion":1}');
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
CREATE TABLE passage_embedding_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    active_embedding_index_id TEXT,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    FOREIGN KEY (active_embedding_index_id) REFERENCES passage_embedding_index(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT;
INSERT INTO passage_embedding_settings VALUES(1,NULL,0);
CREATE TABLE passage_embedding_reap_queue (
    passage_rowid INTEGER PRIMARY KEY CHECK (passage_rowid > 0)
) STRICT;
CREATE TABLE IF NOT EXISTS "attachment" (
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
CREATE TABLE media_blob_reap_candidate (
    sha256 BLOB PRIMARY KEY CHECK (length(sha256) = 32),
    orphaned_at INTEGER NOT NULL CHECK (orphaned_at >= 0),
    reason TEXT NOT NULL CHECK (length(reason) > 0)
) STRICT, WITHOUT ROWID;
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
CREATE TABLE attachment_pdf (
    attachment_id TEXT PRIMARY KEY,
    page_count INTEGER NOT NULL CHECK (page_count > 0 AND page_count <= 2000),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
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
CREATE TABLE attachment_passage_revision (
    passage_id TEXT PRIMARY KEY,
    tidbit_revision_id TEXT NOT NULL,
    FOREIGN KEY (passage_id) REFERENCES passage(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE draft_context (
    draft_id TEXT PRIMARY KEY,
    context_key TEXT NOT NULL UNIQUE CHECK (length(context_key) BETWEEN 1 AND 96),
    tidbit_id TEXT,
    base_revision_id TEXT,
    CHECK (
        (
            context_key IN ('capture', 'quick-add')
            AND tidbit_id IS NULL
            AND base_revision_id IS NULL
        )
        OR (
            context_key = 'edit:' || tidbit_id
            AND tidbit_id IS NOT NULL
            AND base_revision_id IS NOT NULL
        )
    ),
    FOREIGN KEY (draft_id) REFERENCES draft(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (tidbit_id, base_revision_id)
        REFERENCES tidbit_revision(tidbit_id, id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE shortcut_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0)
) STRICT;
INSERT INTO shortcut_settings VALUES(1,1);
CREATE TABLE keyboard_binding (
    command TEXT PRIMARY KEY CHECK (command IN ('QUICK_ADD', 'MAIN_WINDOW')),
    accelerator TEXT NOT NULL CHECK (length(accelerator) BETWEEN 3 AND 96)
) STRICT, WITHOUT ROWID;
INSERT INTO keyboard_binding VALUES('MAIN_WINDOW','control+alt+super+KeyO');
INSERT INTO keyboard_binding VALUES('QUICK_ADD','control+alt+super+KeyK');
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
    rerun_of_id TEXT,
    query TEXT NOT NULL CHECK (length(query) BETWEEN 1 AND 65536),
    status TEXT NOT NULL
        CHECK (status IN (
            'QUEUED',
            'RUNNING',
            'COMPLETED',
            'CANCELED',
            'FAILED',
            'INTERRUPTED'
        )),
    requested_model TEXT CHECK (requested_model IS NULL OR length(requested_model) BETWEEN 1 AND 128),
    requested_effort TEXT
        CHECK (
            requested_effort IS NULL
            OR requested_effort IN ('low', 'medium', 'high', 'xhigh', 'max')
        ),
    actual_model TEXT CHECK (actual_model IS NULL OR length(actual_model) BETWEEN 1 AND 128),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    started_at INTEGER CHECK (started_at IS NULL OR started_at >= created_at),
    completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= created_at),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    last_event_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_event_sequence >= 0),
    final_answer_json TEXT,
    error TEXT,
    stderr_truncated INTEGER NOT NULL DEFAULT 0 CHECK (stderr_truncated IN (0, 1)),
    saved_tidbit_id TEXT,
    FOREIGN KEY (rerun_of_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (saved_tidbit_id) REFERENCES tidbit(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CHECK (rerun_of_id IS NULL OR rerun_of_id <> id),
    CHECK (
        (status IN ('QUEUED', 'RUNNING') AND completed_at IS NULL)
        OR (status IN ('COMPLETED', 'CANCELED', 'FAILED', 'INTERRUPTED') AND completed_at IS NOT NULL)
    ),
    CHECK (status <> 'COMPLETED' OR final_answer_json IS NOT NULL),
    CHECK (saved_tidbit_id IS NULL OR status = 'COMPLETED')
) STRICT;
CREATE TABLE research_run_event (
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    kind TEXT NOT NULL CHECK (kind IN (
        'STARTED',
        'METADATA',
        'UNTRUSTED_TEXT_DELTA',
        'TOOL_ACTIVITY',
        'GROUNDED_FINAL_OUTPUT',
        'FINISHED'
    )),
    payload_json TEXT NOT NULL CHECK (length(payload_json) BETWEEN 2 AND 2097152),
    PRIMARY KEY (run_id, sequence),
    FOREIGN KEY (run_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE research_run_attachment (
    research_run_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    PRIMARY KEY (research_run_id, attachment_id),
    FOREIGN KEY (research_run_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE TABLE tidbit_purge_authorization (
    tidbit_id TEXT PRIMARY KEY,
    expected_revision_id TEXT NOT NULL,
    authorized_at INTEGER NOT NULL CHECK (authorized_at >= 0),
    CHECK (
        length(tidbit_id) = 36
        AND lower(tidbit_id) = tidbit_id
        AND substr(tidbit_id, 15, 1) = '7'
    ),
    CHECK (
        length(expected_revision_id) = 36
        AND lower(expected_revision_id) = expected_revision_id
        AND substr(expected_revision_id, 15, 1) = '7'
    )
) STRICT;
PRAGMA writable_schema=ON;
INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)VALUES('table','passage_fts_word','passage_fts_word',0,'CREATE VIRTUAL TABLE passage_fts_word USING fts5(
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = ''passage_search_document'',
    content_rowid = ''rowid'',
    tokenize = ''unicode61 remove_diacritics 2 tokenchars ''''_''''''
)');
INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)VALUES('table','passage_fts_trigram','passage_fts_trigram',0,'CREATE VIRTUAL TABLE passage_fts_trigram USING fts5(
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = ''passage_search_document'',
    content_rowid = ''rowid'',
    tokenize = ''trigram''
)');
INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)VALUES('table','passage_fts_short','passage_fts_short',0,'CREATE VIRTUAL TABLE passage_fts_short USING fts5(
    title,
    heading_context,
    body,
    source_labels,
    source_domains,
    attachment_names,
    extracted_text,
    content = ''passage_search_document'',
    content_rowid = ''rowid'',
    tokenize = ''unicode61''
)');
CREATE INDEX tidbit_active_updated_idx
    ON tidbit(updated_at DESC, id)
    WHERE deleted_at IS NULL;
CREATE INDEX media_ingest_lease_reconciliation_idx
    ON media_ingest_lease(state, expires_at, id);
CREATE UNIQUE INDEX passage_author_version_ordinal_uq
    ON passage(tidbit_revision_id, construction_version, ordinal)
    WHERE owner_kind = 'AUTHOR';
CREATE UNIQUE INDEX passage_attachment_version_ordinal_uq
    ON passage(attachment_segment_id, construction_version, ordinal)
    WHERE owner_kind = 'ATTACHMENT';
CREATE INDEX active_passage_tidbit_idx ON active_passage(tidbit_id);
CREATE INDEX passage_search_document_tidbit_idx
    ON passage_search_document(tidbit_id);
CREATE INDEX attachment_sha256_idx
    ON attachment(sha256, id);
CREATE INDEX media_ingest_lease_attachment_idx
    ON media_ingest_lease(attachment_id, state, expires_at);
CREATE INDEX media_blob_reap_candidate_age_idx
    ON media_blob_reap_candidate(orphaned_at, sha256);
CREATE INDEX attachment_image_preview_sha256_idx
    ON attachment_image(preview_sha256, attachment_id);
CREATE INDEX image_ocr_queue_eligible_idx
    ON image_ocr_queue(next_attempt_at, extraction_id)
    WHERE state IN ('PENDING', 'RETRY_WAIT');
CREATE INDEX image_ocr_queue_running_idx
    ON image_ocr_queue(started_at, extraction_id)
    WHERE state = 'RUNNING';
CREATE INDEX pdf_extraction_queue_eligible_idx
    ON pdf_extraction_queue(next_attempt_at, extraction_id)
    WHERE state IN ('PENDING', 'RETRY_WAIT');
CREATE INDEX pdf_extraction_queue_running_idx
    ON pdf_extraction_queue(started_at, extraction_id)
    WHERE state = 'RUNNING';
CREATE INDEX research_run_updated_idx
    ON research_run(updated_at DESC, id DESC);
CREATE INDEX research_run_active_idx
    ON research_run(status, id)
    WHERE status IN ('QUEUED', 'RUNNING');
CREATE INDEX research_run_attachment_attachment_idx
    ON research_run_attachment(attachment_id, research_run_id);
CREATE INDEX tidbit_deleted_updated_idx
    ON tidbit(updated_at DESC, id DESC)
    WHERE deleted_at IS NOT NULL;
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
CREATE TRIGGER tidbit_revision_attachment_prevent_update
BEFORE UPDATE ON tidbit_revision_attachment
BEGIN
    SELECT RAISE(ABORT, 'revision attachment links are immutable');
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
    INSERT INTO passage_fts_short(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_short_grams(new.title),
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
    INSERT INTO passage_fts_short(
        passage_fts_short, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_short_grams(old.title),
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
    INSERT INTO passage_fts_short(
        passage_fts_short, rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        'delete', old.rowid,
        kosh_search_short_grams(old.title),
        kosh_search_short_grams(old.heading_context),
        kosh_search_short_grams(old.body),
        kosh_search_short_grams(old.source_labels),
        kosh_search_short_grams(old.source_domains),
        kosh_search_short_grams(old.attachment_names),
        kosh_search_short_grams(old.extracted_text)
    );
    INSERT INTO passage_fts_short(
        rowid, title, heading_context, body, source_labels,
        source_domains, attachment_names, extracted_text
    ) VALUES(
        new.rowid,
        kosh_search_short_grams(new.title),
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
CREATE TRIGGER attachment_identity_prevent_update
BEFORE UPDATE OF created_at, sha256, byte_length, kind ON attachment
BEGIN
    SELECT RAISE(ABORT, 'attachment identity is immutable');
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
CREATE TRIGGER tidbit_purge_authorization_validate_insert
BEFORE INSERT ON tidbit_purge_authorization
BEGIN
    SELECT RAISE(ABORT, 'tidbit is not eligible for permanent deletion')
    WHERE NOT EXISTS (
        SELECT 1
        FROM tidbit
        WHERE tidbit.id = new.tidbit_id
          AND tidbit.current_revision_id = new.expected_revision_id
          AND tidbit.deleted_at IS NOT NULL
          AND new.authorized_at >= tidbit.deleted_at + 2592000000
    );
END;
CREATE TRIGGER tidbit_revision_attachment_prevent_delete
BEFORE DELETE ON tidbit_revision_attachment
WHEN NOT EXISTS (
    SELECT 1
    FROM tidbit_revision
    JOIN tidbit_purge_authorization AS authorization
      ON authorization.tidbit_id = tidbit_revision.tidbit_id
    WHERE tidbit_revision.id = old.tidbit_revision_id
)
BEGIN
    SELECT RAISE(ABORT, 'revision attachment links are retained');
END;
CREATE TRIGGER tidbit_revision_source_prevent_delete
BEFORE DELETE ON tidbit_revision_source
WHEN NOT EXISTS (
    SELECT 1
    FROM tidbit_revision
    JOIN tidbit_purge_authorization AS authorization
      ON authorization.tidbit_id = tidbit_revision.tidbit_id
    WHERE tidbit_revision.id = old.tidbit_revision_id
)
BEGIN
    SELECT RAISE(ABORT, 'revision source links are retained');
END;
CREATE TRIGGER attachment_passage_revision_prevent_delete
BEFORE DELETE ON attachment_passage_revision
WHEN NOT EXISTS (
    SELECT 1
    FROM tidbit_revision
    JOIN tidbit_purge_authorization AS authorization
      ON authorization.tidbit_id = tidbit_revision.tidbit_id
    WHERE tidbit_revision.id = old.tidbit_revision_id
)
BEGIN
    SELECT RAISE(ABORT, 'attachment passage revision provenance is retained');
END;
CREATE TRIGGER passage_prevent_delete
BEFORE DELETE ON passage
WHEN old.owner_kind != 'AUTHOR'
  OR NOT EXISTS (
      SELECT 1
      FROM tidbit_revision
      JOIN tidbit_purge_authorization AS authorization
        ON authorization.tidbit_id = tidbit_revision.tidbit_id
      WHERE tidbit_revision.id = old.tidbit_revision_id
  )
BEGIN
    SELECT RAISE(ABORT, 'passages are retained');
END;
CREATE TRIGGER tidbit_revision_prevent_delete
BEFORE DELETE ON tidbit_revision
WHEN NOT EXISTS (
    SELECT 1
    FROM tidbit_purge_authorization AS authorization
    WHERE authorization.tidbit_id = old.tidbit_id
)
BEGIN
    SELECT RAISE(ABORT, 'tidbit revisions are retained');
END;
CREATE TRIGGER source_prevent_delete
BEFORE DELETE ON source
WHEN NOT EXISTS (
        SELECT 1
        FROM tidbit_revision_source AS membership
        JOIN tidbit_revision
          ON tidbit_revision.id = membership.tidbit_revision_id
        JOIN tidbit_purge_authorization AS authorization
          ON authorization.tidbit_id = tidbit_revision.tidbit_id
        WHERE membership.source_id = old.id
    )
    OR EXISTS (
        SELECT 1
        FROM tidbit_revision_source AS membership
        JOIN tidbit_revision
          ON tidbit_revision.id = membership.tidbit_revision_id
        LEFT JOIN tidbit_purge_authorization AS authorization
          ON authorization.tidbit_id = tidbit_revision.tidbit_id
        WHERE membership.source_id = old.id
          AND authorization.tidbit_id IS NULL
    )
BEGIN
    SELECT RAISE(ABORT, 'sources are retained');
END;
PRAGMA writable_schema=OFF;
COMMIT;
