-- V12 replaced the reserved V1 research tables with durable grounded answers.
-- run_main creates this staging table before V12 only when legacy citations
-- exist, resolving every passage into an immutable citation snapshot.
CREATE TABLE IF NOT EXISTS legacy_research_citation_snapshot (
    run_id TEXT PRIMARY KEY,
    citations_json TEXT NOT NULL
        CHECK (
            json_valid(citations_json)
            AND json_type(citations_json) = 'array'
        )
) STRICT;

UPDATE research_run
SET final_answer_json = json_set(
    final_answer_json,
    '$.citations',
    json((
        SELECT snapshot.citations_json
        FROM legacy_research_citation_snapshot AS snapshot
        WHERE snapshot.run_id = research_run.id
    ))
)
WHERE status = 'COMPLETED'
  AND EXISTS (
      SELECT 1
      FROM legacy_research_citation_snapshot AS snapshot
      WHERE snapshot.run_id = research_run.id
  );

DROP TABLE legacy_research_citation_snapshot;
