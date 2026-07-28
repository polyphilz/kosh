CREATE TABLE draft_context (
    draft_id TEXT PRIMARY KEY,
    context_key TEXT NOT NULL UNIQUE CHECK (length(context_key) BETWEEN 1 AND 96),
    tidbit_id TEXT,
    base_revision_id TEXT,
    CHECK (
        (
            context_key = 'capture'
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

CREATE TABLE draft_source (
    draft_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    label TEXT,
    url TEXT,
    PRIMARY KEY (draft_id, position),
    FOREIGN KEY (draft_id) REFERENCES draft(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
