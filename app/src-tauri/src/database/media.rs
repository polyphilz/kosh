use std::{
    collections::{HashMap, HashSet},
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

use super::{DatabaseError, Result};

const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const MAX_FILENAME_CHARS: usize = 255;
const MAX_MEDIA_TYPE_BYTES: usize = 127;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
pub(crate) const MEDIA_RECONCILE_BATCH_SIZE: u32 = 64;
const IMAGE_TOKEN_PREFIX: &str = "{{kosh:image:";
const ATTACHMENT_TOKEN_PREFIX: &str = "{{kosh:attachment:";
const TOKEN_SUFFIX: &str = "}}";

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
            max_attachment_bytes: 256 * 1024 * 1024,
            max_attachments_per_draft: 32,
            max_protocol_response_bytes: 32 * 1024 * 1024,
            draft_lease_duration_ms: 24 * 60 * 60 * 1_000,
            orphan_grace_period_ms: 7 * 24 * 60 * 60 * 1_000,
            max_reaps_per_maintenance: 32,
        }
    }
}

impl MediaLimits {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.max_attachment_bytes == 0
            || self.max_attachment_bytes
                > u64::try_from(super::connection::MAX_MEDIA_BLOB_BYTES)
                    .expect("positive media schema limit")
        {
            return Err(DatabaseError::InvalidInput(
                "maxAttachmentBytes must be between 1 and the media schema limit".into(),
            ));
        }
        if self.max_attachments_per_draft == 0 || self.max_attachments_per_draft > 256 {
            return Err(DatabaseError::InvalidInput(
                "maxAttachmentsPerDraft must be between 1 and 256".into(),
            ));
        }
        if self.max_protocol_response_bytes == 0
            || self.max_protocol_response_bytes > self.max_attachment_bytes
        {
            return Err(DatabaseError::InvalidInput(
                "maxProtocolResponseBytes must be between 1 and maxAttachmentBytes".into(),
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
    validate_staged_file(&write)?;

    let expires_at = limits.lease_expiry(write.now_ms)?;
    let media_transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let inserted = media_transaction.execute(
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
    let rowid: i64 = media_transaction.query_row(
        "SELECT rowid FROM media_blob WHERE sha256 = ?1",
        params![&write.sha256],
        |row| row.get(0),
    )?;
    if inserted == 1 {
        write_staged_blob(&media_transaction, rowid, &write)?;
    } else {
        validate_existing_blob(&media_transaction, rowid, &write)?;
    }
    media_transaction.execute(
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
    media_transaction.commit()?;

    let kind = AttachmentKind::from_media_type(&write.media_type);
    let main_transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_draft_capacity(
        &main_transaction,
        &write.draft_id,
        limits.max_attachments_per_draft,
    )?;
    main_transaction.execute(
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
    main_transaction.execute(
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
    main_transaction.execute(
        "INSERT INTO draft_media_lease(draft_id, media_ingest_lease_id)
         VALUES(?1, ?2)",
        params![&write.draft_id, &write.ingest_lease_id],
    )?;
    main_transaction.execute(
        "DELETE FROM media_blob_reap_candidate WHERE sha256 = ?1",
        params![&write.sha256],
    )?;
    main_transaction.commit()?;

    if let Err(error) = media.execute(
        "DELETE FROM media_blob_lease WHERE lease_id = ?1 AND sha256 = ?2",
        params![&write.ingest_lease_id, &write.sha256],
    ) {
        log::warn!(
            "attachment {} committed but its staging lease could not be cleared: {error}",
            write.attachment_id
        );
    }

    Ok(AttachmentRecord {
        id: write.attachment_id,
        ingest_lease_id: write.ingest_lease_id,
        display_filename: write.display_filename,
        media_type: write.media_type,
        byte_length: write.byte_length,
        kind,
    })
}

pub(crate) fn load_media_payload(
    main: &Connection,
    media: &Connection,
    attachment_id: &str,
    now_ms: i64,
    requested_range: Option<MediaByteRange>,
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
                attachment.sha256,
                attachment.media_type,
                attachment.byte_length,
                EXISTS (
                    SELECT 1
                    FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = attachment.id
                )
             FROM attachment
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
    let range = requested_range.unwrap_or(MediaByteRange {
        start: 0,
        end_inclusive: total_byte_length - 1,
    });
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

pub(crate) fn integrity_report(
    main: &Connection,
    media: &Connection,
    now_ms: i64,
) -> Result<MediaIntegrityReport> {
    validate_timestamp(now_ms, "nowMs")?;
    let attachments = main
        .prepare(
            "SELECT
                attachment.id,
                attachment.sha256,
                attachment.deleted_at,
                EXISTS(
                    SELECT 1 FROM tidbit_revision_attachment AS membership
                    WHERE membership.attachment_id = attachment.id
                ),
                EXISTS(
                    SELECT 1 FROM media_ingest_lease AS lease
                    WHERE lease.attachment_id = attachment.id
                      AND lease.state = 'COMMITTED'
                      AND lease.expires_at > ?1
                )
             FROM attachment
             ORDER BY attachment.id",
        )?
        .query_map(params![now_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let attachment_hashes = attachments
        .iter()
        .map(|(_, hash, _, _, _)| hash.clone())
        .collect::<HashSet<_>>();
    let blob_rows = media
        .prepare("SELECT rowid, sha256, byte_length FROM media_blob ORDER BY sha256")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let blob_hashes = blob_rows
        .iter()
        .map(|(_, hash, _)| hash.clone())
        .collect::<HashSet<_>>();
    let leased_blob_hashes = media
        .prepare("SELECT DISTINCT sha256 FROM media_blob_lease WHERE expires_at > ?1")?
        .query_map(params![now_ms], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<HashSet<_>, _>>()?;

    let missing_blob_attachment_ids = attachments
        .iter()
        .filter(|(_, hash, deleted_at, referenced, _leased)| {
            (deleted_at.is_none() || *referenced) && !blob_hashes.contains(hash)
        })
        .map(|(id, _, _, _, _)| id.clone())
        .collect();
    let orphaned_attachment_ids = attachments
        .iter()
        .filter(|(_, _, deleted_at, referenced, leased)| {
            deleted_at.is_none() && !*referenced && !*leased
        })
        .map(|(id, _, _, _, _)| id.clone())
        .collect();
    let extra_blob_sha256 = blob_rows
        .iter()
        .filter(|(_, hash, _)| {
            !attachment_hashes.contains(hash) && !leased_blob_hashes.contains(hash)
        })
        .map(|(_, hash, _)| hex(hash))
        .collect();
    let mut corrupt_blob_sha256 = Vec::new();
    for (rowid, expected, byte_length) in blob_rows {
        let mut blob = media.blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)?;
        let actual_length = i64::try_from(blob.len()).expect("SQLite blob length fits i64");
        let actual = digest_reader(&mut blob)?;
        if actual.as_slice() != expected.as_slice() || actual_length != byte_length {
            corrupt_blob_sha256.push(hex(&expected));
        }
    }
    Ok(MediaIntegrityReport {
        missing_blob_attachment_ids,
        corrupt_blob_sha256,
        extra_blob_sha256,
        orphaned_attachment_ids,
    })
}

pub(crate) fn maintain_media(
    main: &mut Connection,
    media: &mut Connection,
    now_ms: i64,
    limits: MediaLimits,
) -> Result<MediaMaintenanceReport> {
    validate_timestamp(now_ms, "nowMs")?;
    let limits = limits.validate()?;
    let cleanup = recover_media_lifecycle(main, media, now_ms, limits)?;
    let integrity = integrity_report(main, media, now_ms)?;
    Ok(MediaMaintenanceReport {
        inspected_at_ms: now_ms,
        integrity,
        cleanup,
    })
}

pub(crate) fn recover_media_lifecycle(
    main: &mut Connection,
    media: &mut Connection,
    now_ms: i64,
    limits: MediaLimits,
) -> Result<MediaCleanupResult> {
    validate_timestamp(now_ms, "nowMs")?;
    reconcile_and_reap(main, media, now_ms, limits.validate()?)
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
        let next = match (next_image, next_attachment) {
            (Some(image), Some(attachment)) => {
                if image.0 <= attachment.0 {
                    image
                } else {
                    attachment
                }
            }
            (Some(image), None) => image,
            (None, Some(attachment)) => attachment,
            (None, None) => break,
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

fn parse_token_payload(payload: &str, role: AttachmentDisplayRole) -> Option<&str> {
    let id = match role {
        AttachmentDisplayRole::Attachment => payload,
        AttachmentDisplayRole::Inline => {
            let (id, width) = payload.split_once(";width=")?;
            let width = width.strip_suffix('%')?;
            let parsed = width.parse::<u32>().ok()?;
            if !(10..=100).contains(&parsed) || parsed.to_string() != width {
                return None;
            }
            id
        }
    };
    validate_uuid_v7(id, "attachmentId").is_ok().then_some(id)
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
                 WHERE draft_lease.draft_id = ?1
                   AND lease.attachment_id = ?2
                   AND lease.state = 'COMMITTED'
                   AND lease.expires_at > ?3",
                params![draft_id, &reference.id, now_ms],
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

fn reconcile_and_reap(
    main: &mut Connection,
    media: &mut Connection,
    now_ms: i64,
    limits: MediaLimits,
) -> Result<MediaCleanupResult> {
    let (cleanup, _) = reconcile_and_reap_from(main, media, now_ms, limits, None, true, true)?;
    Ok(cleanup)
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
             FROM attachment
             WHERE sha256 = ?1
               AND (
                    deleted_at IS NULL
                    OR EXISTS (
                        SELECT 1 FROM tidbit_revision_attachment AS membership
                        WHERE membership.attachment_id = attachment.id
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
