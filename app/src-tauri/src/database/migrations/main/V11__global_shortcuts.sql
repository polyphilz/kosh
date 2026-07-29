ALTER TABLE draft_context RENAME TO draft_context_before_quick_add;

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

INSERT INTO draft_context (
    draft_id,
    context_key,
    tidbit_id,
    base_revision_id
)
SELECT
    draft_id,
    context_key,
    tidbit_id,
    base_revision_id
FROM draft_context_before_quick_add;

DROP TABLE draft_context_before_quick_add;

CREATE TABLE shortcut_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0)
) STRICT;

INSERT INTO shortcut_settings (singleton_id, revision) VALUES (1, 1);

CREATE TABLE keyboard_binding (
    command TEXT PRIMARY KEY CHECK (command IN ('QUICK_ADD', 'MAIN_WINDOW')),
    accelerator TEXT NOT NULL CHECK (length(accelerator) BETWEEN 3 AND 96)
) STRICT, WITHOUT ROWID;

INSERT INTO keyboard_binding (command, accelerator) VALUES
    ('QUICK_ADD', 'control+alt+super+KeyK'),
    ('MAIN_WINDOW', 'control+alt+super+KeyO');

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
