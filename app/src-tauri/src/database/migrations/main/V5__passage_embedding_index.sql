DROP TABLE passage_embedding;
DROP TABLE embedding_index;

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

CREATE VIRTUAL TABLE passage_embedding_vec_jina_v1 USING vec0(
    embedding float[768] distance_metric=cosine
);

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

INSERT INTO passage_embedding_settings(
    singleton_id,
    active_embedding_index_id,
    updated_at
) VALUES(1, NULL, 0);

CREATE TRIGGER passage_embedding_settings_prevent_delete
BEFORE DELETE ON passage_embedding_settings
BEGIN
    SELECT RAISE(ABORT, 'passage embedding settings singleton cannot be deleted');
END;

CREATE TRIGGER passage_embedding_invalidate_after_search_delete
AFTER DELETE ON passage_search_document
BEGIN
    DELETE FROM passage_embedding WHERE passage_id = old.passage_id;
    UPDATE index_state
    SET status = CASE
            WHEN status IN ('RUNNING', 'FAILED') THEN status
            ELSE 'DIRTY'
        END,
        cursor = NULL,
        error = NULL
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
        error = NULL
    WHERE name = 'PASSAGE_EMBEDDING';
END;

CREATE TRIGGER passage_embedding_invalidate_after_search_update
AFTER UPDATE OF passage_id, body, extracted_text, owner_content_hash
ON passage_search_document
BEGIN
    DELETE FROM passage_embedding WHERE passage_id = old.passage_id;
    UPDATE index_state
    SET status = CASE
            WHEN status IN ('RUNNING', 'FAILED') THEN status
            ELSE 'DIRTY'
        END,
        cursor = NULL,
        error = NULL
    WHERE name = 'PASSAGE_EMBEDDING';
END;

INSERT INTO index_state(name, version, status, cursor, updated_at, error)
VALUES(
    'PASSAGE_EMBEDDING',
    'jina_v1',
    CASE
        WHEN EXISTS (SELECT 1 FROM passage_search_document) THEN 'DIRTY'
        ELSE 'IDLE'
    END,
    NULL,
    0,
    NULL
);
