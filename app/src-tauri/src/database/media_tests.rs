use std::{
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

use rusqlite::{params, Connection, MAIN_DB};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{
    drafts::SaveDraftWrite,
    media::{
        referenced_attachments, AttachmentDisplayRole, IngestAttachmentMetadata,
        IngestAttachmentWrite, MediaByteRange, StagedAttachment,
    },
    tidbits::CreateTidbitWrite,
    AttachmentIngestInput, AttachmentKind, ClearDraftInput, Database, DatabaseError, DatabasePaths,
    MediaLimits, SaveDraftInput, TidbitDraft,
};

const CAPTURE_DRAFT_ID: &str = "019f547b-6200-7000-8000-000000007001";

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

    fn save_capture(&self, body_markdown: &str, now_ms: i64) -> super::Draft {
        self.database
            .client()
            .save_draft(SaveDraftWrite {
                input: SaveDraftInput {
                    context_key: "capture".into(),
                    tidbit_id: None,
                    base_revision_id: None,
                    title: None,
                    body_markdown: body_markdown.into(),
                    sources: Vec::new(),
                },
                now_ms,
                draft_id: CAPTURE_DRAFT_ID.into(),
                media_lease_duration_ms: MediaLimits::default().draft_lease_duration_ms,
            })
            .expect("save capture draft")
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
    let markdown = format!(
        "{{{{kosh:attachment:{first}}}}}\n\
         {{{{kosh:image:{second};width=70%}}}}\n\
         {{{{kosh:image:{first};width=100%}}}}\n\
         {{{{kosh:image:{};width=070%}}}}",
        id(0x707)
    );
    let references = referenced_attachments(&markdown);
    assert_eq!(references.len(), 2);
    assert_eq!(references[0].id, first);
    assert_eq!(references[0].display_role, AttachmentDisplayRole::Inline);
    assert_eq!(references[1].id, second);
    assert_eq!(references[1].display_role, AttachmentDisplayRole::Inline);
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
            Some(MediaByteRange {
                start: 5,
                end_inclusive: 9,
            }),
            5,
        )
        .expect("authorized bounded read");
    assert_eq!(payload.bytes, b"bytes");
    assert_eq!(payload.total_byte_length, 10);
    assert_eq!(payload.media_type, "text/plain");
    assert!(!payload.revision_bound);

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
        .clear_draft_at(
            ClearDraftInput {
                context_key: "capture".into(),
                expected_updated_at_ms: 9,
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
        .clear_draft_at(
            ClearDraftInput {
                context_key: "capture".into(),
                expected_updated_at_ms: 10,
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
    library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some("Shared media".into()),
                body_markdown: body,
                sources: Vec::new(),
            },
            now_ms: 14,
            tidbit_id: id(0x746),
            revision_id: id(0x747),
            source_ids: Vec::new(),
        })
        .expect("create revision with attachment");
    library
        .database
        .client()
        .clear_draft_at(
            ClearDraftInput {
                context_key: "capture".into(),
                expected_updated_at_ms: saved.updated_at_ms,
            },
            15,
        )
        .expect("clear committed draft");

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
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some("Illustrated note".into()),
                body_markdown: body.clone(),
                sources: Vec::new(),
            },
            now_ms: 13,
            tidbit_id: id(0x74b),
            revision_id: id(0x74c),
            source_ids: Vec::new(),
        })
        .expect("create revision with media");
    assert!(library
        .database
        .client()
        .clear_draft_at(
            ClearDraftInput {
                context_key: "capture".into(),
                expected_updated_at_ms: capture.updated_at_ms,
            },
            14,
        )
        .expect("clear capture lease"));

    let edit_context = format!("edit:{}", tidbit.id);
    let edit_draft = library
        .database
        .client()
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: edit_context,
                tidbit_id: Some(tidbit.id),
                base_revision_id: Some(tidbit.current_revision_id),
                title: Some("Illustrated note".into()),
                body_markdown: body,
                sources: Vec::new(),
            },
            now_ms: 15,
            draft_id: id(0x74d),
            media_lease_duration_ms: MediaLimits::default().draft_lease_duration_ms,
        })
        .expect("save edit draft with base-revision media");

    assert_eq!(edit_draft.id, id(0x74d));
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
}

#[test]
fn integrity_scan_reports_missing_corrupt_and_extra_blobs() {
    let root = tempfile::tempdir().expect("integrity root");
    let paths = DatabasePaths::new(root.path());
    let staging = root.path().join("staging");
    let database = Database::initialize(paths.clone()).expect("integrity database");
    let draft = database
        .client()
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
            draft_id: CAPTURE_DRAFT_ID.into(),
            media_lease_duration_ms: MediaLimits::default().draft_lease_duration_ms,
        })
        .expect("capture draft");
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
    assert!(matches!(
        database.client().load_media_payload(first.id, 14, None, 64),
        Err(DatabaseError::Validation { kind: "media", .. })
    ));
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
