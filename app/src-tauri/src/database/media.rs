use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rusqlite::{
    blob::ZeroBlob, params, Connection, OptionalExtension, Transaction, TransactionBehavior,
    MAIN_DB,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{search, DatabaseError, Result};

const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const MAX_FILENAME_CHARS: usize = 255;
const MAX_MEDIA_TYPE_BYTES: usize = 127;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_DIRECT_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
const INTEGRITY_ATTACHMENT_BATCH_SIZE: u32 = 64;
const INTEGRITY_BLOB_BATCH_SIZE: u32 = 1;
const MAX_INTEGRITY_DIAGNOSTICS: usize = 256;
pub(crate) const MEDIA_RECONCILE_BATCH_SIZE: u32 = 64;
pub(crate) const PDF_RECOVERY_BATCH_SIZE: usize = 64;
const IMAGE_TOKEN_PREFIX: &str = "{{kosh:image:";
const ATTACHMENT_TOKEN_PREFIX: &str = "{{kosh:attachment:";
const PDF_TOKEN_PREFIX: &str = "{{kosh:pdf:";
const TOKEN_SUFFIX: &str = "}}";
const IMAGE_PREVIEW_MEDIA_TYPE: &str = "image/webp";
const IMAGE_OCR_EXTRACTOR: &str = "ocr";
const PDF_TEXT_EXTRACTOR: &str = "pdf-text";
const MAX_IMAGE_OCR_ATTEMPTS: u32 = 4;
const MAX_PDF_EXTRACTION_ATTEMPTS: u32 = 3;
const PDF_PASSAGE_TARGET_CHARS: usize = 700;
pub(crate) const PDF_PASSAGE_MAX_CHARS: usize = 1_000;
pub(crate) const PDF_PASSAGE_OVERLAP_CHARS: usize = 100;
const IMAGE_OCR_RETRY_DELAYS_MS: [i64; 3] = [5 * 60 * 1_000, 30 * 60 * 1_000, 2 * 60 * 60 * 1_000];
const IMAGE_OCR_RECOVERY_BATCH_SIZE: usize = 64;
const MAX_OCR_REGIONS: usize = 4_096;
const MAX_OCR_REGION_CHARS: usize = 16_384;
const MAX_OCR_TOTAL_CHARS: usize = 1_000_000;
const INTERRUPTED_OCR_ERROR: &str = "OCR was interrupted before it completed";
const STALE_OCR_ERROR: &str = "OCR extractor provenance is no longer current";
const RETIRED_OCR_ERROR: &str = "Image was retired before OCR completed";
const RETIRED_PDF_ERROR: &str = "PDF was retired before extraction completed";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaLimits {
    pub max_attachment_bytes: u64,
    pub max_attachments_per_draft: u32,
    pub max_protocol_response_bytes: u64,
    pub draft_lease_duration_ms: i64,
    pub orphan_grace_period_ms: i64,
    pub max_reaps_per_maintenance: u32,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_attachment_bytes: MAX_DIRECT_ATTACHMENT_BYTES,
            max_attachments_per_draft: 32,
            max_protocol_response_bytes: MAX_DIRECT_ATTACHMENT_BYTES,
            draft_lease_duration_ms: 24 * 60 * 60 * 1_000,
            orphan_grace_period_ms: 7 * 24 * 60 * 60 * 1_000,
            max_reaps_per_maintenance: 32,
        }
    }
}

impl MediaLimits {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.max_attachment_bytes == 0 || self.max_attachment_bytes > MAX_DIRECT_ATTACHMENT_BYTES
        {
            return Err(DatabaseError::InvalidInput(
                "maxAttachmentBytes must be between 1 and the direct media response limit".into(),
            ));
        }
        if self.max_attachments_per_draft == 0 || self.max_attachments_per_draft > 256 {
            return Err(DatabaseError::InvalidInput(
                "maxAttachmentsPerDraft must be between 1 and 256".into(),
            ));
        }
        if self.max_protocol_response_bytes != self.max_attachment_bytes {
            return Err(DatabaseError::InvalidInput(
                "maxProtocolResponseBytes must equal maxAttachmentBytes".into(),
            ));
        }
        if self.draft_lease_duration_ms <= 0
            || self.orphan_grace_period_ms <= 0
            || self.max_reaps_per_maintenance == 0
            || self.max_reaps_per_maintenance > 1_024
        {
            return Err(DatabaseError::InvalidInput(
                "media lease and grace limits must be positive, with 1 to 1024 reaps".into(),
            ));
        }
        Ok(self)
    }

    fn lease_expiry(self, now_ms: i64) -> Result<i64> {
        checked_timestamp_add(now_ms, self.draft_lease_duration_ms, "media lease expiry")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AttachmentKind {
    Image,
    Pdf,
    Text,
    Binary,
}

impl AttachmentKind {
    fn from_media_type(media_type: &str) -> Self {
        if media_type.starts_with("image/") {
            Self::Image
        } else if media_type == "application/pdf" {
            Self::Pdf
        } else if media_type.starts_with("text/") {
            Self::Text
        } else {
            Self::Binary
        }
    }

    fn as_db_str(self) -> &'static str {
        match self {
            Self::Image => "IMAGE",
            Self::Pdf => "PDF",
            Self::Text => "TEXT",
            Self::Binary => "BINARY",
        }
    }

    fn extraction_state(self) -> &'static str {
        match self {
            Self::Binary => "NOT_APPLICABLE",
            Self::Image | Self::Pdf | Self::Text => "PENDING",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    pub id: String,
    pub ingest_lease_id: String,
    pub display_filename: String,
    pub media_type: String,
    pub byte_length: u64,
    pub kind: AttachmentKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImageOcrStatus {
    Pending,
    Running,
    RetryWait,
    Ready,
    Failed,
}

impl ImageOcrStatus {
    fn from_db(value: &str) -> Result<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "RUNNING" => Ok(Self::Running),
            "RETRY_WAIT" => Ok(Self::RetryWait),
            "READY" => Ok(Self::Ready),
            "FAILED" => Ok(Self::Failed),
            _ => Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("unknown image OCR state {value}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRecord {
    #[serde(flatten)]
    pub attachment: AttachmentRecord,
    pub natural_width: u32,
    pub natural_height: u32,
    pub ocr_status: ImageOcrStatus,
    pub ocr_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageStatusRecord {
    pub attachment_id: String,
    pub natural_width: u32,
    pub natural_height: u32,
    pub ocr_status: ImageOcrStatus,
    pub ocr_error: Option<String>,
    pub next_attempt_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PdfExtractionStatus {
    Pending,
    Running,
    RetryWait,
    Ready,
    Failed,
}

impl PdfExtractionStatus {
    fn from_db(value: &str) -> Result<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "RUNNING" => Ok(Self::Running),
            "RETRY_WAIT" => Ok(Self::RetryWait),
            "READY" => Ok(Self::Ready),
            "FAILED" => Ok(Self::Failed),
            _ => Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("unknown PDF extraction state {value}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfRecord {
    #[serde(flatten)]
    pub attachment: AttachmentRecord,
    pub page_count: u32,
    pub extraction_status: PdfExtractionStatus,
    pub extraction_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfStatusRecord {
    pub attachment_id: String,
    pub display_filename: String,
    pub page_count: u32,
    pub extracted_page_count: u32,
    pub unavailable_page_count: u32,
    pub extraction_status: PdfExtractionStatus,
    pub extraction_error: Option<String>,
    pub next_attempt_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct CanonicalImage {
    pub bytes: Vec<u8>,
    pub natural_width: u32,
    pub natural_height: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct IngestImageWrite {
    pub attachment: IngestAttachmentWrite,
    pub extraction_id: String,
    pub preview: CanonicalImage,
}

#[derive(Clone, Debug)]
pub(crate) struct IngestPdfWrite {
    pub attachment: IngestAttachmentWrite,
    pub extraction_id: String,
    pub page_count: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PdfPageSource {
    NativeText,
    Ocr,
}

impl PdfPageSource {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::NativeText => "NATIVE_TEXT",
            Self::Ocr => "OCR",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PdfPageExtraction {
    pub page_number: u32,
    pub result: std::result::Result<(PdfPageSource, String), String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PdfExtractionJob {
    pub extraction_id: String,
    pub attachment_id: String,
    pub extractor_version: String,
    pub content_hash: Vec<u8>,
    pub attempt_count: u32,
    pub page_count: u32,
    pub pdf_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImageOcrRegion {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageOcrJob {
    pub extraction_id: String,
    pub attachment_id: String,
    pub extractor_version: String,
    pub content_hash: Vec<u8>,
    pub attempt_count: u32,
    pub preview_bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOcrRecovery {
    pub requeued: u64,
    pub terminally_failed: u64,
    pub remaining: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOcrDiagnostics {
    pub pending: u64,
    pub running: u64,
    pub retry_wait: u64,
    pub ready: u64,
    pub failed: u64,
    pub oldest_eligible_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AttachmentIngestInput {
    pub draft_id: String,
    pub display_filename: String,
    pub media_type: String,
    pub now_ms: i64,
    pub limits: MediaLimits,
}

#[derive(Clone, Debug)]
pub(crate) struct IngestAttachmentWrite {
    pub attachment_id: String,
    pub ingest_lease_id: String,
    pub draft_id: String,
    pub display_filename: String,
    pub media_type: String,
    pub staged_path: PathBuf,
    pub sha256: Vec<u8>,
    pub byte_length: u64,
    pub now_ms: i64,
    pub limits: MediaLimits,
}

#[derive(Clone, Debug)]
pub(crate) struct IngestAttachmentMetadata {
    pub attachment_id: String,
    pub ingest_lease_id: String,
    pub draft_id: String,
    pub display_filename: String,
    pub media_type: String,
    pub now_ms: i64,
    pub limits: MediaLimits,
}

#[derive(Debug)]
pub(crate) struct StagedAttachment {
    path: PathBuf,
    sha256: Vec<u8>,
    byte_length: u64,
}

impl StagedAttachment {
    pub(crate) fn from_reader(
        reader: impl Read,
        staging_directory: &Path,
        stage_id: &str,
        max_bytes: u64,
    ) -> Result<Self> {
        validate_uuid_v7(stage_id, "stageId")?;
        if max_bytes == 0 {
            return Err(DatabaseError::InvalidInput(
                "attachment size limit must be positive".into(),
            ));
        }
        fs::create_dir_all(staging_directory)?;
        let path = staging_directory.join(format!("{stage_id}.part"));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        match stream_to_stage(reader, file, max_bytes) {
            Ok((sha256, byte_length)) if byte_length > 0 => Ok(Self {
                path,
                sha256,
                byte_length,
            }),
            Ok(_) => {
                let _ = fs::remove_file(&path);
                Err(DatabaseError::InvalidInput(
                    "the selected attachment is empty".into(),
                ))
            }
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    pub(crate) fn write(&self, metadata: IngestAttachmentMetadata) -> IngestAttachmentWrite {
        IngestAttachmentWrite {
            attachment_id: metadata.attachment_id,
            ingest_lease_id: metadata.ingest_lease_id,
            draft_id: metadata.draft_id,
            display_filename: metadata.display_filename,
            media_type: metadata.media_type,
            staged_path: self.path.clone(),
            sha256: self.sha256.clone(),
            byte_length: self.byte_length,
            now_ms: metadata.now_ms,
            limits: metadata.limits,
        }
    }
}

impl Drop for StagedAttachment {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MediaByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaRangeRequest {
    Inclusive(MediaByteRange),
    From(u64),
    Suffix(u64),
}

#[derive(Clone, Debug)]
pub(crate) struct MediaPayload {
    pub bytes: Vec<u8>,
    pub media_type: String,
    pub sha256: Vec<u8>,
    pub total_byte_length: u64,
    pub range: MediaByteRange,
    pub revision_bound: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaIntegrityReport {
    pub missing_blob_attachment_ids: Vec<String>,
    pub corrupt_blob_sha256: Vec<String>,
    pub extra_blob_sha256: Vec<String>,
    pub orphaned_attachment_ids: Vec<String>,
    pub diagnostics_truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCleanupResult {
    pub retired_attachment_count: u64,
    pub deleted_blob_count: u64,
    pub reclaimed_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaMaintenanceReport {
    pub inspected_at_ms: i64,
    pub integrity: MediaIntegrityReport,
    pub cleanup: MediaCleanupResult,
}

pub(crate) struct MediaIntegrityScan {
    now_ms: i64,
    phase: MediaIntegrityPhase,
    report: MediaIntegrityReport,
}

enum MediaIntegrityPhase {
    Initialize,
    Attachments {
        cursor: i64,
        max_rowid: i64,
        max_blob_rowid: i64,
    },
    Blobs {
        cursor: i64,
        max_rowid: i64,
    },
}

pub(crate) enum MediaIntegrityScanStep {
    Continue(MediaIntegrityScan),
    Complete(MediaIntegrityReport),
}

pub(crate) struct MediaMaintenanceScan {
    now_ms: i64,
    limits: MediaLimits,
    phase: MediaMaintenancePhase,
    cleanup: MediaCleanupResult,
}

enum MediaMaintenancePhase {
    Lifecycle {
        cursor: Option<Vec<u8>>,
        first_batch: bool,
    },
    Integrity(MediaIntegrityScan),
}

pub(crate) enum MediaMaintenanceScanStep {
    Continue(MediaMaintenanceScan),
    Complete(MediaMaintenanceReport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentDisplayRole {
    Inline,
    Attachment,
}

impl AttachmentDisplayRole {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Inline => "INLINE",
            Self::Attachment => "ATTACHMENT",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttachmentReference {
    pub id: String,
    pub display_role: AttachmentDisplayRole,
}

pub(crate) fn ingest_attachment(
    main: &mut Connection,
    media: &mut Connection,
    mut write: IngestAttachmentWrite,
) -> Result<AttachmentRecord> {
    let (limits, kind, expires_at) = validate_attachment_ingest(main, &mut write)?;
    let media_transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    stage_attachment_media(&media_transaction, &write, expires_at)?;
    media_transaction.commit()?;

    let main_transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    insert_attachment_records(&main_transaction, &write, limits, kind, expires_at)?;
    main_transaction.commit()?;

    clear_committed_media_lease(
        media,
        &write.ingest_lease_id,
        &write.sha256,
        &write.attachment_id,
        "attachment",
    );
    Ok(attachment_record(write, kind))
}

fn validate_attachment_ingest(
    main: &Connection,
    write: &mut IngestAttachmentWrite,
) -> Result<(MediaLimits, AttachmentKind, i64)> {
    validate_uuid_v7(&write.attachment_id, "attachmentId")?;
    validate_uuid_v7(&write.ingest_lease_id, "ingestLeaseId")?;
    validate_uuid_v7(&write.draft_id, "draftId")?;
    validate_timestamp(write.now_ms, "nowMs")?;
    let limits = write.limits.validate()?;
    validate_filename(&write.display_filename)?;
    validate_media_type(&write.media_type)?;
    write.media_type.make_ascii_lowercase();
    if write.sha256.len() != 32 {
        return Err(DatabaseError::InvalidInput(
            "staged attachment digest must contain 32 bytes".into(),
        ));
    }
    if write.byte_length == 0 || write.byte_length > limits.max_attachment_bytes {
        return Err(DatabaseError::InvalidInput(format!(
            "attachment must contain between 1 and {} bytes",
            limits.max_attachment_bytes
        )));
    }
    validate_draft_capacity(main, &write.draft_id, limits.max_attachments_per_draft)?;
    validate_staged_file(write)?;
    let kind = AttachmentKind::from_media_type(&write.media_type);
    let expires_at = limits.lease_expiry(write.now_ms)?;
    Ok((limits, kind, expires_at))
}

fn stage_attachment_media(
    transaction: &Transaction<'_>,
    write: &IngestAttachmentWrite,
    expires_at: i64,
) -> Result<()> {
    let inserted = transaction.execute(
        "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(sha256) DO NOTHING",
        params![
            &write.sha256,
            ZeroBlob(i32::try_from(write.byte_length).map_err(|_| {
                DatabaseError::InvalidInput("attachment is too large for SQLite blob I/O".into())
            })?),
            i64::try_from(write.byte_length).map_err(|_| {
                DatabaseError::InvalidInput("attachment byte length exceeds SQLite".into())
            })?,
            write.now_ms
        ],
    )?;
    let rowid: i64 = transaction.query_row(
        "SELECT rowid FROM media_blob WHERE sha256 = ?1",
        params![&write.sha256],
        |row| row.get(0),
    )?;
    if inserted == 1 {
        write_staged_blob(transaction, rowid, write)?;
    } else {
        validate_existing_blob(transaction, rowid, write)?;
    }
    transaction.execute(
        "INSERT INTO media_blob_lease(lease_id, sha256, created_at, expires_at)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(lease_id, sha256) DO UPDATE SET
            expires_at = max(expires_at, excluded.expires_at)",
        params![
            &write.ingest_lease_id,
            &write.sha256,
            write.now_ms,
            expires_at
        ],
    )?;
    Ok(())
}

fn insert_attachment_records(
    transaction: &Transaction<'_>,
    write: &IngestAttachmentWrite,
    limits: MediaLimits,
    kind: AttachmentKind,
    expires_at: i64,
) -> Result<()> {
    validate_draft_capacity(
        transaction,
        &write.draft_id,
        limits.max_attachments_per_draft,
    )?;
    transaction.execute(
        "INSERT INTO attachment(
            id, created_at, updated_at, sha256, display_filename,
            media_type, byte_length, kind, extraction_state
         ) VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &write.attachment_id,
            write.now_ms,
            &write.sha256,
            &write.display_filename,
            &write.media_type,
            i64::try_from(write.byte_length).expect("validated media length fits i64"),
            kind.as_db_str(),
            kind.extraction_state()
        ],
    )?;
    transaction.execute(
        "INSERT INTO media_ingest_lease(
            id, sha256, attachment_id, state, created_at, expires_at
         ) VALUES(?1, ?2, ?3, 'COMMITTED', ?4, ?5)",
        params![
            &write.ingest_lease_id,
            &write.sha256,
            &write.attachment_id,
            write.now_ms,
            expires_at
        ],
    )?;
    transaction.execute(
        "INSERT INTO draft_media_lease(draft_id, media_ingest_lease_id)
         VALUES(?1, ?2)",
        params![&write.draft_id, &write.ingest_lease_id],
    )?;
    transaction.execute(
        "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
        params![&write.sha256],
    )?;
    Ok(())
}

fn clear_committed_media_lease(
    media: &Connection,
    lease_id: &str,
    sha256: &[u8],
    attachment_id: &str,
    kind: &str,
) {
    if let Err(error) = media.execute(
        "DELETE FROM media_blob_lease WHERE lease_id = ?1 AND sha256 = ?2",
        params![lease_id, sha256],
    ) {
        log::warn!(
            "{kind} {attachment_id} committed but its staging lease could not be cleared: {error}"
        );
    }
}

fn attachment_record(write: IngestAttachmentWrite, kind: AttachmentKind) -> AttachmentRecord {
    AttachmentRecord {
        id: write.attachment_id,
        ingest_lease_id: write.ingest_lease_id,
        display_filename: write.display_filename,
        media_type: write.media_type,
        byte_length: write.byte_length,
        kind,
    }
}

pub(crate) fn ingest_image(
    main: &mut Connection,
    media: &mut Connection,
    mut write: IngestImageWrite,
) -> Result<ImageRecord> {
    validate_uuid_v7(&write.extraction_id, "extractionId")?;
    if write.preview.bytes.is_empty() {
        return Err(DatabaseError::InvalidInput(
            "canonical image preview is empty".into(),
        ));
    }
    if write.preview.natural_width == 0 || write.preview.natural_height == 0 {
        return Err(DatabaseError::InvalidInput(
            "canonical image preview has invalid dimensions".into(),
        ));
    }
    let (limits, kind, expires_at) = validate_attachment_ingest(main, &mut write.attachment)?;
    if kind != AttachmentKind::Image {
        return Err(DatabaseError::InvalidInput(
            "decoded images must use an image media type".into(),
        ));
    }
    let preview_byte_length = u64::try_from(write.preview.bytes.len())
        .map_err(|_| DatabaseError::InvalidInput("image preview is too large".into()))?;
    if preview_byte_length > limits.max_attachment_bytes {
        return Err(DatabaseError::InvalidInput(format!(
            "canonical image preview exceeds the {}-byte limit",
            limits.max_attachment_bytes
        )));
    }
    let attachment_id = write.attachment.attachment_id.clone();
    let ingest_lease_id = write.attachment.ingest_lease_id.clone();
    let now_ms = write.attachment.now_ms;
    let preview_hash = Sha256::digest(&write.preview.bytes).to_vec();
    let media_transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    stage_attachment_media(&media_transaction, &write.attachment, expires_at)?;
    insert_media_bytes(
        &media_transaction,
        &preview_hash,
        &write.preview.bytes,
        now_ms,
    )?;
    media_transaction.execute(
        "INSERT INTO media_blob_lease(lease_id, sha256, created_at, expires_at)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(lease_id, sha256) DO UPDATE SET
            expires_at = max(expires_at, excluded.expires_at)",
        params![&ingest_lease_id, &preview_hash, now_ms, expires_at],
    )?;
    media_transaction.commit()?;

    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    insert_attachment_records(&transaction, &write.attachment, limits, kind, expires_at)?;
    let extractor_version: String = transaction.query_row(
        "SELECT version
         FROM attachment_extractor_config
         WHERE extractor = ?1",
        params![IMAGE_OCR_EXTRACTOR],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO attachment_image(
            attachment_id, preview_sha256, preview_media_type,
            preview_byte_length, natural_width, natural_height, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &attachment_id,
            &preview_hash,
            IMAGE_PREVIEW_MEDIA_TYPE,
            i64::try_from(preview_byte_length).expect("validated preview byte length fits i64"),
            write.preview.natural_width,
            write.preview.natural_height,
            now_ms
        ],
    )?;
    transaction.execute(
        "INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, error, created_at, started_at, completed_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, 'PENDING', NULL, ?6, NULL, NULL)",
        params![
            &write.extraction_id,
            &attachment_id,
            IMAGE_OCR_EXTRACTOR,
            &extractor_version,
            &write.attachment.sha256,
            now_ms
        ],
    )?;
    transaction.execute(
        "INSERT INTO image_ocr_queue(
            extraction_id, state, attempt_count, next_attempt_at,
            started_at, last_error, updated_at
         ) VALUES(?1, 'PENDING', 0, ?2, NULL, NULL, ?2)",
        params![&write.extraction_id, now_ms],
    )?;
    transaction.execute(
        "DELETE FROM media_blob_reap_candidate
         WHERE sha256 IN (?1, ?2)",
        params![&write.attachment.sha256, &preview_hash],
    )?;
    transaction.commit()?;

    clear_committed_media_lease(
        media,
        &ingest_lease_id,
        &write.attachment.sha256,
        &attachment_id,
        "image original",
    );
    clear_committed_media_lease(
        media,
        &ingest_lease_id,
        &preview_hash,
        &attachment_id,
        "image preview",
    );

    Ok(ImageRecord {
        attachment: attachment_record(write.attachment, kind),
        natural_width: write.preview.natural_width,
        natural_height: write.preview.natural_height,
        ocr_status: ImageOcrStatus::Pending,
        ocr_error: None,
    })
}

pub(crate) fn ingest_pdf(
    main: &mut Connection,
    media: &mut Connection,
    mut write: IngestPdfWrite,
) -> Result<PdfRecord> {
    validate_uuid_v7(&write.extraction_id, "extractionId")?;
    if write.page_count == 0 || write.page_count > 2_000 {
        return Err(DatabaseError::InvalidInput(
            "PDFs must contain between 1 and 2000 pages".into(),
        ));
    }
    let (limits, kind, expires_at) = validate_attachment_ingest(main, &mut write.attachment)?;
    if kind != AttachmentKind::Pdf || write.attachment.media_type != "application/pdf" {
        return Err(DatabaseError::InvalidInput(
            "PDF ingestion requires the application/pdf media type".into(),
        ));
    }
    let attachment_id = write.attachment.attachment_id.clone();
    let ingest_lease_id = write.attachment.ingest_lease_id.clone();
    let now_ms = write.attachment.now_ms;
    let media_transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    stage_attachment_media(&media_transaction, &write.attachment, expires_at)?;
    media_transaction.commit()?;

    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    insert_attachment_records(&transaction, &write.attachment, limits, kind, expires_at)?;
    let extractor_version: String = transaction.query_row(
        "SELECT version
         FROM attachment_extractor_config
         WHERE extractor = ?1",
        params![PDF_TEXT_EXTRACTOR],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO attachment_pdf(attachment_id, page_count, created_at)
         VALUES(?1, ?2, ?3)",
        params![&attachment_id, write.page_count, now_ms],
    )?;
    transaction.execute(
        "INSERT INTO attachment_extraction(
            id, attachment_id, extractor, extractor_version, content_hash,
            status, error, created_at, started_at, completed_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, 'PENDING', NULL, ?6, NULL, NULL)",
        params![
            &write.extraction_id,
            &attachment_id,
            PDF_TEXT_EXTRACTOR,
            &extractor_version,
            &write.attachment.sha256,
            now_ms
        ],
    )?;
    transaction.execute(
        "INSERT INTO pdf_extraction_queue(
            extraction_id, state, attempt_count, next_attempt_at,
            started_at, last_error, updated_at
         ) VALUES(?1, 'PENDING', 0, ?2, NULL, NULL, ?2)",
        params![&write.extraction_id, now_ms],
    )?;
    transaction.commit()?;

    clear_committed_media_lease(
        media,
        &ingest_lease_id,
        &write.attachment.sha256,
        &attachment_id,
        "PDF",
    );
    Ok(PdfRecord {
        attachment: attachment_record(write.attachment, kind),
        page_count: write.page_count,
        extraction_status: PdfExtractionStatus::Pending,
        extraction_error: None,
    })
}

fn insert_media_bytes(
    transaction: &Transaction<'_>,
    sha256: &[u8],
    bytes: &[u8],
    now_ms: i64,
) -> Result<()> {
    if sha256.len() != 32 || Sha256::digest(bytes).as_slice() != sha256 {
        return Err(DatabaseError::InvalidInput(
            "canonical media digest does not match its bytes".into(),
        ));
    }
    let byte_length = i64::try_from(bytes.len())
        .map_err(|_| DatabaseError::InvalidInput("canonical media is too large".into()))?;
    transaction.execute(
        "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(sha256) DO NOTHING",
        params![sha256, bytes, byte_length, now_ms],
    )?;
    let (stored_length, stored_bytes): (i64, Vec<u8>) = transaction.query_row(
        "SELECT byte_length, bytes FROM media_blob WHERE sha256 = ?1",
        params![sha256],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored_length != byte_length || Sha256::digest(&stored_bytes).as_slice() != sha256 {
        return Err(DatabaseError::Validation {
            kind: "media",
            reason: "deduplicated canonical media does not match its digest".into(),
        });
    }
    Ok(())
}

pub(crate) fn load_media_payload(
    main: &Connection,
    media: &Connection,
    attachment_id: &str,
    now_ms: i64,
    requested_range: Option<MediaRangeRequest>,
    max_response_bytes: u64,
) -> Result<MediaPayload> {
    validate_uuid_v7(attachment_id, "attachmentId")?;
    validate_timestamp(now_ms, "nowMs")?;
    if max_response_bytes == 0 {
        return Err(DatabaseError::InvalidInput(
            "media response limit must be positive".into(),
        ));
    }
    let (sha256, media_type, byte_length, revision_bound) = main
        .query_row(
            "SELECT
                coalesce(image.preview_sha256, attachment.sha256),
                coalesce(image.preview_media_type, attachment.media_type),
                coalesce(image.preview_byte_length, attachment.byte_length),
                EXISTS (
                    SELECT 1
                    FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = attachment.id
                )
             FROM attachment
             LEFT JOIN attachment_image AS image
               ON image.attachment_id = attachment.id
             WHERE attachment.id = ?1
               AND attachment.deleted_at IS NULL
               AND (
                    EXISTS (
                        SELECT 1
                        FROM tidbit_revision_attachment AS membership
                        WHERE membership.attachment_id = attachment.id
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM media_ingest_lease AS lease
                        WHERE lease.attachment_id = attachment.id
                          AND lease.state = 'COMMITTED'
                          AND lease.expires_at > ?2
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM media_ingest_lease AS lease
                        JOIN draft_media_lease AS draft_lease
                          ON draft_lease.media_ingest_lease_id = lease.id
                        JOIN draft ON draft.id = draft_lease.draft_id
                        WHERE lease.attachment_id = attachment.id
                          AND lease.state = 'COMMITTED'
                          AND kosh_markdown_references_attachment(
                              draft.body_markdown,
                              attachment.id
                          )
                    )
               )",
            params![attachment_id, now_ms],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "attachment",
            id: attachment_id.into(),
        })?;
    let total_byte_length = u64::try_from(byte_length).map_err(|_| DatabaseError::Validation {
        kind: "main",
        reason: format!("attachment {attachment_id} has a negative byte length"),
    })?;
    if total_byte_length == 0 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: format!("attachment {attachment_id} is empty"),
        });
    }
    let range = match requested_range {
        Some(MediaRangeRequest::Inclusive(range)) => MediaByteRange {
            start: range.start,
            end_inclusive: range.end_inclusive.min(total_byte_length - 1),
        },
        Some(MediaRangeRequest::From(start)) => MediaByteRange {
            start,
            end_inclusive: total_byte_length - 1,
        },
        Some(MediaRangeRequest::Suffix(length)) => MediaByteRange {
            start: total_byte_length.saturating_sub(length),
            end_inclusive: total_byte_length - 1,
        },
        None => MediaByteRange {
            start: 0,
            end_inclusive: total_byte_length - 1,
        },
    };
    if range.start > range.end_inclusive || range.end_inclusive >= total_byte_length {
        return Err(DatabaseError::InvalidInput(
            "requested media range is outside the attachment".into(),
        ));
    }
    let response_length = range.end_inclusive - range.start + 1;
    if response_length > max_response_bytes {
        return Err(DatabaseError::InvalidInput(format!(
            "requested media response exceeds the {max_response_bytes}-byte limit"
        )));
    }

    let (rowid, stored_length) = media
        .query_row(
            "SELECT rowid, byte_length FROM media_blob WHERE sha256 = ?1",
            params![&sha256],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| DatabaseError::Validation {
            kind: "database pair",
            reason: format!("attachment {attachment_id} has no media blob"),
        })?;
    if stored_length != byte_length {
        return Err(DatabaseError::Validation {
            kind: "database pair",
            reason: format!(
                "attachment {attachment_id} expects {byte_length} bytes, media has {stored_length}"
            ),
        });
    }
    let mut blob = media.blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)?;
    let actual_digest = digest_reader(&mut blob)?;
    if actual_digest.as_slice() != sha256.as_slice() {
        return Err(DatabaseError::Validation {
            kind: "media",
            reason: format!("attachment {attachment_id} media bytes are corrupt"),
        });
    }
    blob.seek(SeekFrom::Start(range.start))?;
    let mut bytes = vec![
        0_u8;
        usize::try_from(response_length).map_err(|_| {
            DatabaseError::InvalidInput("requested media range does not fit memory".into())
        })?
    ];
    blob.read_exact(&mut bytes)?;
    Ok(MediaPayload {
        bytes,
        media_type,
        sha256,
        total_byte_length,
        range,
        revision_bound,
    })
}

pub(crate) fn load_image_status(
    main: &Connection,
    attachment_id: &str,
) -> Result<ImageStatusRecord> {
    validate_uuid_v7(attachment_id, "attachmentId")?;
    main.query_row(
        "SELECT
            image.attachment_id,
            image.natural_width,
            image.natural_height,
            queue.state,
            queue.last_error,
            queue.next_attempt_at
         FROM attachment_image AS image
         JOIN attachment ON attachment.id = image.attachment_id
         JOIN attachment_extraction AS extraction
           ON extraction.attachment_id = attachment.id
          AND extraction.content_hash = attachment.sha256
          AND extraction.extractor = ?2
         JOIN attachment_extractor_config AS config
           ON config.extractor = extraction.extractor
          AND config.version = extraction.extractor_version
         JOIN image_ocr_queue AS queue ON queue.extraction_id = extraction.id
         WHERE image.attachment_id = ?1
           AND attachment.deleted_at IS NULL",
        params![attachment_id, IMAGE_OCR_EXTRACTOR],
        |row| {
            let state = row.get::<_, String>(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                state,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        },
    )
    .optional()?
    .ok_or_else(|| DatabaseError::NotFound {
        entity: "image",
        id: attachment_id.into(),
    })
    .and_then(
        |(attachment_id, natural_width, natural_height, state, ocr_error, next_attempt_at_ms)| {
            Ok(ImageStatusRecord {
                attachment_id,
                natural_width,
                natural_height,
                ocr_status: ImageOcrStatus::from_db(&state)?,
                ocr_error,
                next_attempt_at_ms,
            })
        },
    )
}

pub(crate) fn load_pdf_status(main: &Connection, attachment_id: &str) -> Result<PdfStatusRecord> {
    validate_uuid_v7(attachment_id, "attachmentId")?;
    main.query_row(
        "SELECT
            pdf.attachment_id,
            attachment.display_filename,
            pdf.page_count,
            queue.state,
            queue.last_error,
            queue.next_attempt_at,
            count(page.page_number) FILTER (
                WHERE page.source IN ('NATIVE_TEXT', 'OCR')
            ),
            count(page.page_number) FILTER (
                WHERE page.source = 'UNAVAILABLE'
            )
         FROM attachment_pdf AS pdf
         JOIN attachment ON attachment.id = pdf.attachment_id
         JOIN attachment_extraction AS extraction
           ON extraction.attachment_id = attachment.id
          AND extraction.content_hash = attachment.sha256
          AND extraction.extractor = ?2
         JOIN attachment_extractor_config AS config
           ON config.extractor = extraction.extractor
          AND config.version = extraction.extractor_version
         JOIN pdf_extraction_queue AS queue ON queue.extraction_id = extraction.id
         LEFT JOIN pdf_page_extraction AS page ON page.extraction_id = extraction.id
         WHERE pdf.attachment_id = ?1
           AND attachment.deleted_at IS NULL
         GROUP BY
            pdf.attachment_id, attachment.display_filename, pdf.page_count, queue.state,
            queue.last_error, queue.next_attempt_at",
        params![attachment_id, PDF_TEXT_EXTRACTOR],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, u32>(7)?,
            ))
        },
    )
    .optional()?
    .ok_or_else(|| DatabaseError::NotFound {
        entity: "PDF",
        id: attachment_id.into(),
    })
    .and_then(
        |(
            attachment_id,
            display_filename,
            page_count,
            state,
            extraction_error,
            next_attempt_at_ms,
            extracted_page_count,
            unavailable_page_count,
        )| {
            Ok(PdfStatusRecord {
                attachment_id,
                display_filename,
                page_count,
                extracted_page_count,
                unavailable_page_count,
                extraction_status: PdfExtractionStatus::from_db(&state)?,
                extraction_error,
                next_attempt_at_ms,
            })
        },
    )
}

pub(crate) fn claim_next_pdf_extraction(
    main: &mut Connection,
    media: &Connection,
    now_ms: i64,
) -> Result<Option<PdfExtractionJob>> {
    validate_timestamp(now_ms, "nowMs")?;
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let pending = transaction
        .query_row(
            "SELECT
                extraction.id,
                extraction.attachment_id,
                extraction.extractor_version,
                extraction.content_hash,
                queue.attempt_count,
                pdf.page_count,
                attachment.byte_length
             FROM pdf_extraction_queue AS queue
             JOIN attachment_extraction AS extraction
               ON extraction.id = queue.extraction_id
             JOIN attachment_extractor_config AS config
               ON config.extractor = extraction.extractor
              AND config.version = extraction.extractor_version
             JOIN attachment
               ON attachment.id = extraction.attachment_id
              AND attachment.sha256 = extraction.content_hash
              AND attachment.deleted_at IS NULL
             JOIN attachment_pdf AS pdf ON pdf.attachment_id = attachment.id
             WHERE queue.state IN ('PENDING', 'RETRY_WAIT')
               AND queue.next_attempt_at <= ?1
               AND extraction.extractor = ?2
             ORDER BY queue.next_attempt_at, extraction.created_at, extraction.id
             LIMIT 1",
            params![now_ms, PDF_TEXT_EXTRACTOR],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((
        extraction_id,
        attachment_id,
        extractor_version,
        content_hash,
        attempt_count,
        page_count,
        byte_length,
    )) = pending
    else {
        transaction.commit()?;
        return Ok(None);
    };
    let pdf_bytes = load_media_blob_bytes(media, &content_hash, byte_length, &attachment_id)?;
    let next_attempt = attempt_count
        .checked_add(1)
        .ok_or_else(|| DatabaseError::Validation {
            kind: "main",
            reason: format!("PDF extraction attempt counter overflow for {attachment_id}"),
        })?;
    let updated = transaction.execute(
        "UPDATE pdf_extraction_queue
         SET state = 'RUNNING',
             attempt_count = ?1,
             next_attempt_at = NULL,
             started_at = ?2,
             updated_at = ?2
         WHERE extraction_id = ?3
           AND state IN ('PENDING', 'RETRY_WAIT')
           AND attempt_count = ?4",
        params![next_attempt, now_ms, &extraction_id, attempt_count],
    )?;
    if updated != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: format!("PDF extraction claim raced for {attachment_id}"),
        });
    }
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'RUNNING',
             error = NULL,
             started_at = coalesce(started_at, ?1),
             completed_at = NULL
         WHERE id = ?2",
        params![now_ms, &extraction_id],
    )?;
    transaction.commit()?;
    Ok(Some(PdfExtractionJob {
        extraction_id,
        attachment_id,
        extractor_version,
        content_hash,
        attempt_count: next_attempt,
        page_count,
        pdf_bytes,
    }))
}

pub(crate) fn complete_pdf_extraction(
    main: &mut Connection,
    job: &PdfExtractionJob,
    result: std::result::Result<Vec<PdfPageExtraction>, String>,
    completed_at_ms: i64,
) -> Result<()> {
    validate_timestamp(completed_at_ms, "completedAtMs")?;
    validate_uuid_v7(&job.extraction_id, "extractionId")?;
    validate_uuid_v7(&job.attachment_id, "attachmentId")?;
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM pdf_extraction_queue AS queue
            JOIN attachment_extraction AS extraction
              ON extraction.id = queue.extraction_id
            JOIN attachment_extractor_config AS config
              ON config.extractor = extraction.extractor
             AND config.version = extraction.extractor_version
            JOIN attachment
              ON attachment.id = extraction.attachment_id
             AND attachment.sha256 = extraction.content_hash
             AND attachment.deleted_at IS NULL
            JOIN attachment_pdf AS pdf ON pdf.attachment_id = attachment.id
            WHERE queue.extraction_id = ?1
              AND queue.state = 'RUNNING'
              AND queue.attempt_count = ?2
              AND extraction.attachment_id = ?3
              AND extraction.extractor = ?4
              AND extraction.extractor_version = ?5
              AND extraction.content_hash = ?6
              AND pdf.page_count = ?7
         )",
        params![
            &job.extraction_id,
            job.attempt_count,
            &job.attachment_id,
            PDF_TEXT_EXTRACTOR,
            &job.extractor_version,
            &job.content_hash,
            job.page_count
        ],
        |row| row.get(0),
    )?;
    if !current {
        let retired: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM pdf_extraction_queue AS queue
                JOIN attachment_extraction AS extraction
                  ON extraction.id = queue.extraction_id
                JOIN attachment ON attachment.id = extraction.attachment_id
                WHERE queue.extraction_id = ?1
                  AND queue.state = 'FAILED'
                  AND queue.last_error = ?2
                  AND extraction.attachment_id = ?3
                  AND attachment.deleted_at IS NOT NULL
             )",
            params![&job.extraction_id, RETIRED_PDF_ERROR, &job.attachment_id],
            |row| row.get(0),
        )?;
        if retired {
            return Ok(());
        }
        return Err(DatabaseError::InvalidInput(format!(
            "PDF extraction result for {} is stale",
            job.attachment_id
        )));
    }

    match result.and_then(|pages| validate_pdf_pages(pages, job.page_count)) {
        Ok(pages) => {
            let construction_version: String = transaction.query_row(
                "SELECT passage_construction_version
                 FROM attachment_extractor_config
                 WHERE extractor = ?1 AND version = ?2",
                params![PDF_TEXT_EXTRACTOR, &job.extractor_version],
                |row| row.get(0),
            )?;
            transaction.execute(
                "UPDATE attachment_extraction
                 SET status = 'READY', error = NULL, completed_at = ?1
                 WHERE id = ?2",
                params![completed_at_ms, &job.extraction_id],
            )?;
            let mut segment_ordinal = 0_i64;
            for page in pages {
                match page.result {
                    Ok((source, text)) => {
                        let locator_json = serde_json::to_string(&serde_json::json!({
                            "page": page.page_number
                        }))
                        .expect("PDF page locator is serializable");
                        let chunks = split_pdf_page_passages(&text);
                        let mut first_segment_id = None;
                        for chunk in chunks {
                            let segment_id = Uuid::now_v7().to_string();
                            let passage_id = Uuid::now_v7().to_string();
                            let content_hash = Sha256::digest(chunk.as_bytes()).to_vec();
                            transaction.execute(
                                "INSERT INTO attachment_segment(
                                    id, extraction_id, ordinal, locator_kind, page_number,
                                    line_start, line_end, region_json, content, content_hash
                                 ) VALUES(
                                    ?1, ?2, ?3, 'PDF_PAGE', ?4, NULL, NULL, NULL, ?5, ?6
                                 )",
                                params![
                                    &segment_id,
                                    &job.extraction_id,
                                    segment_ordinal,
                                    page.page_number,
                                    &chunk,
                                    &content_hash
                                ],
                            )?;
                            transaction.execute(
                                "INSERT INTO passage(
                                    id, tidbit_revision_id, attachment_segment_id, owner_kind,
                                    ordinal, content, content_hash, locator_kind, locator_json,
                                    created_at, construction_version, heading_context_json
                                 ) VALUES(
                                    ?1, NULL, ?2, 'ATTACHMENT', ?3, ?4, ?5, 'PDF_PAGE',
                                    ?6, ?7, ?8, '[]'
                                 )",
                                params![
                                    &passage_id,
                                    &segment_id,
                                    segment_ordinal,
                                    &chunk,
                                    &content_hash,
                                    &locator_json,
                                    completed_at_ms,
                                    &construction_version
                                ],
                            )?;
                            first_segment_id.get_or_insert(segment_id);
                            segment_ordinal = segment_ordinal.checked_add(1).ok_or_else(|| {
                                DatabaseError::InvalidInput("PDF passage ordinal overflow".into())
                            })?;
                        }
                        transaction.execute(
                            "INSERT INTO pdf_page_extraction(
                                extraction_id, page_number, source, segment_id, error
                             ) VALUES(?1, ?2, ?3, ?4, NULL)",
                            params![
                                &job.extraction_id,
                                page.page_number,
                                source.as_db_str(),
                                first_segment_id.expect("validated PDF page has a passage")
                            ],
                        )?;
                    }
                    Err(error) => {
                        transaction.execute(
                            "INSERT INTO pdf_page_extraction(
                                extraction_id, page_number, source, segment_id, error
                             ) VALUES(?1, ?2, 'UNAVAILABLE', NULL, ?3)",
                            params![
                                &job.extraction_id,
                                page.page_number,
                                truncate_ocr_error(&error)
                            ],
                        )?;
                    }
                }
            }
            transaction.execute(
                "UPDATE pdf_extraction_queue
                 SET state = 'READY', next_attempt_at = NULL, started_at = NULL,
                     last_error = NULL, updated_at = ?1
                 WHERE extraction_id = ?2",
                params![completed_at_ms, &job.extraction_id],
            )?;
            transaction.execute(
                "UPDATE attachment
                 SET extraction_state = 'READY', updated_at = max(updated_at, ?1)
                 WHERE id = ?2",
                params![completed_at_ms, &job.attachment_id],
            )?;
            search::replace_attachment_documents_in_transaction(&transaction, &job.attachment_id)?;
        }
        Err(error) => {
            let error = truncate_ocr_error(&error);
            if job.attempt_count < MAX_PDF_EXTRACTION_ATTEMPTS {
                let retry_at = checked_timestamp_add(
                    completed_at_ms,
                    5 * 60 * 1_000 * i64::from(job.attempt_count),
                    "PDF extraction retry time",
                )?;
                transaction.execute(
                    "UPDATE attachment_extraction
                     SET status = 'PENDING', error = ?1, completed_at = NULL
                     WHERE id = ?2",
                    params![&error, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE pdf_extraction_queue
                     SET state = 'RETRY_WAIT', next_attempt_at = ?1, started_at = NULL,
                         last_error = ?2, updated_at = ?3
                     WHERE extraction_id = ?4",
                    params![retry_at, &error, completed_at_ms, &job.extraction_id],
                )?;
            } else {
                transaction.execute(
                    "UPDATE attachment_extraction
                     SET status = 'FAILED', error = ?1, completed_at = ?2
                     WHERE id = ?3",
                    params![&error, completed_at_ms, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE pdf_extraction_queue
                     SET state = 'FAILED', next_attempt_at = NULL, started_at = NULL,
                         last_error = ?1, updated_at = ?2
                     WHERE extraction_id = ?3",
                    params![&error, completed_at_ms, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE attachment
                     SET extraction_state = 'FAILED', updated_at = max(updated_at, ?1)
                     WHERE id = ?2",
                    params![completed_at_ms, &job.attachment_id],
                )?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

fn validate_pdf_pages(
    mut pages: Vec<PdfPageExtraction>,
    page_count: u32,
) -> std::result::Result<Vec<PdfPageExtraction>, String> {
    if pages.len() != page_count as usize {
        return Err(format!(
            "PDF extractor returned {} page outcomes for a {page_count}-page document",
            pages.len()
        ));
    }
    pages.sort_by_key(|page| page.page_number);
    let mut total_chars = 0_usize;
    for (index, page) in pages.iter_mut().enumerate() {
        let expected = u32::try_from(index + 1).expect("PDF page bound fits u32");
        if page.page_number != expected {
            return Err("PDF extractor returned duplicate or missing page outcomes".into());
        }
        if let Ok((_, text)) = &mut page.result {
            *text = text.trim().to_owned();
            let chars = text.chars().count();
            if chars == 0 {
                page.result = Err("No searchable text could be extracted from this page".into());
            } else if chars > 100_000 {
                return Err(format!(
                    "PDF page {expected} contains too much extracted text"
                ));
            } else {
                total_chars = total_chars
                    .checked_add(chars)
                    .ok_or_else(|| "PDF text length overflow".to_owned())?;
            }
        } else if let Err(error) = &mut page.result {
            *error = truncate_ocr_error(error);
            if error.is_empty() {
                *error = "No searchable text could be extracted from this page".into();
            }
        }
    }
    if total_chars > 4_000_000 {
        return Err("PDF contains too much extracted text".into());
    }
    Ok(pages)
}

pub(crate) fn split_pdf_page_passages(text: &str) -> Vec<String> {
    let characters = text.chars().collect::<Vec<_>>();
    let mut passages = Vec::new();
    let mut start = 0_usize;
    while start < characters.len() {
        let raw_end = if characters.len() - start <= PDF_PASSAGE_MAX_CHARS {
            characters.len()
        } else {
            pdf_passage_boundary(&characters, start)
        };
        let mut trimmed_start = start;
        while trimmed_start < raw_end && characters[trimmed_start].is_whitespace() {
            trimmed_start += 1;
        }
        let mut trimmed_end = raw_end;
        while trimmed_end > trimmed_start && characters[trimmed_end - 1].is_whitespace() {
            trimmed_end -= 1;
        }
        if trimmed_start < trimmed_end {
            passages.push(characters[trimmed_start..trimmed_end].iter().collect());
        }
        if raw_end == characters.len() {
            break;
        }
        let overlap_target = raw_end.saturating_sub(PDF_PASSAGE_OVERLAP_CHARS);
        let next = (overlap_target..raw_end)
            .find(|position| characters[*position].is_whitespace())
            .map_or(overlap_target, |position| position + 1);
        start = next.max(start + 1);
    }
    passages
}

fn pdf_passage_boundary(characters: &[char], start: usize) -> usize {
    let target = (start + PDF_PASSAGE_TARGET_CHARS).min(characters.len());
    let maximum = (start + PDF_PASSAGE_MAX_CHARS).min(characters.len());
    let minimum = (start + PDF_PASSAGE_TARGET_CHARS / 2).min(maximum);
    (minimum..maximum)
        .filter(|position| {
            *position > 0
                && *position < characters.len()
                && matches!(characters[*position - 1], '.' | '!' | '?' | '\n')
                && characters[*position].is_whitespace()
        })
        .min_by_key(|position| position.abs_diff(target))
        .or_else(|| {
            (target..maximum)
                .rev()
                .find(|position| characters[*position].is_whitespace())
        })
        .unwrap_or(maximum)
}

pub(crate) fn retry_pdf_extraction(
    main: &mut Connection,
    attachment_id: &str,
    now_ms: i64,
) -> Result<PdfStatusRecord> {
    validate_uuid_v7(attachment_id, "attachmentId")?;
    validate_timestamp(now_ms, "nowMs")?;
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let extraction_id = transaction
        .query_row(
            "SELECT extraction.id
             FROM attachment_extraction AS extraction
             JOIN attachment_extractor_config AS config
               ON config.extractor = extraction.extractor
              AND config.version = extraction.extractor_version
             JOIN attachment
               ON attachment.id = extraction.attachment_id
              AND attachment.sha256 = extraction.content_hash
              AND attachment.deleted_at IS NULL
             JOIN pdf_extraction_queue AS queue ON queue.extraction_id = extraction.id
             WHERE attachment.id = ?1
               AND extraction.extractor = ?2
               AND queue.state = 'FAILED'",
            params![attachment_id, PDF_TEXT_EXTRACTOR],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            DatabaseError::InvalidInput(
                "only a current, failed PDF extraction can be retried".into(),
            )
        })?;
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'PENDING', error = NULL, started_at = NULL, completed_at = NULL
         WHERE id = ?1",
        params![&extraction_id],
    )?;
    transaction.execute(
        "UPDATE pdf_extraction_queue
         SET state = 'PENDING', attempt_count = 0, next_attempt_at = ?1,
             started_at = NULL, last_error = NULL, updated_at = ?1
         WHERE extraction_id = ?2",
        params![now_ms, &extraction_id],
    )?;
    transaction.execute(
        "UPDATE attachment
         SET extraction_state = 'PENDING', updated_at = max(updated_at, ?1)
         WHERE id = ?2",
        params![now_ms, attachment_id],
    )?;
    transaction.commit()?;
    load_pdf_status(main, attachment_id)
}

pub(crate) fn recover_interrupted_pdf_extraction(
    main: &mut Connection,
    stale_started_at_or_before: i64,
    now_ms: i64,
) -> Result<u64> {
    validate_timestamp(stale_started_at_or_before, "staleStartedAtOrBefore")?;
    validate_timestamp(now_ms, "nowMs")?;
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'FAILED',
             error = 'PDF extractor provenance is no longer current',
             completed_at = max(coalesce(started_at, created_at), ?1)
         WHERE extractor = ?2
           AND status <> 'READY'
           AND EXISTS (
                SELECT 1
                FROM attachment_extractor_config AS config
                JOIN attachment ON attachment.id = attachment_extraction.attachment_id
                WHERE config.extractor = attachment_extraction.extractor
                  AND (
                       config.version <> attachment_extraction.extractor_version
                       OR attachment.sha256 <> attachment_extraction.content_hash
                  )
           )",
        params![now_ms, PDF_TEXT_EXTRACTOR],
    )?;
    transaction.execute(
        "UPDATE pdf_extraction_queue
         SET state = 'FAILED',
             attempt_count = max(attempt_count, 1),
             next_attempt_at = NULL,
             started_at = NULL,
             last_error = 'PDF extractor provenance is no longer current',
             updated_at = ?1
         WHERE state IN ('PENDING', 'RUNNING', 'RETRY_WAIT')
           AND extraction_id IN (
                SELECT extraction.id
                FROM attachment_extraction AS extraction
                JOIN attachment_extractor_config AS config
                  ON config.extractor = extraction.extractor
                JOIN attachment ON attachment.id = extraction.attachment_id
                WHERE extraction.extractor = ?2
                  AND (
                       config.version <> extraction.extractor_version
                       OR attachment.sha256 <> extraction.content_hash
                  )
           )",
        params![now_ms, PDF_TEXT_EXTRACTOR],
    )?;
    let replacement_query = format!(
        "SELECT attachment.id, attachment.sha256, config.version
             FROM attachment
             JOIN attachment_pdf AS pdf ON pdf.attachment_id = attachment.id
             JOIN attachment_extractor_config AS config ON config.extractor = ?1
             WHERE attachment.deleted_at IS NULL
               AND NOT EXISTS (
                    SELECT 1
                    FROM attachment_extraction AS extraction
                    JOIN pdf_extraction_queue AS queue
                      ON queue.extraction_id = extraction.id
                    WHERE extraction.attachment_id = attachment.id
                      AND extraction.extractor = config.extractor
                      AND extraction.extractor_version = config.version
                      AND extraction.content_hash = attachment.sha256
               )
             ORDER BY attachment.id
             LIMIT {PDF_RECOVERY_BATCH_SIZE}"
    );
    let replacements = transaction
        .prepare(&replacement_query)?
        .query_map(params![PDF_TEXT_EXTRACTOR], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let replacement_count = replacements.len() as u64;
    for (attachment_id, content_hash, extractor_version) in replacements {
        let extraction_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO attachment_extraction(
                id, attachment_id, extractor, extractor_version, content_hash,
                status, error, created_at, started_at, completed_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, 'PENDING', NULL, ?6, NULL, NULL)",
            params![
                &extraction_id,
                &attachment_id,
                PDF_TEXT_EXTRACTOR,
                &extractor_version,
                &content_hash,
                now_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO pdf_extraction_queue(
                extraction_id, state, attempt_count, next_attempt_at,
                started_at, last_error, updated_at
             ) VALUES(?1, 'PENDING', 0, ?2, NULL, NULL, ?2)",
            params![&extraction_id, now_ms],
        )?;
        transaction.execute(
            "UPDATE attachment
             SET extraction_state = 'PENDING', updated_at = max(updated_at, ?1)
             WHERE id = ?2",
            params![now_ms, &attachment_id],
        )?;
    }
    let updated = transaction.execute(
        "UPDATE pdf_extraction_queue
         SET state = CASE
                WHEN attempt_count >= ?1 THEN 'FAILED'
                ELSE 'RETRY_WAIT'
             END,
             next_attempt_at = CASE
                WHEN attempt_count >= ?1 THEN NULL
                ELSE ?2
             END,
             started_at = NULL,
             last_error = 'PDF extraction was interrupted before it completed',
             updated_at = ?2
         WHERE state = 'RUNNING'
           AND started_at <= ?3
           AND extraction_id IN (
                SELECT extraction.id
                FROM attachment_extraction AS extraction
                JOIN attachment ON attachment.id = extraction.attachment_id
                WHERE extraction.extractor = ?4
                  AND attachment.deleted_at IS NULL
           )",
        params![
            MAX_PDF_EXTRACTION_ATTEMPTS,
            now_ms,
            stale_started_at_or_before,
            PDF_TEXT_EXTRACTOR
        ],
    )?;
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = CASE
                WHEN (
                    SELECT state FROM pdf_extraction_queue
                    WHERE extraction_id = attachment_extraction.id
                ) = 'FAILED' THEN 'FAILED'
                ELSE 'PENDING'
             END,
             error = 'PDF extraction was interrupted before it completed',
             completed_at = CASE
                WHEN (
                    SELECT state FROM pdf_extraction_queue
                    WHERE extraction_id = attachment_extraction.id
                ) = 'FAILED' THEN ?1
                ELSE NULL
             END
         WHERE id IN (
            SELECT extraction_id
            FROM pdf_extraction_queue
            WHERE updated_at = ?1
              AND last_error = 'PDF extraction was interrupted before it completed'
         )",
        params![now_ms],
    )?;
    transaction.commit()?;
    Ok(updated as u64 + replacement_count)
}

pub(crate) fn claim_next_image_ocr(
    main: &mut Connection,
    media: &Connection,
    now_ms: i64,
) -> Result<Option<ImageOcrJob>> {
    validate_timestamp(now_ms, "nowMs")?;
    loop {
        let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pending = transaction
            .query_row(
                "SELECT
                    extraction.id,
                    extraction.attachment_id,
                    extraction.extractor_version,
                    extraction.content_hash,
                    queue.attempt_count,
                    image.preview_sha256,
                    image.preview_byte_length
                 FROM image_ocr_queue AS queue
                 JOIN attachment_extraction AS extraction
                   ON extraction.id = queue.extraction_id
                 JOIN attachment_extractor_config AS config
                   ON config.extractor = extraction.extractor
                  AND config.version = extraction.extractor_version
                 JOIN attachment
                   ON attachment.id = extraction.attachment_id
                  AND attachment.sha256 = extraction.content_hash
                  AND attachment.deleted_at IS NULL
                 JOIN attachment_image AS image
                   ON image.attachment_id = attachment.id
                 WHERE queue.state IN ('PENDING', 'RETRY_WAIT')
                   AND queue.next_attempt_at <= ?1
                   AND extraction.extractor = ?2
                 ORDER BY queue.next_attempt_at, extraction.created_at, extraction.id
                 LIMIT 1",
                params![now_ms, IMAGE_OCR_EXTRACTOR],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, u32>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            extraction_id,
            attachment_id,
            extractor_version,
            content_hash,
            attempt_count,
            preview_hash,
            preview_byte_length,
        )) = pending
        else {
            transaction.commit()?;
            return Ok(None);
        };
        let next_attempt =
            attempt_count
                .checked_add(1)
                .ok_or_else(|| DatabaseError::Validation {
                    kind: "main",
                    reason: format!("OCR attempt counter overflow for {attachment_id}"),
                })?;
        let preview_bytes = match load_media_blob_bytes(
            media,
            &preview_hash,
            preview_byte_length,
            &attachment_id,
        ) {
            Ok(bytes) => bytes,
            Err(error @ DatabaseError::Validation { .. }) => {
                let error = truncate_ocr_error(&error.to_string());
                quarantine_unreadable_image_ocr(
                    &transaction,
                    &extraction_id,
                    &attachment_id,
                    &error,
                    now_ms,
                )?;
                transaction.commit()?;
                log::warn!("quarantined image OCR job for {attachment_id}: {error}");
                continue;
            }
            Err(error) => return Err(error),
        };
        let updated = transaction.execute(
            "UPDATE image_ocr_queue
             SET state = 'RUNNING',
                 attempt_count = ?1,
                 next_attempt_at = NULL,
                 started_at = ?2,
                 updated_at = ?2
             WHERE extraction_id = ?3
               AND state IN ('PENDING', 'RETRY_WAIT')
               AND attempt_count = ?4",
            params![next_attempt, now_ms, &extraction_id, attempt_count],
        )?;
        if updated != 1 {
            return Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("OCR claim raced for image {attachment_id}"),
            });
        }
        transaction.execute(
            "UPDATE attachment_extraction
             SET status = 'RUNNING',
                 error = NULL,
                 started_at = coalesce(started_at, ?1),
                 completed_at = NULL
             WHERE id = ?2",
            params![now_ms, &extraction_id],
        )?;
        transaction.commit()?;
        return Ok(Some(ImageOcrJob {
            extraction_id,
            attachment_id,
            extractor_version,
            content_hash,
            attempt_count: next_attempt,
            preview_bytes,
        }));
    }
}

fn quarantine_unreadable_image_ocr(
    transaction: &Transaction<'_>,
    extraction_id: &str,
    attachment_id: &str,
    error: &str,
    now_ms: i64,
) -> Result<()> {
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'FAILED', error = ?1, completed_at = ?2
         WHERE id = ?3",
        params![error, now_ms, extraction_id],
    )?;
    transaction.execute(
        "UPDATE image_ocr_queue
         SET state = 'FAILED',
             attempt_count = max(attempt_count, 1),
             next_attempt_at = NULL,
             started_at = NULL,
             last_error = ?1,
             updated_at = ?2
         WHERE extraction_id = ?3",
        params![error, now_ms, extraction_id],
    )?;
    transaction.execute(
        "UPDATE attachment
         SET extraction_state = 'FAILED',
             updated_at = max(updated_at, ?1)
         WHERE id = ?2",
        params![now_ms, attachment_id],
    )?;
    Ok(())
}

fn load_media_blob_bytes(
    media: &Connection,
    sha256: &[u8],
    expected_length: i64,
    attachment_id: &str,
) -> Result<Vec<u8>> {
    if expected_length <= 0 || expected_length > MAX_DIRECT_ATTACHMENT_BYTES as i64 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: format!("image {attachment_id} has an invalid preview length"),
        });
    }
    let (rowid, stored_length) = media
        .query_row(
            "SELECT rowid, byte_length FROM media_blob WHERE sha256 = ?1",
            params![sha256],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| DatabaseError::Validation {
            kind: "database pair",
            reason: format!("image {attachment_id} has no canonical preview"),
        })?;
    if stored_length != expected_length {
        return Err(DatabaseError::Validation {
            kind: "database pair",
            reason: format!("image {attachment_id} preview length does not match metadata"),
        });
    }
    let mut blob = media.blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)?;
    let actual = digest_reader(&mut blob)?;
    if actual.as_slice() != sha256 {
        return Err(DatabaseError::Validation {
            kind: "media",
            reason: format!("image {attachment_id} preview is corrupt"),
        });
    }
    blob.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![
        0;
        usize::try_from(expected_length).map_err(|_| {
            DatabaseError::Validation {
                kind: "main",
                reason: format!("image {attachment_id} preview does not fit memory"),
            }
        })?
    ];
    blob.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn complete_image_ocr(
    main: &mut Connection,
    job: &ImageOcrJob,
    result: std::result::Result<Vec<ImageOcrRegion>, String>,
    completed_at_ms: i64,
) -> Result<()> {
    validate_timestamp(completed_at_ms, "completedAtMs")?;
    validate_uuid_v7(&job.extraction_id, "extractionId")?;
    validate_uuid_v7(&job.attachment_id, "attachmentId")?;
    if job.content_hash.len() != 32 {
        return Err(DatabaseError::InvalidInput(
            "OCR content hash must contain 32 bytes".into(),
        ));
    }
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM image_ocr_queue AS queue
            JOIN attachment_extraction AS extraction
              ON extraction.id = queue.extraction_id
            JOIN attachment_extractor_config AS config
              ON config.extractor = extraction.extractor
             AND config.version = extraction.extractor_version
            JOIN attachment
              ON attachment.id = extraction.attachment_id
             AND attachment.sha256 = extraction.content_hash
             AND attachment.deleted_at IS NULL
            WHERE queue.extraction_id = ?1
              AND queue.state = 'RUNNING'
              AND queue.attempt_count = ?2
              AND extraction.attachment_id = ?3
              AND extraction.extractor = ?4
              AND extraction.extractor_version = ?5
              AND extraction.content_hash = ?6
         )",
        params![
            &job.extraction_id,
            job.attempt_count,
            &job.attachment_id,
            IMAGE_OCR_EXTRACTOR,
            &job.extractor_version,
            &job.content_hash
        ],
        |row| row.get(0),
    )?;
    if !current {
        let retired: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM image_ocr_queue AS queue
                JOIN attachment_extraction AS extraction
                  ON extraction.id = queue.extraction_id
                JOIN attachment
                  ON attachment.id = extraction.attachment_id
                WHERE queue.extraction_id = ?1
                  AND queue.state = 'FAILED'
                  AND queue.last_error = ?2
                  AND extraction.attachment_id = ?3
                  AND attachment.deleted_at IS NOT NULL
             )",
            params![&job.extraction_id, RETIRED_OCR_ERROR, &job.attachment_id],
            |row| row.get(0),
        )?;
        if retired {
            return Ok(());
        }
        return Err(DatabaseError::InvalidInput(format!(
            "OCR result for image {} is stale",
            job.attachment_id
        )));
    }

    let result =
        result.and_then(|regions| validate_ocr_regions(regions).map_err(|error| error.to_string()));
    match result {
        Ok(regions) => {
            let construction_version: String = transaction.query_row(
                "SELECT passage_construction_version
                 FROM attachment_extractor_config
                 WHERE extractor = ?1 AND version = ?2",
                params![IMAGE_OCR_EXTRACTOR, &job.extractor_version],
                |row| row.get(0),
            )?;
            for (ordinal, region) in regions.into_iter().enumerate() {
                let segment_id = Uuid::now_v7().to_string();
                let passage_id = Uuid::now_v7().to_string();
                let region_value = serde_json::json!({
                    "coordinateSystem": "vision-normalized-bottom-left",
                    "height": region.height,
                    "width": region.width,
                    "x": region.x,
                    "y": region.y,
                });
                let region_json = serde_json::to_string(&region_value).map_err(|error| {
                    DatabaseError::InvalidInput(format!("could not serialize OCR region: {error}"))
                })?;
                let locator_json = serde_json::to_string(
                    &serde_json::json!({ "region": region_value }),
                )
                .map_err(|error| {
                    DatabaseError::InvalidInput(format!("could not serialize OCR locator: {error}"))
                })?;
                let content_hash = Sha256::digest(region.text.as_bytes()).to_vec();
                transaction.execute(
                    "INSERT INTO attachment_segment(
                        id, extraction_id, ordinal, locator_kind, page_number,
                        line_start, line_end, region_json, content, content_hash
                     ) VALUES(?1, ?2, ?3, 'OCR_REGION', NULL, NULL, NULL, ?4, ?5, ?6)",
                    params![
                        &segment_id,
                        &job.extraction_id,
                        i64::try_from(ordinal).expect("bounded OCR ordinal fits i64"),
                        &region_json,
                        &region.text,
                        &content_hash
                    ],
                )?;
                transaction.execute(
                    "UPDATE attachment_extraction
                     SET status = 'READY', error = NULL, completed_at = ?1
                     WHERE id = ?2",
                    params![completed_at_ms, &job.extraction_id],
                )?;
                transaction.execute(
                    "INSERT INTO passage(
                        id, tidbit_revision_id, attachment_segment_id, owner_kind,
                        ordinal, content, content_hash, locator_kind, locator_json,
                        created_at, construction_version, heading_context_json
                     ) VALUES(
                        ?1, NULL, ?2, 'ATTACHMENT', 0, ?3, ?4, 'OCR_REGION',
                        ?5, ?6, ?7, '[]'
                     )",
                    params![
                        &passage_id,
                        &segment_id,
                        &region.text,
                        &content_hash,
                        &locator_json,
                        completed_at_ms,
                        &construction_version
                    ],
                )?;
            }
            transaction.execute(
                "UPDATE attachment_extraction
                 SET status = 'READY', error = NULL, completed_at = ?1
                 WHERE id = ?2",
                params![completed_at_ms, &job.extraction_id],
            )?;
            transaction.execute(
                "UPDATE image_ocr_queue
                 SET state = 'READY',
                     next_attempt_at = NULL,
                     started_at = NULL,
                     last_error = NULL,
                     updated_at = ?1
                 WHERE extraction_id = ?2",
                params![completed_at_ms, &job.extraction_id],
            )?;
            transaction.execute(
                "UPDATE attachment
                 SET extraction_state = 'READY',
                     updated_at = max(updated_at, ?1)
                 WHERE id = ?2",
                params![completed_at_ms, &job.attachment_id],
            )?;
        }
        Err(error) => {
            let error = truncate_ocr_error(&error);
            if job.attempt_count < MAX_IMAGE_OCR_ATTEMPTS {
                let retry_at = checked_timestamp_add(
                    completed_at_ms,
                    image_ocr_retry_delay(job.attempt_count)?,
                    "OCR retry time",
                )?;
                transaction.execute(
                    "UPDATE attachment_extraction
                     SET status = 'PENDING', error = ?1, completed_at = NULL
                     WHERE id = ?2",
                    params![&error, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE image_ocr_queue
                     SET state = 'RETRY_WAIT',
                         next_attempt_at = ?1,
                         started_at = NULL,
                         last_error = ?2,
                         updated_at = ?3
                     WHERE extraction_id = ?4",
                    params![retry_at, &error, completed_at_ms, &job.extraction_id],
                )?;
            } else {
                transaction.execute(
                    "UPDATE attachment_extraction
                     SET status = 'FAILED', error = ?1, completed_at = ?2
                     WHERE id = ?3",
                    params![&error, completed_at_ms, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE image_ocr_queue
                     SET state = 'FAILED',
                         next_attempt_at = NULL,
                         started_at = NULL,
                         last_error = ?1,
                         updated_at = ?2
                     WHERE extraction_id = ?3",
                    params![&error, completed_at_ms, &job.extraction_id],
                )?;
                transaction.execute(
                    "UPDATE attachment
                     SET extraction_state = 'FAILED',
                         updated_at = max(updated_at, ?1)
                     WHERE id = ?2",
                    params![completed_at_ms, &job.attachment_id],
                )?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

fn validate_ocr_regions(regions: Vec<ImageOcrRegion>) -> Result<Vec<ImageOcrRegion>> {
    if regions.len() > MAX_OCR_REGIONS {
        return Err(DatabaseError::InvalidInput(format!(
            "OCR returned more than {MAX_OCR_REGIONS} regions"
        )));
    }
    let mut normalized = Vec::with_capacity(regions.len());
    let mut total_chars = 0_usize;
    for mut region in regions {
        region.text = region.text.trim().to_owned();
        if region.text.is_empty() {
            continue;
        }
        let chars = region.text.chars().count();
        if chars > MAX_OCR_REGION_CHARS {
            return Err(DatabaseError::InvalidInput(
                "an OCR region contains too much text".into(),
            ));
        }
        total_chars = total_chars
            .checked_add(chars)
            .ok_or_else(|| DatabaseError::InvalidInput("OCR text length overflow".into()))?;
        if total_chars > MAX_OCR_TOTAL_CHARS {
            return Err(DatabaseError::InvalidInput(
                "OCR returned too much text".into(),
            ));
        }
        let values = [region.x, region.y, region.width, region.height];
        if values.iter().any(|value| !value.is_finite())
            || region.x < 0.0
            || region.y < 0.0
            || region.width <= 0.0
            || region.height <= 0.0
            || region.x + region.width > 1.000_001
            || region.y + region.height > 1.000_001
        {
            return Err(DatabaseError::InvalidInput(
                "OCR returned an invalid normalized image region".into(),
            ));
        }
        normalized.push(region);
    }
    Ok(normalized)
}

pub(crate) fn retry_image_ocr(
    main: &mut Connection,
    attachment_id: &str,
    now_ms: i64,
) -> Result<ImageStatusRecord> {
    validate_uuid_v7(attachment_id, "attachmentId")?;
    validate_timestamp(now_ms, "nowMs")?;
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let extraction_id = transaction
        .query_row(
            "SELECT extraction.id
             FROM attachment_extraction AS extraction
             JOIN attachment_extractor_config AS config
               ON config.extractor = extraction.extractor
              AND config.version = extraction.extractor_version
             JOIN attachment
               ON attachment.id = extraction.attachment_id
              AND attachment.sha256 = extraction.content_hash
              AND attachment.deleted_at IS NULL
             JOIN image_ocr_queue AS queue ON queue.extraction_id = extraction.id
             WHERE attachment.id = ?1
               AND extraction.extractor = ?2
               AND queue.state = 'FAILED'",
            params![attachment_id, IMAGE_OCR_EXTRACTOR],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            DatabaseError::InvalidInput(
                "only a current, failed image OCR job can be retried".into(),
            )
        })?;
    transaction.execute(
        "UPDATE attachment_extraction
         SET status = 'PENDING',
             error = NULL,
             started_at = NULL,
             completed_at = NULL
         WHERE id = ?1",
        params![&extraction_id],
    )?;
    transaction.execute(
        "UPDATE image_ocr_queue
         SET state = 'PENDING',
             attempt_count = 0,
             next_attempt_at = ?1,
             started_at = NULL,
             last_error = NULL,
             updated_at = ?1
         WHERE extraction_id = ?2",
        params![now_ms, &extraction_id],
    )?;
    transaction.execute(
        "UPDATE attachment
         SET extraction_state = 'PENDING',
             updated_at = max(updated_at, ?1)
         WHERE id = ?2",
        params![now_ms, attachment_id],
    )?;
    transaction.commit()?;
    load_image_status(main, attachment_id)
}

pub(crate) fn recover_interrupted_image_ocr(
    main: &mut Connection,
    stale_started_at_or_before: i64,
    now_ms: i64,
) -> Result<ImageOcrRecovery> {
    validate_timestamp(stale_started_at_or_before, "staleStartedAtOrBefore")?;
    validate_timestamp(now_ms, "nowMs")?;
    let batch_limit =
        i64::try_from(IMAGE_OCR_RECOVERY_BATCH_SIZE).expect("OCR recovery batch size fits i64");
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stale = transaction
        .prepare(
            "SELECT
                queue.extraction_id,
                extraction.attachment_id,
                config.version,
                attachment.sha256
             FROM image_ocr_queue AS queue
             JOIN attachment_extraction AS extraction
               ON extraction.id = queue.extraction_id
             JOIN attachment_extractor_config AS config
               ON config.extractor = extraction.extractor
             JOIN attachment
               ON attachment.id = extraction.attachment_id
             JOIN attachment_image AS image
               ON image.attachment_id = attachment.id
             WHERE extraction.extractor = ?1
               AND attachment.deleted_at IS NULL
               AND NOT (
                    queue.state = 'FAILED'
                    AND queue.last_error = ?2
               )
               AND (
                    extraction.extractor_version <> config.version
                    OR extraction.content_hash <> attachment.sha256
               )
             ORDER BY extraction.attachment_id, queue.extraction_id
             LIMIT ?3",
        )?
        .query_map(
            params![IMAGE_OCR_EXTRACTOR, STALE_OCR_ERROR, batch_limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let interrupted_limit = batch_limit.saturating_sub(
        i64::try_from(stale.len()).expect("bounded stale OCR batch length fits i64"),
    );
    let mut recovery = ImageOcrRecovery::default();
    for (extraction_id, attachment_id, current_extractor_version, attachment_hash) in stale {
        transaction.execute(
            "UPDATE attachment_extraction
             SET status = 'FAILED', error = ?1, completed_at = ?2
             WHERE id = ?3
               AND status <> 'READY'",
            params![STALE_OCR_ERROR, now_ms, &extraction_id],
        )?;
        transaction.execute(
            "UPDATE image_ocr_queue
             SET state = 'FAILED',
                 attempt_count = max(attempt_count, 1),
                 next_attempt_at = NULL,
                 started_at = NULL,
                 last_error = ?1,
                 updated_at = ?2
             WHERE extraction_id = ?3",
            params![STALE_OCR_ERROR, now_ms, &extraction_id],
        )?;
        let current_exists: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM attachment_extraction AS extraction
                JOIN image_ocr_queue AS queue
                  ON queue.extraction_id = extraction.id
                WHERE extraction.attachment_id = ?1
                  AND extraction.extractor = ?2
                  AND extraction.extractor_version = ?3
                  AND extraction.content_hash = ?4
             )",
            params![
                &attachment_id,
                IMAGE_OCR_EXTRACTOR,
                &current_extractor_version,
                &attachment_hash
            ],
            |row| row.get(0),
        )?;
        if !current_exists {
            let replacement_extraction_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO attachment_extraction(
                    id, attachment_id, extractor, extractor_version, content_hash,
                    status, error, created_at, started_at, completed_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, 'PENDING', NULL, ?6, NULL, NULL)",
                params![
                    &replacement_extraction_id,
                    &attachment_id,
                    IMAGE_OCR_EXTRACTOR,
                    &current_extractor_version,
                    &attachment_hash,
                    now_ms
                ],
            )?;
            transaction.execute(
                "INSERT INTO image_ocr_queue(
                    extraction_id, state, attempt_count, next_attempt_at,
                    started_at, last_error, updated_at
                 ) VALUES(?1, 'PENDING', 0, ?2, NULL, NULL, ?2)",
                params![&replacement_extraction_id, now_ms],
            )?;
            transaction.execute(
                "UPDATE attachment
                 SET extraction_state = 'PENDING',
                     updated_at = max(updated_at, ?1)
                 WHERE id = ?2",
                params![now_ms, &attachment_id],
            )?;
            recovery.requeued += 1;
        }
        recovery.terminally_failed += 1;
    }

    let interrupted = transaction
        .prepare(
            "SELECT
                queue.extraction_id,
                extraction.attachment_id,
                queue.attempt_count
             FROM image_ocr_queue AS queue
             JOIN attachment_extraction AS extraction
               ON extraction.id = queue.extraction_id
             JOIN attachment_extractor_config AS config
               ON config.extractor = extraction.extractor
              AND config.version = extraction.extractor_version
             JOIN attachment
               ON attachment.id = extraction.attachment_id
              AND attachment.sha256 = extraction.content_hash
             JOIN attachment_image AS image
               ON image.attachment_id = attachment.id
             WHERE queue.state = 'RUNNING'
               AND queue.started_at <= ?1
               AND extraction.extractor = ?2
               AND attachment.deleted_at IS NULL
             ORDER BY queue.started_at, queue.extraction_id
             LIMIT ?3",
        )?
        .query_map(
            params![
                stale_started_at_or_before,
                IMAGE_OCR_EXTRACTOR,
                interrupted_limit
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (extraction_id, attachment_id, attempt_count) in interrupted {
        if attempt_count >= MAX_IMAGE_OCR_ATTEMPTS {
            transaction.execute(
                "UPDATE attachment_extraction
                 SET status = 'FAILED', error = ?1, completed_at = ?2
                 WHERE id = ?3",
                params![INTERRUPTED_OCR_ERROR, now_ms, &extraction_id],
            )?;
            transaction.execute(
                "UPDATE image_ocr_queue
                 SET state = 'FAILED',
                     next_attempt_at = NULL,
                     started_at = NULL,
                     last_error = ?1,
                     updated_at = ?2
                 WHERE extraction_id = ?3",
                params![INTERRUPTED_OCR_ERROR, now_ms, &extraction_id],
            )?;
            transaction.execute(
                "UPDATE attachment
                 SET extraction_state = 'FAILED', updated_at = max(updated_at, ?1)
                 WHERE id = ?2",
                params![now_ms, &attachment_id],
            )?;
            recovery.terminally_failed += 1;
        } else {
            transaction.execute(
                "UPDATE attachment_extraction
                 SET status = 'PENDING', error = ?1, completed_at = NULL
                 WHERE id = ?2",
                params![INTERRUPTED_OCR_ERROR, &extraction_id],
            )?;
            transaction.execute(
                "UPDATE image_ocr_queue
                 SET state = 'RETRY_WAIT',
                     next_attempt_at = ?1,
                     started_at = NULL,
                     last_error = ?2,
                     updated_at = ?1
                 WHERE extraction_id = ?3",
                params![now_ms, INTERRUPTED_OCR_ERROR, &extraction_id],
            )?;
            recovery.requeued += 1;
        }
    }
    recovery.remaining = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM image_ocr_queue AS queue
            JOIN attachment_extraction AS extraction
              ON extraction.id = queue.extraction_id
            JOIN attachment_extractor_config AS config
              ON config.extractor = extraction.extractor
            JOIN attachment
              ON attachment.id = extraction.attachment_id
            JOIN attachment_image AS image
              ON image.attachment_id = attachment.id
            WHERE extraction.extractor = ?1
              AND attachment.deleted_at IS NULL
              AND (
                   (
                       NOT (
                           queue.state = 'FAILED'
                           AND queue.last_error = ?2
                       )
                       AND (
                           extraction.extractor_version <> config.version
                           OR extraction.content_hash <> attachment.sha256
                       )
                   )
                   OR (
                       queue.state = 'RUNNING'
                       AND queue.started_at <= ?3
                       AND extraction.extractor_version = config.version
                       AND extraction.content_hash = attachment.sha256
                   )
              )
         )",
        params![
            IMAGE_OCR_EXTRACTOR,
            STALE_OCR_ERROR,
            stale_started_at_or_before
        ],
        |row| row.get(0),
    )?;
    transaction.commit()?;
    Ok(recovery)
}

pub(crate) fn image_ocr_diagnostics(main: &Connection) -> Result<ImageOcrDiagnostics> {
    let mut diagnostics = ImageOcrDiagnostics::default();
    let mut statement = main.prepare(
        "SELECT state, count(*)
         FROM image_ocr_queue
         GROUP BY state",
    )?;
    let counts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (state, count) in counts {
        let count = u64::try_from(count).map_err(|_| DatabaseError::Validation {
            kind: "main",
            reason: format!("negative image OCR count for {state}"),
        })?;
        match ImageOcrStatus::from_db(&state)? {
            ImageOcrStatus::Pending => diagnostics.pending = count,
            ImageOcrStatus::Running => diagnostics.running = count,
            ImageOcrStatus::RetryWait => diagnostics.retry_wait = count,
            ImageOcrStatus::Ready => diagnostics.ready = count,
            ImageOcrStatus::Failed => diagnostics.failed = count,
        }
    }
    diagnostics.oldest_eligible_at_ms = main.query_row(
        "SELECT min(next_attempt_at)
         FROM image_ocr_queue
         WHERE state IN ('PENDING', 'RETRY_WAIT')",
        [],
        |row| row.get(0),
    )?;
    diagnostics.last_error = main
        .query_row(
            "SELECT last_error
             FROM image_ocr_queue
             WHERE last_error IS NOT NULL
             ORDER BY updated_at DESC, extraction_id DESC
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(diagnostics)
}

fn image_ocr_retry_delay(attempt_count: u32) -> Result<i64> {
    let index = attempt_count
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| DatabaseError::InvalidInput("invalid OCR attempt count".into()))?;
    IMAGE_OCR_RETRY_DELAYS_MS
        .get(index)
        .copied()
        .ok_or_else(|| {
            DatabaseError::InvalidInput(format!("OCR attempt {attempt_count} has no retry delay"))
        })
}

fn truncate_ocr_error(error: &str) -> String {
    let error = error.trim();
    let error = if error.is_empty() {
        "OCR failed without an error"
    } else {
        error
    };
    error.chars().take(1_000).collect()
}

impl MediaIntegrityScan {
    pub(crate) fn new(now_ms: i64) -> Result<Self> {
        validate_timestamp(now_ms, "nowMs")?;
        Ok(Self {
            now_ms,
            phase: MediaIntegrityPhase::Initialize,
            report: MediaIntegrityReport::default(),
        })
    }

    pub(crate) fn step(
        mut self,
        main: &Connection,
        media: &Connection,
    ) -> Result<MediaIntegrityScanStep> {
        match &self.phase {
            MediaIntegrityPhase::Initialize => {
                let max_attachment_rowid = main.query_row(
                    "SELECT coalesce(max(rowid), 0) FROM attachment",
                    [],
                    |row| row.get(0),
                )?;
                let max_blob_rowid = media.query_row(
                    "SELECT coalesce(max(rowid), 0) FROM media_blob",
                    [],
                    |row| row.get(0),
                )?;
                self.phase = MediaIntegrityPhase::Attachments {
                    cursor: 0,
                    max_rowid: max_attachment_rowid,
                    max_blob_rowid,
                };
                Ok(MediaIntegrityScanStep::Continue(self))
            }
            MediaIntegrityPhase::Attachments {
                cursor,
                max_rowid,
                max_blob_rowid,
            } => {
                let rows = load_integrity_attachment_batch(main, self.now_ms, *cursor, *max_rowid)?;
                if rows.is_empty() {
                    self.phase = MediaIntegrityPhase::Blobs {
                        cursor: 0,
                        max_rowid: *max_blob_rowid,
                    };
                    return Ok(MediaIntegrityScanStep::Continue(self));
                }
                for (_, id, hash, preview_hash, deleted_at, referenced, leased) in &rows {
                    let original_exists: bool = media.query_row(
                        "SELECT EXISTS(SELECT 1 FROM media_blob WHERE sha256 = ?1)",
                        params![hash],
                        |row| row.get(0),
                    )?;
                    let preview_exists = preview_hash
                        .as_deref()
                        .map(|preview_hash| {
                            media.query_row(
                                "SELECT EXISTS(SELECT 1 FROM media_blob WHERE sha256 = ?1)",
                                params![preview_hash],
                                |row| row.get::<_, bool>(0),
                            )
                        })
                        .transpose()?
                        .unwrap_or(true);
                    if (deleted_at.is_none() || *referenced)
                        && (!original_exists || !preview_exists)
                    {
                        record_integrity_finding(
                            &mut self.report,
                            IntegrityFinding::MissingBlobAttachment(id.clone()),
                        );
                    }
                    if deleted_at.is_none() && !*referenced && !*leased {
                        record_integrity_finding(
                            &mut self.report,
                            IntegrityFinding::OrphanedAttachment(id.clone()),
                        );
                    }
                }
                self.phase = MediaIntegrityPhase::Attachments {
                    cursor: rows.last().expect("nonempty attachment batch").0,
                    max_rowid: *max_rowid,
                    max_blob_rowid: *max_blob_rowid,
                };
                Ok(MediaIntegrityScanStep::Continue(self))
            }
            MediaIntegrityPhase::Blobs { cursor, max_rowid } => {
                let rows = load_integrity_blob_batch(media, *cursor, *max_rowid)?;
                if rows.is_empty() {
                    return Ok(MediaIntegrityScanStep::Complete(self.report));
                }
                for (rowid, expected, byte_length) in &rows {
                    let attachment_exists: bool = main.query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM attachment WHERE sha256 = ?1
                            UNION ALL
                            SELECT 1 FROM attachment_image WHERE preview_sha256 = ?1
                         )",
                        params![expected],
                        |row| row.get(0),
                    )?;
                    let leased: bool = media.query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM media_blob_lease
                            WHERE sha256 = ?1 AND expires_at > ?2
                         )",
                        params![expected, self.now_ms],
                        |row| row.get(0),
                    )?;
                    if !attachment_exists && !leased {
                        record_integrity_finding(
                            &mut self.report,
                            IntegrityFinding::ExtraBlob(hex(expected)),
                        );
                    }
                    let mut blob = media.blob_open(MAIN_DB, "media_blob", "bytes", *rowid, true)?;
                    let actual_length =
                        i64::try_from(blob.len()).expect("SQLite blob length fits i64");
                    let actual = digest_reader(&mut blob)?;
                    if actual.as_slice() != expected.as_slice() || actual_length != *byte_length {
                        record_integrity_finding(
                            &mut self.report,
                            IntegrityFinding::CorruptBlob(hex(expected)),
                        );
                    }
                }
                self.phase = MediaIntegrityPhase::Blobs {
                    cursor: rows.last().expect("nonempty media blob batch").0,
                    max_rowid: *max_rowid,
                };
                Ok(MediaIntegrityScanStep::Continue(self))
            }
        }
    }
}

impl MediaMaintenanceScan {
    pub(crate) fn new(now_ms: i64, limits: MediaLimits) -> Result<Self> {
        validate_timestamp(now_ms, "nowMs")?;
        Ok(Self {
            now_ms,
            limits: limits.validate()?,
            phase: MediaMaintenancePhase::Lifecycle {
                cursor: None,
                first_batch: true,
            },
            cleanup: MediaCleanupResult::default(),
        })
    }

    pub(crate) fn step(
        mut self,
        main: &mut Connection,
        media: &mut Connection,
    ) -> Result<MediaMaintenanceScanStep> {
        match &self.phase {
            MediaMaintenancePhase::Lifecycle {
                cursor,
                first_batch,
            } => {
                let (cleanup, next_cursor) = reconcile_and_reap_from(
                    main,
                    media,
                    self.now_ms,
                    self.limits,
                    cursor.clone(),
                    false,
                    *first_batch,
                )?;
                accumulate_cleanup(&mut self.cleanup, cleanup)?;
                if let Some(cursor) = next_cursor {
                    self.phase = MediaMaintenancePhase::Lifecycle {
                        cursor: Some(cursor),
                        first_batch: false,
                    };
                } else {
                    self.phase =
                        MediaMaintenancePhase::Integrity(MediaIntegrityScan::new(self.now_ms)?);
                }
                Ok(MediaMaintenanceScanStep::Continue(self))
            }
            MediaMaintenancePhase::Integrity(_) => {
                let phase = std::mem::replace(
                    &mut self.phase,
                    MediaMaintenancePhase::Lifecycle {
                        cursor: None,
                        first_batch: false,
                    },
                );
                let MediaMaintenancePhase::Integrity(scan) = phase else {
                    unreachable!("maintenance phase was checked above");
                };
                match scan.step(main, media)? {
                    MediaIntegrityScanStep::Continue(scan) => {
                        self.phase = MediaMaintenancePhase::Integrity(scan);
                        Ok(MediaMaintenanceScanStep::Continue(self))
                    }
                    MediaIntegrityScanStep::Complete(integrity) => {
                        Ok(MediaMaintenanceScanStep::Complete(MediaMaintenanceReport {
                            inspected_at_ms: self.now_ms,
                            integrity,
                            cleanup: self.cleanup,
                        }))
                    }
                }
            }
        }
    }
}

type IntegrityAttachmentRow = (
    i64,
    String,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<i64>,
    bool,
    bool,
);

fn load_integrity_attachment_batch(
    main: &Connection,
    now_ms: i64,
    after_rowid: i64,
    max_rowid: i64,
) -> Result<Vec<IntegrityAttachmentRow>> {
    let mut statement = main.prepare(
        "SELECT
            attachment.rowid,
            attachment.id,
            attachment.sha256,
            image.preview_sha256,
            attachment.deleted_at,
            EXISTS(
                SELECT 1 FROM tidbit_revision_attachment AS membership
                WHERE membership.attachment_id = attachment.id
            ),
            (
                EXISTS(
                    SELECT 1 FROM media_ingest_lease AS lease
                    WHERE lease.attachment_id = attachment.id
                      AND lease.state = 'COMMITTED'
                      AND lease.expires_at > ?1
                )
                OR EXISTS(
                    SELECT 1
                    FROM media_ingest_lease AS lease
                    JOIN draft_media_lease AS draft_lease
                      ON draft_lease.media_ingest_lease_id = lease.id
                    JOIN draft ON draft.id = draft_lease.draft_id
                    WHERE lease.attachment_id = attachment.id
                      AND lease.state = 'COMMITTED'
                      AND kosh_markdown_references_attachment(
                          draft.body_markdown,
                          attachment.id
                      )
                )
            )
         FROM attachment
         LEFT JOIN attachment_image AS image
           ON image.attachment_id = attachment.id
         WHERE attachment.rowid > ?2 AND attachment.rowid <= ?3
         ORDER BY attachment.rowid
         LIMIT ?4",
    )?;
    let rows = statement
        .query_map(
            params![
                now_ms,
                after_rowid,
                max_rowid,
                i64::from(INTEGRITY_ATTACHMENT_BATCH_SIZE)
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

type IntegrityBlobRow = (i64, Vec<u8>, i64);

fn load_integrity_blob_batch(
    media: &Connection,
    after_rowid: i64,
    max_rowid: i64,
) -> Result<Vec<IntegrityBlobRow>> {
    let mut statement = media.prepare(
        "SELECT rowid, sha256, byte_length
         FROM media_blob
         WHERE rowid > ?1 AND rowid <= ?2
         ORDER BY rowid
         LIMIT ?3",
    )?;
    let rows = statement
        .query_map(
            params![after_rowid, max_rowid, i64::from(INTEGRITY_BLOB_BATCH_SIZE)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

enum IntegrityFinding {
    MissingBlobAttachment(String),
    CorruptBlob(String),
    ExtraBlob(String),
    OrphanedAttachment(String),
}

fn record_integrity_finding(report: &mut MediaIntegrityReport, finding: IntegrityFinding) {
    let finding_count = report.missing_blob_attachment_ids.len()
        + report.corrupt_blob_sha256.len()
        + report.extra_blob_sha256.len()
        + report.orphaned_attachment_ids.len();
    if finding_count >= MAX_INTEGRITY_DIAGNOSTICS {
        report.diagnostics_truncated = true;
        return;
    }
    match finding {
        IntegrityFinding::MissingBlobAttachment(id) => {
            report.missing_blob_attachment_ids.push(id);
        }
        IntegrityFinding::CorruptBlob(hash) => report.corrupt_blob_sha256.push(hash),
        IntegrityFinding::ExtraBlob(hash) => report.extra_blob_sha256.push(hash),
        IntegrityFinding::OrphanedAttachment(id) => report.orphaned_attachment_ids.push(id),
    }
}

fn accumulate_cleanup(total: &mut MediaCleanupResult, increment: MediaCleanupResult) -> Result<()> {
    total.retired_attachment_count = total
        .retired_attachment_count
        .checked_add(increment.retired_attachment_count)
        .ok_or_else(|| DatabaseError::InvalidInput("retired attachment count overflow".into()))?;
    total.deleted_blob_count = total
        .deleted_blob_count
        .checked_add(increment.deleted_blob_count)
        .ok_or_else(|| DatabaseError::InvalidInput("deleted media count overflow".into()))?;
    total.reclaimed_bytes = total
        .reclaimed_bytes
        .checked_add(increment.reclaimed_bytes)
        .ok_or_else(|| DatabaseError::InvalidInput("reclaimed media byte count overflow".into()))?;
    Ok(())
}

pub(crate) fn referenced_attachments(markdown: &str) -> Vec<AttachmentReference> {
    let mut references = Vec::<AttachmentReference>::new();
    let mut positions = HashMap::<String, usize>::new();
    let mut cursor = 0;
    while cursor < markdown.len() {
        let remainder = &markdown[cursor..];
        let next_image = remainder
            .find(IMAGE_TOKEN_PREFIX)
            .map(|offset| (offset, IMAGE_TOKEN_PREFIX, AttachmentDisplayRole::Inline));
        let next_attachment = remainder.find(ATTACHMENT_TOKEN_PREFIX).map(|offset| {
            (
                offset,
                ATTACHMENT_TOKEN_PREFIX,
                AttachmentDisplayRole::Attachment,
            )
        });
        let next_pdf = remainder
            .find(PDF_TOKEN_PREFIX)
            .map(|offset| (offset, PDF_TOKEN_PREFIX, AttachmentDisplayRole::Attachment));
        let Some(next) = [next_image, next_attachment, next_pdf]
            .into_iter()
            .flatten()
            .min_by_key(|candidate| candidate.0)
        else {
            break;
        };
        let payload_start = cursor + next.0 + next.1.len();
        let payload = &markdown[payload_start..];
        let Some(end) = payload.find(TOKEN_SUFFIX) else {
            cursor = payload_start;
            continue;
        };
        if let Some(id) = parse_token_payload(&payload[..end], next.2) {
            if let Some(position) = positions.get(id).copied() {
                if next.2 == AttachmentDisplayRole::Inline {
                    references[position].display_role = AttachmentDisplayRole::Inline;
                }
            } else {
                positions.insert(id.to_owned(), references.len());
                references.push(AttachmentReference {
                    id: id.to_owned(),
                    display_role: next.2,
                });
            }
        }
        cursor = payload_start + end + TOKEN_SUFFIX.len();
    }
    references
}

pub(crate) fn markdown_references_attachment(markdown: &str, attachment_id: &str) -> bool {
    referenced_attachments(markdown)
        .iter()
        .any(|reference| reference.id == attachment_id)
}

fn parse_token_payload(payload: &str, role: AttachmentDisplayRole) -> Option<&str> {
    let id = match role {
        AttachmentDisplayRole::Attachment => payload,
        AttachmentDisplayRole::Inline => {
            let mut fields = payload.split(';');
            let id = fields.next()?;
            let width = fields.next()?.strip_prefix("width=")?;
            let width = width.strip_suffix('%')?;
            let parsed = width.parse::<u32>().ok()?;
            if !(10..=100).contains(&parsed) || parsed.to_string() != width {
                return None;
            }
            let mut saw_alt = false;
            let mut saw_caption = false;
            for field in fields {
                if let Some(value) = field.strip_prefix("alt=") {
                    if saw_alt || saw_caption || !valid_encoded_token_field(value, 500) {
                        return None;
                    }
                    saw_alt = true;
                } else {
                    let value = field.strip_prefix("caption=")?;
                    if saw_caption || !valid_encoded_token_field(value, 2_000) {
                        return None;
                    }
                    saw_caption = true;
                }
            }
            id
        }
    };
    validate_uuid_v7(id, "attachmentId").is_ok().then_some(id)
}

fn valid_encoded_token_field(value: &str, max_characters: usize) -> bool {
    let Some(max_encoded_bytes) = max_characters.checked_mul(12) else {
        return false;
    };
    if value.is_empty() || value.len() > max_encoded_bytes {
        return false;
    }
    let encoded = value.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        let byte = encoded[index];
        if canonical_token_field_leaves_unescaped(byte) {
            decoded.push(byte);
            index += 1;
            continue;
        }
        if byte != b'%' || index + 2 >= encoded.len() {
            return false;
        }
        let Some(high) = canonical_hex_value(encoded[index + 1]) else {
            return false;
        };
        let Some(low) = canonical_hex_value(encoded[index + 2]) else {
            return false;
        };
        let decoded_byte = (high << 4) | low;
        if canonical_token_field_leaves_unescaped(decoded_byte) {
            return false;
        }
        decoded.push(decoded_byte);
        index += 3;
    }
    let Ok(decoded) = std::str::from_utf8(&decoded) else {
        return false;
    };
    !decoded.is_empty() && decoded.trim() == decoded && decoded.chars().count() <= max_characters
}

fn canonical_token_field_leaves_unescaped(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.')
}

fn canonical_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn sync_draft_media_leases(
    transaction: &Transaction<'_>,
    draft_id: &str,
    body_markdown: &str,
    now_ms: i64,
    max_attachments_per_draft: u32,
    lease_duration_ms: i64,
) -> Result<()> {
    validate_timestamp(now_ms, "nowMs")?;
    if lease_duration_ms <= 0 {
        return Err(DatabaseError::InvalidInput(
            "draft media lease duration must be positive".into(),
        ));
    }
    let references = referenced_attachments(body_markdown);
    let reference_count = u32::try_from(references.len())
        .map_err(|_| DatabaseError::InvalidInput("too many attachment references".into()))?;
    if reference_count > max_attachments_per_draft {
        return Err(DatabaseError::InvalidInput(format!(
            "a draft may contain at most {max_attachments_per_draft} attachments"
        )));
    }
    for reference in references {
        let lease_id = transaction
            .query_row(
                "SELECT lease.id
                 FROM media_ingest_lease AS lease
                 JOIN draft_media_lease AS draft_lease
                   ON draft_lease.media_ingest_lease_id = lease.id
                 JOIN attachment ON attachment.id = lease.attachment_id
                 WHERE draft_lease.draft_id = ?1
                   AND lease.attachment_id = ?2
                   AND lease.state = 'COMMITTED'
                   AND attachment.deleted_at IS NULL",
                params![draft_id, &reference.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(lease_id) = lease_id {
            transaction.execute(
                "UPDATE media_ingest_lease
                 SET expires_at = max(expires_at, ?1)
                 WHERE id = ?2",
                params![
                    checked_timestamp_add(now_ms, lease_duration_ms, "draft media lease renewal")?,
                    lease_id
                ],
            )?;
            continue;
        }
        let inherited_from_base_revision = transaction
            .query_row(
                "SELECT 1
                 FROM draft_context AS context
                 JOIN tidbit_revision_attachment AS membership
                   ON membership.tidbit_revision_id = context.base_revision_id
                 JOIN attachment
                   ON attachment.id = membership.attachment_id
                 WHERE context.draft_id = ?1
                   AND membership.attachment_id = ?2
                   AND attachment.deleted_at IS NULL",
                params![draft_id, &reference.id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !inherited_from_base_revision {
            return Err(DatabaseError::InvalidInput(format!(
                "attachment {} is not authorized for this draft",
                reference.id
            )));
        }
    }
    Ok(())
}

pub(crate) fn abandon_draft_media_leases(
    transaction: &Transaction<'_>,
    context_key: &str,
    now_ms: i64,
) -> Result<()> {
    validate_timestamp(now_ms, "nowMs")?;
    transaction.execute(
        "UPDATE media_ingest_lease
         SET state = 'ABANDONED',
             expires_at = max(created_at, min(expires_at, ?1))
         WHERE id IN (
            SELECT draft_lease.media_ingest_lease_id
            FROM draft_media_lease AS draft_lease
            JOIN draft_context AS context ON context.draft_id = draft_lease.draft_id
            WHERE context.context_key = ?2
         )
           AND state = 'COMMITTED'",
        params![now_ms, context_key],
    )?;
    Ok(())
}

pub(crate) fn link_revision_attachments(
    transaction: &Transaction<'_>,
    revision_id: &str,
    current_revision_id: Option<&str>,
    draft_context_key: &str,
    body_markdown: &str,
    now_ms: i64,
) -> Result<()> {
    let references = referenced_attachments(body_markdown);
    for (sort_order, reference) in references.iter().enumerate() {
        let already_linked = current_revision_id
            .map(|current_revision_id| {
                transaction
                    .query_row(
                        "SELECT 1
                         FROM tidbit_revision_attachment
                         WHERE tidbit_revision_id = ?1 AND attachment_id = ?2",
                        params![current_revision_id, &reference.id],
                        |_| Ok(()),
                    )
                    .optional()
                    .map(|row| row.is_some())
            })
            .transpose()?
            .unwrap_or(false);
        let actively_leased = transaction
            .query_row(
                "SELECT 1
                 FROM attachment
                 JOIN media_ingest_lease AS lease
                   ON lease.attachment_id = attachment.id
                 JOIN draft_media_lease AS draft_lease
                   ON draft_lease.media_ingest_lease_id = lease.id
                 JOIN draft_context AS context ON context.draft_id = draft_lease.draft_id
                 WHERE attachment.id = ?1
                   AND attachment.deleted_at IS NULL
                   AND lease.state = 'COMMITTED'
                   AND lease.expires_at > ?2
                   AND context.context_key = ?3",
                params![&reference.id, now_ms, draft_context_key],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !already_linked && !actively_leased {
            return Err(DatabaseError::InvalidInput(format!(
                "attachment {} is not authorized for this revision",
                reference.id
            )));
        }
        transaction.execute(
            "INSERT INTO tidbit_revision_attachment(
                tidbit_revision_id, attachment_id, sort_order, display_role
             ) VALUES(?1, ?2, ?3, ?4)",
            params![
                revision_id,
                &reference.id,
                i64::try_from(sort_order)
                    .map_err(|_| DatabaseError::InvalidInput("too many attachments".into()))?,
                reference.display_role.as_db_str()
            ],
        )?;
    }
    Ok(())
}

fn stream_to_stage(
    mut reader: impl Read,
    mut file: File,
    max_bytes: u64,
) -> Result<(Vec<u8>, u64)> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| DatabaseError::InvalidInput("attachment byte length overflow".into()))?;
        if total > max_bytes {
            return Err(DatabaseError::InvalidInput(format!(
                "the selected attachment is larger than {max_bytes} bytes"
            )));
        }
        digest.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
    }
    file.sync_all()?;
    Ok((digest.finalize().to_vec(), total))
}

fn validate_staged_file(write: &IngestAttachmentWrite) -> Result<()> {
    let metadata = fs::metadata(&write.staged_path)?;
    if !metadata.is_file() || metadata.len() != write.byte_length {
        return Err(DatabaseError::InvalidInput(
            "staged attachment changed before ingestion".into(),
        ));
    }
    Ok(())
}

fn write_staged_blob(
    transaction: &Transaction<'_>,
    rowid: i64,
    write: &IngestAttachmentWrite,
) -> Result<()> {
    let mut source = File::open(&write.staged_path)?;
    let mut blob = transaction.blob_open(MAIN_DB, "media_blob", "bytes", rowid, false)?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        digest.update(&buffer[..read]);
        blob.write_all(&buffer[..read])?;
    }
    if total != write.byte_length || digest.finalize().as_slice() != write.sha256.as_slice() {
        return Err(DatabaseError::InvalidInput(
            "staged attachment changed before ingestion".into(),
        ));
    }
    Ok(())
}

fn validate_existing_blob(
    transaction: &Transaction<'_>,
    rowid: i64,
    write: &IngestAttachmentWrite,
) -> Result<()> {
    let source_digest = digest_reader(File::open(&write.staged_path)?)?;
    if source_digest.as_slice() != write.sha256.as_slice() {
        return Err(DatabaseError::InvalidInput(
            "staged attachment changed before ingestion".into(),
        ));
    }
    let mut blob = transaction.blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)?;
    if blob.len() as u64 != write.byte_length {
        return Err(DatabaseError::Validation {
            kind: "media",
            reason: "deduplicated media length does not match its digest".into(),
        });
    }
    let stored_digest = digest_reader(&mut blob)?;
    if stored_digest.as_slice() != write.sha256.as_slice() {
        return Err(DatabaseError::Validation {
            kind: "media",
            reason: "deduplicated media bytes do not match their digest".into(),
        });
    }
    Ok(())
}

fn digest_reader(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().to_vec())
}

fn validate_draft_capacity(
    connection: &Connection,
    draft_id: &str,
    max_attachments: u32,
) -> Result<()> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM draft WHERE id = ?1",
            params![draft_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Err(DatabaseError::NotFound {
            entity: "draft",
            id: draft_id.into(),
        });
    }
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM draft_media_lease WHERE draft_id = ?1",
        params![draft_id],
        |row| row.get(0),
    )?;
    if count >= i64::from(max_attachments) {
        return Err(DatabaseError::InvalidInput(format!(
            "a draft may contain at most {max_attachments} attachments"
        )));
    }
    Ok(())
}

pub(crate) fn recover_media_lifecycle_batch(
    main: &mut Connection,
    media: &mut Connection,
    now_ms: i64,
    limits: MediaLimits,
    cursor: Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    validate_timestamp(now_ms, "nowMs")?;
    let limits = limits.validate()?;
    let first_batch = cursor.is_none();
    let (_, next_cursor) =
        reconcile_and_reap_from(main, media, now_ms, limits, cursor, false, first_batch)?;
    Ok(next_cursor)
}

fn reconcile_and_reap_from(
    main: &mut Connection,
    media: &mut Connection,
    now_ms: i64,
    limits: MediaLimits,
    cursor: Option<Vec<u8>>,
    scan_all_blobs: bool,
    run_lifecycle_work: bool,
) -> Result<(MediaCleanupResult, Option<Vec<u8>>)> {
    let retired_attachment_count = if run_lifecycle_work {
        let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let renewed_expiry = checked_timestamp_add(
            now_ms,
            limits.draft_lease_duration_ms,
            "recovered draft media lease renewal",
        )?;
        transaction.execute(
            "UPDATE media_ingest_lease
             SET expires_at = max(expires_at, ?1)
             WHERE state = 'COMMITTED'
               AND expires_at <= ?2
               AND attachment_id IS NOT NULL
               AND EXISTS (
                    SELECT 1
                    FROM draft_media_lease AS draft_lease
                    JOIN draft ON draft.id = draft_lease.draft_id
                    WHERE draft_lease.media_ingest_lease_id = media_ingest_lease.id
                      AND kosh_markdown_references_attachment(
                          draft.body_markdown,
                          media_ingest_lease.attachment_id
                      )
               )",
            params![renewed_expiry, now_ms],
        )?;
        transaction.execute(
            "UPDATE media_ingest_lease
         SET state = 'ABANDONED', expires_at = min(expires_at, ?1)
         WHERE state IN ('STAGED', 'COMMITTED')
           AND expires_at <= ?1
           AND (
                attachment_id IS NULL
                OR NOT EXISTS (
                    SELECT 1 FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = media_ingest_lease.attachment_id
                )
           )",
            params![now_ms],
        )?;
        let retired_attachment_count = transaction.execute(
            "UPDATE attachment
         SET deleted_at = max(created_at, ?1),
             updated_at = max(updated_at, ?1)
         WHERE deleted_at IS NULL
           AND NOT EXISTS (
                SELECT 1 FROM tidbit_revision_attachment AS membership
                WHERE membership.attachment_id = attachment.id
           )
           AND NOT EXISTS (
                SELECT 1 FROM media_ingest_lease AS lease
                WHERE lease.attachment_id = attachment.id
                  AND lease.state = 'COMMITTED'
                  AND lease.expires_at > ?1
           )",
            params![now_ms],
        )? as u64;
        transaction.execute(
            "UPDATE attachment_extraction
             SET status = 'FAILED',
                 error = ?1,
                 completed_at = max(coalesce(started_at, created_at), ?2)
             WHERE extractor = ?3
               AND id IN (
                    SELECT queue.extraction_id
                    FROM image_ocr_queue AS queue
                    JOIN attachment_extraction AS extraction
                      ON extraction.id = queue.extraction_id
                    JOIN attachment
                      ON attachment.id = extraction.attachment_id
                    WHERE queue.state IN ('PENDING', 'RUNNING', 'RETRY_WAIT')
                      AND attachment.deleted_at IS NOT NULL
               )",
            params![RETIRED_OCR_ERROR, now_ms, IMAGE_OCR_EXTRACTOR],
        )?;
        transaction.execute(
            "UPDATE image_ocr_queue
             SET state = 'FAILED',
                 attempt_count = max(attempt_count, 1),
                 next_attempt_at = NULL,
                 started_at = NULL,
                 last_error = ?1,
                 updated_at = max(updated_at, ?2)
             WHERE state IN ('PENDING', 'RUNNING', 'RETRY_WAIT')
               AND extraction_id IN (
                    SELECT extraction.id
                    FROM attachment_extraction AS extraction
                    JOIN attachment
                      ON attachment.id = extraction.attachment_id
                    WHERE extraction.extractor = ?3
                      AND attachment.deleted_at IS NOT NULL
               )",
            params![RETIRED_OCR_ERROR, now_ms, IMAGE_OCR_EXTRACTOR],
        )?;
        transaction.execute(
            "UPDATE attachment
             SET extraction_state = 'FAILED',
                 updated_at = max(updated_at, ?1)
             WHERE deleted_at IS NOT NULL
               AND EXISTS (
                    SELECT 1
                    FROM attachment_extraction AS extraction
                    JOIN image_ocr_queue AS queue
                      ON queue.extraction_id = extraction.id
                    WHERE extraction.attachment_id = attachment.id
                      AND extraction.extractor = ?2
                      AND queue.state = 'FAILED'
                      AND queue.last_error = ?3
               )",
            params![now_ms, IMAGE_OCR_EXTRACTOR, RETIRED_OCR_ERROR],
        )?;
        transaction.execute(
            "UPDATE attachment_extraction
             SET status = 'FAILED',
                 error = ?1,
                 completed_at = max(coalesce(started_at, created_at), ?2)
             WHERE extractor = ?3
               AND id IN (
                    SELECT queue.extraction_id
                    FROM pdf_extraction_queue AS queue
                    JOIN attachment_extraction AS extraction
                      ON extraction.id = queue.extraction_id
                    JOIN attachment
                      ON attachment.id = extraction.attachment_id
                    WHERE queue.state IN ('PENDING', 'RUNNING', 'RETRY_WAIT')
                      AND attachment.deleted_at IS NOT NULL
               )",
            params![RETIRED_PDF_ERROR, now_ms, PDF_TEXT_EXTRACTOR],
        )?;
        transaction.execute(
            "UPDATE pdf_extraction_queue
             SET state = 'FAILED',
                 attempt_count = max(attempt_count, 1),
                 next_attempt_at = NULL,
                 started_at = NULL,
                 last_error = ?1,
                 updated_at = max(updated_at, ?2)
             WHERE state IN ('PENDING', 'RUNNING', 'RETRY_WAIT')
               AND extraction_id IN (
                    SELECT extraction.id
                    FROM attachment_extraction AS extraction
                    JOIN attachment
                      ON attachment.id = extraction.attachment_id
                    WHERE extraction.extractor = ?3
                      AND attachment.deleted_at IS NOT NULL
               )",
            params![RETIRED_PDF_ERROR, now_ms, PDF_TEXT_EXTRACTOR],
        )?;
        transaction.execute(
            "DELETE FROM draft_media_lease
         WHERE media_ingest_lease_id IN (
            SELECT id FROM media_ingest_lease
            WHERE state = 'ABANDONED' AND expires_at <= ?1
         )",
            params![now_ms],
        )?;
        transaction.execute(
            "DELETE FROM media_blob_reap_candidate
         WHERE sha256 IN (
            SELECT attachment.sha256
            FROM attachment
            WHERE attachment.deleted_at IS NULL
               OR EXISTS (
                    SELECT 1 FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = attachment.id
               )
            UNION
            SELECT image.preview_sha256
            FROM attachment_image AS image
            JOIN attachment ON attachment.id = image.attachment_id
            WHERE attachment.deleted_at IS NULL
               OR EXISTS (
                    SELECT 1 FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = attachment.id
               )
         )",
            [],
        )?;
        transaction.commit()?;

        media.execute(
            "DELETE FROM media_blob_lease WHERE expires_at <= ?1",
            params![now_ms],
        )?;
        retired_attachment_count
    } else {
        0
    };
    let next_cursor = if scan_all_blobs {
        let mut cursor = cursor;
        loop {
            let next = reconcile_blob_candidate_batch(main, media, now_ms, cursor.as_deref())?;
            let Some(next) = next else {
                break;
            };
            cursor = Some(next);
        }
        None
    } else {
        reconcile_blob_candidate_batch(main, media, now_ms, cursor.as_deref())?
    };
    if !run_lifecycle_work {
        return Ok((MediaCleanupResult::default(), next_cursor));
    }
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cutoff = now_ms.saturating_sub(limits.orphan_grace_period_ms);
    let candidates = transaction
        .prepare(
            "SELECT sha256
             FROM media_blob_reap_candidate
             WHERE orphaned_at <= ?1
             ORDER BY orphaned_at, sha256
             LIMIT ?2",
        )?
        .query_map(
            params![cutoff, i64::from(limits.max_reaps_per_maintenance)],
            |row| row.get::<_, Vec<u8>>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    transaction.commit()?;

    let mut cleanup = MediaCleanupResult {
        retired_attachment_count,
        ..MediaCleanupResult::default()
    };
    for hash in candidates {
        let main_transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let references: i64 = main_transaction.query_row(
            "SELECT count(*)
             FROM (
                SELECT attachment.id
                FROM attachment
                WHERE attachment.sha256 = ?1
                  AND (
                       attachment.deleted_at IS NULL
                       OR EXISTS (
                           SELECT 1 FROM tidbit_revision_attachment AS membership
                           WHERE membership.attachment_id = attachment.id
                       )
                  )
                UNION ALL
                SELECT image.attachment_id
                FROM attachment_image AS image
                JOIN attachment ON attachment.id = image.attachment_id
                WHERE image.preview_sha256 = ?1
                  AND (
                       attachment.deleted_at IS NULL
                       OR EXISTS (
                           SELECT 1 FROM tidbit_revision_attachment AS membership
                           WHERE membership.attachment_id = attachment.id
                       )
                  )
             )",
            params![&hash],
            |row| row.get(0),
        )?;
        if references > 0 {
            main_transaction.execute(
                "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
                params![&hash],
            )?;
            main_transaction.commit()?;
            continue;
        }
        let media_transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_lease: bool = media_transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM media_blob_lease
                WHERE sha256 = ?1 AND expires_at > ?2
             )",
            params![&hash, now_ms],
            |row| row.get(0),
        )?;
        if active_lease {
            media_transaction.rollback()?;
            main_transaction.execute(
                "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
                params![&hash],
            )?;
            main_transaction.commit()?;
            continue;
        }
        let byte_length = media_transaction
            .query_row(
                "SELECT byte_length FROM media_blob WHERE sha256 = ?1",
                params![&hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(byte_length) = byte_length else {
            media_transaction.rollback()?;
            main_transaction.execute(
                "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
                params![&hash],
            )?;
            main_transaction.commit()?;
            continue;
        };
        media_transaction.execute(
            "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
             VALUES(?1, ?2, 'grace period elapsed')",
            params![&hash, now_ms],
        )?;
        let deleted = media_transaction
            .execute("DELETE FROM media_blob WHERE sha256 = ?1", params![&hash])?;
        media_transaction.commit()?;
        if deleted == 1 {
            cleanup.deleted_blob_count += 1;
            cleanup.reclaimed_bytes = cleanup
                .reclaimed_bytes
                .checked_add(
                    u64::try_from(byte_length).map_err(|_| DatabaseError::Validation {
                        kind: "media",
                        reason: "media blob has negative byte length".into(),
                    })?,
                )
                .ok_or_else(|| {
                    DatabaseError::InvalidInput("reclaimed media byte count overflow".into())
                })?;
        }
        main_transaction.execute(
            "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
            params![&hash],
        )?;
        main_transaction.commit()?;
    }
    Ok((cleanup, next_cursor))
}

fn reconcile_blob_candidate_batch(
    main: &mut Connection,
    media: &Connection,
    now_ms: i64,
    cursor: Option<&[u8]>,
) -> Result<Option<Vec<u8>>> {
    let hashes = load_media_blob_hash_batch(media, cursor)?;
    if hashes.is_empty() {
        return Ok(None);
    }
    let next_cursor = (hashes.len()
        == usize::try_from(MEDIA_RECONCILE_BATCH_SIZE)
            .expect("media reconciliation batch size fits usize"))
    .then(|| hashes.last().expect("nonempty media hash batch").clone());
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut live_statement = transaction.prepare(
            "SELECT EXISTS(
                    SELECT 1
                    FROM attachment
                    WHERE sha256 = ?1
                      AND (
                           deleted_at IS NULL
                           OR EXISTS (
                               SELECT 1
                               FROM tidbit_revision_attachment AS membership
                               WHERE membership.attachment_id = attachment.id
                           )
                      )
                    UNION ALL
                    SELECT 1
                    FROM attachment_image AS image
                    JOIN attachment ON attachment.id = image.attachment_id
                    WHERE image.preview_sha256 = ?1
                      AND (
                           attachment.deleted_at IS NULL
                           OR EXISTS (
                               SELECT 1
                               FROM tidbit_revision_attachment AS membership
                               WHERE membership.attachment_id = attachment.id
                           )
                      )
                 )",
        )?;
        let mut lease_statement = media.prepare(
            "SELECT EXISTS(
                    SELECT 1
                    FROM media_blob_lease
                    WHERE sha256 = ?1 AND expires_at > ?2
                 )",
        )?;
        let mut candidate_statement = transaction.prepare(
            "INSERT OR IGNORE INTO media_blob_reap_candidate(
                    sha256, orphaned_at, reason
                 ) VALUES(?1, ?2, 'unreferenced media blob')",
        )?;
        for hash in hashes {
            let live: bool = live_statement.query_row(params![&hash], |row| row.get(0))?;
            let leased: bool =
                lease_statement.query_row(params![&hash, now_ms], |row| row.get(0))?;
            if !live && !leased {
                candidate_statement.execute(params![hash, now_ms])?;
            }
        }
    }
    transaction.commit()?;
    Ok(next_cursor)
}

fn load_media_blob_hash_batch(
    media: &Connection,
    after_hash: Option<&[u8]>,
) -> Result<Vec<Vec<u8>>> {
    let limit = i64::from(MEDIA_RECONCILE_BATCH_SIZE);
    if let Some(after_hash) = after_hash {
        let mut statement = media.prepare(
            "SELECT sha256
             FROM media_blob
             WHERE sha256 > ?1
             ORDER BY sha256
             LIMIT ?2",
        )?;
        let hashes = statement
            .query_map(params![after_hash, limit], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        return Ok(hashes);
    }
    let mut statement = media.prepare("SELECT sha256 FROM media_blob ORDER BY sha256 LIMIT ?1")?;
    let hashes = statement
        .query_map(params![limit], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(hashes)
}

fn validate_filename(filename: &str) -> Result<()> {
    if filename.trim() != filename
        || filename.is_empty()
        || filename.chars().count() > MAX_FILENAME_CHARS
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
        || matches!(filename, "." | "..")
    {
        return Err(DatabaseError::InvalidInput(
            "display filename must be a plain filename without path components".into(),
        ));
    }
    Ok(())
}

fn validate_media_type(media_type: &str) -> Result<()> {
    let valid = !media_type.is_empty()
        && media_type.len() <= MAX_MEDIA_TYPE_BYTES
        && media_type.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-' | b'/'
                )
        })
        && media_type.matches('/').count() == 1
        && !media_type.starts_with('/')
        && !media_type.ends_with('/');
    if valid {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(
            "media type must be a simple MIME type".into(),
        ))
    }
}

fn validate_uuid_v7(value: &str, field: &str) -> Result<()> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| DatabaseError::InvalidInput(format!("{field} must be a UUIDv7")))?;
    if parsed.get_version_num() != 7 || value != parsed.hyphenated().to_string() {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a lowercase UUIDv7"
        )));
    }
    Ok(())
}

fn validate_timestamp(value: i64, field: &str) -> Result<()> {
    if (0..=MAX_SAFE_INTEGER).contains(&value) {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(format!(
            "{field} must be a non-negative JavaScript-safe integer"
        )))
    }
}

fn checked_timestamp_add(value: i64, duration: i64, field: &str) -> Result<i64> {
    value
        .checked_add(duration)
        .filter(|result| *result <= MAX_SAFE_INTEGER)
        .ok_or_else(|| DatabaseError::InvalidInput(format!("{field} overflow")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
