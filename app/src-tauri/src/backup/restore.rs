#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::Path,
};

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::database::{
    create_empty_restore_media_database, install_restored_pair, validate_restored_pair,
    DatabasePaths, RestoreInstallReport,
};

use super::{
    domain::{
        BackupSetId, CheckpointId, CheckpointManifestV1, ContentSha256, R2Keyspace, ReplicaEpochId,
        MAX_MANIFEST_BYTES, OBJECT_FORMAT_VERSION,
    },
    litestream::{
        LitestreamError, LitestreamTxid, RelationalRestoreEngine, ReplicaKind, RestorePlan,
    },
    object_store::{ObjectContentType, ObjectStore, ObjectStoreError},
    owner::{inspect_remote_owner, RemoteOwnerError, RemoteOwnerSnapshot},
};

const MAX_DISCOVERED_CHECKPOINTS: usize = 10_000;
const MAX_LIST_PAGES: usize = 100;
const MEDIA_RESTORE_PAGE_SIZE: u32 = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteCheckpoint {
    manifest: CheckpointManifestV1,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StagedRestore {
    pub(crate) checkpoint: RemoteCheckpoint,
    pub(crate) paths: DatabasePaths,
    pub(crate) restored_media_count: u64,
    pub(crate) restored_media_bytes: u64,
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

pub(crate) fn discover_checkpoints(
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
                || !seen_checkpoint_ids.insert(manifest.checkpoint_id().clone())
            {
                return Err(RestoreError::Manifest);
            }
            checkpoints.push(RemoteCheckpoint { manifest });
        }
        continuation = page.next;
        if continuation.is_none() {
            break;
        }
    }
    checkpoints.sort_by(|left, right| {
        right
            .created_at()
            .cmp(left.created_at())
            .then_with(|| right.checkpoint_id().cmp(left.checkpoint_id()))
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
    let paths = DatabasePaths::new(staging_root);
    prepare_empty_staging(staging_root)?;
    let operation = (|| {
        let txid = checkpoint.txid()?;
        let plan = engine.preview(source_database_path, &paths.main, txid)?;
        validate_plan(&plan, source_database_path, &paths.main, checkpoint)?;
        let result = engine.restore(source_database_path, &paths.main, txid)?;
        if result.database_path != paths.main
            || result.replica != ReplicaKind::S3
            || result.txid != txid
        {
            return Err(RestoreError::Manifest);
        }
        let (restored_media_count, restored_media_bytes) =
            rebuild_media(store, keyspace, checkpoint, &paths)?;
        validate_restored_pair(&paths)?;
        Ok(StagedRestore {
            checkpoint: checkpoint.clone(),
            paths,
            restored_media_count,
            restored_media_bytes,
        })
    })();
    if operation.is_err() {
        let _ = remove_owned_staging(staging_root);
    }
    operation
}

pub(crate) fn install_checkpoint(
    live_paths: &DatabasePaths,
    staged: &StagedRestore,
) -> Result<RestoreInstallReport, RestoreError> {
    install_restored_pair(
        live_paths,
        &staged.paths,
        staged.checkpoint.checkpoint_id().clone(),
    )
    .map_err(RestoreError::Database)
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
    remove_owned_staging(drill_root)?;
    Ok(report)
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

fn rebuild_media(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    paths: &DatabasePaths,
) -> Result<(u64, u64), RestoreError> {
    let main = Connection::open_with_flags(
        &paths.main,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(crate::database::DatabaseError::from)?;
    let mut media = create_empty_restore_media_database(&paths.media)?;
    let transaction = media
        .transaction()
        .map_err(crate::database::DatabaseError::from)?;
    let mut count = 0_u64;
    let mut total_bytes = 0_u64;
    let mut digest = Sha256::new();
    let mut cursor = None;
    loop {
        let references = load_referenced_media_page(&main, cursor, MEDIA_RESTORE_PAGE_SIZE)?;
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
    if count != checkpoint.manifest.referenced_hash_count()
        || total_bytes != checkpoint.manifest.referenced_total_bytes()
        || ContentSha256::from_bytes(digest.finalize().into())
            != checkpoint.manifest.referenced_hash_set_sha256()
    {
        return Err(RestoreError::MediaMismatch);
    }
    Ok((count, total_bytes))
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
                   OR EXISTS (
                        SELECT 1 FROM research_run_attachment
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
                   OR EXISTS (
                        SELECT 1 FROM research_run_attachment
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

fn prepare_empty_staging(root: &Path) -> Result<(), RestoreError> {
    match fs::create_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(root)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || fs::read_dir(root)?.next().is_some()
            {
                return Err(RestoreError::InvalidStaging);
            }
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    File::open(root)?.sync_all()?;
    Ok(())
}

fn remove_owned_staging(root: &Path) -> Result<(), RestoreError> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RestoreError::InvalidStaging);
    }
    let allowed = [
        "kosh.sqlite3",
        "kosh.sqlite3-wal",
        "kosh.sqlite3-shm",
        "media.sqlite3",
        "media.sqlite3-wal",
        "media.sqlite3-shm",
        "kosh.lock",
    ];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || !entry
                .file_name()
                .to_str()
                .is_some_and(|name| allowed.contains(&name))
        {
            return Err(RestoreError::InvalidStaging);
        }
        fs::remove_file(entry.path())?;
    }
    fs::remove_dir(root)?;
    if let Some(parent) = root.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
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
                R2Target, UtcTimestamp,
            },
            litestream::{IntegrityCheck, RestoreFile, RestoreResult},
            object_store::{
                fake::FakeObjectStore, ObjectContentType, PutCondition, PutMediaRequest,
                PutObjectRequest,
            },
            owner::claim_remote_owner,
        },
        database::{
            drafts::SaveDraftWrite, tidbits::CreateTidbitWrite, AttachmentIngestInput, Database,
            LexicalSearchMode, MediaLimits, SaveDraftInput, SearchPassagesInput, SourceDraft,
            TidbitDraft,
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
        txid: LitestreamTxid,
    }

    impl RelationalRestoreEngine for FakeRestoreEngine {
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
            target_path: &Path,
            txid: LitestreamTxid,
        ) -> Result<RestoreResult, LitestreamError> {
            assert_eq!(source_database_path, self.source);
            assert_eq!(txid, self.txid);
            fs::copy(source_database_path, target_path).map_err(LitestreamError::Execute)?;
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
            database
                .client()
                .create_tidbit(CreateTidbitWrite {
                    input: TidbitDraft {
                        title: Some("Remote recovery".into()),
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
                .save_draft(SaveDraftWrite {
                    input: SaveDraftInput {
                        context_key: "capture".into(),
                        tidbit_id: None,
                        base_revision_id: None,
                        title: None,
                        body_markdown: String::new(),
                        sources: Vec::new(),
                    },
                    now_ms: 11,
                    draft_id: DRAFT_ID.into(),
                    media_limits: MediaLimits::default(),
                })
                .expect("source draft");
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
            database.shutdown().expect("close source");

            let main = rusqlite::Connection::open(&source_paths.main).expect("source main");
            let main_head: u32 = main
                .query_row(
                    "SELECT max(version) FROM refinery_schema_history",
                    [],
                    |row| row.get(0),
                )
                .expect("main head");
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
            let media = rusqlite::Connection::open(&source_paths.media).expect("source media");
            let media_head: u32 = media
                .query_row(
                    "SELECT max(version) FROM refinery_schema_history",
                    [],
                    |row| row.get(0),
                )
                .expect("media head");
            drop(main);
            drop(media);

            let target = R2Target {
                account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef")
                    .expect("account"),
                jurisdiction: R2Jurisdiction::Default,
                bucket: R2BucketName::parse("kosh-restore-test").expect("bucket"),
            };
            let backup_set_id = BackupSetId::new();
            let keyspace = target.keyspace(&backup_set_id);
            let store = FakeObjectStore::new(keyspace.clone());
            store
                .put_media(
                    PutMediaRequest::new(&keyspace, sha256, media_bytes.clone())
                        .expect("verified media"),
                )
                .expect("remote media");
            let epoch = ReplicaEpochId::new();
            claim_remote_owner(
                &store,
                &keyspace,
                &backup_set_id,
                &epoch,
                &BackupWriterId::new(),
            )
            .expect("remote owner");
            let checkpoint_id = CheckpointId::new();
            let txid = LitestreamTxid::from_local(42);
            let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
                backup_set_id: backup_set_id.clone(),
                replica_epoch_id: epoch.clone(),
                checkpoint_id,
                created_at: UtcTimestamp::parse("2026-07-30T19:00:00Z").expect("timestamp"),
                kosh_version: env!("CARGO_PKG_VERSION").into(),
                content_revision: 2,
                main_migration_head: main_head,
                litestream_path: keyspace.litestream(&epoch),
                txid: txid.to_string(),
                media_migration_head: media_head,
                referenced_hash_count: 1,
                referenced_total_bytes: media_bytes.len() as u64,
                referenced_hash_set_sha256: ContentSha256::from_bytes(
                    Sha256::digest(sha256.as_bytes()).into(),
                ),
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
    }

    #[test]
    fn clean_directory_install_is_idempotent_and_restores_search_citations_and_media() {
        let fixture = Fixture::new();
        let staging_parent = tempfile::tempdir().expect("staging parent");
        let staged = stage_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &fixture.engine,
            &fixture.source_paths.main,
            &staging_parent.path().join("restore"),
        )
        .expect("staged restore");
        let clean_root = tempfile::tempdir().expect("clean destination");
        let live_paths = DatabasePaths::new(clean_root.path());
        let first = install_checkpoint(&live_paths, &staged).expect("first install");
        assert!(first.safety_snapshot_id.is_none());
        let second = install_checkpoint(&live_paths, &staged).expect("idempotent install");
        assert_eq!(
            format!("{:?}", second.outcome),
            "AlreadyInstalled",
            "same checkpoint must not replace the pair twice"
        );

        let restored = Database::initialize(live_paths.clone()).expect("restored Kosh library");
        let results = restored
            .client()
            .search_passages(SearchPassagesInput {
                query: "citrine".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("restored exact search");
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]
                .citation
                .tidbit
                .as_ref()
                .expect("tidbit citation")
                .id,
            TIDBIT_ID
        );
        let media = restored.open_media_read_only().expect("restored media");
        assert_eq!(
            media
                .query_row("SELECT count(*) FROM media_blob", [], |row| row
                    .get::<_, i64>(0))
                .expect("media count"),
            1
        );
        restored.shutdown().expect("close restored library");
    }

    #[test]
    fn replacing_an_existing_pair_creates_a_verified_pre_restore_snapshot() {
        let fixture = Fixture::new();
        let staging_parent = tempfile::tempdir().expect("staging parent");
        let staged = stage_checkpoint(
            &fixture.store,
            &fixture.keyspace,
            &fixture.checkpoint,
            &fixture.engine,
            &fixture.source_paths.main,
            &staging_parent.path().join("restore"),
        )
        .expect("staged restore");
        let destination = tempfile::tempdir().expect("existing destination");
        let live_paths = DatabasePaths::new(destination.path());
        let existing =
            Database::initialize(live_paths.clone()).expect("existing local database pair");
        existing.shutdown().expect("close existing pair");

        let installed = install_checkpoint(&live_paths, &staged).expect("replacement install");
        let snapshot_id = installed
            .safety_snapshot_id
            .expect("pre-restore safety snapshot");
        assert!(snapshot_id.starts_with("restore-"));
        assert!(live_paths
            .root
            .join("safety-snapshots")
            .join(snapshot_id)
            .join("manifest.json")
            .is_file());
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
             CREATE TABLE research_run_attachment(attachment_id TEXT NOT NULL);
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
