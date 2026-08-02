#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    collections::BTreeSet,
    ffi::{CStr, CString, OsString},
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStringExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::database::{
    create_empty_restore_media_database_at, open_restore_main_read_only_at,
    validate_restored_pair_at, DatabasePaths,
};

use super::{
    domain::{
        BackupSetId, CheckpointId, CheckpointManifestV1, ContentSha256, R2Keyspace, ReplicaEpochId,
        MAX_MANIFEST_BYTES, OBJECT_FORMAT_VERSION,
    },
    litestream::{
        LitestreamError, LitestreamTxid, RelationalRestoreEngine, ReplicaKind, RestorePlan,
    },
    object_store::{ListedObject, ObjectContentType, ObjectStore, ObjectStoreError},
    owner::{inspect_remote_owner, RemoteOwnerError, RemoteOwnerSnapshot},
};

const MAX_DISCOVERED_CHECKPOINTS: usize = 10_000;
const MAX_LIST_PAGES: usize = 100;
const MEDIA_RESTORE_PAGE_SIZE: u32 = 256;
const STAGING_FILENAMES: [&str; 7] = [
    "kosh.sqlite3",
    "kosh.sqlite3-wal",
    "kosh.sqlite3-shm",
    "media.sqlite3",
    "media.sqlite3-wal",
    "media.sqlite3-shm",
    "kosh.lock",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteCheckpoint {
    manifest: CheckpointManifestV1,
    created_at_unix_nanos: i128,
}

impl RemoteCheckpoint {
    pub(crate) fn checkpoint_id(&self) -> &CheckpointId {
        self.manifest.checkpoint_id()
    }

    pub(crate) fn replica_epoch_id(&self) -> &ReplicaEpochId {
        self.manifest.replica_epoch_id()
    }

    pub(crate) fn created_at(&self) -> &str {
        self.manifest.created_at().as_str()
    }

    pub(crate) fn kosh_version(&self) -> &str {
        self.manifest.kosh_version()
    }

    pub(crate) fn litestream_path(&self) -> &str {
        self.manifest.litestream_path()
    }

    pub(crate) const fn content_revision(&self) -> u64 {
        self.manifest.content_revision()
    }

    pub(crate) fn txid(&self) -> Result<LitestreamTxid, RestoreError> {
        self.manifest
            .txid()
            .parse()
            .map_err(RestoreError::Litestream)
    }

    pub(crate) const fn referenced_hash_count(&self) -> u64 {
        self.manifest.referenced_hash_count()
    }

    pub(crate) const fn referenced_total_bytes(&self) -> u64 {
        self.manifest.referenced_total_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestorePreview {
    pub(crate) checkpoint: RemoteCheckpoint,
    pub(crate) owner: RemoteOwnerSnapshot,
    pub(crate) plan_file_count: u64,
    pub(crate) plan_total_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct StagedRestore {
    pub(crate) paths: DatabasePaths,
    pub(crate) restored_media_count: u64,
    pub(crate) restored_media_bytes: u64,
    cleanup: StagingOwnership,
}

#[derive(Debug)]
pub(crate) struct StagedDatabasePair {
    main: File,
    media: File,
}

impl StagedDatabasePair {
    pub(crate) const fn main(&self) -> &File {
        &self.main
    }

    pub(crate) const fn media(&self) -> &File {
        &self.media
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(paths: &DatabasePaths) -> Result<Self, RestoreError> {
        let directory = open_directory_no_follow(&paths.root)?;
        open_validated_database_pair(paths, &directory)
    }
}

impl StagedRestore {
    pub(crate) fn open_validated_database_pair(&self) -> Result<StagedDatabasePair, RestoreError> {
        open_validated_database_pair(&self.paths, &self.cleanup.directory)
    }

    pub(crate) const fn staging_directory(&self) -> &File {
        &self.cleanup.directory
    }
}

#[derive(Debug)]
struct StagingOwnership {
    root: PathBuf,
    directory: File,
    cleanup_authorized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StagingDirectoryIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Debug)]
struct StagingChild {
    name: String,
    file: File,
}

impl Drop for StagingOwnership {
    fn drop(&mut self) {
        if self.cleanup_authorized {
            let _ = self.remove();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestoreDrillReport {
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) restored_media_count: u64,
    pub(crate) restored_media_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RestoreError {
    #[error("remote checkpoint storage is unavailable")]
    Store(#[from] ObjectStoreError),
    #[error("remote checkpoint manifest is invalid")]
    Manifest,
    #[error("too many remote checkpoints were returned")]
    TooManyCheckpoints,
    #[error("the requested remote checkpoint was not found")]
    CheckpointNotFound,
    #[error("remote backup ownership could not be inspected")]
    Owner(#[from] RemoteOwnerError),
    #[error("relational restore failed")]
    Litestream(#[from] LitestreamError),
    #[error("restored database validation or installation failed")]
    Database(#[from] crate::database::DatabaseError),
    #[error("restore filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("restored media does not match the checkpoint manifest")]
    MediaMismatch,
    #[error("restore staging directory is invalid")]
    InvalidStaging,
}

fn load_checkpoint_manifests(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
) -> Result<Vec<RemoteCheckpoint>, RestoreError> {
    let prefix = keyspace.checkpoint_prefix();
    let mut continuation = None;
    let mut checkpoints = Vec::new();
    let mut seen_keys = BTreeSet::new();
    let mut seen_checkpoint_ids = BTreeSet::new();
    let mut pages = 0_usize;
    loop {
        pages += 1;
        if pages > MAX_LIST_PAGES {
            return Err(RestoreError::TooManyCheckpoints);
        }
        let page = store.list(&prefix, continuation.as_ref())?;
        if page.objects.is_empty() && page.next.is_some() {
            return Err(RestoreError::Manifest);
        }
        for listed in page.objects {
            if !seen_keys.insert(listed.key.as_str().to_owned()) {
                return Err(RestoreError::Manifest);
            }
            if checkpoints.len() >= MAX_DISCOVERED_CHECKPOINTS {
                return Err(RestoreError::TooManyCheckpoints);
            }
            let checkpoint = load_checkpoint_manifest(store, keyspace, backup_set_id, &listed)?;
            if !seen_checkpoint_ids.insert(checkpoint.checkpoint_id().clone()) {
                return Err(RestoreError::Manifest);
            }
            checkpoints.push(checkpoint);
        }
        continuation = page.next;
        if continuation.is_none() {
            break;
        }
    }
    Ok(checkpoints)
}

fn load_checkpoint_manifest(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
    listed: &ListedObject,
) -> Result<RemoteCheckpoint, RestoreError> {
    if listed.byte_length == 0 || listed.byte_length > MAX_MANIFEST_BYTES as u64 {
        return Err(RestoreError::Manifest);
    }
    let result = store.get_bounded(&listed.key, MAX_MANIFEST_BYTES)?;
    if result.metadata.byte_length != listed.byte_length
        || result.metadata.version != listed.version
        || result.metadata.byte_length != result.bytes.len() as u64
        || result.metadata.content_type != Some(ObjectContentType::Json)
        || result.metadata.kosh_sha256.is_some()
        || result.metadata.object_format_version != Some(OBJECT_FORMAT_VERSION)
    {
        return Err(RestoreError::Manifest);
    }
    let manifest = CheckpointManifestV1::from_json(&result.bytes, keyspace)
        .map_err(|_| RestoreError::Manifest)?;
    if manifest.backup_set_id() != backup_set_id
        || manifest
            .object_key(keyspace)
            .map_err(|_| RestoreError::Manifest)?
            != listed.key
    {
        return Err(RestoreError::Manifest);
    }
    let created_at_unix_nanos = manifest
        .created_at()
        .unix_timestamp_nanos()
        .map_err(|_| RestoreError::Manifest)?;
    Ok(RemoteCheckpoint {
        manifest,
        created_at_unix_nanos,
    })
}

fn is_exact_checkpoint_candidate(prefix: &str, key: &str, checkpoint_suffix: &str) -> bool {
    let Some(relative) = key.strip_prefix(prefix) else {
        return false;
    };
    let Some((epoch, filename)) = relative.split_once('/') else {
        return false;
    };
    !epoch.is_empty() && !filename.contains('/') && filename.ends_with(checkpoint_suffix)
}

pub(crate) fn discover_checkpoint(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
    checkpoint_id: &CheckpointId,
) -> Result<RemoteCheckpoint, RestoreError> {
    let prefix = keyspace.checkpoint_prefix();
    let checkpoint_suffix = format!("-{}.json", checkpoint_id.as_str());
    let mut continuation = None;
    let mut candidate = None;
    let mut pages = 0_usize;
    loop {
        pages += 1;
        if pages > MAX_LIST_PAGES {
            return Err(RestoreError::TooManyCheckpoints);
        }
        let page = store.list(&prefix, continuation.as_ref())?;
        if page.objects.is_empty() && page.next.is_some() {
            return Err(RestoreError::Manifest);
        }
        for listed in page.objects {
            if !is_exact_checkpoint_candidate(
                prefix.as_str(),
                listed.key.as_str(),
                &checkpoint_suffix,
            ) {
                continue;
            }
            if candidate.replace(listed).is_some() {
                return Err(RestoreError::Manifest);
            }
        }
        continuation = page.next;
        if continuation.is_none() {
            break;
        }
    }
    let listed = candidate.ok_or(RestoreError::CheckpointNotFound)?;
    let checkpoint = load_checkpoint_manifest(store, keyspace, backup_set_id, &listed)?;
    if checkpoint.checkpoint_id() != checkpoint_id {
        return Err(RestoreError::Manifest);
    }
    Ok(checkpoint)
}

pub(crate) fn discover_checkpoints(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
) -> Result<Vec<RemoteCheckpoint>, RestoreError> {
    let mut checkpoints = load_checkpoint_manifests(store, keyspace, backup_set_id)?;
    let owner = inspect_remote_owner(store, keyspace)?;
    if owner.backup_set_id() != backup_set_id {
        return Err(RestoreError::Owner(RemoteOwnerError::Invalid));
    }
    checkpoints.sort_by(|left, right| {
        let left_is_active = left.replica_epoch_id() == &owner.replica_epoch_id;
        let right_is_active = right.replica_epoch_id() == &owner.replica_epoch_id;
        right_is_active.cmp(&left_is_active).then_with(|| {
            if left_is_active {
                right
                    .content_revision()
                    .cmp(&left.content_revision())
                    .then_with(|| right.created_at_unix_nanos.cmp(&left.created_at_unix_nanos))
                    .then_with(|| right.checkpoint_id().cmp(left.checkpoint_id()))
            } else {
                right
                    .created_at_unix_nanos
                    .cmp(&left.created_at_unix_nanos)
                    .then_with(|| right.replica_epoch_id().cmp(left.replica_epoch_id()))
                    .then_with(|| right.content_revision().cmp(&left.content_revision()))
                    .then_with(|| right.checkpoint_id().cmp(left.checkpoint_id()))
            }
        })
    });
    Ok(checkpoints)
}

pub(crate) fn preview_checkpoint(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
    checkpoint_id: &CheckpointId,
    engine: &dyn RelationalRestoreEngine,
    source_database_path: &Path,
    preview_target_path: &Path,
) -> Result<RestorePreview, RestoreError> {
    let checkpoint = discover_checkpoints(store, keyspace, backup_set_id)?
        .into_iter()
        .find(|checkpoint| checkpoint.checkpoint_id() == checkpoint_id)
        .ok_or(RestoreError::CheckpointNotFound)?;
    validate_replica_binding(engine, &checkpoint)?;
    let plan = engine.preview(
        source_database_path,
        preview_target_path,
        checkpoint.txid()?,
    )?;
    validate_plan(
        &plan,
        source_database_path,
        preview_target_path,
        &checkpoint,
    )?;
    let plan_file_count = u64::try_from(plan.files.len()).map_err(|_| RestoreError::Manifest)?;
    let plan_total_bytes = plan.files.iter().try_fold(0_u64, |total, file| {
        total.checked_add(file.size).ok_or(RestoreError::Manifest)
    })?;
    Ok(RestorePreview {
        checkpoint,
        owner: inspect_remote_owner(store, keyspace)?,
        plan_file_count,
        plan_total_bytes,
    })
}

pub(crate) fn stage_checkpoint(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    engine: &dyn RelationalRestoreEngine,
    source_database_path: &Path,
    staging_root: &Path,
) -> Result<StagedRestore, RestoreError> {
    stage_checkpoint_with_identity(
        store,
        keyspace,
        checkpoint,
        engine,
        source_database_path,
        staging_root,
        None,
    )
}

pub(crate) fn stage_checkpoint_at_identity(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    engine: &dyn RelationalRestoreEngine,
    source_database_path: &Path,
    staging_root: &Path,
    identity: StagingDirectoryIdentity,
) -> Result<StagedRestore, RestoreError> {
    stage_checkpoint_with_identity(
        store,
        keyspace,
        checkpoint,
        engine,
        source_database_path,
        staging_root,
        Some(identity),
    )
}

fn stage_checkpoint_with_identity(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    engine: &dyn RelationalRestoreEngine,
    source_database_path: &Path,
    staging_root: &Path,
    expected_identity: Option<StagingDirectoryIdentity>,
) -> Result<StagedRestore, RestoreError> {
    let paths = DatabasePaths::new(staging_root);
    let cleanup = StagingOwnership::prepare(staging_root, expected_identity)?;
    (|| {
        validate_replica_binding(engine, checkpoint)?;
        let txid = checkpoint.txid()?;
        let plan = engine.preview(source_database_path, &paths.main, txid)?;
        validate_plan(&plan, source_database_path, &paths.main, checkpoint)?;
        let result = engine.restore(source_database_path, &cleanup.directory, &paths.main, txid)?;
        if result.database_path != paths.main
            || result.replica != ReplicaKind::S3
            || result.txid != txid
        {
            return Err(RestoreError::Manifest);
        }
        let main_file = open_regular_child_read_write(&cleanup.directory, "kosh.sqlite3")?;
        let main = open_restore_main_read_only_at(&main_file)?;
        validate_checkpoint_evidence(&main, checkpoint)?;
        let (restored_media_count, restored_media_bytes, media_file) =
            rebuild_media(store, keyspace, checkpoint, &main, &cleanup.directory)?;
        drop(main);
        validate_restored_pair_at(&main_file, &media_file)?;
        if !path_matches_open_file(&paths.root, &cleanup.directory) {
            return Err(RestoreError::InvalidStaging);
        }
        Ok(StagedRestore {
            paths,
            restored_media_count,
            restored_media_bytes,
            cleanup,
        })
    })()
}

pub(crate) fn remove_staged_checkpoint(staged: &StagedRestore) -> Result<(), RestoreError> {
    staged.cleanup.remove()
}

#[cfg(test)]
pub(crate) fn remove_staging_root(root: &Path) -> Result<(), RestoreError> {
    let Some(mut cleanup) = StagingOwnership::open(root)? else {
        return Ok(());
    };
    cleanup.authorize_existing_cleanup()?;
    cleanup.remove()
}

pub(crate) fn remove_staging_root_at_identity(
    root: &Path,
    identity: StagingDirectoryIdentity,
) -> Result<bool, RestoreError> {
    let Some(mut cleanup) = StagingOwnership::open(root)? else {
        return Ok(true);
    };
    if cleanup.identity()? != identity {
        return Ok(false);
    }
    cleanup.authorize_existing_cleanup()?;
    cleanup.remove()?;
    Ok(true)
}

pub(crate) fn drill_checkpoint(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    engine: &dyn RelationalRestoreEngine,
    source_database_path: &Path,
    drill_root: &Path,
) -> Result<RestoreDrillReport, RestoreError> {
    let staged = stage_checkpoint(
        store,
        keyspace,
        checkpoint,
        engine,
        source_database_path,
        drill_root,
    )?;
    let report = RestoreDrillReport {
        checkpoint_id: checkpoint.checkpoint_id().clone(),
        restored_media_count: staged.restored_media_count,
        restored_media_bytes: staged.restored_media_bytes,
    };
    staged.cleanup.remove()?;
    Ok(report)
}

fn validate_replica_binding(
    engine: &dyn RelationalRestoreEngine,
    checkpoint: &RemoteCheckpoint,
) -> Result<(), RestoreError> {
    if engine.replica_path() != checkpoint.litestream_path() {
        return Err(RestoreError::Manifest);
    }
    Ok(())
}

fn validate_plan(
    plan: &RestorePlan,
    source: &Path,
    target: &Path,
    checkpoint: &RemoteCheckpoint,
) -> Result<(), RestoreError> {
    let txid = checkpoint.txid()?;
    if plan.source != source.to_string_lossy()
        || plan.target_path != target
        || plan.replica != ReplicaKind::S3
        || plan.min_txid > txid
        || plan.max_txid < txid
        || plan.files.is_empty()
        || !plan
            .files
            .iter()
            .any(|file| file.min_txid <= txid && file.max_txid >= txid)
    {
        return Err(RestoreError::Manifest);
    }
    Ok(())
}

fn validate_checkpoint_evidence(
    connection: &Connection,
    checkpoint: &RemoteCheckpoint,
) -> Result<(), RestoreError> {
    let content_revision = connection
        .query_row(
            "SELECT revision
             FROM offsite_backup_content_clock
             WHERE singleton_id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(crate::database::DatabaseError::from)?;
    let evidence = connection
        .query_row(
            "SELECT backup_set_id, replica_epoch_id, content_revision, created_at,
                    kosh_version, main_migration_head, media_migration_head,
                    referenced_hash_count, referenced_total_bytes,
                    referenced_hash_set_sha256
             FROM offsite_backup_checkpoint
             WHERE checkpoint_id = ?1
               AND phase = 'PREPARED'
               AND litestream_txid IS NULL
               AND manifest_object_key IS NULL
               AND publication_sequence IS NULL
               AND last_error_code IS NULL",
            [checkpoint.checkpoint_id().as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(crate::database::DatabaseError::from)?;
    let Some((
        backup_set_id,
        replica_epoch_id,
        checkpoint_content_revision,
        created_at_ms,
        kosh_version,
        main_migration_head,
        media_migration_head,
        referenced_hash_count,
        referenced_total_bytes,
        referenced_hash_set_sha256,
    )) = evidence
    else {
        return Err(RestoreError::Manifest);
    };
    let matches = u64::try_from(content_revision) == Ok(checkpoint.content_revision())
        && backup_set_id == checkpoint.manifest.backup_set_id().as_str()
        && replica_epoch_id == checkpoint.replica_epoch_id().as_str()
        && u64::try_from(checkpoint_content_revision) == Ok(checkpoint.content_revision())
        && created_at_ms >= 0
        && i128::from(created_at_ms).checked_mul(1_000_000)
            == Some(checkpoint.created_at_unix_nanos)
        && kosh_version == checkpoint.kosh_version()
        && u32::try_from(main_migration_head) == Ok(checkpoint.manifest.main_migration_head())
        && u32::try_from(media_migration_head) == Ok(checkpoint.manifest.media_migration_head())
        && u64::try_from(referenced_hash_count) == Ok(checkpoint.referenced_hash_count())
        && u64::try_from(referenced_total_bytes) == Ok(checkpoint.referenced_total_bytes())
        && referenced_hash_set_sha256
            == checkpoint
                .manifest
                .referenced_hash_set_sha256()
                .as_bytes()
                .as_slice();
    if !matches {
        return Err(RestoreError::Manifest);
    }
    Ok(())
}

fn rebuild_media(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    main: &Connection,
    staging_directory: &File,
) -> Result<(u64, u64, File), RestoreError> {
    let media_file = create_regular_child(staging_directory, "media.sqlite3")?;
    let mut media = create_empty_restore_media_database_at(&media_file)?;
    let transaction = media
        .transaction()
        .map_err(crate::database::DatabaseError::from)?;
    let mut count = 0_u64;
    let mut total_bytes = 0_u64;
    let mut digest = Sha256::new();
    let mut cursor = None;
    loop {
        let references = load_referenced_media_page(main, cursor, MEDIA_RESTORE_PAGE_SIZE)?;
        if references.is_empty() {
            break;
        }
        if cursor.is_some_and(|previous| references[0].sha256.as_bytes() <= previous.as_bytes())
            || references
                .windows(2)
                .any(|pair| pair[0].sha256.as_bytes() >= pair[1].sha256.as_bytes())
        {
            return Err(RestoreError::MediaMismatch);
        }
        for reference in &references {
            let key = keyspace.media(reference.sha256);
            let result = store.get(&key)?;
            if result.metadata.byte_length != reference.byte_length
                || result.metadata.byte_length != result.bytes.len() as u64
                || result.metadata.content_type != Some(ObjectContentType::Binary)
                || result.metadata.kosh_sha256 != Some(reference.sha256)
                || result.metadata.object_format_version != Some(OBJECT_FORMAT_VERSION)
                || ContentSha256::from_bytes(Sha256::digest(&result.bytes).into())
                    != reference.sha256
            {
                return Err(RestoreError::MediaMismatch);
            }
            transaction
                .execute(
                    "INSERT INTO media_blob(sha256, bytes, byte_length, created_at)
                     VALUES(?1, ?2, ?3, 0)",
                    params![
                        reference.sha256.as_bytes().as_slice(),
                        result.bytes,
                        i64::try_from(reference.byte_length)
                            .map_err(|_| RestoreError::MediaMismatch)?,
                    ],
                )
                .map_err(crate::database::DatabaseError::from)?;
            count = count.checked_add(1).ok_or(RestoreError::MediaMismatch)?;
            total_bytes = total_bytes
                .checked_add(reference.byte_length)
                .ok_or(RestoreError::MediaMismatch)?;
            digest.update(reference.sha256.as_bytes());
        }
        cursor = references.last().map(|reference| reference.sha256);
    }
    transaction
        .commit()
        .map_err(crate::database::DatabaseError::from)?;
    drop(media);
    media_file.sync_all()?;
    staging_directory.sync_all()?;
    if count != checkpoint.manifest.referenced_hash_count()
        || total_bytes != checkpoint.manifest.referenced_total_bytes()
        || ContentSha256::from_bytes(digest.finalize().into())
            != checkpoint.manifest.referenced_hash_set_sha256()
    {
        return Err(RestoreError::MediaMismatch);
    }
    Ok((count, total_bytes, media_file))
}

#[derive(Clone, Copy)]
struct MediaReference {
    sha256: ContentSha256,
    byte_length: u64,
}

fn load_referenced_media_page(
    main: &Connection,
    after_sha256: Option<ContentSha256>,
    limit: u32,
) -> Result<Vec<MediaReference>, RestoreError> {
    let mut statement = main
        .prepare(
            "WITH referenced(sha256, byte_length) AS (
                SELECT attachment.sha256, attachment.byte_length
                FROM attachment
                WHERE attachment.deleted_at IS NULL
                   OR EXISTS (
                        SELECT 1 FROM tidbit_revision_attachment
                        WHERE attachment_id = attachment.id
                   )
                UNION ALL
                SELECT image.preview_sha256, image.preview_byte_length
                FROM attachment_image AS image
                JOIN attachment ON attachment.id = image.attachment_id
                WHERE attachment.deleted_at IS NULL
                   OR EXISTS (
                        SELECT 1 FROM tidbit_revision_attachment
                        WHERE attachment_id = attachment.id
                   )
             )
             SELECT sha256, min(byte_length), max(byte_length)
             FROM referenced
             WHERE (?1 IS NULL OR sha256 > ?1)
             GROUP BY sha256
             ORDER BY sha256
             LIMIT ?2",
        )
        .map_err(crate::database::DatabaseError::from)?;
    let cursor = after_sha256.map(|value| value.as_bytes().to_vec());
    let rows = statement
        .query_map(params![cursor, i64::from(limit)], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(crate::database::DatabaseError::from)?;
    let mut references = Vec::new();
    for row in rows {
        let (sha256, minimum, maximum) = row.map_err(crate::database::DatabaseError::from)?;
        if minimum <= 0 || minimum != maximum {
            return Err(RestoreError::MediaMismatch);
        }
        let bytes: [u8; 32] = sha256.try_into().map_err(|_| RestoreError::MediaMismatch)?;
        references.push(MediaReference {
            sha256: ContentSha256::from_bytes(bytes),
            byte_length: u64::try_from(minimum).map_err(|_| RestoreError::MediaMismatch)?,
        });
    }
    Ok(references)
}

impl StagingOwnership {
    fn prepare(
        root: &Path,
        expected_identity: Option<StagingDirectoryIdentity>,
    ) -> Result<Self, RestoreError> {
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        match builder.create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let Some(mut ownership) = Self::open(root)? else {
            return Err(RestoreError::InvalidStaging);
        };
        if !directory_entries(&ownership.directory)?.is_empty() {
            return Err(RestoreError::InvalidStaging);
        }
        if expected_identity.is_some_and(|expected| {
            ownership
                .identity()
                .map_or(true, |actual| actual != expected)
        }) {
            return Err(RestoreError::InvalidStaging);
        }
        ownership.cleanup_authorized = true;
        let chmod = unsafe { libc::fchmod(ownership.directory.as_raw_fd(), 0o700) };
        if chmod != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        ownership.directory.sync_all()?;
        Ok(ownership)
    }

    fn open(root: &Path) -> Result<Option<Self>, RestoreError> {
        let metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RestoreError::InvalidStaging);
        }
        let directory = open_directory_no_follow(root)?;
        if !path_matches_open_file(root, &directory) || !is_current_user_directory(&directory)? {
            return Err(RestoreError::InvalidStaging);
        }
        Ok(Some(Self {
            root: root.to_owned(),
            directory,
            cleanup_authorized: false,
        }))
    }

    fn authorize_existing_cleanup(&mut self) -> Result<(), RestoreError> {
        let _ = inspect_staging_children(&self.directory)?;
        self.cleanup_authorized = true;
        Ok(())
    }

    fn identity(&self) -> Result<StagingDirectoryIdentity, RestoreError> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self.directory.metadata()?;
        Ok(StagingDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn remove(&self) -> Result<(), RestoreError> {
        if !self.cleanup_authorized {
            return Err(RestoreError::InvalidStaging);
        }
        let inspected = inspect_staging_children(&self.directory)?;
        for child in &inspected {
            unlink_owned_child(&self.directory, &child.name, &child.file)?;
        }
        self.directory.sync_all()?;
        if !directory_entries(&self.directory)?.is_empty() {
            return Err(RestoreError::InvalidStaging);
        }
        if path_matches_open_file(&self.root, &self.directory) {
            fs::remove_dir(&self.root)?;
            if let Some(parent) = self.root.parent() {
                File::open(parent)?.sync_all()?;
            }
        }
        Ok(())
    }
}

fn inspect_staging_children(directory: &File) -> Result<Vec<StagingChild>, RestoreError> {
    let mut children = Vec::new();
    for entry in directory_entries(directory)? {
        let Some(name) = entry.to_str() else {
            return Err(RestoreError::InvalidStaging);
        };
        if !STAGING_FILENAMES.contains(&name) {
            return Err(RestoreError::InvalidStaging);
        }
        let file = open_regular_child(directory, name)?;
        if !is_current_user_file(&file)? {
            return Err(RestoreError::InvalidStaging);
        }
        children.push(StagingChild {
            name: name.to_owned(),
            file,
        });
    }
    Ok(children)
}

fn open_validated_database_pair(
    paths: &DatabasePaths,
    directory: &File,
) -> Result<StagedDatabasePair, RestoreError> {
    if paths.main != paths.root.join("kosh.sqlite3")
        || paths.media != paths.root.join("media.sqlite3")
        || !path_matches_open_file(&paths.root, directory)
    {
        return Err(RestoreError::InvalidStaging);
    }
    let main = open_regular_child_read_write(directory, "kosh.sqlite3")?;
    let media = open_regular_child_read_write(directory, "media.sqlite3")?;
    if !is_current_user_file(&main)? || !is_current_user_file(&media)? {
        return Err(RestoreError::InvalidStaging);
    }

    // Validate through the retained child descriptors, then prove the
    // discoverable staging path still identifies the retained directory and
    // exact files. Every later copy and comparison uses these descriptors, so
    // a same-UID rename/replacement after this point cannot substitute another
    // library.
    validate_restored_pair_at(&main, &media)?;
    let current_main = open_regular_child_read_write(directory, "kosh.sqlite3")?;
    let current_media = open_regular_child_read_write(directory, "media.sqlite3")?;
    if !path_matches_open_file(&paths.root, directory)
        || !same_open_file(&main, &current_main)
        || !same_open_file(&media, &current_media)
    {
        return Err(RestoreError::InvalidStaging);
    }
    Ok(StagedDatabasePair { main, media })
}

fn open_directory_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let directory = options.open(path)?;
    if !directory.metadata()?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "restore staging root is not a directory",
        ));
    }
    Ok(directory)
}

fn open_regular_child(directory: &File, name: &str) -> std::io::Result<File> {
    let name = child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "restore staging child is not a regular file",
        ));
    }
    Ok(file)
}

fn open_regular_child_read_write(directory: &File, name: &str) -> std::io::Result<File> {
    let name = child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "restore staging child is not a regular file",
        ));
    }
    Ok(file)
}

fn create_regular_child(directory: &File, name: &str) -> std::io::Result<File> {
    let name = child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "restore staging child is not a regular file",
        ));
    }
    Ok(file)
}

fn unlink_owned_child(directory: &File, name: &str, owned: &File) -> std::io::Result<()> {
    let current = match open_regular_child(directory, name) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !same_open_file(&current, owned) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "restore staging child identity changed",
        ));
    }
    let name = child_name(name)?;
    let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn directory_entries(directory: &File) -> std::io::Result<Vec<OsString>> {
    let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        unsafe {
            libc::close(descriptor);
        }
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        libc::rewinddir(stream);
    }
    let mut entries = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            entries.push(OsString::from_vec(name.to_vec()));
        }
    }
    if unsafe { libc::closedir(stream) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    entries.sort();
    Ok(entries)
}

fn child_name(name: &str) -> std::io::Result<CString> {
    if name.is_empty() || name.as_bytes().contains(&b'/') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid restore staging child name",
        ));
    }
    CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid restore staging child name",
        )
    })
}

fn path_matches_open_file(path: &Path, open: &File) -> bool {
    let Ok(path_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_dir() {
        return false;
    }
    let Ok(open_metadata) = open.metadata() else {
        return false;
    };
    same_metadata(&path_metadata, &open_metadata)
}

fn same_open_file(left: &File, right: &File) -> bool {
    left.metadata()
        .ok()
        .zip(right.metadata().ok())
        .is_some_and(|(left, right)| same_metadata(&left, &right))
}

fn same_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn is_current_user_directory(directory: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = directory.metadata()?;
    Ok(metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() })
}

fn is_current_user_file(file: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(metadata.is_file() && metadata.uid() == unsafe { libc::geteuid() })
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, path::PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::{
        backup::{
            domain::{
                BackupWriterId, CheckpointManifestInput, R2AccountId, R2BucketName, R2Jurisdiction,
                R2ObjectKey, R2Target, UtcTimestamp,
            },
            litestream::{IntegrityCheck, RestoreFile, RestoreResult},
            object_store::{
                fake::{FakeObjectStore, ObjectOperation},
                ObjectContentType, PutCondition, PutMediaRequest, PutObjectRequest,
            },
            owner::claim_remote_owner,
        },
        database::{
            tidbits::CreateTidbitWrite, AttachmentIngestInput, Database, MediaLimits,
            PrepareOffsiteCheckpointInput, SaveOffsiteBackupConfigInput, SourceDraft, TidbitDraft,
        },
    };

    const TIDBIT_ID: &str = "019f547b-6200-7000-8000-00000000f001";
    const REVISION_ID: &str = "019f547b-6200-7000-8000-00000000f002";
    const SOURCE_ID: &str = "019f547b-6200-7000-8000-00000000f003";
    const DRAFT_ID: &str = "019f547b-6200-7000-8000-00000000f004";

    struct Fixture {
        _source_root: TempDir,
        source_paths: DatabasePaths,
        backup_set_id: BackupSetId,
        keyspace: R2Keyspace,
        store: FakeObjectStore,
        checkpoint: RemoteCheckpoint,
        engine: FakeRestoreEngine,
    }

    struct FakeRestoreEngine {
        source: PathBuf,
        replica_path: R2ObjectKey,
        txid: LitestreamTxid,
    }

    struct ParentReplacingRestoreEngine<'a> {
        inner: &'a FakeRestoreEngine,
        requested_root: PathBuf,
        displaced_root: PathBuf,
    }

    impl RelationalRestoreEngine for ParentReplacingRestoreEngine<'_> {
        fn replica_path(&self) -> &str {
            self.inner.replica_path()
        }

        fn preview(
            &self,
            source_database_path: &Path,
            target_path: &Path,
            txid: LitestreamTxid,
        ) -> Result<RestorePlan, LitestreamError> {
            self.inner.preview(source_database_path, target_path, txid)
        }

        fn restore(
            &self,
            source_database_path: &Path,
            target_directory: &File,
            target_path: &Path,
            txid: LitestreamTxid,
        ) -> Result<RestoreResult, LitestreamError> {
            fs::rename(&self.requested_root, &self.displaced_root)
                .map_err(LitestreamError::Execute)?;
            let replacement = Database::initialize(DatabasePaths::new(&self.requested_root))
                .map_err(|error| LitestreamError::Execute(std::io::Error::other(error)))?;
            replacement
                .shutdown()
                .map_err(|error| LitestreamError::Execute(std::io::Error::other(error)))?;
            self.inner
                .restore(source_database_path, target_directory, target_path, txid)
        }
    }

    impl RelationalRestoreEngine for FakeRestoreEngine {
        fn replica_path(&self) -> &str {
            self.replica_path.as_str()
        }

        fn preview(
            &self,
            source_database_path: &Path,
            target_path: &Path,
            txid: LitestreamTxid,
        ) -> Result<RestorePlan, LitestreamError> {
            assert_eq!(source_database_path, self.source);
            assert_eq!(txid, self.txid);
            Ok(RestorePlan {
                source: source_database_path.to_string_lossy().into_owned(),
                target_path: target_path.to_owned(),
                replica: ReplicaKind::S3,
                min_txid: txid,
                max_txid: txid,
                files: vec![RestoreFile {
                    level: 0,
                    name: format!("{txid}-{txid}.ltx"),
                    min_txid: txid,
                    max_txid: txid,
                    size: 4096,
                    timestamp: "2026-07-30T19:00:00Z".into(),
                }],
            })
        }

        fn restore(
            &self,
            source_database_path: &Path,
            target_directory: &File,
            target_path: &Path,
            txid: LitestreamTxid,
        ) -> Result<RestoreResult, LitestreamError> {
            assert_eq!(source_database_path, self.source);
            assert_eq!(txid, self.txid);
            let mut source = File::open(source_database_path).map_err(LitestreamError::Execute)?;
            let target_name = target_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LitestreamError::InvalidRestoreDestination)?;
            let mut target = create_regular_child(target_directory, target_name)
                .map_err(LitestreamError::Execute)?;
            std::io::copy(&mut source, &mut target).map_err(LitestreamError::Execute)?;
            target.sync_all().map_err(LitestreamError::Execute)?;
            target_directory
                .sync_all()
                .map_err(LitestreamError::Execute)?;
            Ok(RestoreResult {
                database_path: target_path.to_owned(),
                replica: ReplicaKind::S3,
                txid,
                duration_ms: 1,
                integrity_check: IntegrityCheck::Full,
            })
        }
    }

    impl Fixture {
        fn new() -> Self {
            let source_root = tempfile::tempdir().expect("source root");
            let source_paths = DatabasePaths::new(source_root.path());
            let database =
                Database::initialize(source_paths.clone()).expect("source database pair");
            let target = R2Target {
                account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef")
                    .expect("account"),
                jurisdiction: R2Jurisdiction::Default,
                bucket: R2BucketName::parse("kosh-restore-test").expect("bucket"),
            };
            let backup_set_id = BackupSetId::new();
            let epoch = ReplicaEpochId::new();
            database
                .client()
                .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                    expected_revision: 0,
                    backup_set_id: backup_set_id.clone(),
                    replica_epoch_id: epoch.clone(),
                    enabled: true,
                    target: target.clone(),
                    now_ms: 1,
                })
                .expect("enable source backup");
            database
                .client()
                .create_tidbit(CreateTidbitWrite {
                    input: TidbitDraft {
                        body_markdown: "Exact citrine recovery evidence.".into(),
                        sources: vec![SourceDraft {
                            label: Some("Recovery source".into()),
                            url: Some("https://example.com/recovery".into()),
                        }],
                    },
                    now_ms: 10,
                    tidbit_id: TIDBIT_ID.into(),
                    revision_id: REVISION_ID.into(),
                    source_ids: vec![SOURCE_ID.into()],
                })
                .expect("source tidbit");
            database
                .client()
                .save_working_copy_for_test(DRAFT_ID.into(), None, 1, String::new(), Vec::new(), 11)
                .expect("source working copy");
            let media_bytes = b"remote attachment evidence".to_vec();
            let attachment = database
                .ingest_attachment(
                    AttachmentIngestInput {
                        draft_id: DRAFT_ID.into(),
                        display_filename: "evidence.txt".into(),
                        media_type: "text/plain".into(),
                        now_ms: 12,
                        limits: MediaLimits::default(),
                    },
                    Cursor::new(media_bytes.clone()),
                )
                .expect("source attachment");
            let upload = database
                .client()
                .claim_next_offsite_media_upload(13, "019f547b-6200-7000-8000-00000000f005".into())
                .expect("claim source media upload")
                .expect("pending source media upload");
            assert!(database
                .client()
                .complete_offsite_media_upload(upload, "\"remote-version\"".into(), 14)
                .expect("complete source media upload"));
            let checkpoint_id = CheckpointId::new();
            let created_at =
                UtcTimestamp::parse("2026-07-30T19:00:00Z").expect("checkpoint timestamp");
            let created_at_ms = i64::try_from(
                created_at.unix_timestamp_nanos().expect("checkpoint epoch") / 1_000_000,
            )
            .expect("millisecond timestamp");
            let prepared = database
                .client()
                .prepare_offsite_checkpoint(PrepareOffsiteCheckpointInput {
                    checkpoint_id: checkpoint_id.clone(),
                    backup_set_id: backup_set_id.clone(),
                    replica_epoch_id: epoch.clone(),
                    created_at_ms,
                    kosh_version: env!("CARGO_PKG_VERSION").into(),
                })
                .expect("prepare source checkpoint");
            database.shutdown().expect("close source");

            let main = rusqlite::Connection::open(&source_paths.main).expect("source main");
            let sha256_bytes: Vec<u8> = main
                .query_row(
                    "SELECT sha256 FROM attachment WHERE id = ?1",
                    [attachment.id],
                    |row| row.get(0),
                )
                .expect("attachment hash");
            let sha256 = ContentSha256::from_bytes(
                sha256_bytes.try_into().expect("32-byte attachment hash"),
            );
            drop(main);

            let keyspace = target.keyspace(&backup_set_id);
            let store = FakeObjectStore::new(keyspace.clone());
            store
                .put_media(
                    PutMediaRequest::new(&keyspace, sha256, media_bytes.clone())
                        .expect("verified media"),
                )
                .expect("remote media");
            claim_remote_owner(
                &store,
                &keyspace,
                &backup_set_id,
                &epoch,
                &BackupWriterId::new(),
            )
            .expect("remote owner");
            let txid = LitestreamTxid::from_local(42);
            let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
                backup_set_id: prepared.backup_set_id,
                replica_epoch_id: prepared.replica_epoch_id,
                checkpoint_id: prepared.checkpoint_id,
                created_at,
                kosh_version: prepared.kosh_version,
                content_revision: prepared.content_revision,
                main_migration_head: prepared.main_migration_head,
                litestream_path: keyspace.litestream(&epoch),
                txid: txid.to_string(),
                media_migration_head: prepared.media_migration_head,
                referenced_hash_count: prepared.referenced_hash_count,
                referenced_total_bytes: prepared.referenced_total_bytes,
                referenced_hash_set_sha256: prepared.referenced_hash_set_sha256,
            })
            .expect("checkpoint manifest");
            let key = manifest.object_key(&keyspace).expect("manifest key");
            store
                .put(PutObjectRequest {
                    key,
                    bytes: manifest.to_json().expect("manifest bytes"),
                    content_type: ObjectContentType::Json,
                    kosh_sha256: None,
                    condition: PutCondition::IfAbsent,
                })
                .expect("remote manifest");
            let checkpoint = discover_checkpoints(&store, &keyspace, &backup_set_id)
                .expect("checkpoint discovery")
                .pop()
                .expect("checkpoint");
            let engine = FakeRestoreEngine {
                source: source_paths.main.clone(),
                replica_path: keyspace.litestream(&epoch),
                txid,
            };
            Self {
                _source_root: source_root,
                source_paths,
                backup_set_id,
                keyspace,
                store,
                checkpoint,
                engine,
            }
        }

        fn checkpoint_with_evidence(
            &self,
            checkpoint_id: CheckpointId,
            content_revision: u64,
        ) -> RemoteCheckpoint {
            let original = &self.checkpoint.manifest;
            let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
                backup_set_id: original.backup_set_id().clone(),
                replica_epoch_id: original.replica_epoch_id().clone(),
                checkpoint_id,
                created_at: original.created_at().clone(),
                kosh_version: original.kosh_version().into(),
                content_revision,
                main_migration_head: original.main_migration_head(),
                litestream_path: self.keyspace.litestream(original.replica_epoch_id()),
                txid: original.txid().into(),
                media_migration_head: original.media_migration_head(),
                referenced_hash_count: original.referenced_hash_count(),
                referenced_total_bytes: original.referenced_total_bytes(),
                referenced_hash_set_sha256: original.referenced_hash_set_sha256(),
            })
            .expect("modified checkpoint manifest");
            RemoteCheckpoint {
                created_at_unix_nanos: manifest
                    .created_at()
                    .unix_timestamp_nanos()
                    .expect("manifest timestamp"),
                manifest,
            }
        }
    }

    #[test]
    fn discovery_preview_and_non_mutating_drill_verify_the_complete_checkpoint() {
        let fixture = Fixture::new();
        let preview_root = tempfile::tempdir().expect("preview root");
        let preview = preview_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.backup_set_id,
            fixture.checkpoint.checkpoint_id(),
            &fixture.engine,
            &fixture.source_paths.main,
            &preview_root.path().join("preview.sqlite3"),
        )
        .expect("restore preview");
        assert_eq!(preview.plan_file_count, 1);
        assert_eq!(preview.plan_total_bytes, 4096);
        assert_eq!(preview.checkpoint.referenced_hash_count(), 1);

        let source_before = fs::read(&fixture.source_paths.main).expect("source bytes");
        let drill_parent = tempfile::tempdir().expect("drill parent");
        let drill_root = drill_parent.path().join("drill");
        let drill = drill_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &fixture.engine,
            &fixture.source_paths.main,
            &drill_root,
        )
        .expect("restore drill");
        assert_eq!(drill.restored_media_count, 1);
        assert!(!drill_root.exists());
        assert_eq!(
            fs::read(&fixture.source_paths.main).expect("source bytes after drill"),
            source_before
        );
    }

    #[test]
    fn preview_rejects_the_same_txid_from_a_different_replica_epoch() {
        let fixture = Fixture::new();
        let wrong_epoch_engine = FakeRestoreEngine {
            source: fixture.source_paths.main.clone(),
            replica_path: fixture.keyspace.litestream(&ReplicaEpochId::new()),
            txid: fixture.engine.txid,
        };
        let preview_root = tempfile::tempdir().expect("preview root");

        assert!(matches!(
            preview_checkpoint(
                &fixture.store,
                &fixture.keyspace,
                &fixture.backup_set_id,
                fixture.checkpoint.checkpoint_id(),
                &wrong_epoch_engine,
                &fixture.source_paths.main,
                &preview_root.path().join("preview.sqlite3"),
            ),
            Err(RestoreError::Manifest)
        ));
    }

    #[test]
    fn staging_rejects_manifest_evidence_not_bound_to_the_restored_txid() {
        let fixture = Fixture::new();
        let cases = [
            fixture.checkpoint_with_evidence(
                CheckpointId::new(),
                fixture.checkpoint.content_revision(),
            ),
            fixture.checkpoint_with_evidence(
                fixture.checkpoint.checkpoint_id().clone(),
                fixture.checkpoint.content_revision() + 1,
            ),
        ];
        let staging_parent = tempfile::tempdir().expect("staging parent");

        for (ordinal, checkpoint) in cases.iter().enumerate() {
            let staging_root = staging_parent.path().join(format!("mismatch-{ordinal}"));
            assert!(matches!(
                stage_checkpoint(
                    &fixture.store,
                    &fixture.keyspace,
                    checkpoint,
                    &fixture.engine,
                    &fixture.source_paths.main,
                    &staging_root,
                ),
                Err(RestoreError::Manifest)
            ));
            assert!(!staging_root.exists());
        }
    }

    #[test]
    fn staging_writes_stay_bound_after_the_requested_root_is_replaced() {
        let fixture = Fixture::new();
        let parent = tempfile::tempdir().expect("staging replacement parent");
        let staging_root = parent.path().join("staging");
        let displaced_root = parent.path().join("displaced-staging");
        let engine = ParentReplacingRestoreEngine {
            inner: &fixture.engine,
            requested_root: staging_root.clone(),
            displaced_root: displaced_root.clone(),
        };

        let result = stage_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &engine,
            &fixture.source_paths.main,
            &staging_root,
        );
        assert!(
            matches!(result, Err(RestoreError::InvalidStaging)),
            "unexpected staging result: {result:?}"
        );

        let replacement =
            Database::initialize(DatabasePaths::new(&staging_root)).expect("replacement database");
        let main = replacement
            .open_main_read_only()
            .expect("replacement main database");
        let media = replacement
            .open_media_read_only()
            .expect("replacement media database");
        assert_eq!(
            main.query_row("SELECT count(*) FROM tidbit", [], |row| row
                .get::<_, i64>(0))
                .expect("replacement tidbit count"),
            0
        );
        assert_eq!(
            media
                .query_row("SELECT count(*) FROM media_blob", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("replacement media count"),
            0
        );
        drop(media);
        drop(main);
        replacement.shutdown().expect("close replacement database");
        assert!(
            fs::read_dir(&displaced_root)
                .expect("displaced staging entries")
                .next()
                .is_none(),
            "descriptor-bound cleanup must remove private restored data"
        );
    }

    #[test]
    fn discovery_sorts_content_revision_before_a_backward_clock_timestamp() {
        let fixture = Fixture::new();
        let original = &fixture.checkpoint.manifest;
        let newer_checkpoint_id = CheckpointId::new();
        let newer = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: original.backup_set_id().clone(),
            replica_epoch_id: original.replica_epoch_id().clone(),
            checkpoint_id: newer_checkpoint_id.clone(),
            created_at: UtcTimestamp::parse("2026-07-30T18:59:59Z")
                .expect("backward-clock timestamp"),
            kosh_version: original.kosh_version().into(),
            content_revision: original.content_revision() + 1,
            main_migration_head: original.main_migration_head(),
            litestream_path: fixture.keyspace.litestream(original.replica_epoch_id()),
            txid: original.txid().into(),
            media_migration_head: original.media_migration_head(),
            referenced_hash_count: original.referenced_hash_count(),
            referenced_total_bytes: original.referenced_total_bytes(),
            referenced_hash_set_sha256: original.referenced_hash_set_sha256(),
        })
        .expect("newer manifest");
        fixture
            .store
            .put(PutObjectRequest {
                key: newer.object_key(&fixture.keyspace).expect("manifest key"),
                bytes: newer.to_json().expect("manifest bytes"),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("newer remote manifest");

        let checkpoints =
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id)
                .expect("checkpoint discovery");

        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].checkpoint_id(), &newer_checkpoint_id);
        assert_eq!(checkpoints[0].created_at(), "2026-07-30T18:59:59Z");
        assert_eq!(checkpoints[1].created_at(), "2026-07-30T19:00:00Z");
    }

    #[test]
    fn discovery_sorts_equal_revisions_by_fractional_timestamp() {
        let fixture = Fixture::new();
        let original = &fixture.checkpoint.manifest;
        let later_checkpoint_id = CheckpointId::new();
        let later = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: original.backup_set_id().clone(),
            replica_epoch_id: original.replica_epoch_id().clone(),
            checkpoint_id: later_checkpoint_id.clone(),
            created_at: UtcTimestamp::parse("2026-07-30T19:00:00.900Z")
                .expect("fractional timestamp"),
            kosh_version: original.kosh_version().into(),
            content_revision: original.content_revision(),
            main_migration_head: original.main_migration_head(),
            litestream_path: fixture.keyspace.litestream(original.replica_epoch_id()),
            txid: original.txid().into(),
            media_migration_head: original.media_migration_head(),
            referenced_hash_count: original.referenced_hash_count(),
            referenced_total_bytes: original.referenced_total_bytes(),
            referenced_hash_set_sha256: original.referenced_hash_set_sha256(),
        })
        .expect("later manifest");
        fixture
            .store
            .put(PutObjectRequest {
                key: later.object_key(&fixture.keyspace).expect("manifest key"),
                bytes: later.to_json().expect("manifest bytes"),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("later remote manifest");

        let checkpoints =
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id)
                .expect("checkpoint discovery");

        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].checkpoint_id(), &later_checkpoint_id);
        assert_eq!(checkpoints[0].created_at(), "2026-07-30T19:00:00.900Z");
        assert_eq!(checkpoints[1].created_at(), "2026-07-30T19:00:00Z");
    }

    #[test]
    fn discovery_prefers_the_active_epoch_before_cross_lineage_revisions() {
        let fixture = Fixture::new();
        let active = &fixture.checkpoint.manifest;
        let abandoned_epoch = ReplicaEpochId::new();
        let abandoned_checkpoint_id = CheckpointId::new();
        let abandoned = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: active.backup_set_id().clone(),
            replica_epoch_id: abandoned_epoch.clone(),
            checkpoint_id: abandoned_checkpoint_id.clone(),
            created_at: UtcTimestamp::parse("2026-07-30T20:00:00Z").expect("abandoned timestamp"),
            kosh_version: active.kosh_version().into(),
            content_revision: active.content_revision() + 100,
            main_migration_head: active.main_migration_head(),
            litestream_path: fixture.keyspace.litestream(&abandoned_epoch),
            txid: active.txid().into(),
            media_migration_head: active.media_migration_head(),
            referenced_hash_count: active.referenced_hash_count(),
            referenced_total_bytes: active.referenced_total_bytes(),
            referenced_hash_set_sha256: active.referenced_hash_set_sha256(),
        })
        .expect("abandoned manifest");
        fixture
            .store
            .put(PutObjectRequest {
                key: abandoned
                    .object_key(&fixture.keyspace)
                    .expect("manifest key"),
                bytes: abandoned.to_json().expect("manifest bytes"),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("abandoned remote manifest");

        let checkpoints =
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id)
                .expect("checkpoint discovery");

        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].replica_epoch_id(), active.replica_epoch_id());
        assert_eq!(checkpoints[0].checkpoint_id(), active.checkpoint_id());
        assert_eq!(checkpoints[1].replica_epoch_id(), &abandoned_epoch);
        assert_eq!(checkpoints[1].checkpoint_id(), &abandoned_checkpoint_id);
        assert!(checkpoints[1].content_revision() > checkpoints[0].content_revision());
    }

    #[test]
    fn exact_checkpoint_discovery_does_not_depend_on_the_mutable_owner_record() {
        let fixture = Fixture::new();
        fixture.store.remove_for_test(&fixture.keyspace.owner());

        let exact = discover_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.backup_set_id,
            fixture.checkpoint.checkpoint_id(),
        )
        .expect("immutable exact checkpoint");

        assert_eq!(exact.checkpoint_id(), fixture.checkpoint.checkpoint_id());
        assert!(matches!(
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id),
            Err(RestoreError::Owner(_))
        ));
    }

    #[test]
    fn exact_checkpoint_discovery_ignores_unrelated_corrupt_manifests() {
        let fixture = Fixture::new();
        let epoch = fixture.checkpoint.replica_epoch_id();
        let corrupt_manifests = [
            (
                CheckpointId::new(),
                UtcTimestamp::parse("2026-07-30T19:00:01Z").expect("malformed timestamp"),
                b"not-json".to_vec(),
                ObjectContentType::Json,
            ),
            (
                CheckpointId::new(),
                UtcTimestamp::parse("2026-07-30T19:00:02Z").expect("oversized timestamp"),
                vec![b'x'; MAX_MANIFEST_BYTES + 1],
                ObjectContentType::Json,
            ),
            (
                CheckpointId::new(),
                UtcTimestamp::parse("2026-07-30T19:00:03Z").expect("binary timestamp"),
                b"{}".to_vec(),
                ObjectContentType::Binary,
            ),
        ];
        for (checkpoint_id, created_at, bytes, content_type) in corrupt_manifests {
            fixture
                .store
                .put(PutObjectRequest {
                    key: fixture
                        .keyspace
                        .checkpoint(epoch, &checkpoint_id, &created_at)
                        .expect("corrupt manifest key"),
                    bytes,
                    content_type,
                    kosh_sha256: None,
                    condition: PutCondition::IfAbsent,
                })
                .expect("unrelated corrupt manifest");
        }
        fixture.store.clear_operations();

        let exact = discover_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.backup_set_id,
            fixture.checkpoint.checkpoint_id(),
        )
        .expect("isolated exact checkpoint");

        assert_eq!(exact.checkpoint_id(), fixture.checkpoint.checkpoint_id());
        assert_eq!(
            fixture.store.operations(),
            [ObjectOperation::List, ObjectOperation::Get],
            "exact discovery must not fetch unrelated manifests"
        );
        assert!(matches!(
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id),
            Err(RestoreError::Manifest)
        ));
    }

    #[test]
    fn discovery_rejects_duplicate_logical_checkpoint_ids_across_manifest_keys() {
        let fixture = Fixture::new();
        let original = &fixture.checkpoint.manifest;
        let duplicate = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: original.backup_set_id().clone(),
            replica_epoch_id: original.replica_epoch_id().clone(),
            checkpoint_id: original.checkpoint_id().clone(),
            created_at: UtcTimestamp::parse("2026-07-30T19:00:01Z").expect("timestamp"),
            kosh_version: original.kosh_version().into(),
            content_revision: original.content_revision(),
            main_migration_head: original.main_migration_head(),
            litestream_path: fixture.keyspace.litestream(original.replica_epoch_id()),
            txid: original.txid().into(),
            media_migration_head: original.media_migration_head(),
            referenced_hash_count: original.referenced_hash_count(),
            referenced_total_bytes: original.referenced_total_bytes(),
            referenced_hash_set_sha256: original.referenced_hash_set_sha256(),
        })
        .expect("duplicate manifest");
        fixture
            .store
            .put(PutObjectRequest {
                key: duplicate
                    .object_key(&fixture.keyspace)
                    .expect("manifest key"),
                bytes: duplicate.to_json().expect("manifest bytes"),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("second manifest");

        assert!(matches!(
            discover_checkpoints(&fixture.store, &fixture.keyspace, &fixture.backup_set_id),
            Err(RestoreError::Manifest)
        ));
        assert!(matches!(
            discover_checkpoint(
                &fixture.store,
                &fixture.keyspace,
                &fixture.backup_set_id,
                original.checkpoint_id(),
            ),
            Err(RestoreError::Manifest)
        ));
    }

    #[test]
    fn staged_restore_lifetime_removes_an_uninstalled_owned_pair() {
        let fixture = Fixture::new();
        let staging_parent = tempfile::tempdir().expect("staging parent");
        let staging_root = staging_parent.path().join("restore");
        let staged = stage_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &fixture.engine,
            &fixture.source_paths.main,
            &staging_root,
        )
        .expect("staged restore");
        assert!(staged.paths.main.is_file());
        assert!(staged.paths.media.is_file());
        let audited =
            Database::initialize(staged.paths.clone()).expect("reopen staged restore for audit");
        audited.shutdown().expect("close staged audit");

        drop(staged);

        assert!(
            !staging_root.exists(),
            "dropping an uninstalled staged restore must remove authored copies"
        );
    }

    #[test]
    fn staged_restore_cleanup_is_bound_to_its_opened_directory() {
        let fixture = Fixture::new();
        let staging_parent = tempfile::tempdir().expect("staging parent");
        let staging_root = staging_parent.path().join("restore");
        let displaced_root = staging_parent.path().join("displaced");
        let staged = stage_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &fixture.engine,
            &fixture.source_paths.main,
            &staging_root,
        )
        .expect("staged restore");
        fs::rename(&staging_root, &displaced_root).expect("displace staging root");
        let replacement_paths = DatabasePaths::new(&staging_root);
        let replacement =
            Database::initialize(replacement_paths.clone()).expect("replacement library");
        replacement.shutdown().expect("close replacement library");
        let replacement_main = fs::read(&replacement_paths.main).expect("replacement main");
        let replacement_media = fs::read(&replacement_paths.media).expect("replacement media");

        drop(staged);

        assert_eq!(
            fs::read(&replacement_paths.main).expect("preserved replacement main"),
            replacement_main
        );
        assert_eq!(
            fs::read(&replacement_paths.media).expect("preserved replacement media"),
            replacement_media
        );
        assert_eq!(
            fs::read_dir(&displaced_root)
                .expect("descriptor-owned displaced staging root")
                .count(),
            0,
            "cleanup must unlink only children in its opened directory"
        );
    }

    #[test]
    fn staging_rejection_never_claims_or_cleans_an_existing_library() {
        let fixture = Fixture::new();
        let existing_parent = tempfile::tempdir().expect("existing parent");
        let existing_root = existing_parent.path().join("existing");
        let existing_paths = DatabasePaths::new(&existing_root);
        let existing = Database::initialize(existing_paths.clone()).expect("existing Kosh library");
        existing.shutdown().expect("close existing library");
        let existing_main = fs::read(&existing_paths.main).expect("existing main");
        let existing_media = fs::read(&existing_paths.media).expect("existing media");

        assert!(matches!(
            stage_checkpoint(
                &fixture.store,
                &fixture.keyspace,
                &fixture.checkpoint,
                &fixture.engine,
                &fixture.source_paths.main,
                &existing_root,
            ),
            Err(RestoreError::InvalidStaging)
        ));
        assert_eq!(
            fs::read(&existing_paths.main).expect("preserved existing main"),
            existing_main
        );
        assert_eq!(
            fs::read(&existing_paths.media).expect("preserved existing media"),
            existing_media
        );
    }

    #[test]
    fn media_reference_loading_pages_without_a_restore_only_total_limit() {
        let main = rusqlite::Connection::open_in_memory().expect("reference fixture");
        main.execute_batch(
            "CREATE TABLE attachment(
                id TEXT PRIMARY KEY,
                sha256 BLOB NOT NULL,
                byte_length INTEGER NOT NULL,
                deleted_at INTEGER
             );
             CREATE TABLE tidbit_revision_attachment(attachment_id TEXT NOT NULL);
             CREATE TABLE attachment_image(
                attachment_id TEXT NOT NULL,
                preview_sha256 BLOB NOT NULL,
                preview_byte_length INTEGER NOT NULL
             );",
        )
        .expect("reference schema");
        for ordinal in 0_u32..257 {
            let mut sha256 = [0_u8; 32];
            sha256[28..].copy_from_slice(&ordinal.to_be_bytes());
            main.execute(
                "INSERT INTO attachment(id, sha256, byte_length, deleted_at)
                 VALUES(?1, ?2, 1, NULL)",
                params![format!("attachment-{ordinal}"), sha256.as_slice()],
            )
            .expect("reference");
        }

        let first =
            load_referenced_media_page(&main, None, MEDIA_RESTORE_PAGE_SIZE).expect("first page");
        assert_eq!(first.len(), 256);
        let second = load_referenced_media_page(
            &main,
            first.last().map(|reference| reference.sha256),
            MEDIA_RESTORE_PAGE_SIZE,
        )
        .expect("second page");
        assert_eq!(second.len(), 1);
        assert!(second[0].sha256.as_bytes() > first[255].sha256.as_bytes());
    }
}
