ALTER TABLE draft_context RENAME TO draft_context_before_working_copies;

CREATE TABLE draft_context (
    draft_id TEXT PRIMARY KEY,
    context_key TEXT NOT NULL UNIQUE CHECK (length(context_key) BETWEEN 1 AND 96),
    tidbit_id TEXT,
    base_revision_id TEXT,
    note_id TEXT UNIQUE,
    edit_generation INTEGER NOT NULL DEFAULT 0
        CHECK (edit_generation BETWEEN 0 AND 9007199254740991),
    CHECK (
        (
            context_key IN ('capture', 'quick-add')
            AND tidbit_id IS NULL
            AND base_revision_id IS NULL
            AND note_id IS NULL
            AND edit_generation = 0
        )
        OR (
            context_key = 'edit:' || tidbit_id
            AND tidbit_id IS NOT NULL
            AND base_revision_id IS NOT NULL
            AND note_id IS NULL
            AND edit_generation = 0
        )
        OR (
            context_key = 'note:' || note_id
            AND note_id IS NOT NULL
            AND edit_generation > 0
            AND (
                (tidbit_id IS NULL AND base_revision_id IS NULL)
                OR (tidbit_id = note_id AND base_revision_id IS NOT NULL)
            )
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

INSERT INTO draft_context (
    draft_id,
    context_key,
    tidbit_id,
    base_revision_id,
    note_id,
    edit_generation
)
SELECT
    draft_id,
    context_key,
    tidbit_id,
    base_revision_id,
    NULL,
    0
FROM draft_context_before_working_copies;

DROP TABLE draft_context_before_working_copies;

CREATE TRIGGER offsite_clock_draft_context_insert
AFTER INSERT ON draft_context
BEGIN
    UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1;
END;

CREATE TRIGGER offsite_clock_draft_context_update
AFTER UPDATE ON draft_context
BEGIN
    UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1;
END;

CREATE TRIGGER offsite_clock_draft_context_delete
AFTER DELETE ON draft_context
BEGIN
    UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1;
END;
