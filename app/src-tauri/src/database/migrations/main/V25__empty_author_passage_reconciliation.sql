CREATE TABLE empty_author_passage_revision (
    tidbit_revision_id TEXT NOT NULL,
    construction_version TEXT NOT NULL CHECK (length(construction_version) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (tidbit_revision_id, construction_version),
    FOREIGN KEY (tidbit_revision_id) REFERENCES tidbit_revision(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

UPDATE index_state
SET status = 'DIRTY', cursor = NULL, error = NULL
WHERE name = 'PASSAGE_BUILD';
