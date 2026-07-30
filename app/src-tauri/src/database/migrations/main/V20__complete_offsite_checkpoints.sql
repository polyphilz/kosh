CREATE TABLE offsite_backup_checkpoint (
    checkpoint_id TEXT PRIMARY KEY
        CHECK (
            length(checkpoint_id) = 36
            AND lower(checkpoint_id) = checkpoint_id
            AND substr(checkpoint_id, 9, 1) = '-'
            AND substr(checkpoint_id, 14, 1) = '-'
            AND substr(checkpoint_id, 15, 1) = '7'
            AND substr(checkpoint_id, 19, 1) = '-'
            AND substr(checkpoint_id, 20, 1) GLOB '[89ab]'
            AND substr(checkpoint_id, 24, 1) = '-'
            AND length(replace(checkpoint_id, '-', '')) = 32
            AND replace(checkpoint_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    backup_set_id TEXT NOT NULL
        CHECK (
            length(backup_set_id) = 36
            AND lower(backup_set_id) = backup_set_id
            AND substr(backup_set_id, 9, 1) = '-'
            AND substr(backup_set_id, 14, 1) = '-'
            AND substr(backup_set_id, 15, 1) = '7'
            AND substr(backup_set_id, 19, 1) = '-'
            AND substr(backup_set_id, 20, 1) GLOB '[89ab]'
            AND substr(backup_set_id, 24, 1) = '-'
            AND length(replace(backup_set_id, '-', '')) = 32
            AND replace(backup_set_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    replica_epoch_id TEXT NOT NULL
        CHECK (
            length(replica_epoch_id) = 36
            AND lower(replica_epoch_id) = replica_epoch_id
            AND substr(replica_epoch_id, 9, 1) = '-'
            AND substr(replica_epoch_id, 14, 1) = '-'
            AND substr(replica_epoch_id, 15, 1) = '7'
            AND substr(replica_epoch_id, 19, 1) = '-'
            AND substr(replica_epoch_id, 20, 1) GLOB '[89ab]'
            AND substr(replica_epoch_id, 24, 1) = '-'
            AND length(replace(replica_epoch_id, '-', '')) = 32
            AND replace(replica_epoch_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    phase TEXT NOT NULL
        CHECK (phase IN ('PREPARED', 'FENCED', 'REPLICATED', 'PUBLISHED', 'FAILED')),
    config_revision INTEGER NOT NULL CHECK (config_revision > 0),
    content_revision INTEGER NOT NULL CHECK (content_revision >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    kosh_version TEXT NOT NULL
        CHECK (
            length(CAST(kosh_version AS BLOB)) BETWEEN 1 AND 64
            AND instr(kosh_version, char(0)) = 0
            AND instr(kosh_version, char(10)) = 0
            AND instr(kosh_version, char(13)) = 0
        ),
    main_migration_head INTEGER NOT NULL CHECK (main_migration_head > 0),
    media_migration_head INTEGER NOT NULL CHECK (media_migration_head > 0),
    referenced_hash_count INTEGER NOT NULL CHECK (referenced_hash_count >= 0),
    referenced_total_bytes INTEGER NOT NULL CHECK (referenced_total_bytes >= 0),
    referenced_hash_set_sha256 BLOB NOT NULL
        CHECK (length(referenced_hash_set_sha256) = 32),
    litestream_txid TEXT
        CHECK (
            litestream_txid IS NULL
            OR (
                length(litestream_txid) = 16
                AND lower(litestream_txid) = litestream_txid
                AND litestream_txid NOT GLOB '*[^0-9a-f]*'
            )
        ),
    manifest_object_key TEXT
        CHECK (
            manifest_object_key IS NULL
            OR length(CAST(manifest_object_key AS BLOB)) BETWEEN 1 AND 1024
        ),
    publication_sequence INTEGER
        CHECK (publication_sequence IS NULL OR publication_sequence > 0),
    last_error_code TEXT
        CHECK (
            last_error_code IS NULL
            OR (
                length(last_error_code) BETWEEN 1 AND 64
                AND last_error_code NOT GLOB '*[^A-Z0-9_]*'
            )
        ),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK (
        phase IN ('PREPARED', 'FAILED')
        OR litestream_txid IS NOT NULL
    ),
    CHECK (
        phase = 'PUBLISHED'
        OR (manifest_object_key IS NULL AND publication_sequence IS NULL)
    ),
    CHECK (
        phase <> 'PUBLISHED'
        OR (
            litestream_txid IS NOT NULL
            AND manifest_object_key IS NOT NULL
            AND publication_sequence IS NOT NULL
            AND last_error_code IS NULL
        )
    ),
    CHECK (phase <> 'FAILED' OR last_error_code IS NOT NULL),
    CHECK (phase = 'FAILED' OR last_error_code IS NULL)
) STRICT;

CREATE UNIQUE INDEX offsite_backup_checkpoint_publication_sequence_idx
    ON offsite_backup_checkpoint(publication_sequence)
    WHERE publication_sequence IS NOT NULL;

CREATE INDEX offsite_backup_checkpoint_lineage_idx
    ON offsite_backup_checkpoint(
        backup_set_id,
        replica_epoch_id,
        publication_sequence DESC
    );

CREATE TABLE offsite_backup_content_clock (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;

INSERT INTO offsite_backup_content_clock(singleton_id, revision) VALUES (1, 0);

CREATE TRIGGER offsite_backup_content_clock_prevent_delete
BEFORE DELETE ON offsite_backup_content_clock
BEGIN
    SELECT RAISE(ABORT, 'off-site backup content clock cannot be deleted');
END;

-- Every recoverable main-database mutation advances this durable clock in the
-- same transaction. Backup bookkeeping tables are deliberately excluded so
-- publishing a checkpoint cannot schedule another checkpoint by itself.
CREATE TRIGGER offsite_clock_active_passage_insert AFTER INSERT ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_active_passage_update AFTER UPDATE ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_active_passage_delete AFTER DELETE ON active_passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_app_settings_insert AFTER INSERT ON app_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_app_settings_update AFTER UPDATE ON app_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_app_settings_delete AFTER DELETE ON app_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_insert AFTER INSERT ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_update AFTER UPDATE ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_delete AFTER DELETE ON attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_insert AFTER INSERT ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_update AFTER UPDATE ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extraction_delete AFTER DELETE ON attachment_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_insert AFTER INSERT ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_update AFTER UPDATE ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_extractor_config_delete AFTER DELETE ON attachment_extractor_config BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_insert AFTER INSERT ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_update AFTER UPDATE ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_image_delete AFTER DELETE ON attachment_image BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_insert AFTER INSERT ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_update AFTER UPDATE ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_passage_revision_delete AFTER DELETE ON attachment_passage_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_pdf_insert AFTER INSERT ON attachment_pdf BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_pdf_update AFTER UPDATE ON attachment_pdf BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_pdf_delete AFTER DELETE ON attachment_pdf BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_insert AFTER INSERT ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_update AFTER UPDATE ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_attachment_segment_delete AFTER DELETE ON attachment_segment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_insert AFTER INSERT ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_update AFTER UPDATE ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_delete AFTER DELETE ON draft BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_context_insert AFTER INSERT ON draft_context BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_context_update AFTER UPDATE ON draft_context BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_context_delete AFTER DELETE ON draft_context BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_insert AFTER INSERT ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_update AFTER UPDATE ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_media_lease_delete AFTER DELETE ON draft_media_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_insert AFTER INSERT ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_update AFTER UPDATE ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_draft_source_delete AFTER DELETE ON draft_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_insert AFTER INSERT ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_update AFTER UPDATE ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_image_ocr_queue_delete AFTER DELETE ON image_ocr_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_insert AFTER INSERT ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_update AFTER UPDATE ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_index_state_delete AFTER DELETE ON index_state BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_insert AFTER INSERT ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_update AFTER UPDATE ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_keyboard_binding_delete AFTER DELETE ON keyboard_binding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_insert AFTER INSERT ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_update AFTER UPDATE ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_blob_reap_candidate_delete AFTER DELETE ON media_blob_reap_candidate BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_insert AFTER INSERT ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_update AFTER UPDATE ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_media_ingest_lease_delete AFTER DELETE ON media_ingest_lease BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_insert AFTER INSERT ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_update AFTER UPDATE ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_delete AFTER DELETE ON passage BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_insert AFTER INSERT ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_update AFTER UPDATE ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_delete AFTER DELETE ON passage_embedding BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_insert AFTER INSERT ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_update AFTER UPDATE ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_index_delete AFTER DELETE ON passage_embedding_index BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_insert AFTER INSERT ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_update AFTER UPDATE ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_reap_queue_delete AFTER DELETE ON passage_embedding_reap_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_insert AFTER INSERT ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_update AFTER UPDATE ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_embedding_settings_delete AFTER DELETE ON passage_embedding_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_insert AFTER INSERT ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_update AFTER UPDATE ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_passage_search_document_delete AFTER DELETE ON passage_search_document BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_extraction_queue_insert AFTER INSERT ON pdf_extraction_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_extraction_queue_update AFTER UPDATE ON pdf_extraction_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_extraction_queue_delete AFTER DELETE ON pdf_extraction_queue BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_page_extraction_insert AFTER INSERT ON pdf_page_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_page_extraction_update AFTER UPDATE ON pdf_page_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_pdf_page_extraction_delete AFTER DELETE ON pdf_page_extraction BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_insert AFTER INSERT ON research_run BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_update AFTER UPDATE ON research_run BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_delete AFTER DELETE ON research_run BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_attachment_insert AFTER INSERT ON research_run_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_attachment_update AFTER UPDATE ON research_run_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_attachment_delete AFTER DELETE ON research_run_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_event_insert AFTER INSERT ON research_run_event BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_event_update AFTER UPDATE ON research_run_event BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_research_run_event_delete AFTER DELETE ON research_run_event BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_insert AFTER INSERT ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_update AFTER UPDATE ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_shortcut_settings_delete AFTER DELETE ON shortcut_settings BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_insert AFTER INSERT ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_update AFTER UPDATE ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_source_delete AFTER DELETE ON source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_insert AFTER INSERT ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_update AFTER UPDATE ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_delete AFTER DELETE ON tidbit BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_purge_authorization_insert AFTER INSERT ON tidbit_purge_authorization BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_purge_authorization_update AFTER UPDATE ON tidbit_purge_authorization BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_purge_authorization_delete AFTER DELETE ON tidbit_purge_authorization BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_insert AFTER INSERT ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_update AFTER UPDATE ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_delete AFTER DELETE ON tidbit_revision BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_insert AFTER INSERT ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_update AFTER UPDATE ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_attachment_delete AFTER DELETE ON tidbit_revision_attachment BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_insert AFTER INSERT ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_update AFTER UPDATE ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
CREATE TRIGGER offsite_clock_tidbit_revision_source_delete AFTER DELETE ON tidbit_revision_source BEGIN UPDATE offsite_backup_content_clock SET revision = revision + 1 WHERE singleton_id = 1; END;
