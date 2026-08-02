use std::{io::Cursor, path::Path};

use serde::Deserialize;

use crate::backup::domain::{
    BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target, ReplicaEpochId,
};

use super::{
    backup_state::SaveOffsiteBackupConfigInput,
    drafts::SaveDraftWrite,
    media::{
        CanonicalImage, ImageOcrRegion, IngestAttachmentMetadata, IngestGenericAttachmentWrite,
        IngestImageWrite, IngestPdfWrite, PdfPageExtraction, PdfPageSource, StagedAttachment,
        TextFileSegment,
    },
    settings::SetShortcutSettingsInput,
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    CitationLocator, CitationState, Database, DatabasePaths, DeleteTidbitInput, EditTidbitInput,
    KeyboardBinding, KoshCommand, LexicalSearchMode, MediaLimits, SaveDraftInput,
    SearchPassagesInput, SourceDraft, TidbitDraft,
};

const FIXTURE_JSON: &str = include_str!("fixtures/redesign-baseline-v1.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fixture {
    schema_version: u32,
    active_note: ActiveNoteFixture,
    media: MediaFixture,
    saved_draft: SavedDraftFixture,
    deleted_note: DeletedNoteFixture,
    settings: SettingsFixture,
    backup: BackupFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveNoteFixture {
    id: String,
    initial_revision_id: String,
    current_revision_id: String,
    initial_source_id: String,
    current_source_id: String,
    title: String,
    initial_marker: String,
    current_marker: String,
    source_label: String,
    source_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MediaFixture {
    capture_draft_id: String,
    image: MediaItemFixture,
    pdf: MediaItemFixture,
    text: MediaItemFixture,
    opaque: MediaItemFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MediaItemFixture {
    attachment_id: String,
    lease_id: String,
    staging_id: String,
    extraction_id: String,
    filename: String,
    ocr_marker: Option<String>,
    page_marker: Option<String>,
    line_marker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedDraftFixture {
    id: String,
    context_key: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeletedNoteFixture {
    id: String,
    revision_id: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsFixture {
    quick_add: String,
    main_window: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupFixture {
    backup_set_id: BackupSetId,
    replica_epoch_id: ReplicaEpochId,
    account_id: String,
    bucket: String,
}

fn stage(bytes: &[u8], staging_root: &Path, item: &MediaItemFixture) -> StagedAttachment {
    StagedAttachment::from_reader(
        Cursor::new(bytes),
        staging_root,
        &item.staging_id,
        MediaLimits::default().max_attachment_bytes,
    )
    .expect("stage deterministic baseline attachment")
}

fn metadata(
    fixture: &Fixture,
    item: &MediaItemFixture,
    media_type: &str,
    now_ms: i64,
) -> IngestAttachmentMetadata {
    IngestAttachmentMetadata {
        attachment_id: item.attachment_id.clone(),
        ingest_lease_id: item.lease_id.clone(),
        draft_id: fixture.media.capture_draft_id.clone(),
        display_filename: item.filename.clone(),
        media_type: media_type.into(),
        now_ms,
        limits: MediaLimits::default(),
    }
}

fn rich_body(fixture: &Fixture, marker: &str) -> String {
    format!(
        "# Arrays and vectors\n\n\
         - contiguous storage\n  - nested cache detail\n\n\
         1. infer the dtype\n2. inspect the shape\n\n\
         **Bold**, *italic*, ~~obsolete~~, and `ndarray`.\n\n\
         ```python\na = np.array([1, 2, 3])\n```\n\n\
         Inline math $a_i$ and display math:\n\n$$\\sum_i a_i$$\n\n\
         {marker}\n\n\
         {{{{kosh:image:{};width=70%;alt=Vector%20board}}}}\n\n\
         {{{{kosh:pdf:{}}}}}\n\n\
         {{{{kosh:attachment:{};caption=Vector%20scraps}}}}\n\n\
         {{{{kosh:attachment:{}}}}}",
        fixture.media.image.attachment_id,
        fixture.media.pdf.attachment_id,
        fixture.media.text.attachment_id,
        fixture.media.opaque.attachment_id,
    )
}

fn exact_result(database: &Database, query: &str) -> super::PassageSearchResult {
    let results = database
        .client()
        .search_passages(SearchPassagesInput {
            query: query.into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search deterministic baseline");
    assert_eq!(results.len(), 1, "expected one result for {query:?}");
    results.into_iter().next().expect("single result")
}

#[test]
fn redesign_baseline_fixture_reopens_with_legacy_content_media_and_provenance() {
    let fixture: Fixture = serde_json::from_str(FIXTURE_JSON).expect("parse baseline fixture");
    assert_eq!(fixture.schema_version, 1);
    let root = tempfile::tempdir().expect("temporary redesign baseline");
    let paths = DatabasePaths::new(root.path());
    let staging = root.path().join("baseline-staging");
    let database = Database::initialize(paths.clone()).expect("initialize baseline database");
    let client = database.client();

    client
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: "capture".into(),
                tidbit_id: None,
                base_revision_id: None,
                title: None,
                body_markdown: String::new(),
                sources: Vec::new(),
            },
            now_ms: 10,
            draft_id: fixture.media.capture_draft_id.clone(),
            media_limits: MediaLimits::default(),
        })
        .expect("create media capture draft");

    client
        .ingest_image(IngestImageWrite {
            attachment: stage(b"baseline original image", &staging, &fixture.media.image)
                .write(metadata(&fixture, &fixture.media.image, "image/png", 11)),
            extraction_id: fixture.media.image.extraction_id.clone(),
            preview: CanonicalImage {
                bytes: b"baseline canonical preview".to_vec(),
                natural_width: 1_200,
                natural_height: 800,
            },
        })
        .expect("ingest baseline image");
    client
        .ingest_pdf(IngestPdfWrite {
            attachment: stage(b"%PDF-1.7 baseline", &staging, &fixture.media.pdf).write(metadata(
                &fixture,
                &fixture.media.pdf,
                "application/pdf",
                12,
            )),
            extraction_id: fixture.media.pdf.extraction_id.clone(),
            page_count: 1,
        })
        .expect("ingest baseline PDF");
    client
        .ingest_generic_attachment(IngestGenericAttachmentWrite {
            attachment: stage(
                b"first line\ntext_vector_evidence\nthird line",
                &staging,
                &fixture.media.text,
            )
            .write(metadata(&fixture, &fixture.media.text, "text/markdown", 13)),
            extraction_id: fixture.media.text.extraction_id.clone(),
            extraction: Some(Ok(vec![TextFileSegment {
                start_line: 1,
                end_line: 3,
                content: "first line\ntext_vector_evidence\nthird line".into(),
            }])),
        })
        .expect("ingest baseline text attachment");
    client
        .ingest_generic_attachment(IngestGenericAttachmentWrite {
            attachment: stage(b"opaque baseline bytes", &staging, &fixture.media.opaque).write(
                metadata(
                    &fixture,
                    &fixture.media.opaque,
                    "application/octet-stream",
                    14,
                ),
            ),
            extraction_id: fixture.media.opaque.extraction_id.clone(),
            extraction: None,
        })
        .expect("ingest baseline opaque attachment");

    let initial_body = rich_body(&fixture, &fixture.active_note.initial_marker);
    client
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: "capture".into(),
                tidbit_id: None,
                base_revision_id: None,
                title: Some(fixture.active_note.title.clone()),
                body_markdown: initial_body.clone(),
                sources: Vec::new(),
            },
            now_ms: 15,
            draft_id: fixture.media.capture_draft_id.clone(),
            media_limits: MediaLimits::default(),
        })
        .expect("lease baseline media in authored body");
    let created = client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some(fixture.active_note.title.clone()),
                body_markdown: initial_body,
                sources: vec![SourceDraft {
                    label: Some(fixture.active_note.source_label.clone()),
                    url: Some(fixture.active_note.source_url.clone()),
                }],
            },
            now_ms: 16,
            tidbit_id: fixture.active_note.id.clone(),
            revision_id: fixture.active_note.initial_revision_id.clone(),
            source_ids: vec![fixture.active_note.initial_source_id.clone()],
        })
        .expect("create legacy active note");
    let historical_passage =
        exact_result(&database, &fixture.active_note.initial_marker).passage_id;

    let image_job = client
        .claim_next_image_ocr(17)
        .expect("claim baseline image OCR")
        .expect("baseline image OCR job");
    client
        .complete_image_ocr(
            image_job,
            Ok(vec![ImageOcrRegion {
                text: fixture
                    .media
                    .image
                    .ocr_marker
                    .clone()
                    .expect("image marker"),
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.25,
            }]),
            18,
        )
        .expect("complete baseline image OCR");
    let pdf_job = client
        .claim_next_pdf_extraction(19)
        .expect("claim baseline PDF extraction")
        .expect("baseline PDF extraction job");
    client
        .complete_pdf_extraction(
            pdf_job,
            Ok(vec![PdfPageExtraction {
                page_number: 1,
                result: Ok((
                    PdfPageSource::NativeText,
                    fixture.media.pdf.page_marker.clone().expect("PDF marker"),
                )),
            }]),
            20,
        )
        .expect("complete baseline PDF extraction");

    let current_body = rich_body(&fixture, &fixture.active_note.current_marker);
    let current = client
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: fixture.active_note.id.clone(),
                expected_revision_id: created.current_revision_id,
                title: Some(fixture.active_note.title.clone()),
                body_markdown: current_body.clone(),
                sources: vec![SourceDraft {
                    label: Some(fixture.active_note.source_label.clone()),
                    url: Some(fixture.active_note.source_url.clone()),
                }],
            },
            now_ms: 21,
            revision_id: fixture.active_note.current_revision_id.clone(),
            source_ids: vec![fixture.active_note.current_source_id.clone()],
        })
        .expect("create current baseline revision");

    client
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: fixture.saved_draft.context_key.clone(),
                tidbit_id: None,
                base_revision_id: None,
                title: None,
                body_markdown: fixture.saved_draft.body.clone(),
                sources: Vec::new(),
            },
            now_ms: 22,
            draft_id: fixture.saved_draft.id.clone(),
            media_limits: MediaLimits::default(),
        })
        .expect("save baseline recovery draft");
    let deleted = client
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: None,
                body_markdown: fixture.deleted_note.body.clone(),
                sources: Vec::new(),
            },
            now_ms: 23,
            tidbit_id: fixture.deleted_note.id.clone(),
            revision_id: fixture.deleted_note.revision_id.clone(),
            source_ids: Vec::new(),
        })
        .expect("create baseline deleted note");
    client
        .delete_tidbit(
            DeleteTidbitInput {
                id: deleted.id,
                expected_revision_id: deleted.current_revision_id,
            },
            24,
        )
        .expect("soft-delete baseline note");

    let initial_settings = client
        .load_shortcut_settings()
        .expect("load baseline settings");
    client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: initial_settings.revision,
            keyboard_bindings: vec![
                KeyboardBinding {
                    command: KoshCommand::QuickAdd,
                    accelerator: fixture.settings.quick_add.clone(),
                },
                KeyboardBinding {
                    command: KoshCommand::MainWindow,
                    accelerator: fixture.settings.main_window.clone(),
                },
            ],
        })
        .expect("save baseline shortcut settings");
    client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: fixture.backup.backup_set_id.clone(),
            replica_epoch_id: fixture.backup.replica_epoch_id.clone(),
            enabled: false,
            target: R2Target {
                account_id: R2AccountId::parse(&fixture.backup.account_id).expect("account ID"),
                jurisdiction: R2Jurisdiction::Default,
                bucket: R2BucketName::parse(&fixture.backup.bucket).expect("bucket name"),
            },
            now_ms: 25,
        })
        .expect("save non-secret baseline backup config");

    assert_eq!(
        client
            .resolve_citation(historical_passage.clone())
            .expect("resolve historical citation before restart")
            .state,
        CitationState::Historical
    );
    database.shutdown().expect("shutdown baseline database");

    let reopened = Database::initialize(paths).expect("reopen baseline database");
    let reopened_client = reopened.client();
    let loaded = reopened_client
        .load_tidbit(fixture.active_note.id.clone())
        .expect("load active baseline note");
    assert_eq!(loaded.current_revision_id, current.current_revision_id);
    assert_eq!(
        loaded.title.as_deref(),
        Some(fixture.active_note.title.as_str())
    );
    assert_eq!(loaded.body_markdown, current_body);
    assert_eq!(
        reopened_client
            .load_draft(fixture.saved_draft.context_key.clone())
            .expect("load baseline saved draft")
            .expect("baseline saved draft")
            .body_markdown,
        fixture.saved_draft.body
    );
    assert!(reopened
        .open_main_read_only()
        .expect("read reopened deleted note")
        .query_row(
            "SELECT deleted_at IS NOT NULL FROM tidbit WHERE id = ?1",
            [&fixture.deleted_note.id],
            |row| row.get::<_, bool>(0),
        )
        .expect("baseline deleted note state"));
    assert_eq!(
        reopened_client
            .resolve_citation(historical_passage)
            .expect("resolve historical citation after restart")
            .state,
        CitationState::Historical
    );

    let authored = exact_result(&reopened, &fixture.active_note.current_marker);
    assert_eq!(authored.citation.state, CitationState::Current);
    assert_eq!(
        authored.citation.sources[0].url.as_deref(),
        Some(fixture.active_note.source_url.as_str())
    );
    let image = exact_result(
        &reopened,
        fixture
            .media
            .image
            .ocr_marker
            .as_deref()
            .expect("image marker"),
    );
    assert!(matches!(
        image.citation.locator,
        CitationLocator::OcrRegion { .. }
    ));
    let pdf = exact_result(
        &reopened,
        fixture
            .media
            .pdf
            .page_marker
            .as_deref()
            .expect("PDF marker"),
    );
    assert_eq!(pdf.citation.locator, CitationLocator::PdfPage { page: 1 });
    let text = exact_result(
        &reopened,
        fixture
            .media
            .text
            .line_marker
            .as_deref()
            .expect("text marker"),
    );
    assert_eq!(
        text.citation.locator,
        CitationLocator::TextLines {
            start_line: 1,
            end_line: 3,
        }
    );
    assert!(reopened_client
        .search_passages(SearchPassagesInput {
            query: "opaque baseline bytes".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("opaque bytes are not indexed")
        .is_empty());
    assert_eq!(
        reopened_client
            .load_generic_attachment_status(fixture.media.opaque.attachment_id)
            .expect("load opaque attachment status")
            .extracted_line_count,
        0
    );
    let settings = reopened_client
        .load_shortcut_settings()
        .expect("load reopened settings");
    assert_eq!(
        settings.keyboard_bindings[0].accelerator,
        fixture.settings.quick_add
    );
    assert_eq!(
        settings.keyboard_bindings[1].accelerator,
        fixture.settings.main_window
    );
    let backup = reopened_client
        .load_offsite_backup_config()
        .expect("load reopened backup config")
        .expect("baseline backup config");
    assert!(!backup.enabled);
    assert_eq!(backup.backup_set_id, fixture.backup.backup_set_id);
}
