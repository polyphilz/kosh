ALTER TABLE passage
    ADD COLUMN construction_version TEXT NOT NULL DEFAULT 'legacy'
        CHECK (length(construction_version) > 0);

ALTER TABLE passage
    ADD COLUMN heading_context_json TEXT NOT NULL DEFAULT '[]'
        CHECK (
            json_valid(heading_context_json)
            AND json_type(heading_context_json) = 'array'
        );

DROP INDEX passage_author_ordinal_uq;
CREATE UNIQUE INDEX passage_author_version_ordinal_uq
    ON passage(tidbit_revision_id, construction_version, ordinal)
    WHERE owner_kind = 'AUTHOR';

DROP INDEX passage_attachment_ordinal_uq;
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

INSERT INTO index_state(name, version, status, cursor, updated_at, error)
VALUES('PASSAGE_BUILD', 'markdown-blocks-v1', 'DIRTY', NULL, 0, NULL);
