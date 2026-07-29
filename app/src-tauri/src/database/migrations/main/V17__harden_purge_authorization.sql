-- V16 shipped on review branches and may already be present in developer
-- profiles. Harden it additively so those profiles remain migratable.
CREATE TRIGGER tidbit_purge_authorization_prevent_update
BEFORE UPDATE ON tidbit_purge_authorization
BEGIN
    SELECT RAISE(ABORT, 'purge authorizations are immutable');
END;

CREATE TRIGGER tidbit_purge_authorization_revoke_on_tidbit_update
AFTER UPDATE OF current_revision_id, deleted_at ON tidbit
BEGIN
    DELETE FROM tidbit_purge_authorization WHERE tidbit_id = new.id;
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
    JOIN tidbit
      ON tidbit.id = authorization.tidbit_id
     AND tidbit.current_revision_id = authorization.expected_revision_id
     AND tidbit.deleted_at IS NOT NULL
     AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
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
    JOIN tidbit
      ON tidbit.id = authorization.tidbit_id
     AND tidbit.current_revision_id = authorization.expected_revision_id
     AND tidbit.deleted_at IS NOT NULL
     AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
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
    JOIN tidbit
      ON tidbit.id = authorization.tidbit_id
     AND tidbit.current_revision_id = authorization.expected_revision_id
     AND tidbit.deleted_at IS NOT NULL
     AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
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
      JOIN tidbit
        ON tidbit.id = authorization.tidbit_id
       AND tidbit.current_revision_id = authorization.expected_revision_id
       AND tidbit.deleted_at IS NOT NULL
       AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
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
    JOIN tidbit
      ON tidbit.id = authorization.tidbit_id
     AND tidbit.current_revision_id = authorization.expected_revision_id
     AND tidbit.deleted_at IS NOT NULL
     AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
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
        JOIN tidbit
          ON tidbit.id = authorization.tidbit_id
         AND tidbit.current_revision_id = authorization.expected_revision_id
         AND tidbit.deleted_at IS NOT NULL
         AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
        WHERE membership.source_id = old.id
    )
    OR EXISTS (
        SELECT 1
        FROM tidbit_revision_source AS membership
        JOIN tidbit_revision
          ON tidbit_revision.id = membership.tidbit_revision_id
        WHERE membership.source_id = old.id
          AND NOT EXISTS (
              SELECT 1
              FROM tidbit_purge_authorization AS authorization
              JOIN tidbit
                ON tidbit.id = authorization.tidbit_id
               AND tidbit.current_revision_id = authorization.expected_revision_id
               AND tidbit.deleted_at IS NOT NULL
               AND authorization.authorized_at >= tidbit.deleted_at + 2592000000
              WHERE authorization.tidbit_id = tidbit_revision.tidbit_id
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'sources are retained');
END;
