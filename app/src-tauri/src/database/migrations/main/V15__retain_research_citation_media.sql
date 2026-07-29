-- Grounded research answers retain immutable evidence snapshots. Materialize
-- their attachment owners so media lifecycle maintenance can treat those
-- blobs as durable even when no authored revision references them.
CREATE TABLE research_run_attachment (
    research_run_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    PRIMARY KEY (research_run_id, attachment_id),
    FOREIGN KEY (research_run_id) REFERENCES research_run(id)
        ON UPDATE RESTRICT ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attachment_id) REFERENCES attachment(id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

CREATE INDEX research_run_attachment_attachment_idx
    ON research_run_attachment(attachment_id, research_run_id);

INSERT OR IGNORE INTO research_run_attachment(research_run_id, attachment_id)
SELECT
    research_run.id,
    attachment.id
FROM research_run
JOIN json_each(research_run.final_answer_json, '$.citations') AS citation
JOIN attachment
  ON attachment.id = json_extract(
      citation.value,
      '$.evidence.attachment.id'
  )
WHERE research_run.final_answer_json IS NOT NULL
  AND json_type(citation.value, '$.evidence.attachment.id') = 'text';
