use std::{
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use rusqlite::{params, Connection, MAIN_DB};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{
    media::{
        media_blob_reclamation_preflight, recover_media_lifecycle_batch, referenced_attachments,
        validate_filename, AttachmentDisplayRole, CanonicalImage, ImageOcrRegion, ImageOcrStatus,
        IngestAttachmentMetadata, IngestAttachmentWrite, IngestGenericAttachmentWrite,
        IngestImageWrite, IngestPdfWrite, MediaBlobReclamationPreflight, MediaByteRange,
        MediaRangeRequest, StagedAttachment, TextFileSegment, MEDIA_RECONCILE_BATCH_SIZE,
    },
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    AttachmentExtractionStatus, AttachmentIngestInput, AttachmentKind, CheckpointWorkingCopyInput,
    CitationLocator, Database, DatabaseClient, DatabaseError, DatabasePaths,
    DiscardWorkingCopyInput, LexicalSearchMode, MediaLimits, MediaMaintenanceReport,
    SaveWorkingCopyInput, SearchField, SearchPassagesInput, SourceDraft, Tidbit,
};

const CAPTURE_DRAFT_ID: &str = "019f547b-6200-7000-8000-000000007001";

trait MaintainMediaForTest {
    fn maintain_media(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> super::Result<MediaMaintenanceReport>;
}

impl MaintainMediaForTest for DatabaseClient {
    fn maintain_media(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> super::Result<MediaMaintenanceReport> {
        self.maintain_media_with_safety_snapshot(now_ms, limits)
            .map(|(_, report)| report)
    }
}

#[test]
fn display_filenames_reject_paths_controls_and_bidirectional_spoofing() {
    for filename in [
        "../notes.txt",
        r"folder\notes.txt",
        "volume:notes.txt",
        "line\nbreak.txt",
        "invoice\u{202e}fdp.exe",
        "\u{2066}isolated\u{2069}.txt",
        ".",
        "..",
    ] {
        assert!(
            validate_filename(filename).is_err(),
            "unsafe filename was accepted: {filename:?}"
        );
    }
    validate_filename("考え-notes (final).pdf").expect("ordinary Unicode filename");
}

struct TestLibrary {
    _root: TempDir,
    paths: DatabasePaths,
    staging: std::path::PathBuf,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary media library");
        let paths = DatabasePaths::new(root.path());
        let staging = root.path().join("staging");
        let database = Database::initialize(paths.clone()).expect("media database");
        let library = Self {
            _root: root,
            paths,
            staging,
            database,
        };
        library.save_capture("", 10);
        library
    }

    fn save_capture(&self, body_markdown: &str, now_ms: i64) -> super::WorkingCopy {
        self.save_capture_with_sources(body_markdown, Vec::new(), now_ms)
    }

    fn save_capture_with_sources(
        &self,
        body_markdown: &str,
        sources: Vec<SourceDraft>,
        now_ms: i64,
    ) -> super::WorkingCopy {
        self.database
            .client()
            .save_working_copy_for_test(
                CAPTURE_DRAFT_ID.into(),
                None,
                now_ms,
                body_markdown.into(),
                sources,
                now_ms,
            )
            .expect("save capture working copy")
    }

    fn checkpoint_capture(
        &self,
        expected_edit_generation: i64,
        now_ms: i64,
        revision_id: String,
        source_ids: Vec<String>,
    ) -> Tidbit {
        self.database
            .client()
            .checkpoint_working_copy_for_test(
                CAPTURE_DRAFT_ID.into(),
                expected_edit_generation,
                now_ms,
                revision_id,
                source_ids,
            )
            .expect("checkpoint capture working copy")
            .note
            .expect("checkpointed note")
    }

    fn ingest(
        &self,
        suffixes: (u64, u64, u64),
        filename: &str,
        media_type: &str,
        bytes: &[u8],
        now_ms: i64,
        limits: MediaLimits,
    ) -> super::AttachmentRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(bytes),
            &self.staging,
            &id(suffixes.2),
            limits.max_attachment_bytes,
        )
        .expect("stage attachment");
        self.database
            .client()
            .ingest_attachment(staged.write(IngestAttachmentMetadata {
                attachment_id: id(suffixes.0),
                ingest_lease_id: id(suffixes.1),
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: filename.into(),
                media_type: media_type.into(),
                now_ms,
                limits,
            }))
            .expect("ingest attachment")
    }

    fn ingest_image(
        &self,
        suffixes: (u64, u64, u64, u64),
        original: &[u8],
        preview: &[u8],
        now_ms: i64,
    ) -> super::ImageRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(original),
            &self.staging,
            &id(suffixes.2),
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage image");
        self.database
            .client()
            .ingest_image(IngestImageWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(suffixes.0),
                    ingest_lease_id: id(suffixes.1),
                    draft_id: CAPTURE_DRAFT_ID.into(),
                    display_filename: "knowledge.png".into(),
                    media_type: "image/png".into(),
                    now_ms,
                    limits: MediaLimits::default(),
                }),
                extraction_id: id(suffixes.3),
                preview: CanonicalImage {
                    bytes: preview.to_vec(),
                    natural_width: 1_200,
                    natural_height: 800,
                },
            })
            .expect("ingest image")
    }

    fn ingest_pdf(
        &self,
        suffixes: (u64, u64, u64, u64),
        bytes: &[u8],
        page_count: u32,
        now_ms: i64,
    ) -> super::PdfRecord {
        self.ingest_pdf_with_limits(suffixes, bytes, page_count, now_ms, MediaLimits::default())
    }

    fn ingest_pdf_with_limits(
        &self,
        suffixes: (u64, u64, u64, u64),
        bytes: &[u8],
        page_count: u32,
        now_ms: i64,
        limits: MediaLimits,
    ) -> super::PdfRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(bytes),
            &self.staging,
            &id(suffixes.2),
            limits.max_attachment_bytes,
        )
        .expect("stage PDF");
        self.database
            .client()
            .ingest_pdf(IngestPdfWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(suffixes.0),
                    ingest_lease_id: id(suffixes.1),
                    draft_id: CAPTURE_DRAFT_ID.into(),
                    display_filename: "chapter.pdf".into(),
                    media_type: "application/pdf".into(),
                    now_ms,
                    limits,
                }),
                page_count,
            })
            .expect("ingest PDF")
    }

    fn ingest_generic(
        &self,
        suffixes: (u64, u64, u64, u64),
        filename: &str,
        media_type: &str,
        bytes: &[u8],
        extraction: Option<std::result::Result<Vec<TextFileSegment>, String>>,
        now_ms: i64,
    ) -> super::GenericAttachmentRecord {
        let staged = StagedAttachment::from_reader(
            Cursor::new(bytes),
            &self.staging,
            &id(suffixes.2),
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage generic attachment");
        self.database
            .client()
            .ingest_generic_attachment(IngestGenericAttachmentWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(suffixes.0),
                    ingest_lease_id: id(suffixes.1),
                    draft_id: CAPTURE_DRAFT_ID.into(),
                    display_filename: filename.into(),
                    media_type: media_type.into(),
                    now_ms,
                    limits: MediaLimits::default(),
                }),
                extraction_id: id(suffixes.3),
                extraction,
            })
            .expect("ingest generic attachment")
    }
}

fn id(suffix: u64) -> String {
    format!("019f547b-6200-7000-8000-{suffix:012x}")
}

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn blob_count(database: &Database) -> i64 {
    database
        .open_media_read_only()
        .expect("media reader")
        .query_row("SELECT count(*) FROM media_blob", [], |row| row.get(0))
        .expect("media blob count")
}

#[test]
fn media_tokens_require_canonical_syntax_and_preserve_authored_order() {
    let first = id(0x705);
    let second = id(0x706);
    let third = id(0x707);
    let markdown = format!(
        "{{{{kosh:attachment:{first}}}}}\n\
         {{{{kosh:image:{second};width=70%;alt=%2AArchitecture%2A%20%5Fdiagram%5F;caption=Chapter%20%7E%7E2%7E%7E}}}}\n\
         {{{{kosh:pdf:{third}}}}}\n\
         {{{{kosh:image:{first};width=100%}}}}\n\
         {{{{kosh:image:{};width=070%}}}}\n\
         {{{{kosh:image:{};width=70%;alt=%41}}}}\n\
         {{{{kosh:image:{};width=70%;alt=%ff}}}}\n\
         {{{{kosh:image:{};width=70%;alt=*raw*}}}}",
        id(0x708),
        id(0x709),
        id(0x70a),
        id(0x70b)
    );
    let references = referenced_attachments(&markdown);
    assert_eq!(references.len(), 3);
    assert_eq!(references[0].id, first);
    assert_eq!(references[0].display_role, AttachmentDisplayRole::Inline);
    assert_eq!(references[1].id, second);
    assert_eq!(references[1].display_role, AttachmentDisplayRole::Inline);
    assert_eq!(references[2].id, third);
    assert_eq!(
        references[2].display_role,
        AttachmentDisplayRole::Attachment
    );

    let unicode_caption = "%C3%A9".repeat(2_000);
    let unicode_markdown =
        format!("{{{{kosh:image:{second};width=70%;caption={unicode_caption}}}}}");
    assert_eq!(
        referenced_attachments(&unicode_markdown)
            .into_iter()
            .map(|reference| reference.id)
            .collect::<Vec<_>>(),
        [second]
    );
    let oversized_caption = format!("{unicode_caption}%C3%A9");
    assert!(referenced_attachments(&format!(
        "{{{{kosh:image:{first};width=70%;caption={oversized_caption}}}}}"
    ))
    .is_empty());
    let caption_references = referenced_attachments(&format!(
        "{{{{kosh:attachment:{third};caption=Useful%20appendix}}}}"
    ));
    assert_eq!(caption_references.len(), 1);
    assert_eq!(caption_references[0].id, third);
    assert!(referenced_attachments(&format!(
        "{{{{kosh:attachment:{third};caption=raw space}}}}"
    ))
    .is_empty());
}

#[test]
fn text_attachments_create_exact_line_evidence_with_revision_bound_sources() {
    let library = TestLibrary::new();
    let attachment = library.ingest_generic(
        (0x720, 0x721, 0x722, 0x723),
        "chapter-notes.md",
        "text/markdown",
        b"one\ntwo\nthree\nfour",
        Some(Ok(vec![
            TextFileSegment {
                start_line: 1,
                end_line: 2,
                content: "one\ntwo".into(),
            },
            TextFileSegment {
                start_line: 3,
                end_line: 4,
                content: "three exact_attachment_evidence\nfour".into(),
            },
        ])),
        11,
    );
    assert_eq!(
        attachment.extraction_status,
        AttachmentExtractionStatus::Ready
    );
    assert_eq!(attachment.extracted_line_count, 4);
    let body = format!(
        "Course notes.\n\n{{{{kosh:attachment:{};caption=Useful%20appendix}}}}",
        attachment.attachment.id
    );
    library.save_capture_with_sources(
        &body,
        vec![SourceDraft {
            label: Some("Course source".into()),
            url: Some("https://example.com/text".into()),
        }],
        12,
    );
    library.checkpoint_capture(12, 13, id(0x725), vec![id(0x726)]);

    let client = library.database.client();
    let results = client
        .search_passages(SearchPassagesInput {
            query: "exact_attachment_evidence".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search text attachment");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].citation.locator,
        CitationLocator::TextLines {
            start_line: 3,
            end_line: 4,
        }
    );
    assert_eq!(
        results[0]
            .citation
            .attachment
            .as_ref()
            .expect("attachment citation")
            .display_filename,
        "chapter-notes.md"
    );
    assert_eq!(
        results[0].citation.sources[0].url.as_deref(),
        Some("https://example.com/text")
    );
    let mime_results = client
        .search_passages(SearchPassagesInput {
            query: "text/markdown".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search text attachment MIME");
    assert!(mime_results.iter().any(|result| {
        result
            .citation
            .attachment
            .as_ref()
            .is_some_and(|candidate| candidate.id == attachment.attachment.id)
    }));
    let status = client
        .load_generic_attachment_status(attachment.attachment.id)
        .expect("text attachment status");
    assert_eq!(status.extraction_status, AttachmentExtractionStatus::Ready);
    assert_eq!(status.extracted_line_count, 4);
}

#[test]
fn opaque_and_failed_text_attachments_remain_available_without_false_evidence() {
    let library = TestLibrary::new();
    let opaque = library.ingest_generic(
        (0x727, 0x728, 0x729, 0x72a),
        "raw-shower-thought.bin",
        "application/octet-stream",
        b"\0opaque payload",
        None,
        11,
    );
    let failed = library.ingest_generic(
        (0x72b, 0x72c, 0x72d, 0x72e),
        "invalid-notes.txt",
        "text/plain",
        &[0xff, 0xfe, 0x00],
        Some(Err(
            "The UTF-16 text file has an incomplete code unit".into()
        )),
        12,
    );
    assert_eq!(
        opaque.extraction_status,
        AttachmentExtractionStatus::NotApplicable
    );
    assert_eq!(failed.extraction_status, AttachmentExtractionStatus::Failed);
    assert!(failed.extraction_error.is_some());
    let body = format!(
        "{{{{kosh:attachment:{};caption=Shower%20thought%20archive}}}}\n\n\
         {{{{kosh:attachment:{}}}}}",
        opaque.attachment.id, failed.attachment.id
    );
    library.save_capture(&body, 13);
    library.checkpoint_capture(13, 14, id(0x730), Vec::new());
    let client = library.database.client();
    assert!(client
        .search_passages(SearchPassagesInput {
            query: "opaque payload".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("opaque bytes are not indexed")
        .is_empty());
    let filename_results = client
        .search_passages(SearchPassagesInput {
            query: "raw-shower-thought.bin".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("filename metadata is searchable");
    assert_eq!(filename_results.len(), 1);
    assert_eq!(
        client
            .search_passages(SearchPassagesInput {
                query: "application/octet-stream".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("opaque MIME type is searchable")
            .len(),
        1
    );
    assert_eq!(
        client
            .search_passages(SearchPassagesInput {
                query: "Shower thought archive".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("attachment caption is searchable")
            .len(),
        1
    );
}

#[test]
fn duplicate_generic_files_share_bytes_but_keep_independent_provenance() {
    let library = TestLibrary::new();
    let bytes = b"same attachment bytes";
    let first = library.ingest_generic(
        (0x731, 0x732, 0x733, 0x734),
        "first.bin",
        "application/octet-stream",
        bytes,
        None,
        11,
    );
    let second = library.ingest_generic(
        (0x735, 0x736, 0x737, 0x738),
        "second.bin",
        "application/octet-stream",
        bytes,
        None,
        12,
    );
    assert_ne!(first.attachment.id, second.attachment.id);
    assert_eq!(blob_count(&library.database), 1);
}

#[test]
fn image_ingestion_preserves_originals_deduplicates_previews_and_serves_only_previews() {
    let library = TestLibrary::new();
    let original = b"immutable original image";
    let preview = b"bounded canonical webp";
    let first = library.ingest_image((0x708, 0x709, 0x70a, 0x70b), original, preview, 11);
    let second = library.ingest_image((0x70c, 0x70d, 0x70e, 0x70f), original, preview, 12);

    assert_ne!(first.attachment.id, second.attachment.id);
    assert_eq!(first.ocr_status, ImageOcrStatus::Pending);
    assert_eq!(blob_count(&library.database), 2);
    let payload = library
        .database
        .client()
        .load_media_payload(first.attachment.id, 13, None, 64)
        .expect("bounded image preview");
    assert_eq!(payload.bytes, preview);
    assert_eq!(payload.media_type, "image/webp");
    assert_ne!(payload.bytes, original);
    library
        .database
        .client()
        .full_integrity_check()
        .expect("image originals and previews pass integrity");
}

#[test]
fn pdf_search_indexes_only_the_attachment_filename() {
    let library = TestLibrary::new();
    let pdf = library.ingest_pdf(
        (0x900, 0x901, 0x902, 0x903),
        b"%PDF-1.7 secret page contents",
        3,
        11,
    );
    let body = format!("Chapter notes.\n\n{{{{kosh:pdf:{}}}}}", pdf.attachment.id);
    library.save_capture_with_sources(&body, Vec::new(), 12);
    library.checkpoint_capture(12, 13, id(0x905), Vec::new());

    let client = library.database.client();
    let filename_results = client
        .search_passages(SearchPassagesInput {
            query: "chapter.pdf".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search PDF filename");
    assert_eq!(filename_results.len(), 1);
    assert!(filename_results[0]
        .matched_fields
        .contains(&SearchField::AttachmentName));
    assert!(client
        .search_passages(SearchPassagesInput {
            query: "secret page contents".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("PDF bytes are not searchable")
        .is_empty());
    assert_eq!(
        client
            .load_media_payload(pdf.attachment.id, 14, None, 64)
            .expect("original PDF remains available")
            .bytes,
        b"%PDF-1.7 secret page contents"
    );
}

#[test]
fn image_ingestion_rolls_back_base_attachment_when_image_setup_fails() {
    let library = TestLibrary::new();
    library.ingest_image(
        (0x7a4, 0x7a5, 0x7a6, 0x7a7),
        b"existing original image",
        b"existing canonical preview",
        11,
    );
    let failed_attachment_id = id(0x7a8);
    let failed_lease_id = id(0x7a9);
    let staged = StagedAttachment::from_reader(
        Cursor::new(b"uncommitted original image"),
        &library.staging,
        &id(0x7aa),
        MediaLimits::default().max_attachment_bytes,
    )
    .expect("stage failing image");
    let error = library
        .database
        .client()
        .ingest_image(IngestImageWrite {
            attachment: staged.write(IngestAttachmentMetadata {
                attachment_id: failed_attachment_id.clone(),
                ingest_lease_id: failed_lease_id.clone(),
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "uncommitted.png".into(),
                media_type: "image/png".into(),
                now_ms: 12,
                limits: MediaLimits::default(),
            }),
            extraction_id: id(0x7a7),
            preview: CanonicalImage {
                bytes: b"uncommitted canonical preview".to_vec(),
                natural_width: 640,
                natural_height: 480,
            },
        })
        .expect_err("duplicate OCR identity rejects image setup");
    assert!(matches!(error, DatabaseError::Sqlite(_)));

    let main = library.database.open_main_read_only().expect("main reader");
    let leaked: bool = main
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM attachment
                WHERE id = ?1
                UNION ALL
                SELECT 1
                FROM media_ingest_lease
                WHERE id = ?2
                UNION ALL
                SELECT 1
                FROM draft_media_lease
                WHERE media_ingest_lease_id = ?2
             )",
            params![failed_attachment_id, failed_lease_id],
            |row| row.get(0),
        )
        .expect("failed image residue");
    assert!(
        !leaked,
        "failed image setup must not consume draft capacity"
    );
}

#[test]
fn image_ocr_creates_searchable_region_citations_without_mutating_authored_revision() {
    let library = TestLibrary::new();
    let image = library.ingest_image(
        (0x780, 0x781, 0x782, 0x783),
        b"original image evidence",
        b"canonical searchable preview",
        11,
    );
    let body = format!(
        "A photographed whiteboard.\n\n\
         {{{{kosh:image:{};width=70%;alt=Architecture%20diagram;caption=Searchable%20evidence}}}}",
        image.attachment.id
    );
    library.save_capture(&body, 12);
    let tidbit = library.checkpoint_capture(12, 13, id(0x785), Vec::new());
    let original_revision_id = tidbit.current_revision_id.clone();

    let client = library.database.client();
    let job = client
        .claim_next_image_ocr(14)
        .expect("claim OCR")
        .expect("queued OCR job");
    assert_eq!(job.preview_bytes, b"canonical searchable preview");
    client
        .complete_image_ocr(
            job,
            Ok(vec![ImageOcrRegion {
                text: "Event sourcing preserves exact image evidence".into(),
                x: 0.125,
                y: 0.25,
                width: 0.5,
                height: 0.375,
            }]),
            15,
        )
        .expect("complete OCR");

    let loaded = client
        .load_tidbit(tidbit.id.clone())
        .expect("load unchanged tidbit");
    assert_eq!(loaded.current_revision_id, original_revision_id);
    let results = client
        .search_passages(SearchPassagesInput {
            query: "exact image evidence".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search OCR text");
    assert_eq!(results.len(), 1);
    let citation = &results[0].citation;
    let attachment = citation.attachment.as_ref().expect("attachment citation");
    assert_eq!(attachment.id, image.attachment.id);
    assert_eq!(attachment.extraction_id, id(0x783));
    assert!(citation.tidbit.is_none());
    match &citation.locator {
        CitationLocator::OcrRegion { page, region } => {
            assert_eq!(*page, None);
            assert_eq!(region["coordinateSystem"], "vision-normalized-bottom-left");
            assert_eq!(region["x"], 0.125);
            assert_eq!(region["y"], 0.25);
            assert_eq!(region["width"], 0.5);
            assert_eq!(region["height"], 0.375);
        }
        locator => panic!("expected an OCR region locator, got {locator:?}"),
    }
    assert_eq!(
        client
            .load_image_status(image.attachment.id)
            .expect("ready image status")
            .ocr_status,
        ImageOcrStatus::Ready
    );
}

#[test]
fn image_ocr_quarantines_a_corrupt_preview_and_claims_the_next_job() {
    let library = TestLibrary::new();
    let corrupt_preview = b"corrupt OCR preview";
    let corrupt = library.ingest_image(
        (0x79c, 0x79d, 0x79e, 0x79f),
        b"first original image",
        corrupt_preview,
        11,
    );
    let healthy_preview = b"healthy OCR preview";
    let healthy = library.ingest_image(
        (0x7a0, 0x7a1, 0x7a2, 0x7a3),
        b"second original image",
        healthy_preview,
        12,
    );

    let media = Connection::open(&library.paths.media).expect("media writer");
    let corrupt_preview_hash = digest(corrupt_preview);
    let corrupt_rowid = media
        .query_row(
            "SELECT rowid FROM media_blob WHERE sha256 = ?1",
            params![&corrupt_preview_hash],
            |row| row.get::<_, i64>(0),
        )
        .expect("corrupt preview rowid");
    let mut blob = media
        .blob_open(MAIN_DB, "media_blob", "bytes", corrupt_rowid, false)
        .expect("writable preview blob");
    blob.seek(SeekFrom::Start(0)).expect("seek preview blob");
    blob.write_all(b"X").expect("corrupt preview blob");
    drop(blob);
    drop(media);

    let client = library.database.client();
    let job = client
        .claim_next_image_ocr(13)
        .expect("claim past corrupt preview")
        .expect("healthy OCR job");
    assert_eq!(job.attachment_id, healthy.attachment.id);
    assert_eq!(job.preview_bytes, healthy_preview);
    let failed = client
        .load_image_status(corrupt.attachment.id)
        .expect("corrupt image status");
    assert_eq!(failed.ocr_status, ImageOcrStatus::Failed);
    assert!(failed
        .ocr_error
        .as_deref()
        .is_some_and(|error| error.contains("preview is corrupt")));

    client
        .complete_image_ocr(job, Ok(Vec::new()), 14)
        .expect("complete healthy OCR");
    let diagnostics = client.image_ocr_diagnostics().expect("OCR diagnostics");
    assert_eq!(diagnostics.failed, 1);
    assert_eq!(diagnostics.running, 0);
    assert!(client
        .claim_next_image_ocr(15)
        .expect("inspect drained queue")
        .is_none());
}

#[test]
fn draft_only_image_ocr_never_enters_search_and_remains_hidden_after_discard() {
    let library = TestLibrary::new();
    let image = library.ingest_image(
        (0x792, 0x793, 0x794, 0x795),
        b"draft-only original image",
        b"draft-only preview",
        11,
    );
    let body = format!(
        "{{{{kosh:image:{};width=100%;caption=Temporary}}}}",
        image.attachment.id
    );
    let draft = library.save_capture(&body, 12);
    let client = library.database.client();
    let job = client
        .claim_next_image_ocr(13)
        .expect("claim draft OCR")
        .expect("draft OCR job");
    client
        .complete_image_ocr(
            job,
            Ok(vec![ImageOcrRegion {
                text: "draft_only_evidence must stay private".into(),
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.25,
            }]),
            14,
        )
        .expect("complete draft OCR");

    let search = || {
        client
            .search_passages(SearchPassagesInput {
                query: "draft_only_evidence".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("search draft-only OCR")
    };
    assert!(search().is_empty());
    assert!(client
        .discard_working_copy(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: draft.edit_generation,
            },
            15,
        )
        .expect("discard draft-only image"));
    assert!(search().is_empty());
}

#[test]
fn retiring_a_draft_only_image_terminally_fails_in_flight_ocr() {
    let library = TestLibrary::new();
    let image = library.ingest_image(
        (0x7a8, 0x7a9, 0x7aa, 0x7ab),
        b"retired original image",
        b"retired preview",
        11,
    );
    let client = library.database.client();
    let job = client
        .claim_next_image_ocr(12)
        .expect("claim draft-only OCR")
        .expect("draft-only OCR job");
    assert_eq!(job.attachment_id, image.attachment.id);
    assert!(client
        .discard_working_copy(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: 10,
            },
            13,
        )
        .expect("discard blank draft"));

    let maintenance = client
        .maintain_media(13, MediaLimits::default())
        .expect("retire draft-only image");
    assert_eq!(maintenance.cleanup.retired_attachment_count, 1);
    let diagnostics = client.image_ocr_diagnostics().expect("OCR diagnostics");
    assert_eq!(diagnostics.running, 0);
    assert_eq!(diagnostics.failed, 1);
    let main = library.database.open_main_read_only().expect("main reader");
    let (queue_state, extraction_status, last_error): (String, String, String) = main
        .query_row(
            "SELECT queue.state, extraction.status, queue.last_error
             FROM image_ocr_queue AS queue
             JOIN attachment_extraction AS extraction
               ON extraction.id = queue.extraction_id
             WHERE queue.extraction_id = ?1",
            params![job.extraction_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("retired OCR state");
    assert_eq!(queue_state, "FAILED");
    assert_eq!(extraction_status, "FAILED");
    assert_eq!(last_error, "Image was retired before OCR completed");

    client
        .complete_image_ocr(job, Ok(Vec::new()), 14)
        .expect("late completion is ignored");
    assert!(client
        .claim_next_image_ocr(14)
        .expect("inspect drained OCR queue")
        .is_none());
}

#[test]
fn completed_draft_image_ocr_is_indexed_when_a_revision_takes_ownership() {
    let library = TestLibrary::new();
    let image = library.ingest_image(
        (0x796, 0x797, 0x798, 0x799),
        b"pre-save original image",
        b"pre-save preview",
        11,
    );
    let body = format!(
        "{{{{kosh:image:{};width=100%;caption=Promoted}}}}",
        image.attachment.id
    );
    library.save_capture(&body, 12);
    let client = library.database.client();
    let job = client
        .claim_next_image_ocr(13)
        .expect("claim pre-save OCR")
        .expect("pre-save OCR job");
    client
        .complete_image_ocr(
            job,
            Ok(vec![ImageOcrRegion {
                text: "promoted_image_evidence becomes durable".into(),
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.25,
            }]),
            14,
        )
        .expect("complete pre-save OCR");
    assert!(client
        .search_passages(SearchPassagesInput {
            query: "promoted_image_evidence".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search pre-save OCR")
        .is_empty());

    library.checkpoint_capture(12, 15, id(0x79b), Vec::new());
    assert_eq!(
        client
            .search_passages(SearchPassagesInput {
                query: "promoted_image_evidence".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("search promoted OCR")
            .len(),
        1
    );
}

#[test]
fn image_ocr_recovers_interrupted_work_and_bounds_terminal_retries() {
    let library = TestLibrary::new();
    let image = library.ingest_image(
        (0x786, 0x787, 0x788, 0x789),
        b"retry original image",
        b"retry canonical preview",
        11,
    );
    let client = library.database.client();
    let interrupted = client
        .claim_next_image_ocr(12)
        .expect("initial OCR claim")
        .expect("initial OCR job");
    assert_eq!(interrupted.attempt_count, 1);
    let recovery = client
        .recover_interrupted_image_ocr(12, 13)
        .expect("recover interrupted OCR");
    assert_eq!(recovery.requeued, 1);
    assert_eq!(recovery.terminally_failed, 0);

    for (expected_attempt, claim_at) in [(2, 13), (3, 2_000_000), (4, 10_000_000)] {
        let job = client
            .claim_next_image_ocr(claim_at)
            .expect("retry OCR claim")
            .expect("eligible retry job");
        assert_eq!(job.attempt_count, expected_attempt);
        let failure = if expected_attempt == 2 {
            Ok(vec![ImageOcrRegion {
                text: "invalid geometry".into(),
                x: 2.0,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            }])
        } else {
            Err("recognizer failed ".repeat(2_000))
        };
        client
            .complete_image_ocr(job, failure, claim_at + 1)
            .expect("record bounded OCR failure");
    }

    let failed = client
        .load_image_status(image.attachment.id.clone())
        .expect("failed image status");
    assert_eq!(failed.ocr_status, ImageOcrStatus::Failed);
    assert!(
        failed.ocr_error.as_deref().unwrap_or_default().len() <= 2_048,
        "OCR errors must remain bounded"
    );
    let diagnostics = client.image_ocr_diagnostics().expect("OCR diagnostics");
    assert_eq!(diagnostics.failed, 1);
    assert_eq!(diagnostics.running, 0);
    assert!(diagnostics.last_error.is_some());

    let retried = client
        .retry_image_ocr(image.attachment.id, 10_000_002)
        .expect("manual retry");
    assert_eq!(retried.ocr_status, ImageOcrStatus::Pending);
    assert_eq!(
        client
            .claim_next_image_ocr(10_000_002)
            .expect("claim manual retry")
            .expect("manual retry job")
            .attempt_count,
        1
    );
}

#[test]
fn image_ocr_recovery_replaces_stale_extractor_provenance() {
    let library = TestLibrary::new();
    let cases = [
        ("PENDING", (0x78a, 0x78b, 0x78c, 0x78d)),
        ("RUNNING", (0x7ab, 0x7ac, 0x7ad, 0x7ae)),
        ("RETRY_WAIT", (0x7af, 0x7b0, 0x7b1, 0x7b2)),
        ("READY", (0x7b3, 0x7b4, 0x7b5, 0x7b6)),
        ("FAILED", (0x7b7, 0x7b8, 0x7b9, 0x7ba)),
    ];
    let images = cases
        .iter()
        .enumerate()
        .map(|(index, (state, suffixes))| {
            (
                *state,
                library.ingest_image(
                    *suffixes,
                    format!("{state} original image").as_bytes(),
                    format!("{state} canonical preview").as_bytes(),
                    11 + i64::try_from(index).expect("case index fits i64"),
                ),
            )
        })
        .collect::<Vec<_>>();
    let mut writer = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("open configured extractor writer");
    let transaction = writer.transaction().expect("state setup transaction");
    for (state, image) in &images {
        let extraction_id = transaction
            .query_row(
                "SELECT id
                 FROM attachment_extraction
                 WHERE attachment_id = ?1 AND extractor = 'ocr'",
                params![image.attachment.id],
                |row| row.get::<_, String>(0),
            )
            .expect("image extraction id");
        match *state {
            "PENDING" => {}
            "RUNNING" => {
                transaction
                    .execute(
                        "UPDATE attachment_extraction
                         SET status = 'RUNNING', started_at = 20
                         WHERE id = ?1",
                        params![&extraction_id],
                    )
                    .expect("running extraction");
                transaction
                    .execute(
                        "UPDATE image_ocr_queue
                         SET state = 'RUNNING', attempt_count = 1,
                             next_attempt_at = NULL, started_at = 20, updated_at = 20
                         WHERE extraction_id = ?1",
                        params![&extraction_id],
                    )
                    .expect("running queue");
            }
            "RETRY_WAIT" => {
                transaction
                    .execute(
                        "UPDATE attachment_extraction
                         SET error = 'retry later'
                         WHERE id = ?1",
                        params![&extraction_id],
                    )
                    .expect("retry extraction");
                transaction
                    .execute(
                        "UPDATE image_ocr_queue
                         SET state = 'RETRY_WAIT', attempt_count = 1,
                             next_attempt_at = 100, last_error = 'retry later', updated_at = 20
                         WHERE extraction_id = ?1",
                        params![&extraction_id],
                    )
                    .expect("retry queue");
            }
            "READY" => {
                transaction
                    .execute(
                        "UPDATE attachment_extraction
                         SET status = 'READY', completed_at = 20
                         WHERE id = ?1",
                        params![&extraction_id],
                    )
                    .expect("ready extraction");
                transaction
                    .execute(
                        "UPDATE image_ocr_queue
                         SET state = 'READY', attempt_count = 1,
                             next_attempt_at = NULL, updated_at = 20
                         WHERE extraction_id = ?1",
                        params![&extraction_id],
                    )
                    .expect("ready queue");
            }
            "FAILED" => {
                transaction
                    .execute(
                        "UPDATE attachment_extraction
                         SET status = 'FAILED', error = 'terminal', completed_at = 20
                         WHERE id = ?1",
                        params![&extraction_id],
                    )
                    .expect("failed extraction");
                transaction
                    .execute(
                        "UPDATE image_ocr_queue
                         SET state = 'FAILED', attempt_count = 1,
                             next_attempt_at = NULL, last_error = 'terminal', updated_at = 20
                         WHERE extraction_id = ?1",
                        params![&extraction_id],
                    )
                    .expect("failed queue");
            }
            state => panic!("unexpected OCR state {state}"),
        }
    }
    transaction
        .execute(
            "UPDATE attachment_extractor_config
             SET version = '2', updated_at = 13
             WHERE extractor = 'ocr'",
            [],
        )
        .expect("advance OCR extractor version");
    transaction.commit().expect("commit stale OCR states");
    drop(writer);

    let client = library.database.client();
    let recovery = client
        .recover_interrupted_image_ocr(20, 30)
        .expect("replace stale OCR provenance");

    assert_eq!(recovery.requeued, 5);
    assert_eq!(recovery.terminally_failed, 5);
    assert_eq!(
        client
            .recover_interrupted_image_ocr(20, 31)
            .expect("repeat stale OCR reconciliation"),
        super::media::ImageOcrRecovery::default()
    );
    let main = library.database.open_main_read_only().expect("main reader");
    for (original_state, image) in &images {
        let (queue_state, extraction_status) = main
            .query_row(
                "SELECT queue.state, extraction.status
                 FROM image_ocr_queue AS queue
                 JOIN attachment_extraction AS extraction ON extraction.id = queue.extraction_id
                 WHERE extraction.attachment_id = ?1
                   AND extraction.extractor_version = '1'",
                params![image.attachment.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("stale OCR state");
        assert_eq!(queue_state, "FAILED");
        assert_eq!(
            extraction_status,
            if *original_state == "READY" {
                "READY"
            } else {
                "FAILED"
            }
        );
    }
    drop(main);
    let mut replacement_attachment_ids = Vec::new();
    while let Some(replacement) = client
        .claim_next_image_ocr(31)
        .expect("claim replacement OCR")
    {
        assert_eq!(replacement.extractor_version, "2");
        assert_eq!(replacement.attempt_count, 1);
        replacement_attachment_ids.push(replacement.attachment_id);
    }
    replacement_attachment_ids.sort();
    let mut expected_attachment_ids = images
        .iter()
        .map(|(_, image)| image.attachment.id.clone())
        .collect::<Vec<_>>();
    expected_attachment_ids.sort();
    assert_eq!(replacement_attachment_ids, expected_attachment_ids);
}

#[test]
fn image_ocr_recovery_batches_large_stale_backlogs() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        max_attachments_per_draft: 128,
        ..MediaLimits::default()
    };
    let client = library.database.client();
    for index in 0_u64..65 {
        let suffix = 0xa00 + index * 4;
        let staged = StagedAttachment::from_reader(
            Cursor::new(b"shared recovery original"),
            &library.staging,
            &id(suffix + 2),
            limits.max_attachment_bytes,
        )
        .expect("stage recovery image");
        client
            .ingest_image(IngestImageWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(suffix),
                    ingest_lease_id: id(suffix + 1),
                    draft_id: CAPTURE_DRAFT_ID.into(),
                    display_filename: "recovery.png".into(),
                    media_type: "image/png".into(),
                    now_ms: 11 + i64::try_from(index).expect("image index fits i64"),
                    limits,
                }),
                extraction_id: id(suffix + 3),
                preview: CanonicalImage {
                    bytes: b"shared recovery preview".to_vec(),
                    natural_width: 640,
                    natural_height: 480,
                },
            })
            .expect("ingest recovery image");
    }
    super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("open extractor writer")
    .execute(
        "UPDATE attachment_extractor_config
         SET version = '2', updated_at = 100
         WHERE extractor = 'ocr'",
        [],
    )
    .expect("advance extractor version");

    let first = client
        .recover_interrupted_image_ocr(100, 100)
        .expect("first recovery batch");
    assert_eq!(first.requeued, 64);
    assert_eq!(first.terminally_failed, 64);
    assert!(first.remaining);
    let second = client
        .recover_interrupted_image_ocr(100, 101)
        .expect("second recovery batch");
    assert_eq!(second.requeued, 1);
    assert_eq!(second.terminally_failed, 1);
    assert!(!second.remaining);
}

#[test]
fn ingestion_deduplicates_bytes_preserves_metadata_and_bounds_reads() {
    let library = TestLibrary::new();
    let first = library.ingest(
        (0x710, 0x711, 0x712),
        "thought.txt",
        "text/plain",
        b"same bytes",
        11,
        MediaLimits::default(),
    );
    let second = library.ingest(
        (0x713, 0x714, 0x715),
        "chapter.txt",
        "text/plain",
        b"same bytes",
        12,
        MediaLimits::default(),
    );

    assert_ne!(first.id, second.id);
    assert_eq!(first.display_filename, "thought.txt");
    assert_eq!(second.display_filename, "chapter.txt");
    assert_eq!(blob_count(&library.database), 1);
    let main = library.database.open_main_read_only().expect("main reader");
    assert_eq!(
        main.query_row(
            "SELECT count(DISTINCT sha256) FROM attachment WHERE id IN (?1, ?2)",
            params![&first.id, &second.id],
            |row| row.get::<_, i64>(0),
        )
        .expect("deduplicated digest"),
        1
    );

    let client = library.database.client();
    let payload = client
        .load_media_payload(
            first.id.clone(),
            13,
            Some(MediaRangeRequest::Inclusive(MediaByteRange {
                start: 5,
                end_inclusive: 9,
            })),
            5,
        )
        .expect("authorized bounded read");
    assert_eq!(payload.bytes, b"bytes");
    assert_eq!(payload.total_byte_length, 10);
    assert_eq!(payload.media_type, "text/plain");
    assert!(!payload.revision_bound);

    let clamped = client
        .load_media_payload(
            first.id.clone(),
            13,
            Some(MediaRangeRequest::Inclusive(MediaByteRange {
                start: 5,
                end_inclusive: 65_535,
            })),
            5,
        )
        .expect("authorized range clamped to EOF");
    assert_eq!(clamped.bytes, b"bytes");
    assert_eq!(clamped.range.start, 5);
    assert_eq!(clamped.range.end_inclusive, 9);

    let from = client
        .load_media_payload(first.id.clone(), 13, Some(MediaRangeRequest::From(5)), 5)
        .expect("authorized open-ended read");
    assert_eq!(from.bytes, b"bytes");
    let suffix = client
        .load_media_payload(first.id.clone(), 13, Some(MediaRangeRequest::Suffix(5)), 5)
        .expect("authorized suffix read");
    assert_eq!(suffix.bytes, b"bytes");

    assert!(matches!(
        client.load_media_payload(first.id.clone(), 13, None, 9),
        Err(DatabaseError::InvalidInput(_))
    ));
    assert!(matches!(
        client.load_media_payload(id(0x799), 13, None, 64),
        Err(DatabaseError::NotFound {
            entity: "attachment",
            ..
        })
    ));

    let one_attachment = MediaLimits {
        max_attachments_per_draft: 2,
        ..MediaLimits::default()
    };
    let staged = StagedAttachment::from_reader(
        Cursor::new(b"third bytes"),
        &library.staging,
        &id(0x718),
        one_attachment.max_attachment_bytes,
    )
    .expect("stage capacity probe");
    let error = library
        .database
        .client()
        .ingest_attachment(staged.write(IngestAttachmentMetadata {
            attachment_id: id(0x716),
            ingest_lease_id: id(0x717),
            draft_id: CAPTURE_DRAFT_ID.into(),
            display_filename: "third.txt".into(),
            media_type: "text/plain".into(),
            now_ms: 14,
            limits: one_attachment,
        }))
        .expect_err("draft attachment cap");
    assert!(matches!(error, DatabaseError::InvalidInput(_)));
    assert_eq!(blob_count(&library.database), 1);
}

#[test]
fn ingestion_canonicalizes_mime_case_before_classification() {
    let library = TestLibrary::new();
    let attachment = library
        .database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "mixed.png".into(),
                media_type: "IMAGE/PNG".into(),
                now_ms: 11,
                limits: MediaLimits::default(),
            },
            Cursor::new(b"image bytes"),
        )
        .expect("bounded reader ingestion");

    assert_eq!(attachment.kind, AttachmentKind::Image);
    assert_eq!(attachment.media_type, "image/png");
    assert_eq!(
        library
            .database
            .open_main_read_only()
            .expect("main reader")
            .query_row(
                "SELECT media_type, kind, extraction_state
                 FROM attachment
                 WHERE id = ?1",
                params![attachment.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("canonical attachment classification"),
        ("image/png".into(), "IMAGE".into(), "PENDING".into())
    );
}

struct UnexpectedRead;

impl Read for UnexpectedRead {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        panic!("invalid limits must be rejected before reading attachment bytes");
    }
}

#[test]
fn public_ingestion_validates_limits_before_staging() {
    let library = TestLibrary::new();
    assert_eq!(
        MediaLimits::default().max_attachment_bytes,
        MediaLimits::default().max_protocol_response_bytes
    );
    let invalid_limits = MediaLimits {
        max_attachment_bytes: u64::MAX,
        ..MediaLimits::default()
    };

    let error = library
        .database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "unbounded.bin".into(),
                media_type: "application/octet-stream".into(),
                now_ms: 11,
                limits: invalid_limits,
            },
            UnexpectedRead,
        )
        .expect_err("reject invalid limits before staging");

    assert!(matches!(error, DatabaseError::InvalidInput(_)));
    assert!(!library.paths.root().join("media-staging").exists());

    let mismatched_limits = MediaLimits {
        max_attachment_bytes: 16,
        max_protocol_response_bytes: 8,
        ..MediaLimits::default()
    };
    assert!(matches!(
        library.database.ingest_attachment(
            AttachmentIngestInput {
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "mismatch.bin".into(),
                media_type: "application/octet-stream".into(),
                now_ms: 11,
                limits: mismatched_limits,
            },
            UnexpectedRead,
        ),
        Err(DatabaseError::InvalidInput(_))
    ));
}

struct GatedReader {
    bytes: Cursor<Vec<u8>>,
    read_started: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
}

impl Read for GatedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(read_started) = self.read_started.take() {
            let _ = read_started.send(());
            if let Some(release) = self.release.take() {
                release.recv().map_err(|_| {
                    io::Error::new(io::ErrorKind::Interrupted, "staging release was dropped")
                })?;
            }
        }
        self.bytes.read(buffer)
    }
}

#[test]
fn concurrent_ingestion_serializes_before_reading_attachment_bytes() {
    let root = tempfile::tempdir().expect("concurrent staging root");
    let database = Arc::new(
        Database::initialize(DatabasePaths::new(root.path())).expect("concurrent staging database"),
    );
    database
        .client()
        .save_working_copy_for_test(
            CAPTURE_DRAFT_ID.into(),
            None,
            1,
            String::new(),
            Vec::new(),
            10,
        )
        .expect("concurrent staging working copy");

    let (first_started_tx, first_started_rx) = mpsc::channel();
    let (first_release_tx, first_release_rx) = mpsc::channel();
    let first_database = Arc::clone(&database);
    let first = thread::spawn(move || {
        first_database.ingest_attachment(
            AttachmentIngestInput {
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "first.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 11,
                limits: MediaLimits {
                    max_attachments_per_draft: 2,
                    ..MediaLimits::default()
                },
            },
            GatedReader {
                bytes: Cursor::new(b"first".to_vec()),
                read_started: Some(first_started_tx),
                release: Some(first_release_rx),
            },
        )
    });
    first_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first staging read started");

    let (second_started_tx, second_started_rx) = mpsc::channel();
    let second_database = Arc::clone(&database);
    let second = thread::spawn(move || {
        second_database.ingest_attachment(
            AttachmentIngestInput {
                draft_id: CAPTURE_DRAFT_ID.into(),
                display_filename: "second.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 12,
                limits: MediaLimits {
                    max_attachments_per_draft: 2,
                    ..MediaLimits::default()
                },
            },
            GatedReader {
                bytes: Cursor::new(b"second".to_vec()),
                read_started: Some(second_started_tx),
                release: None,
            },
        )
    });
    assert!(matches!(
        second_started_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    first_release_tx
        .send(())
        .expect("release first staging read");
    first
        .join()
        .expect("first ingestion thread")
        .expect("first ingestion");
    second_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second staging read started after first completed");
    second
        .join()
        .expect("second ingestion thread")
        .expect("second ingestion");
}

struct InterruptedReader {
    emitted: bool,
}

impl Read for InterruptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.emitted {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "simulated interrupted upload",
            ));
        }
        self.emitted = true;
        buffer[..4].copy_from_slice(b"part");
        Ok(4)
    }
}

#[test]
fn interrupted_and_oversized_staging_remove_partial_files() {
    let root = tempfile::tempdir().expect("staging root");
    let interrupted = StagedAttachment::from_reader(
        InterruptedReader { emitted: false },
        root.path(),
        &id(0x720),
        64,
    )
    .expect_err("interrupted staging");
    assert!(matches!(interrupted, DatabaseError::Io(_)));
    assert_directory_empty(root.path());

    let oversized =
        StagedAttachment::from_reader(Cursor::new(b"too large"), root.path(), &id(0x721), 3)
            .expect_err("oversized staging");
    assert!(matches!(oversized, DatabaseError::InvalidInput(_)));
    assert_directory_empty(root.path());
}

fn assert_directory_empty(path: &Path) {
    let mut entries = std::fs::read_dir(path).expect("read staging directory");
    assert!(entries.next().is_none(), "staging directory was not empty");
}

#[test]
fn removed_draft_media_remains_authorized_for_undo_until_expiry() {
    let library = TestLibrary::new();
    let attachment = library.ingest(
        (0x728, 0x729, 0x72a),
        "undo.png",
        "image/png",
        b"undo image",
        11,
        MediaLimits::default(),
    );
    let body = format!("{{{{kosh:image:{};width=70%}}}}", attachment.id);
    library.save_capture(&body, 12);
    library.save_capture("", 13);

    let main = library.database.open_main_read_only().expect("main reader");
    assert_eq!(
        main.query_row(
            "SELECT lease.state
             FROM draft_media_lease AS draft_lease
             JOIN media_ingest_lease AS lease
               ON lease.id = draft_lease.media_ingest_lease_id
             WHERE draft_lease.draft_id = ?1 AND lease.attachment_id = ?2",
            params![CAPTURE_DRAFT_ID, attachment.id],
            |row| row.get::<_, String>(0),
        )
        .expect("retained draft media capability"),
        "COMMITTED"
    );
    drop(main);

    let restored = library.save_capture(&body, 14);
    assert_eq!(restored.body_markdown, body);
}

#[test]
fn owned_draft_media_remains_readable_and_renews_after_expiry_without_restart() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 10,
        ..MediaLimits::default()
    };
    let attachment = library.ingest(
        (0x72e, 0x72f, 0x730),
        "sleep.png",
        "image/png",
        b"sleep image",
        11,
        limits,
    );
    let body = format!("{{{{kosh:image:{};width=90%}}}}", attachment.id);
    let save = |now_ms| {
        library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: CAPTURE_DRAFT_ID.into(),
                    base_revision_id: None,
                    edit_generation: now_ms,
                    document_json: super::document::fixture_from_markdown(&body),
                    body_markdown: body.clone(),
                    sources: Vec::new(),
                },
                now_ms,
                media_limits: limits,
                allow_empty_ephemeral: true,
            })
    };
    save(12).expect("save short-lived media draft");

    let integrity = library
        .database
        .client()
        .media_integrity_report(23)
        .expect("expired saved draft integrity report");
    assert!(integrity.orphaned_attachment_ids.is_empty());
    assert_eq!(
        library
            .database
            .client()
            .load_media_payload(attachment.id.clone(), 23, None, 64)
            .expect("persisted draft authorizes expired media")
            .bytes,
        b"sleep image"
    );
    save(23).expect("autosave renews owned expired media");
    assert_eq!(
        library
            .database
            .open_main_read_only()
            .expect("main reader")
            .query_row(
                "SELECT expires_at
                 FROM media_ingest_lease
                 WHERE attachment_id = ?1",
                params![attachment.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("renewed media expiry"),
        33
    );
}

#[test]
fn attachment_ownership_follows_the_stable_note_block_identity() {
    let library = TestLibrary::new();
    let attachment = library.ingest(
        (0x752, 0x753, 0x754),
        "owned.png",
        "image/png",
        b"owned image",
        11,
        MediaLimits::default(),
    );
    let body = format!("{{{{kosh:image:{};width=90%}}}}", attachment.id);
    let document = |block_id: &str, paragraph_first: bool| {
        let media = serde_json::json!({
            "id": block_id,
            "type": "koshImage",
            "props": {"attachmentId": attachment.id},
            "content": [],
            "children": [],
        });
        let paragraph = serde_json::json!({
            "id": "ownership-paragraph",
            "type": "paragraph",
            "props": {},
            "content": [],
            "children": [],
        });
        let blocks = if paragraph_first {
            vec![paragraph, media]
        } else {
            vec![media, paragraph]
        };
        serde_json::json!({"schemaVersion": 1, "blocks": blocks}).to_string()
    };
    let save = |generation, document_json| {
        library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: CAPTURE_DRAFT_ID.into(),
                    base_revision_id: None,
                    edit_generation: generation,
                    document_json,
                    body_markdown: body.clone(),
                    sources: Vec::new(),
                },
                now_ms: generation,
                media_limits: MediaLimits::default(),
                allow_empty_ephemeral: true,
            })
    };

    save(12, document("owned-image-block", false)).expect("claim attachment for note block");
    let owner = library
        .database
        .open_main_read_only()
        .expect("main reader")
        .query_row(
            "SELECT owner_note_id, owner_block_id FROM attachment WHERE id = ?1",
            params![&attachment.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("attachment owner");
    assert_eq!(owner, (CAPTURE_DRAFT_ID.into(), "owned-image-block".into()));

    save(13, document("owned-image-block", true))
        .expect("moving the stable block keeps attachment ownership");
    let error = save(14, document("copied-image-block", true))
        .expect_err("copying the attachment into a new block must fail");
    assert!(error
        .to_string()
        .contains("already belongs to a different note block"));
}

#[test]
fn working_copy_media_and_note_survive_restart() {
    let library = TestLibrary::new();
    let note_id = id(0x731);
    let initial = library
        .database
        .client()
        .save_working_copy_for_test(note_id.clone(), None, 1, String::new(), Vec::new(), 11)
        .expect("create working copy");
    assert_eq!(initial.id, note_id);
    let attachment = library
        .database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id: note_id.clone(),
                display_filename: "quick-note.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 12,
                limits: MediaLimits::default(),
            },
            Cursor::new(b"working-copy attachment"),
        )
        .expect("ingest working-copy attachment");
    let body = format!("{{{{kosh:attachment:{}}}}}", attachment.id);
    let saved = library
        .database
        .client()
        .save_working_copy_for_test(
            note_id.clone(),
            None,
            2,
            body.clone(),
            vec![SourceDraft {
                label: Some("Recovered source".into()),
                url: Some("https://example.com/working-copy".into()),
            }],
            13,
        )
        .expect("save media working copy");
    assert_eq!(
        library
            .database
            .open_main_read_only()
            .expect("main reader")
            .query_row(
                "SELECT count(*) FROM draft_media_lease WHERE draft_id = ?1",
                params![note_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("working-copy lease count"),
        1
    );

    library.database.shutdown().expect("clean shutdown");
    let reopened = Database::initialize(library.paths.clone()).expect("reopened database");
    assert_eq!(
        reopened
            .client()
            .load_working_copy(note_id.clone())
            .expect("recovered working copy"),
        Some(saved)
    );
    assert_eq!(
        reopened
            .client()
            .load_media_payload(attachment.id.clone(), 14, None, 64)
            .expect("recovered attachment")
            .bytes,
        b"working-copy attachment"
    );

    let checkpoint = reopened
        .client()
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: CheckpointWorkingCopyInput {
                note_id: note_id.clone(),
                expected_edit_generation: 2,
            },
            now_ms: 15,
            revision_id: id(0x733),
            source_ids: vec![id(0x734)],
        })
        .expect("checkpoint recovered working copy");
    let note = checkpoint.note.expect("recovered note");
    assert_eq!(note.id, note_id);
    assert_eq!(note.body_markdown, body);
    assert_eq!(note.sources.len(), 1);
    assert_eq!(note.sources[0].label.as_deref(), Some("Recovered source"));
    assert_eq!(
        reopened
            .client()
            .load_working_copy(note.id)
            .expect("consumed working copy"),
        None
    );
}

#[test]
fn malformed_media_text_does_not_renew_an_expired_draft_lease() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 10,
        orphan_grace_period_ms: 5,
        ..MediaLimits::default()
    };
    let attachment = library.ingest(
        (0x733, 0x734, 0x735),
        "malformed.png",
        "image/png",
        b"malformed image",
        11,
        limits,
    );
    let canonical = format!("{{{{kosh:image:{};width=80%}}}}", attachment.id);
    library
        .database
        .client()
        .save_working_copy(SaveWorkingCopyWrite {
            input: SaveWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                base_revision_id: None,
                edit_generation: 12,
                document_json: super::document::fixture_from_markdown(&canonical),
                body_markdown: canonical,
                sources: Vec::new(),
            },
            now_ms: 12,
            media_limits: limits,
            allow_empty_ephemeral: true,
        })
        .expect("save canonical media token");
    let malformed = format!("{{{{kosh:image:{};width=garbage", attachment.id);
    library
        .database
        .client()
        .save_working_copy(SaveWorkingCopyWrite {
            input: SaveWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                base_revision_id: None,
                edit_generation: 13,
                document_json: super::document::fixture_from_markdown(&malformed),
                body_markdown: malformed,
                sources: Vec::new(),
            },
            now_ms: 13,
            media_limits: limits,
            allow_empty_ephemeral: true,
        })
        .expect("save malformed media text");

    let maintenance = library
        .database
        .client()
        .maintain_media(22, limits)
        .expect("expire malformed media lease");
    assert_eq!(maintenance.cleanup.retired_attachment_count, 1);
    assert!(matches!(
        library
            .database
            .client()
            .load_media_payload(attachment.id, 22, None, 64),
        Err(DatabaseError::NotFound { .. })
    ));
}

#[test]
fn startup_recovery_renews_expired_media_still_referenced_by_a_saved_draft() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 10,
        orphan_grace_period_ms: 5,
        ..MediaLimits::default()
    };
    let attachment = library.ingest(
        (0x72b, 0x72c, 0x72d),
        "recovered.png",
        "image/png",
        b"recovered image",
        11,
        limits,
    );
    let body = format!("{{{{kosh:image:{};width=80%}}}}", attachment.id);
    library
        .database
        .client()
        .save_working_copy(SaveWorkingCopyWrite {
            input: SaveWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                base_revision_id: None,
                edit_generation: 12,
                document_json: super::document::fixture_from_markdown(&body),
                body_markdown: body,
                sources: Vec::new(),
            },
            now_ms: 12,
            media_limits: limits,
            allow_empty_ephemeral: true,
        })
        .expect("save expiring media draft");

    let maintenance = library
        .database
        .client()
        .maintain_media(22, limits)
        .expect("recover expired draft media");
    assert_eq!(maintenance.cleanup.retired_attachment_count, 0);
    assert_eq!(
        library
            .database
            .client()
            .load_media_payload(attachment.id.clone(), 22, None, 64)
            .expect("recovered media remains readable")
            .bytes,
        b"recovered image"
    );
    let main = library.database.open_main_read_only().expect("main reader");
    assert_eq!(
        main.query_row(
            "SELECT lease.state || ':' || lease.expires_at
             FROM media_ingest_lease AS lease
             WHERE lease.attachment_id = ?1",
            params![attachment.id],
            |row| row.get::<_, String>(0),
        )
        .expect("renewed lease"),
        "COMMITTED:32"
    );
}

#[test]
fn canceled_draft_requires_grace_and_explicit_authorization_before_reaping() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 100,
        orphan_grace_period_ms: 50,
        ..MediaLimits::default()
    };
    let attachment = library.ingest(
        (0x730, 0x731, 0x732),
        "cancel.txt",
        "text/plain",
        b"temporary",
        11,
        limits,
    );
    assert!(!library
        .database
        .client()
        .discard_working_copy(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: 9,
            },
            12,
        )
        .expect("stale cancellation"));
    assert_eq!(
        library
            .database
            .client()
            .load_media_payload(attachment.id.clone(), 12, None, 64)
            .expect("stale cancellation preserved lease")
            .bytes,
        b"temporary"
    );
    assert!(library
        .database
        .client()
        .discard_working_copy(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: 10,
            },
            12,
        )
        .expect("cancel draft"));

    let first = library
        .database
        .client()
        .maintain_media(12, limits)
        .expect("first maintenance");
    assert_eq!(first.cleanup.retired_attachment_count, 1);
    assert_eq!(first.cleanup.deleted_blob_count, 0);
    assert_eq!(blob_count(&library.database), 1);
    assert!(matches!(
        library
            .database
            .client()
            .load_media_payload(attachment.id, 13, None, 64),
        Err(DatabaseError::NotFound { .. })
    ));

    let writer = Connection::open(&library.paths.media).expect("non-cooperating media writer");
    let delete_error = writer
        .execute("DELETE FROM media_blob", [])
        .expect_err("unguarded deletion must fail");
    assert!(delete_error
        .to_string()
        .contains("media blob deletion requires authorization"));
    drop(writer);

    let before_grace = library
        .database
        .client()
        .maintain_media(61, limits)
        .expect("maintenance before grace");
    assert_eq!(before_grace.cleanup.deleted_blob_count, 0);
    let after_grace = library
        .database
        .client()
        .maintain_media(62, limits)
        .expect("maintenance after grace");
    assert_eq!(after_grace.cleanup.deleted_blob_count, 1);
    assert_eq!(after_grace.cleanup.reclaimed_bytes, 9);
    assert_eq!(blob_count(&library.database), 0);
}

#[test]
fn reclamation_preflight_cleans_stale_candidate_windows_incrementally() {
    let library = TestLibrary::new();
    library
        .database
        .shutdown()
        .expect("stop writer before direct preflight");
    let mut main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("direct main writer");
    let media = super::connection::open_writer(
        &library.paths.media,
        super::connection::DatabaseKind::Media,
        super::connection::FileState::Existing,
    )
    .expect("direct media writer");
    let limits = MediaLimits {
        orphan_grace_period_ms: 1,
        max_reaps_per_maintenance: 2,
        ..MediaLimits::default()
    };

    for byte in [1_u8, 2] {
        main.execute(
            "INSERT INTO media_blob_reap_candidate(sha256, orphaned_at, reason)
             VALUES(?1, 1, 'stale test candidate')",
            params![[byte; 32].as_slice()],
        )
        .expect("insert stale missing-blob candidate");
    }
    let eligible_hash = Sha256::digest(b"eligible after stale candidates");
    main.execute(
        "INSERT INTO media_blob_reap_candidate(sha256, orphaned_at, reason)
         VALUES(?1, 2, 'eligible test candidate')",
        params![eligible_hash.as_slice()],
    )
    .expect("insert eligible candidate");
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, ?2, ?3, 1)",
            params![
                eligible_hash.as_slice(),
                b"eligible after stale candidates".as_slice(),
                i64::try_from(b"eligible after stale candidates".len())
                    .expect("eligible byte length")
            ],
        )
        .expect("insert eligible orphan blob");

    assert_eq!(
        media_blob_reclamation_preflight(&mut main, &media, 10, limits)
            .expect("clean bounded stale candidate window"),
        MediaBlobReclamationPreflight::Continue
    );
    assert_eq!(
        main.query_row(
            "SELECT count(*) FROM media_blob_reap_candidate",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("remaining candidate count"),
        1
    );
    assert_eq!(
        media_blob_reclamation_preflight(&mut main, &media, 10, limits)
            .expect("resume after yielding"),
        MediaBlobReclamationPreflight::Eligible
    );
}

#[test]
fn attachment_eligibility_is_rechecked_after_candidate_preflight_yields() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 1,
        orphan_grace_period_ms: 1,
        max_reaps_per_maintenance: 1,
        ..MediaLimits::default()
    };
    let attachment = library.ingest(
        (0x6f0, 0x6f1, 0x6f2),
        "queued-clear.txt",
        "text/plain",
        b"queued clear evidence",
        11,
        limits,
    );
    library.save_capture(
        &format!("Before clear ![queued](/media/{})", attachment.id),
        12,
    );
    library
        .database
        .shutdown()
        .expect("stop writer before inserting stale candidate");
    let main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("main writer");
    main.execute(
        "INSERT INTO media_blob_reap_candidate(sha256, orphaned_at, reason)
         VALUES(?1, 0, 'queued clear regression')",
        params![vec![0xa5_u8; 32]],
    )
    .expect("insert one full stale candidate batch");
    drop(main);

    let restarted = Database::initialize(library.paths.clone()).expect("restart database");
    let client = restarted.client();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    client
        .pause_for_test(started_tx, release_rx)
        .expect("queue writer pause");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer paused");

    let maintenance = client
        .enqueue_media_maintenance_for_test(20, limits)
        .expect("queue maintenance");
    let clear = client
        .enqueue_discard_working_copy_for_test(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: 12,
            },
            20,
        )
        .expect("queue draft clear behind initial maintenance");
    release_tx.send(()).expect("release writer");

    assert!(clear
        .recv_timeout(Duration::from_secs(2))
        .expect("clear reply")
        .expect("clear draft"));
    let (snapshot, report) = maintenance
        .recv_timeout(Duration::from_secs(2))
        .expect("maintenance reply")
        .expect("maintenance after queued clear");
    assert!(
        snapshot.is_some(),
        "the post-yield attachment mutation must be snapshot-backed"
    );
    assert_eq!(report.cleanup.retired_attachment_count, 1);
}

#[test]
fn startup_lifecycle_recovery_never_reaps_without_a_verified_snapshot() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 10,
        orphan_grace_period_ms: 50,
        ..MediaLimits::default()
    };
    library.ingest(
        (0x733, 0x734, 0x735),
        "restart-orphan.txt",
        "text/plain",
        b"restart orphan",
        11,
        limits,
    );
    assert!(library
        .database
        .client()
        .discard_working_copy(
            DiscardWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: 10,
            },
            12,
        )
        .expect("cancel draft"));
    library
        .database
        .shutdown()
        .expect("stop writer before startup recovery probes");

    let mut main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("main writer");
    let mut media = super::connection::open_writer(
        &library.paths.media,
        super::connection::DatabaseKind::Media,
        super::connection::FileState::Existing,
    )
    .expect("media writer");
    assert_eq!(
        recover_media_lifecycle_batch(&mut main, &mut media, 12, limits, None)
            .expect("initial startup recovery"),
        None
    );
    assert_eq!(
        recover_media_lifecycle_batch(&mut main, &mut media, 62, limits, None)
            .expect("eligible startup recovery"),
        None
    );
    assert_eq!(
        media
            .query_row("SELECT count(*) FROM media_blob", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("preserved startup blob"),
        1
    );
    assert_eq!(
        media
            .query_row(
                "SELECT count(*) FROM media_blob_reap_authorization",
                [],
                |row| row.get::<_, i64>(0)
            )
            .expect("startup authorization count"),
        0
    );
    drop(main);
    drop(media);

    let restarted = Database::initialize(library.paths.clone()).expect("restart database");
    let (snapshot, maintenance) = restarted
        .client()
        .maintain_media_with_safety_snapshot(62, limits)
        .expect("explicit snapshot-backed reclamation");
    let snapshot = snapshot.expect("reclamation requires a verified snapshot");
    assert!(snapshot.directory.join("manifest.json").is_file());
    assert_eq!(maintenance.cleanup.deleted_blob_count, 1);
    assert_eq!(blob_count(&restarted), 0);
}

#[test]
fn lifecycle_reconciliation_yields_between_bounded_hash_batches() {
    let library = TestLibrary::new();
    let mut media = Connection::open(&library.paths.media).expect("media writer");
    let transaction = media.transaction().expect("media transaction");
    let blob_count = MEDIA_RECONCILE_BATCH_SIZE + 1;
    for index in 0..blob_count {
        let bytes = index.to_be_bytes();
        let hash = Sha256::digest(bytes);
        transaction
            .execute(
                "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
                 VALUES(?1, ?2, ?3, 11)",
                params![
                    hash.as_slice(),
                    bytes.as_slice(),
                    i64::try_from(bytes.len()).expect("test byte length")
                ],
            )
            .expect("insert orphan media blob");
    }
    transaction.commit().expect("commit orphan media blobs");
    drop(media);
    library
        .database
        .shutdown()
        .expect("stop database writer before direct batch probes");

    let mut main = super::connection::open_writer(
        &library.paths.main,
        super::connection::DatabaseKind::Main,
        super::connection::FileState::Existing,
    )
    .expect("direct main writer");
    let mut media = super::connection::open_writer(
        &library.paths.media,
        super::connection::DatabaseKind::Media,
        super::connection::FileState::Existing,
    )
    .expect("direct media writer");
    let limits = MediaLimits {
        orphan_grace_period_ms: 100,
        ..MediaLimits::default()
    };
    let cursor = recover_media_lifecycle_batch(&mut main, &mut media, 12, limits, None)
        .expect("first bounded lifecycle batch")
        .expect("more media hashes remain");
    assert_eq!(
        main.query_row(
            "SELECT count(*) FROM media_blob_reap_candidate",
            [],
            |row| { row.get::<_, u32>(0) }
        )
        .expect("first reap candidate count"),
        MEDIA_RECONCILE_BATCH_SIZE
    );

    let completed = recover_media_lifecycle_batch(&mut main, &mut media, 12, limits, Some(cursor))
        .expect("second bounded lifecycle batch");
    assert_eq!(completed, None);
    assert_eq!(
        main.query_row(
            "SELECT count(*) FROM media_blob_reap_candidate",
            [],
            |row| { row.get::<_, u32>(0) }
        )
        .expect("complete reap candidate count"),
        blob_count
    );
}

#[test]
fn revision_membership_keeps_shared_blob_and_authenticates_long_lived_reads() {
    let library = TestLibrary::new();
    let limits = MediaLimits {
        draft_lease_duration_ms: 10,
        orphan_grace_period_ms: 5,
        ..MediaLimits::default()
    };
    let first = library.ingest(
        (0x740, 0x741, 0x742),
        "kept.txt",
        "text/plain",
        b"shared",
        11,
        limits,
    );
    let second = library.ingest(
        (0x743, 0x744, 0x745),
        "discarded.txt",
        "text/plain",
        b"shared",
        12,
        limits,
    );
    let body = format!("evidence {{{{kosh:attachment:{}}}}}", first.id);
    let saved = library.save_capture(&body, 13);
    let checkpoint = library
        .database
        .client()
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: CheckpointWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: saved.edit_generation,
            },
            now_ms: 14,
            revision_id: id(0x747),
            source_ids: Vec::new(),
        })
        .expect("checkpoint revision with attachment");
    assert_eq!(
        checkpoint.note.expect("checkpoint note").body_markdown,
        body
    );

    let maintenance = library
        .database
        .client()
        .maintain_media(30, limits)
        .expect("retire unused shared attachment");
    assert_eq!(maintenance.cleanup.deleted_blob_count, 0);
    assert_eq!(blob_count(&library.database), 1);
    let payload = library
        .database
        .client()
        .load_media_payload(first.id, 30, None, 64)
        .expect("revision-authorized media");
    assert_eq!(payload.bytes, b"shared");
    assert!(payload.revision_bound);
    assert!(matches!(
        library
            .database
            .client()
            .load_media_payload(second.id, 30, None, 64),
        Err(DatabaseError::NotFound { .. })
    ));
}

#[test]
fn edit_draft_authorizes_media_inherited_from_its_base_revision() {
    let library = TestLibrary::new();
    let attachment = library.ingest(
        (0x748, 0x749, 0x74a),
        "inherited.png",
        "image/png",
        b"inherited",
        11,
        MediaLimits::default(),
    );
    let body = format!(
        "existing image {{{{kosh:image:{};width=80%}}}}",
        attachment.id
    );
    let capture = library.save_capture(&body, 12);
    let tidbit = library
        .database
        .client()
        .checkpoint_working_copy(CheckpointWorkingCopyWrite {
            input: CheckpointWorkingCopyInput {
                note_id: CAPTURE_DRAFT_ID.into(),
                expected_edit_generation: capture.edit_generation,
            },
            now_ms: 13,
            revision_id: id(0x74c),
            source_ids: Vec::new(),
        })
        .expect("checkpoint revision with media")
        .note
        .expect("checkpointed note");

    let edit_limits = MediaLimits {
        max_attachments_per_draft: 1,
        ..MediaLimits::default()
    };
    let edit_draft = library
        .database
        .client()
        .save_working_copy(SaveWorkingCopyWrite {
            input: SaveWorkingCopyInput {
                note_id: tidbit.id.clone(),
                base_revision_id: Some(tidbit.current_revision_id.clone()),
                edit_generation: 14,
                document_json: super::document::fixture_from_markdown(&body),
                body_markdown: body.clone(),
                sources: Vec::new(),
            },
            now_ms: 14,
            media_limits: edit_limits,
            allow_empty_ephemeral: false,
        })
        .expect("save edit working copy with base-revision media")
        .working_copy
        .expect("saved edit working copy");

    assert_eq!(edit_draft.id, tidbit.id);
    assert_eq!(
        library
            .database
            .open_main_read_only()
            .expect("main reader")
            .query_row(
                "SELECT count(*) FROM draft_media_lease WHERE draft_id = ?1",
                params![edit_draft.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("edit draft lease count"),
        0
    );

    let added = library
        .database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id: edit_draft.id.clone(),
                display_filename: "added.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 16,
                limits: edit_limits,
            },
            Cursor::new(b"new edit media"),
        )
        .expect("stage one new edit attachment");
    let over_capacity = library
        .database
        .client()
        .save_working_copy(SaveWorkingCopyWrite {
            input: SaveWorkingCopyInput {
                note_id: tidbit.id.clone(),
                base_revision_id: Some(tidbit.current_revision_id.clone()),
                edit_generation: 15,
                document_json: super::document::single_paragraph(&format!(
                    "{body}\n{{{{kosh:attachment:{}}}}}",
                    added.id
                )),
                body_markdown: format!("{body}\n{{{{kosh:attachment:{}}}}}", added.id),
                sources: Vec::new(),
            },
            now_ms: 15,
            media_limits: edit_limits,
            allow_empty_ephemeral: false,
        })
        .expect_err("cap inherited and newly leased attachment references together");
    assert!(matches!(over_capacity, DatabaseError::InvalidInput(_)));
    assert_eq!(
        library
            .database
            .client()
            .load_working_copy(tidbit.id)
            .expect("load unchanged edit working copy")
            .expect("edit working copy remains")
            .body_markdown,
        body
    );
}

#[test]
fn integrity_scan_reports_missing_corrupt_and_extra_blobs() {
    let root = tempfile::tempdir().expect("integrity root");
    let paths = DatabasePaths::new(root.path());
    let staging = root.path().join("staging");
    let database = Database::initialize(paths.clone()).expect("integrity database");
    let draft = database
        .client()
        .save_working_copy_for_test(
            CAPTURE_DRAFT_ID.into(),
            None,
            1,
            String::new(),
            Vec::new(),
            10,
        )
        .expect("capture working copy");
    assert_eq!(draft.id, CAPTURE_DRAFT_ID);
    let first_bytes = b"corrupt me";
    let second_bytes = b"remove me";
    let first = ingest_direct(&database, &staging, 0x750, 0x751, 0x752, first_bytes, 11);
    let second = ingest_direct(&database, &staging, 0x753, 0x754, 0x755, second_bytes, 12);
    let media = Connection::open(&paths.media).expect("media writer");
    let first_hash = digest(first_bytes);
    let first_rowid = media
        .query_row(
            "SELECT rowid FROM media_blob WHERE sha256 = ?1",
            params![&first_hash],
            |row| row.get::<_, i64>(0),
        )
        .expect("corrupt blob rowid");
    let mut blob = media
        .blob_open(MAIN_DB, "media_blob", "bytes", first_rowid, false)
        .expect("writable blob");
    blob.seek(SeekFrom::Start(0)).expect("seek blob");
    blob.write_all(b"X").expect("corrupt blob");
    drop(blob);

    let second_hash = digest(second_bytes);
    media
        .execute(
            "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
             VALUES(?1, 13, 'integrity test')",
            params![&second_hash],
        )
        .expect("authorize missing test");
    media
        .execute(
            "DELETE FROM media_blob WHERE sha256 = ?1",
            params![&second_hash],
        )
        .expect("remove blob");
    let extra_bytes = b"extra";
    let extra_hash = digest(extra_bytes);
    media
        .execute(
            "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
             VALUES(?1, ?2, ?3, 13)",
            params![
                &extra_hash,
                extra_bytes.as_slice(),
                i64::try_from(extra_bytes.len()).expect("extra byte length")
            ],
        )
        .expect("insert extra blob");
    drop(media);

    let report = database
        .client()
        .media_integrity_report(14)
        .expect("integrity report");
    assert_eq!(report.missing_blob_attachment_ids, vec![second.id]);
    assert_eq!(report.corrupt_blob_sha256, vec![hex(&first_hash)]);
    assert_eq!(report.extra_blob_sha256, vec![hex(&extra_hash)]);
    assert!(report.orphaned_attachment_ids.is_empty());
    assert!(!report.diagnostics_truncated);
    assert!(matches!(
        database.client().load_media_payload(first.id, 14, None, 64),
        Err(DatabaseError::Validation { kind: "media", .. })
    ));
}

#[test]
fn integrity_scan_batches_work_and_caps_returned_diagnostics() {
    let root = tempfile::tempdir().expect("bounded integrity root");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("bounded integrity database");
    let mut main = Connection::open(&paths.main).expect("bounded integrity writer");
    let transaction = main.transaction().expect("bounded integrity transaction");
    for index in 0_u64..300 {
        let attachment_id = id(0x800 + index);
        let hash = Sha256::digest(index.to_be_bytes());
        transaction
            .execute(
                "INSERT INTO attachment(
                    id, created_at, updated_at, sha256, display_filename,
                    media_type, byte_length, kind, extraction_state
                 ) VALUES(?1, 10, 10, ?2, ?3, 'application/octet-stream', 1,
                          'BINARY', 'NOT_APPLICABLE')",
                params![attachment_id, hash.as_slice(), format!("{index}.bin")],
            )
            .expect("insert bounded integrity attachment");
    }
    transaction.commit().expect("commit bounded integrity rows");
    drop(main);

    let report = database
        .client()
        .media_integrity_report(11)
        .expect("bounded integrity report");
    let diagnostic_count = report.missing_blob_attachment_ids.len()
        + report.corrupt_blob_sha256.len()
        + report.extra_blob_sha256.len()
        + report.orphaned_attachment_ids.len();
    assert_eq!(diagnostic_count, 256);
    assert!(report.diagnostics_truncated);
}

fn ingest_direct(
    database: &Database,
    staging: &Path,
    attachment_suffix: u64,
    lease_suffix: u64,
    stage_suffix: u64,
    bytes: &[u8],
    now_ms: i64,
) -> super::AttachmentRecord {
    let staged = StagedAttachment::from_reader(
        Cursor::new(bytes),
        staging,
        &id(stage_suffix),
        MediaLimits::default().max_attachment_bytes,
    )
    .expect("stage integrity attachment");
    let write: IngestAttachmentWrite = staged.write(IngestAttachmentMetadata {
        attachment_id: id(attachment_suffix),
        ingest_lease_id: id(lease_suffix),
        draft_id: CAPTURE_DRAFT_ID.into(),
        display_filename: format!("{attachment_suffix}.bin"),
        media_type: "application/octet-stream".into(),
        now_ms,
        limits: MediaLimits::default(),
    });
    database
        .client()
        .ingest_attachment(write)
        .expect("ingest integrity attachment")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
