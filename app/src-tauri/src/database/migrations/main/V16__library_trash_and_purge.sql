CREATE INDEX tidbit_deleted_updated_idx
    ON tidbit(updated_at DESC, id DESC)
    WHERE deleted_at IS NOT NULL;

-- Keep the immutable-history guarantees while allowing the writer's explicit,
-- time-delayed purge transaction to remove one complete authored graph. The
-- authorization row is transaction-scoped by application convention, validates
-- the exact current revision and grace period in SQLite, and is removed before
-- the transaction commits.
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

DROP TRIGGER tidbit_revision_attachment_prevent_delete;
DROP TRIGGER tidbit_revision_source_prevent_delete;
DROP TRIGGER attachment_passage_revision_prevent_delete;
DROP TRIGGER passage_prevent_delete;
DROP TRIGGER tidbit_revision_prevent_delete;
DROP TRIGGER source_prevent_delete;

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
