-- Legacy answers already contain numbered markers, while V13 restores their
-- exact citation registry. Rebuild trusted byte ranges from markers that are
-- visible Markdown text and refer to a preserved registry entry.
UPDATE research_run
SET final_answer_json = json_set(
    final_answer_json,
    '$.mentions',
    json(kosh_research_citation_mentions(
        json_extract(final_answer_json, '$.markdown'),
        json_array_length(final_answer_json, '$.citations')
    ))
)
WHERE status = 'COMPLETED'
  AND json_array_length(final_answer_json, '$.citations') > 0
  AND json_array_length(final_answer_json, '$.mentions') = 0;
