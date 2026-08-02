ALTER TABLE draft_context
ADD COLUMN media_reservation INTEGER NOT NULL DEFAULT 0
    CHECK (media_reservation IN (0, 1));
