-- V1 reserved research tables before the process protocol existed. Rebuild
-- them now that runs have an exact event envelope and grounded snapshots.
ALTER TABLE research_citation RENAME TO research_citation_legacy;
ALTER TABLE research_event RENAME TO research_event_legacy;
ALTER TABLE research_run RENAME TO research_run_legacy;

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

INSERT INTO research_run(
    id,
    query,
    status,
    created_at,
    started_at,
    completed_at,
    updated_at,
    final_answer_json,
    error
)
SELECT
    id,
    query,
    CASE
        WHEN status = 'PENDING' THEN 'QUEUED'
        WHEN status = 'COMPLETED' AND answer_markdown IS NULL THEN 'FAILED'
        ELSE status
    END,
    created_at,
    started_at,
    CASE
        WHEN status IN ('COMPLETED', 'FAILED', 'CANCELED')
            THEN coalesce(completed_at, started_at, created_at)
        ELSE NULL
    END,
    coalesce(completed_at, started_at, created_at),
    CASE
        WHEN status = 'COMPLETED' AND answer_markdown IS NOT NULL
            THEN json_object(
                'markdown', answer_markdown,
                'citations', json('[]'),
                'mentions', json('[]'),
                'issues', json('[]')
            )
        ELSE NULL
    END,
    CASE
        WHEN status = 'COMPLETED' AND answer_markdown IS NULL
            THEN 'Legacy research answer was incomplete.'
        ELSE error
    END
FROM research_run_legacy;

CREATE INDEX research_run_updated_idx
    ON research_run(updated_at DESC, id DESC);

CREATE INDEX research_run_active_idx
    ON research_run(status, id)
    WHERE status IN ('QUEUED', 'RUNNING');

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

DROP TABLE research_citation_legacy;
DROP TABLE research_event_legacy;
DROP TABLE research_run_legacy;
