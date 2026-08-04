use std::{
    ffi::{CStr, CString, OsString},
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Seek, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStringExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::database::{Database, DatabasePaths};

use super::{
    credentials::R2Credentials,
    domain::{
        BackupSetId, CheckpointId, ContentSha256, R2AccountId, R2BucketName, R2Jurisdiction,
        R2Target,
    },
    litestream::{CommandLitestreamRestore, EphemeralLitestreamRuntime, VerifiedLitestreamBinary},
    object_store::R2ObjectStore,
    restore::{
        discover_checkpoint, discover_checkpoints, remove_staged_checkpoint,
        remove_staging_root_at_identity, stage_checkpoint_at_identity, RemoteCheckpoint,
        StagedDatabasePair, StagedRestore, StagingDirectoryIdentity,
    },
};

#[cfg(test)]
use super::restore::remove_staging_root;

const COMMAND: &str = "recovery";
const REMOTE_RESTORE: &str = "remote-restore";
const LATEST: &str = "latest";
const RESTORE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const ACCOUNT_ID_ENV: &str = "KOSH_LITESTREAM_R2_ACCOUNT_ID";
const JURISDICTION_ENV: &str = "KOSH_LITESTREAM_R2_JURISDICTION";
const BUCKET_ENV: &str = "KOSH_LITESTREAM_R2_BUCKET";
const ACCESS_KEY_ENV: &str = "KOSH_LITESTREAM_R2_ACCESS_KEY_ID";
const SECRET_KEY_ENV: &str = "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY";
const RESERVATION_FILENAME: &str = ".kosh-recovery-reservation-v1";
const COMPLETION_FILENAME: &str = ".kosh-recovery-completed-v1.json";
const MAIN_TEMP_FILENAME: &str = ".kosh-recovery-main-v1.tmp";
const MEDIA_TEMP_FILENAME: &str = ".kosh-recovery-media-v1.tmp";
const MAIN_FILENAME: &str = "kosh.sqlite3";
const MEDIA_FILENAME: &str = "media.sqlite3";
const RESERVATION_SCHEMA_VERSION: u32 = 1;
const MAX_RESERVATION_BYTES: u64 = 4 * 1024;
const COMPLETION_SCHEMA_VERSION: u32 = 1;
const MAX_COMPLETION_BYTES: u64 = 32 * 1024;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryReservationControl {
    schema_version: u32,
    token: String,
    target_data_directory: String,
    #[serde(default)]
    target_device: Option<u64>,
    #[serde(default)]
    target_inode: Option<u64>,
    #[serde(default)]
    staging_device: Option<u64>,
    #[serde(default)]
    staging_inode: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteRestoreReport {
    schema_version: u32,
    result: String,
    backup_set_id: String,
    checkpoint_id: String,
    target_data_directory: String,
    restored_media_count: u64,
    restored_media_bytes: u64,
    active_tidbits: u64,
    revisions: u64,
    attachments: u64,
    media_blobs: u64,
    search_documents_rebuilt: u64,
    safety_snapshot_created: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StagedAcceptanceEvidence {
    active_tidbits: u64,
    revisions: u64,
    attachments: u64,
    media_blobs: u64,
    search_documents_rebuilt: u64,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompletedRecoveryReceipt {
    schema_version: u32,
    reservation_token: String,
    backup_set_id: String,
    requested_checkpoint_selector: String,
    target_data_directory: String,
    main_sha256: ContentSha256,
    media_sha256: ContentSha256,
    report: RemoteRestoreReport,
}

pub(crate) fn run_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(OsString::from(COMMAND).as_os_str()) {
        return None;
    }
    let remaining = arguments.collect::<Vec<_>>();
    Some(match run(remaining) {
        Ok(report) => match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(_) => {
                eprintln!(
                    "Kosh recovery failed: the redacted recovery receipt could not be written"
                );
                1
            }
        },
        Err(message) => {
            eprintln!("Kosh recovery failed: {message}");
            1
        }
    })
}

fn run(arguments: Vec<OsString>) -> Result<RemoteRestoreReport, String> {
    if arguments.first().and_then(|value| value.to_str()) != Some(REMOTE_RESTORE)
        || arguments.len() != 4
    {
        return Err(usage());
    }
    let backup_set_id = parse_utf8(&arguments[1], "backup set ID")
        .and_then(|value| BackupSetId::parse(value).map_err(|_| "invalid backup set ID".into()))?;
    let selector = parse_utf8(&arguments[2], "checkpoint selector")?;
    let target_root = PathBuf::from(&arguments[3]);
    if let Some(report) = load_completed_recovery(&target_root, &backup_set_id, selector.as_str())?
    {
        return Ok(report);
    }
    let reservation = RecoveryTargetReservation::reserve(&target_root)?;
    let target = load_target()?;
    let credentials = load_credentials()?;
    restore_remote(
        &target,
        &credentials,
        &backup_set_id,
        &selector,
        reservation,
    )
}

fn restore_remote(
    target: &R2Target,
    credentials: &R2Credentials,
    backup_set_id: &BackupSetId,
    selector: &str,
    mut reservation: RecoveryTargetReservation,
) -> Result<RemoteRestoreReport, String> {
    let target_root = reservation.path().to_owned();
    let keyspace = target.keyspace(backup_set_id);
    let store = R2ObjectStore::new(target.clone(), keyspace.clone(), credentials)
        .map_err(|_| "the private R2 target could not be opened")?;
    let checkpoint = if selector == LATEST {
        let checkpoints = discover_checkpoints(&store, &keyspace, backup_set_id)
            .map_err(|_| "complete recovery points could not be discovered")?;
        select_checkpoint(checkpoints, selector)?
    } else {
        let checkpoint_id =
            CheckpointId::parse(selector).map_err(|_| "invalid checkpoint selector".to_owned())?;
        discover_checkpoint(&store, &keyspace, backup_set_id, &checkpoint_id).map_err(|error| {
            match error {
                super::restore::RestoreError::CheckpointNotFound => {
                    "the selected complete recovery point was not found".to_owned()
                }
                _ => "complete recovery points could not be discovered".to_owned(),
            }
        })?
    };

    let mut ephemeral = EphemeralLitestreamRuntime::create()
        .map_err(|_| "the isolated recovery runtime could not be created")?;
    let staged = {
        let resource_dir = packaged_resource_directory()?;
        let binary = VerifiedLitestreamBinary::resolve(&resource_dir)
            .and_then(|verified| verified.stage_immutable(ephemeral.paths()))
            .map_err(|_| "the bundled recovery runtime failed verification")?;
        let engine = CommandLitestreamRestore::new(
            &binary,
            ephemeral.paths(),
            target,
            &keyspace.litestream(checkpoint.replica_epoch_id()),
            ephemeral.source_database_path(),
            credentials,
            RESTORE_TIMEOUT,
        )
        .map_err(|_| "the bounded recovery runtime could not be prepared")?;
        let (staging_root, staging_identity) = reservation.prepare_staging_root()?;
        stage_checkpoint_at_identity(
            &store,
            &keyspace,
            &checkpoint,
            &engine,
            ephemeral.source_database_path(),
            &staging_root,
            staging_identity,
        )
        .map_err(|_| "the selected recovery point failed exact database or media validation")?
    };
    ephemeral
        .cleanup()
        .map_err(|_| "the isolated recovery runtime could not be removed")?;
    let restored_media_count = staged.restored_media_count;
    let restored_media_bytes = staged.restored_media_bytes;

    // The recovery command is a dedicated one-shot process. Bind its working
    // directory to the retained staging descriptor for the entire write-
    // capable reopen so SQLite lock, migration, WAL, and search paths cannot
    // follow a renamed or substituted staging pathname.
    let evidence = audit_staged_database(staged.staging_directory())?;

    let report = RemoteRestoreReport {
        schema_version: 1,
        result: "PASSED".into(),
        backup_set_id: backup_set_id.to_string(),
        checkpoint_id: checkpoint.checkpoint_id().to_string(),
        target_data_directory: target_root.to_string_lossy().into_owned(),
        restored_media_count,
        restored_media_bytes,
        active_tidbits: evidence.active_tidbits,
        revisions: evidence.revisions,
        attachments: evidence.attachments,
        media_blobs: evidence.media_blobs,
        search_documents_rebuilt: evidence.search_documents_rebuilt,
        safety_snapshot_created: false,
    };

    let staged_pair = staged
        .open_validated_database_pair()
        .map_err(|_| "the independently staged recovery pair is no longer valid")?;
    reservation.install_validated_pair(&staged_pair)?;
    reservation.verify_installed_pair_matches(&staged_pair)?;
    reservation.finish_after_staging_cleanup(&staged, backup_set_id, selector, &report)?;

    Ok(report)
}

#[cfg(test)]
pub(crate) fn install_staged_for_test(
    target_root: &Path,
    staged: &StagedRestore,
    backup_set_id: &BackupSetId,
    checkpoint: &RemoteCheckpoint,
) -> Result<(), String> {
    let mut reservation = RecoveryTargetReservation::reserve(target_root)?;
    let evidence = audit_staged_database(staged.staging_directory())?;
    let report = RemoteRestoreReport {
        schema_version: 1,
        result: "PASSED".into(),
        backup_set_id: backup_set_id.to_string(),
        checkpoint_id: checkpoint.checkpoint_id().to_string(),
        target_data_directory: target_root.to_string_lossy().into_owned(),
        restored_media_count: staged.restored_media_count,
        restored_media_bytes: staged.restored_media_bytes,
        active_tidbits: evidence.active_tidbits,
        revisions: evidence.revisions,
        attachments: evidence.attachments,
        media_blobs: evidence.media_blobs,
        search_documents_rebuilt: evidence.search_documents_rebuilt,
        safety_snapshot_created: false,
    };
    let staged_pair = staged
        .open_validated_database_pair()
        .map_err(|_| "the independently staged recovery pair is no longer valid")?;
    reservation.install_validated_pair(&staged_pair)?;
    reservation.verify_installed_pair_matches(&staged_pair)?;
    reservation.finish_after_staging_cleanup(staged, backup_set_id, LATEST, &report)
}

#[derive(Debug)]
struct RecoveryTargetReservation {
    root: PathBuf,
    token: String,
    control: RecoveryReservationControl,
    directory: File,
    marker: File,
    staging_identity: Option<StagingDirectoryIdentity>,
    installed_main: Option<File>,
    installed_media: Option<File>,
    active: bool,
}

impl RecoveryTargetReservation {
    fn reserve(path: &Path) -> Result<Self, String> {
        prepare_new_or_abandoned_target(path)?;
        validate_new_target(path)?;
        let target_data_directory = path
            .to_str()
            .ok_or_else(|| "the recovery target must be valid UTF-8".to_owned())?
            .to_owned();
        let token = uuid::Uuid::now_v7().to_string();
        let mut control = RecoveryReservationControl {
            schema_version: RESERVATION_SCHEMA_VERSION,
            token: token.clone(),
            target_data_directory,
            target_device: None,
            target_inode: None,
            staging_device: None,
            staging_inode: None,
        };
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "the recovery target stopped being new; no data was changed".to_owned()
            } else {
                "the recovery target could not be reserved".to_owned()
            }
        })?;
        let directory = match open_directory_no_follow(path) {
            Ok(directory) => directory,
            Err(_) => {
                let _ = fs::remove_dir(path);
                return Err("the recovery target reservation could not be opened".into());
            }
        };
        let directory_metadata = match directory.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                let _ = remove_empty_open_directory(path, &directory);
                return Err("the recovery target reservation could not be inspected".into());
            }
        };
        let directory_is_empty =
            matches!(directory_entries(&directory), Ok(entries) if entries.is_empty());
        if !path_matches_open_file(path, &directory)
            || !is_private_owned_directory(&directory_metadata)
            || !directory_is_empty
        {
            let _ = remove_empty_open_directory(path, &directory);
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        use std::os::unix::fs::MetadataExt;
        control.target_device = Some(directory_metadata.dev());
        control.target_inode = Some(directory_metadata.ino());
        let control_bytes = serde_json::to_vec(&control)
            .map_err(|_| "the recovery target reservation could not be encoded")?;
        let mut marker = match create_private_child(&directory, RESERVATION_FILENAME) {
            Ok(marker) => marker,
            Err(_) => {
                let _ = remove_empty_open_directory(path, &directory);
                return Err("the recovery target reservation could not be created".into());
            }
        };
        if marker.try_lock().is_err()
            || marker.write_all(&control_bytes).is_err()
            || marker.sync_all().is_err()
            || directory.sync_all().is_err()
            || sync_parent_directory(path).is_err()
        {
            let _ = unlink_owned_child(&directory, RESERVATION_FILENAME, &marker);
            let _ = remove_empty_open_directory(path, &directory);
            return Err("the recovery target reservation could not be persisted".into());
        }
        Ok(Self {
            root: path.to_owned(),
            token,
            control,
            directory,
            marker,
            staging_identity: None,
            installed_main: None,
            installed_media: None,
            active: true,
        })
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn staging_root(&self) -> PathBuf {
        self.root
            .parent()
            .expect("validated recovery target parent")
            .join(format!(".kosh-restore-{}", self.token))
    }

    fn prepare_staging_root(&mut self) -> Result<(PathBuf, StagingDirectoryIdentity), String> {
        let root = self.staging_root();
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&root)
            .map_err(|_| "the recovery staging directory could not be reserved")?;
        let directory = match open_directory_no_follow(&root) {
            Ok(directory) => directory,
            Err(_) => {
                let _ = fs::remove_dir(&root);
                return Err("the recovery staging directory could not be opened".into());
            }
        };
        let metadata = directory
            .metadata()
            .map_err(|_| "the recovery staging directory could not be inspected")?;
        if !path_matches_open_file(&root, &directory)
            || !is_private_owned_directory(&metadata)
            || !directory_entries(&directory).is_ok_and(|entries| entries.is_empty())
        {
            let _ = remove_empty_open_directory(&root, &directory);
            return Err("the recovery staging directory was replaced".into());
        }
        use std::os::unix::fs::MetadataExt;
        let identity = StagingDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        self.control.staging_device = Some(identity.device);
        self.control.staging_inode = Some(identity.inode);
        if sync_parent_directory(&root).is_err()
            || write_reservation_control(&mut self.marker, &self.control).is_err()
            || self.directory.sync_all().is_err()
        {
            self.control.staging_device = None;
            self.control.staging_inode = None;
            let _ = remove_empty_open_directory(&root, &directory);
            return Err("the recovery staging ownership could not be persisted".into());
        }
        self.staging_identity = Some(identity);
        Ok((root, identity))
    }

    fn verify_install_ready(&self) -> Result<(), String> {
        if !path_matches_open_file(&self.root, &self.directory) {
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        let entries = directory_entries(&self.directory)
            .map_err(|_| "the recovery target reservation could not be inspected")?;
        if entries.as_slice() != [OsString::from(RESERVATION_FILENAME)] {
            return Err("the recovery target stopped being empty; no data was changed".into());
        }
        let marker = open_regular_child(&self.directory, RESERVATION_FILENAME)
            .map_err(|_| "the recovery target reservation was replaced; no data was changed")?;
        if !same_open_file(&marker, &self.marker) {
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        Ok(())
    }

    fn install_validated_pair(&mut self, staged: &StagedDatabasePair) -> Result<(), String> {
        self.install_validated_pair_with_hook(staged, || {})
    }

    fn install_validated_pair_with_hook(
        &mut self,
        staged: &StagedDatabasePair,
        before_publish: impl FnOnce(),
    ) -> Result<(), String> {
        self.verify_install_ready()?;

        let main_temporary =
            copy_open_regular_into_child(staged.main(), &self.directory, MAIN_TEMP_FILENAME)
                .map_err(|_| "the recovered main database could not be privately staged")?;
        let media_temporary = match copy_open_regular_into_child(
            staged.media(),
            &self.directory,
            MEDIA_TEMP_FILENAME,
        ) {
            Ok(file) => file,
            Err(_) => {
                let _ = unlink_owned_child(&self.directory, MAIN_TEMP_FILENAME, &main_temporary);
                return Err("the recovered media database could not be privately staged".into());
            }
        };

        before_publish();
        let publication = (|| {
            link_child_exclusive(&self.directory, MAIN_TEMP_FILENAME, MAIN_FILENAME)?;
            if let Err(error) =
                link_child_exclusive(&self.directory, MEDIA_TEMP_FILENAME, MEDIA_FILENAME)
            {
                let _ = unlink_owned_child(&self.directory, MAIN_FILENAME, &main_temporary);
                return Err(error);
            }
            self.directory.sync_all()?;
            if !path_matches_open_file(&self.root, &self.directory) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the reserved recovery directory was replaced",
                ));
            }
            unlink_owned_child(&self.directory, MAIN_TEMP_FILENAME, &main_temporary)?;
            unlink_owned_child(&self.directory, MEDIA_TEMP_FILENAME, &media_temporary)?;
            self.directory.sync_all()?;
            if !path_matches_open_file(&self.root, &self.directory) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the reserved recovery directory was replaced",
                ));
            }
            Ok(())
        })();
        if publication.is_err() {
            let _ = unlink_owned_child(&self.directory, MAIN_FILENAME, &main_temporary);
            let _ = unlink_owned_child(&self.directory, MEDIA_FILENAME, &media_temporary);
            let _ = unlink_owned_child(&self.directory, MAIN_TEMP_FILENAME, &main_temporary);
            let _ = unlink_owned_child(&self.directory, MEDIA_TEMP_FILENAME, &media_temporary);
            let _ = self.directory.sync_all();
            return Err(
                "the clean recovery target changed during installation; no existing data was modified"
                .into(),
            );
        }
        self.installed_main = Some(main_temporary);
        self.installed_media = Some(media_temporary);
        Ok(())
    }

    fn verify_installed_pair_matches(&self, staged: &StagedDatabasePair) -> Result<(), String> {
        if !path_matches_open_file(&self.root, &self.directory) {
            return Err("the recovery target was replaced after installation".into());
        }
        let mut expected_entries = vec![
            OsString::from(MAIN_FILENAME),
            OsString::from(MEDIA_FILENAME),
            OsString::from(RESERVATION_FILENAME),
        ];
        expected_entries.sort();
        let entries = directory_entries(&self.directory)
            .map_err(|_| "the installed recovery target could not be inspected")?;
        if entries != expected_entries {
            return Err("the installed recovery target contains unexpected data".into());
        }
        let installed_main = self
            .installed_main
            .as_ref()
            .ok_or_else(|| "the installed main database identity is unavailable".to_owned())?;
        let installed_media = self
            .installed_media
            .as_ref()
            .ok_or_else(|| "the installed media database identity is unavailable".to_owned())?;
        verify_owned_child_matches_source(
            &self.directory,
            MAIN_FILENAME,
            installed_main,
            staged.main(),
        )
        .map_err(|_| "the installed main database does not match its audited staging file")?;
        verify_owned_child_matches_source(
            &self.directory,
            MEDIA_FILENAME,
            installed_media,
            staged.media(),
        )
        .map_err(|_| "the installed media database does not match its audited staging file")?;
        if !path_matches_open_file(&self.root, &self.directory) {
            return Err("the recovery target was replaced during installed-pair validation".into());
        }
        Ok(())
    }

    fn publish_completed(
        &mut self,
        backup_set_id: &BackupSetId,
        selector: &str,
        report: &RemoteRestoreReport,
    ) -> Result<(), String> {
        if !path_matches_open_file(&self.root, &self.directory) {
            return Err("the recovery target was replaced before completion".into());
        }
        let installed_main = self
            .installed_main
            .as_ref()
            .ok_or_else(|| "the installed main database identity is unavailable".to_owned())?;
        let installed_media = self
            .installed_media
            .as_ref()
            .ok_or_else(|| "the installed media database identity is unavailable".to_owned())?;
        let target_data_directory = self
            .root
            .to_str()
            .ok_or_else(|| "the recovery target must be valid UTF-8".to_owned())?
            .to_owned();
        let receipt = CompletedRecoveryReceipt {
            schema_version: COMPLETION_SCHEMA_VERSION,
            reservation_token: self.token.clone(),
            backup_set_id: backup_set_id.to_string(),
            requested_checkpoint_selector: selector.to_owned(),
            target_data_directory,
            main_sha256: ContentSha256::from_bytes(
                sha256_file(installed_main)
                    .map_err(|_| "the completed main database could not be identified")?,
            ),
            media_sha256: ContentSha256::from_bytes(
                sha256_file(installed_media)
                    .map_err(|_| "the completed media database could not be identified")?,
            ),
            report: report.clone(),
        };
        let bytes = serde_json::to_vec(&receipt)
            .map_err(|_| "the completed recovery receipt could not be encoded")?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_COMPLETION_BYTES {
            return Err("the completed recovery receipt is outside its size limit".into());
        }
        let mut completed = create_private_child(&self.directory, COMPLETION_FILENAME)
            .map_err(|_| "the completed recovery receipt could not be created")?;
        let publication = (|| {
            completed.write_all(&bytes)?;
            completed.sync_all()?;
            self.directory.sync_all()?;
            if !path_matches_open_file(&self.root, &self.directory) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the completed recovery target was replaced",
                ));
            }
            Ok(())
        })();
        if publication.is_err() {
            let _ = unlink_owned_child(&self.directory, COMPLETION_FILENAME, &completed);
            let _ = self.directory.sync_all();
            return Err("the completed recovery receipt could not be persisted".into());
        }

        // The descriptor-bound completion receipt is the commit point. From
        // here onward a retry can verify and return the same report even if
        // the process dies before stdout is flushed or marker cleanup runs.
        self.installed_main = None;
        self.installed_media = None;
        self.active = false;
        let _ = unlink_owned_child(&self.directory, RESERVATION_FILENAME, &self.marker);
        let _ = self.directory.sync_all();
        drop(completed);
        Ok(())
    }

    fn finish_after_staging_cleanup(
        &mut self,
        staged: &StagedRestore,
        backup_set_id: &BackupSetId,
        selector: &str,
        report: &RemoteRestoreReport,
    ) -> Result<(), String> {
        if staged.paths.root != self.staging_root() {
            return Err("the validated recovery staging pair identity changed".into());
        }
        remove_staged_checkpoint(staged)
            .map_err(|_| "the validated recovery staging pair could not be removed")?;
        if !path_matches_open_file(&self.root, &self.directory) {
            return Err("the recovery target was replaced before completion".into());
        }
        self.publish_completed(backup_set_id, selector, report)
    }
}

impl Drop for RecoveryTargetReservation {
    fn drop(&mut self) {
        if self.active {
            if let Some(identity) = self.staging_identity {
                let _ = remove_staging_root_at_identity(&self.staging_root(), identity);
            }
            if let Some(main) = self.installed_main.as_ref() {
                let _ = unlink_owned_child(&self.directory, MAIN_FILENAME, main);
            }
            if let Some(media) = self.installed_media.as_ref() {
                let _ = unlink_owned_child(&self.directory, MEDIA_FILENAME, media);
            }
            let _ = unlink_owned_child(&self.directory, RESERVATION_FILENAME, &self.marker);
            let _ = self.directory.sync_all();
            let _ = remove_empty_open_directory(&self.root, &self.directory);
        }
    }
}

fn load_completed_recovery(
    path: &Path,
    backup_set_id: &BackupSetId,
    selector: &str,
) -> Result<Option<RemoteRestoreReport>, String> {
    validate_target_location(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) => metadata,
        Err(_) => return Err("the recovery target could not be inspected".into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }
    let directory = match open_directory_no_follow(path) {
        Ok(directory) => directory,
        Err(_) => return Ok(None),
    };
    let directory_metadata = directory
        .metadata()
        .map_err(|_| "the completed recovery target could not be inspected")?;
    if !path_matches_open_file(path, &directory) || !is_private_owned_directory(&directory_metadata)
    {
        return Ok(None);
    }
    let entries = directory_entries(&directory)
        .map_err(|_| "the completed recovery target could not be inspected")?;
    let has_completion = entries.contains(&OsString::from(COMPLETION_FILENAME));
    let has_marker = entries.contains(&OsString::from(RESERVATION_FILENAME));
    if !has_completion {
        return Ok(None);
    }

    let mut marker = if has_marker {
        let mut marker = open_regular_child(&directory, RESERVATION_FILENAME)
            .map_err(|_| "the completed recovery reservation marker is invalid")?;
        let marker_metadata = marker
            .metadata()
            .map_err(|_| "the completed recovery reservation marker is invalid")?;
        if !is_private_owned_file(&marker_metadata) || link_count(&marker_metadata) != 1 {
            return Err("the completed recovery reservation marker is invalid".into());
        }
        match marker.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err("another recovery command still owns this target".into());
            }
            Err(TryLockError::Error(_)) => {
                return Err("the completed recovery reservation could not be locked".into());
            }
        }
        Some((
            read_reservation_control(&mut marker)
                .map_err(|_| "the completed recovery reservation marker is invalid")?,
            marker,
        ))
    } else {
        None
    };

    let verified = verify_completed_recovery(
        path,
        &directory,
        &entries,
        marker.as_ref().map(|(control, _)| control),
        backup_set_id,
        selector,
    );
    let report = match verified {
        Ok(report) => report,
        Err(_) if marker.is_some() => return Ok(None),
        Err(message) => return Err(message),
    };
    if let Some((_, marker_file)) = marker.take() {
        let _ = unlink_owned_child(&directory, RESERVATION_FILENAME, &marker_file);
        let _ = directory.sync_all();
    }
    Ok(Some(report))
}

fn verify_completed_recovery(
    path: &Path,
    directory: &File,
    entries: &[OsString],
    reservation: Option<&RecoveryReservationControl>,
    backup_set_id: &BackupSetId,
    selector: &str,
) -> Result<RemoteRestoreReport, String> {
    let mut expected_entries = vec![
        OsString::from(COMPLETION_FILENAME),
        OsString::from(MAIN_FILENAME),
        OsString::from(MEDIA_FILENAME),
    ];
    if reservation.is_some() {
        expected_entries.push(OsString::from(RESERVATION_FILENAME));
    }
    expected_entries.sort();
    if entries != expected_entries {
        return Err("the completed recovery target contains unexpected data".into());
    }

    let mut completed = open_regular_child(directory, COMPLETION_FILENAME)
        .map_err(|_| "the completed recovery receipt is invalid")?;
    let completed_metadata = completed
        .metadata()
        .map_err(|_| "the completed recovery receipt is invalid")?;
    if !is_private_owned_file(&completed_metadata) || link_count(&completed_metadata) != 1 {
        return Err("the completed recovery receipt is invalid".into());
    }
    let receipt = read_completed_recovery(&mut completed)
        .map_err(|_| "the completed recovery receipt is invalid")?;
    let target = path
        .to_str()
        .ok_or_else(|| "the recovery target must be valid UTF-8".to_owned())?;
    let canonical_token = uuid::Uuid::parse_str(&receipt.reservation_token)
        .ok()
        .is_some_and(|token| token.to_string() == receipt.reservation_token);
    if receipt.schema_version != COMPLETION_SCHEMA_VERSION
        || !canonical_token
        || receipt.backup_set_id != backup_set_id.to_string()
        || receipt.requested_checkpoint_selector != selector
        || receipt.target_data_directory != target
        || receipt.report.schema_version != 1
        || receipt.report.result != "PASSED"
        || receipt.report.backup_set_id != receipt.backup_set_id
        || receipt.report.target_data_directory != receipt.target_data_directory
        || (selector != LATEST && receipt.report.checkpoint_id != selector)
        || reservation.is_some_and(|control| {
            !reservation_control_matches(path, control)
                || control.token != receipt.reservation_token
        })
    {
        return Err("the completed recovery receipt does not match this request".into());
    }

    let main = open_regular_child(directory, MAIN_FILENAME)
        .map_err(|_| "the completed main database is unavailable")?;
    let media = open_regular_child(directory, MEDIA_FILENAME)
        .map_err(|_| "the completed media database is unavailable")?;
    for file in [&main, &media] {
        let metadata = file
            .metadata()
            .map_err(|_| "the completed recovery database identity is unavailable")?;
        if !is_private_owned_file(&metadata) || link_count(&metadata) != 1 {
            return Err("the completed recovery database identity is invalid".into());
        }
    }
    if ContentSha256::from_bytes(
        sha256_file(&main).map_err(|_| "the completed main database could not be verified")?,
    ) != receipt.main_sha256
        || ContentSha256::from_bytes(
            sha256_file(&media)
                .map_err(|_| "the completed media database could not be verified")?,
        ) != receipt.media_sha256
        || !path_matches_open_file(path, directory)
    {
        return Err("the completed recovery databases do not match their receipt".into());
    }
    Ok(receipt.report)
}

fn read_completed_recovery(file: &mut File) -> std::io::Result<CompletedRecoveryReceipt> {
    let length = file.metadata()?.len();
    if length == 0 || length > MAX_COMPLETION_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid completed recovery receipt length",
        ));
    }
    file.rewind()?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_COMPLETION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unstable completed recovery receipt length",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid completed recovery receipt",
        )
    })
}

fn prepare_new_or_abandoned_target(path: &Path) -> Result<(), String> {
    validate_target_location(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => reclaim_abandoned_reservation(path),
        Err(_) => Err("the recovery target could not be inspected".into()),
    }
}

fn reclaim_abandoned_reservation(path: &Path) -> Result<(), String> {
    let directory = open_directory_no_follow(path)
        .map_err(|_| "the recovery target already exists and is not a Kosh reservation")?;
    let metadata = directory
        .metadata()
        .map_err(|_| "the existing recovery reservation could not be inspected")?;
    if !path_matches_open_file(path, &directory) || !is_private_owned_directory(&metadata) {
        return Err("the recovery target already exists and is not a Kosh reservation".into());
    }
    let entries = directory_entries(&directory)
        .map_err(|_| "the existing recovery reservation could not be inspected")?;
    if entries.is_empty() {
        remove_empty_open_directory(path, &directory)
            .and_then(|()| sync_parent_directory(path))
            .map_err(|_| "the interrupted empty recovery reservation could not be reclaimed")?;
        return Ok(());
    }
    if !entries.contains(&OsString::from(RESERVATION_FILENAME)) {
        return Err("the recovery target already exists and is not a Kosh reservation".into());
    }
    let mut marker = open_regular_child(&directory, RESERVATION_FILENAME)
        .map_err(|_| "the existing recovery reservation marker is invalid")?;
    let marker_metadata = marker
        .metadata()
        .map_err(|_| "the existing recovery reservation marker is invalid")?;
    if !is_private_owned_file(&marker_metadata) || link_count(&marker_metadata) != 1 {
        return Err("the existing recovery reservation marker is invalid".into());
    }
    match marker.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err("another recovery command still owns this target".into());
        }
        Err(TryLockError::Error(_)) => {
            return Err("the existing recovery reservation could not be locked".into());
        }
    }
    let control = match read_reservation_control(&mut marker) {
        Ok(control) => control,
        Err(_) if entries.as_slice() == [OsString::from(RESERVATION_FILENAME)] => {
            unlink_owned_child(&directory, RESERVATION_FILENAME, &marker)
                .and_then(|()| directory.sync_all())
                .and_then(|()| remove_empty_open_directory(path, &directory))
                .and_then(|()| sync_parent_directory(path))
                .map_err(|_| {
                    "the interrupted partial recovery reservation could not be reclaimed"
                })?;
            return Ok(());
        }
        Err(_) => return Err("the existing recovery reservation marker is invalid".into()),
    };
    if !reservation_control_matches_directory(path, &directory, &control) {
        return Err("the existing recovery reservation marker is invalid".into());
    }

    let allowed = [
        RESERVATION_FILENAME,
        COMPLETION_FILENAME,
        MAIN_TEMP_FILENAME,
        MEDIA_TEMP_FILENAME,
        MAIN_FILENAME,
        MEDIA_FILENAME,
    ];
    let mut owned_entries = Vec::new();
    for entry in &entries {
        let Some(name) = entry.to_str() else {
            return Err("the existing recovery reservation contains unexpected data".into());
        };
        if !allowed.contains(&name) {
            return Err("the existing recovery reservation contains unexpected data".into());
        }
        if name == RESERVATION_FILENAME {
            continue;
        }
        let file = open_regular_child(&directory, name)
            .map_err(|_| "the existing recovery reservation contains invalid data")?;
        if !is_private_owned_file(
            &file
                .metadata()
                .map_err(|_| "the existing recovery reservation contains invalid data")?,
        ) {
            return Err("the existing recovery reservation contains invalid data".into());
        }
        owned_entries.push((name.to_owned(), file));
    }

    if let Some(identity) = reservation_staging_identity(&control) {
        let staging_root = path
            .parent()
            .expect("validated recovery target parent")
            .join(format!(".kosh-restore-{}", control.token));
        // A mismatched or invalid path is not the directory this reservation
        // created. Preserve it and reclaim only the descriptor-owned target so
        // disaster recovery can safely continue with a fresh token.
        let _ = remove_staging_root_at_identity(&staging_root, identity);
    }
    for (name, file) in owned_entries {
        unlink_owned_child(&directory, &name, &file)
            .map_err(|_| "the interrupted recovery files could not be reclaimed")?;
    }
    unlink_owned_child(&directory, RESERVATION_FILENAME, &marker)
        .and_then(|()| directory.sync_all())
        .and_then(|()| remove_empty_open_directory(path, &directory))
        .and_then(|()| sync_parent_directory(path))
        .map_err(|_| "the interrupted recovery reservation could not be reclaimed")?;
    Ok(())
}

fn read_reservation_control(file: &mut File) -> std::io::Result<RecoveryReservationControl> {
    let length = file.metadata()?.len();
    if length == 0 || length > MAX_RESERVATION_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid recovery reservation length",
        ));
    }
    file.rewind()?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_RESERVATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unstable recovery reservation length",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid recovery reservation control",
        )
    })
}

fn write_reservation_control(
    file: &mut File,
    control: &RecoveryReservationControl,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(control)
        .map_err(|_| std::io::Error::other("invalid recovery reservation control"))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_RESERVATION_BYTES {
        return Err(std::io::Error::other(
            "invalid recovery reservation control length",
        ));
    }
    file.rewind()?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn reservation_staging_identity(
    control: &RecoveryReservationControl,
) -> Option<StagingDirectoryIdentity> {
    Some(StagingDirectoryIdentity {
        device: control.staging_device?,
        inode: control.staging_inode?,
    })
}

fn reservation_target_identity(
    control: &RecoveryReservationControl,
) -> Option<StagingDirectoryIdentity> {
    Some(StagingDirectoryIdentity {
        device: control.target_device?,
        inode: control.target_inode?,
    })
}

fn reservation_control_matches(path: &Path, control: &RecoveryReservationControl) -> bool {
    control.schema_version == RESERVATION_SCHEMA_VERSION
        && reservation_target_identity(control).is_some()
        && path.to_str() == Some(control.target_data_directory.as_str())
        && uuid::Uuid::parse_str(&control.token)
            .ok()
            .is_some_and(|token| token.to_string() == control.token)
        && ((control.staging_device.is_none() && control.staging_inode.is_none())
            || reservation_staging_identity(control).is_some())
}

fn reservation_control_matches_directory(
    path: &Path,
    directory: &File,
    control: &RecoveryReservationControl,
) -> bool {
    let Some(identity) = reservation_target_identity(control) else {
        return false;
    };
    reservation_control_matches(path, control)
        && directory.metadata().ok().is_some_and(|metadata| {
            use std::os::unix::fs::MetadataExt;
            metadata.dev() == identity.device && metadata.ino() == identity.inode
        })
}

fn is_private_owned_directory(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_dir()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o777 == 0o700
}

fn is_private_owned_file(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o777 == 0o600
}

fn link_count(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink()
}

fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recovery target has no parent",
        )
    })?;
    File::open(parent)?.sync_all()
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
            "recovery reservation is not a directory",
        ));
    }
    Ok(directory)
}

fn create_private_child(directory: &File, name: &str) -> std::io::Result<File> {
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
    Ok(unsafe { File::from_raw_fd(descriptor) })
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
            "recovery child is not a regular file",
        ));
    }
    Ok(file)
}

fn copy_open_regular_into_child(
    source: &File,
    directory: &File,
    name: &str,
) -> std::io::Result<File> {
    let mut source = source.try_clone()?;
    if !source.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recovery source is not a regular file",
        ));
    }
    let source_length = source.metadata()?.len();
    source.rewind()?;
    let mut target = create_private_child(directory, name)?;
    let copy = (|| {
        let copied = std::io::copy(&mut source, &mut target)?;
        if copied != source_length {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "recovery database copy was incomplete",
            ));
        }
        target.sync_all()
    })();
    if let Err(error) = copy {
        let _ = unlink_owned_child(directory, name, &target);
        return Err(error);
    }
    Ok(target)
}

fn link_child_exclusive(directory: &File, source: &str, target: &str) -> std::io::Result<()> {
    let source = child_name(source)?;
    let target = child_name(target)?;
    let result = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
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
            "recovery child ownership changed",
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

fn remove_empty_open_directory(path: &Path, directory: &File) -> std::io::Result<()> {
    if path_matches_open_file(path, directory) && directory_entries(directory)?.is_empty() {
        fs::remove_dir(path)?;
    }
    Ok(())
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

fn verify_owned_child_matches_source(
    directory: &File,
    name: &str,
    owned: &File,
    source: &File,
) -> std::io::Result<()> {
    let installed = open_regular_child(directory, name)?;
    if !same_open_file(&installed, owned) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "installed recovery child identity changed",
        ));
    }
    if source.metadata()?.len() != installed.metadata()?.len()
        || sha256_file(source)? != sha256_file(&installed)?
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "installed recovery child bytes changed",
        ));
    }
    Ok(())
}

fn sha256_file(file: &File) -> std::io::Result<[u8; 32]> {
    let mut file = file.try_clone()?;
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
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
            "invalid recovery child name",
        ));
    }
    CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid recovery child name",
        )
    })
}

fn select_checkpoint(
    checkpoints: Vec<RemoteCheckpoint>,
    selector: &str,
) -> Result<RemoteCheckpoint, String> {
    if selector == LATEST {
        return checkpoints
            .into_iter()
            .next()
            .ok_or_else(|| "the backup set has no complete recovery point".into());
    }
    let checkpoint_id =
        CheckpointId::parse(selector).map_err(|_| "invalid checkpoint selector".to_owned())?;
    checkpoints
        .into_iter()
        .find(|checkpoint| checkpoint.checkpoint_id() == &checkpoint_id)
        .ok_or_else(|| "the selected complete recovery point was not found".into())
}

fn load_target() -> Result<R2Target, String> {
    let account_id = take_required_environment(ACCOUNT_ID_ENV)?;
    let jurisdiction = take_required_environment(JURISDICTION_ENV)?;
    let bucket = take_required_environment(BUCKET_ENV)?;
    Ok(R2Target {
        account_id: R2AccountId::parse(account_id)
            .map_err(|_| "the recovery R2 account ID is invalid".to_owned())?,
        jurisdiction: R2Jurisdiction::from_db(&jurisdiction)
            .map_err(|_| "the recovery R2 jurisdiction is invalid".to_owned())?,
        bucket: R2BucketName::parse(bucket)
            .map_err(|_| "the recovery R2 bucket is invalid".to_owned())?,
    })
}

fn load_credentials() -> Result<R2Credentials, String> {
    let mut access_key = take_required_environment(ACCESS_KEY_ENV)?;
    let mut secret_key = take_required_environment(SECRET_KEY_ENV)?;
    let credentials = R2Credentials::new(
        std::mem::take(&mut access_key),
        std::mem::take(&mut secret_key),
    )
    .map_err(|_| "the recovery R2 credentials are invalid".to_owned());
    access_key.zeroize();
    secret_key.zeroize();
    credentials
}

fn take_required_environment(name: &str) -> Result<String, String> {
    let value = std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is required"))?;
    std::env::remove_var(name);
    Ok(value)
}

fn packaged_resource_directory() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|_| "the Kosh executable path is unavailable")?;
    let directory = executable
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources"))
        .ok_or_else(|| "the Kosh resource directory is unavailable".to_owned())?;
    if cfg!(debug_assertions) && std::env::var_os("KOSH_LITESTREAM_PATH").is_some() {
        return Ok(directory);
    }
    let metadata = fs::symlink_metadata(&directory)
        .map_err(|_| "the packaged Kosh resource directory is unavailable")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the packaged Kosh resource directory is invalid".into());
    }
    Ok(directory)
}

fn validate_new_target(path: &Path) -> Result<(), String> {
    validate_target_location(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("the recovery target already exists; no data was changed".into()),
        Err(_) => Err("the recovery target could not be inspected".into()),
    }
}

fn validate_target_location(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("the recovery target must be an absolute, new data directory".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the recovery target needs a parent directory".to_owned())?;
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| "the recovery target parent does not exist")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the recovery target parent must be a real directory".into());
    }
    Ok(())
}

fn parse_utf8(value: &OsString, label: &str) -> Result<String, String> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("the {label} is not valid UTF-8"))
}

fn count(connection: &rusqlite::Connection, sql: &str) -> Result<u64, String> {
    let value = connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map_err(|_| "the restored acceptance evidence could not be counted")?;
    u64::try_from(value).map_err(|_| "the restored acceptance count is invalid".into())
}

fn audit_staged_database(directory: &File) -> Result<StagedAcceptanceEvidence, String> {
    with_bound_working_directory(directory, || {
        let database = Database::initialize(DatabasePaths::new("."))
            .map_err(|_| "the independently staged Kosh library did not reopen normally")?;
        let search_documents_rebuilt = database
            .client()
            .rebuild_search()
            .map_err(|_| "the staged lexical search projection could not be rebuilt")?;
        database
            .client()
            .full_integrity_check()
            .map_err(|_| "the staged Kosh library failed its full integrity check")?;
        let main = database
            .open_main_read_only()
            .map_err(|_| "the staged Kosh evidence could not be inspected")?;
        let media = database
            .open_media_read_only()
            .map_err(|_| "the staged Kosh media could not be inspected")?;
        let evidence = StagedAcceptanceEvidence {
            active_tidbits: count(
                &main,
                "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL",
            )?,
            revisions: count(&main, "SELECT count(*) FROM tidbit_revision")?,
            attachments: count(&main, "SELECT count(*) FROM attachment")?,
            media_blobs: count(&media, "SELECT count(*) FROM media_blob")?,
            search_documents_rebuilt,
        };
        drop(media);
        drop(main);
        database
            .shutdown()
            .map_err(|_| "the staged Kosh library did not close cleanly")?;
        Ok(evidence)
    })
}

fn with_bound_working_directory<T>(
    directory: &File,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let binding = WorkingDirectoryBinding::enter(directory)
        .map_err(|_| "the staged Kosh directory identity could not be bound")?;
    let result = operation();
    binding
        .restore()
        .map_err(|_| "the recovery process working directory could not be restored")?;
    result
}

#[derive(Debug)]
struct WorkingDirectoryBinding {
    previous: File,
    active: bool,
}

impl WorkingDirectoryBinding {
    fn enter(directory: &File) -> std::io::Result<Self> {
        let previous = open_directory_no_follow(Path::new("."))?;
        if unsafe { libc::fchdir(directory.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            previous,
            active: true,
        })
    }

    fn restore(mut self) -> std::io::Result<()> {
        if unsafe { libc::fchdir(self.previous.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for WorkingDirectoryBinding {
    fn drop(&mut self) {
        if self.active {
            let _ = unsafe { libc::fchdir(self.previous.as_raw_fd()) };
        }
    }
}

fn usage() -> String {
    "usage: kosh recovery remote-restore <backup-set-id> <latest|checkpoint-id> <new-absolute-data-directory>".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt;

    fn descriptor_bytes(file: &File) -> Vec<u8> {
        let length = usize::try_from(file.metadata().expect("descriptor metadata").len())
            .expect("descriptor length fits memory");
        let mut bytes = vec![0_u8; length];
        file.read_exact_at(&mut bytes, 0)
            .expect("read descriptor bytes");
        bytes
    }

    fn completed_report(backup_set_id: &BackupSetId, target: &Path) -> RemoteRestoreReport {
        RemoteRestoreReport {
            schema_version: 1,
            result: "PASSED".into(),
            backup_set_id: backup_set_id.to_string(),
            checkpoint_id: CheckpointId::new().to_string(),
            target_data_directory: target.to_string_lossy().into_owned(),
            restored_media_count: 0,
            restored_media_bytes: 0,
            active_tidbits: 0,
            revisions: 0,
            attachments: 0,
            media_blobs: 0,
            search_documents_rebuilt: 0,
            safety_snapshot_created: false,
        }
    }

    #[test]
    fn command_atomically_reserves_a_canonical_new_absolute_target() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        validate_new_target(&target).expect("new target");
        let reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");
        assert!(target.join(RESERVATION_FILENAME).is_file());
        assert_eq!(
            validate_new_target(&target),
            Err("the recovery target already exists; no data was changed".into())
        );
        assert!(RecoveryTargetReservation::reserve(&target).is_err());
        assert!(BackupSetId::parse("not-a-backup-set").is_err());
        assert!(CheckpointId::parse("not-a-checkpoint").is_err());
        drop(reservation);
        assert!(!target.exists(), "unused reservation must clean up");
    }

    #[test]
    fn interrupted_private_reservation_and_staging_are_reclaimed_before_retry() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let mut interrupted =
            RecoveryTargetReservation::reserve(&target).expect("initial reservation");
        let first_token = interrupted.token.clone();
        let (staging_root, _) = interrupted
            .prepare_staging_root()
            .expect("persist interrupted staging identity");
        let staged = Database::initialize(DatabasePaths::new(&staging_root))
            .expect("interrupted staged pair");
        staged.shutdown().expect("close interrupted staged pair");
        interrupted.active = false;
        drop(interrupted);

        let retry = RecoveryTargetReservation::reserve(&target).expect("reclaimed retry");
        assert_ne!(retry.token, first_token);
        assert!(
            !staging_root.exists(),
            "reclaim must remove only the token-bound staging pair"
        );
        assert!(target.join(RESERVATION_FILENAME).is_file());
        drop(retry);
        assert!(!target.exists(), "retry reservation must remain owned");
    }

    #[test]
    fn abandoned_target_reclamation_rejects_a_copied_marker_in_a_substituted_directory() {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let mut interrupted =
            RecoveryTargetReservation::reserve(&target).expect("initial reservation");
        let marker_bytes = fs::read(target.join(RESERVATION_FILENAME)).expect("reservation marker");
        interrupted.active = false;
        drop(interrupted);

        let displaced = root.path().join("displaced-reservation");
        fs::rename(&target, &displaced).expect("displace original reservation");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&target).expect("replacement directory");
        let replacement_paths = DatabasePaths::new(&target);
        let replacement =
            Database::initialize(replacement_paths.clone()).expect("replacement library");
        replacement.shutdown().expect("close replacement library");
        let replacement_main = fs::read(&replacement_paths.main).expect("replacement main bytes");
        let replacement_media =
            fs::read(&replacement_paths.media).expect("replacement media bytes");
        let mut copied_marker = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(target.join(RESERVATION_FILENAME))
            .expect("copied reservation marker");
        copied_marker
            .write_all(&marker_bytes)
            .expect("persist copied marker");
        copied_marker.sync_all().expect("sync copied marker");
        drop(copied_marker);

        assert!(RecoveryTargetReservation::reserve(&target).is_err());
        assert_eq!(
            fs::read(&replacement_paths.main).expect("preserved replacement main"),
            replacement_main
        );
        assert_eq!(
            fs::read(&replacement_paths.media).expect("preserved replacement media"),
            replacement_media
        );
        assert!(target.join(RESERVATION_FILENAME).is_file());
        assert!(displaced.join(RESERVATION_FILENAME).is_file());
    }

    #[test]
    fn abandoned_staging_reclamation_preserves_a_replacement_library() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let mut interrupted =
            RecoveryTargetReservation::reserve(&target).expect("initial reservation");
        let (staging_root, _) = interrupted
            .prepare_staging_root()
            .expect("persist staging identity");
        let original =
            Database::initialize(DatabasePaths::new(&staging_root)).expect("original staged pair");
        original.shutdown().expect("close original staged pair");
        let displaced = root.path().join("displaced-staging");
        fs::rename(&staging_root, &displaced).expect("displace owned staging directory");

        let replacement_paths = DatabasePaths::new(&staging_root);
        let replacement =
            Database::initialize(replacement_paths.clone()).expect("replacement library");
        replacement.shutdown().expect("close replacement library");
        let replacement_main = fs::read(&replacement_paths.main).expect("replacement main bytes");
        let replacement_media =
            fs::read(&replacement_paths.media).expect("replacement media bytes");
        interrupted.active = false;
        drop(interrupted);

        let retry = RecoveryTargetReservation::reserve(&target)
            .expect("retry must reclaim only the owned target reservation");
        assert_eq!(
            fs::read(&replacement_paths.main).expect("preserved replacement main"),
            replacement_main
        );
        assert_eq!(
            fs::read(&replacement_paths.media).expect("preserved replacement media"),
            replacement_media
        );
        assert!(
            displaced.join(MAIN_FILENAME).is_file(),
            "the renamed original is outside the authenticated cleanup path"
        );
        drop(retry);
    }

    #[test]
    fn partial_private_reservation_is_reclaimable_but_unexpected_data_is_preserved() {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        let root = tempfile::tempdir().expect("recovery parent");
        let partial = root.path().join("partial");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&partial).expect("partial reservation");
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(partial.join(RESERVATION_FILENAME))
            .expect("partial marker");
        let reclaimed =
            RecoveryTargetReservation::reserve(&partial).expect("reclaim partial reservation");
        drop(reclaimed);
        assert!(!partial.exists());

        let protected = root.path().join("protected");
        let mut abandoned =
            RecoveryTargetReservation::reserve(&protected).expect("owned reservation");
        abandoned.active = false;
        drop(abandoned);
        fs::write(protected.join("notes.txt"), b"preserve me").expect("unexpected user data");
        assert!(RecoveryTargetReservation::reserve(&protected).is_err());
        assert_eq!(
            fs::read(protected.join("notes.txt")).expect("preserved user data"),
            b"preserve me"
        );
    }

    #[test]
    fn staging_cleanup_failure_rolls_back_the_uncommitted_installed_pair() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let staged_root = root.path().join("source");
        let staged_paths = DatabasePaths::new(&staged_root);
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let staged_pair =
            StagedDatabasePair::open_for_test(&staged_paths).expect("bind staged pair");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");
        let recovery_staging_root = reservation.staging_root();
        fs::create_dir(&recovery_staging_root).expect("recovery staging root");
        fs::write(recovery_staging_root.join("foreign.txt"), b"preserve me")
            .expect("unexpected staging data");

        reservation
            .install_validated_pair(&staged_pair)
            .expect("descriptor-bound install");
        assert!(matches!(
            remove_staging_root(&recovery_staging_root),
            Err(super::super::restore::RestoreError::InvalidStaging)
        ));
        drop(reservation);

        assert!(
            !target.exists(),
            "a cleanup failure must roll back the uncommitted installed pair"
        );
        assert_eq!(
            fs::read(recovery_staging_root.join("foreign.txt")).expect("preserved staging data"),
            b"preserve me"
        );
    }

    #[test]
    fn descriptor_bound_install_publishes_a_durable_retry_receipt() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let staged_root = tempfile::tempdir().expect("staged pair");
        let staged_paths = DatabasePaths::new(staged_root.path());
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let staged_pair =
            StagedDatabasePair::open_for_test(&staged_paths).expect("bind staged pair");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");

        reservation
            .install_validated_pair(&staged_pair)
            .expect("descriptor-bound install");
        reservation
            .verify_installed_pair_matches(&staged_pair)
            .expect("descriptor-bound byte verification");
        let backup_set_id = BackupSetId::new();
        let report = completed_report(&backup_set_id, &target);
        reservation
            .publish_completed(&backup_set_id, LATEST, &report)
            .expect("publish completed recovery");

        assert!(target.join(MAIN_FILENAME).is_file());
        assert!(target.join(MEDIA_FILENAME).is_file());
        assert!(target.join(COMPLETION_FILENAME).is_file());
        for control in [
            RESERVATION_FILENAME,
            MAIN_TEMP_FILENAME,
            MEDIA_TEMP_FILENAME,
        ] {
            assert!(!target.join(control).exists(), "{control} must be removed");
        }
        assert_eq!(
            load_completed_recovery(&target, &backup_set_id, LATEST)
                .expect("load durable completed recovery"),
            Some(report.clone())
        );
        assert_eq!(
            run(vec![
                OsString::from(REMOTE_RESTORE),
                OsString::from(backup_set_id.to_string()),
                OsString::from(LATEST),
                target.as_os_str().to_owned(),
            ])
            .expect("retry exact completed command without remote credentials"),
            report
        );
        let restored =
            Database::initialize(DatabasePaths::new(&target)).expect("reopen installed pair");
        restored.shutdown().expect("close installed pair");
    }

    #[test]
    fn descriptor_bound_install_uses_the_audited_staging_pair_after_parent_replacement() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let staging_root = root.path().join("staging");
        let displaced_staging = root.path().join("displaced-staging");
        let staged_paths = DatabasePaths::new(&staging_root);
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let staged_pair =
            StagedDatabasePair::open_for_test(&staged_paths).expect("bind audited staged pair");
        let audited_main = descriptor_bytes(staged_pair.main());
        let audited_media = descriptor_bytes(staged_pair.media());

        fs::rename(&staging_root, &displaced_staging).expect("displace audited staging root");
        let replacement_paths = DatabasePaths::new(&staging_root);
        let replacement =
            Database::initialize(replacement_paths.clone()).expect("replacement database pair");
        replacement.shutdown().expect("close replacement pair");
        let replacement_main = fs::read(&replacement_paths.main).expect("replacement main bytes");
        let replacement_media =
            fs::read(&replacement_paths.media).expect("replacement media bytes");
        assert_ne!(
            audited_main, replacement_main,
            "the substitution fixture must contain a different valid Kosh library"
        );

        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");
        reservation
            .install_validated_pair(&staged_pair)
            .expect("install descriptor-bound audited pair");
        reservation
            .verify_installed_pair_matches(&staged_pair)
            .expect("verify descriptor-bound audited pair");

        assert_eq!(
            fs::read(target.join(MAIN_FILENAME)).expect("installed main bytes"),
            audited_main
        );
        assert_eq!(
            fs::read(target.join(MEDIA_FILENAME)).expect("installed media bytes"),
            audited_media
        );
        assert_eq!(
            fs::read(&replacement_paths.main).expect("preserved replacement main"),
            replacement_main
        );
        assert_eq!(
            fs::read(&replacement_paths.media).expect("preserved replacement media"),
            replacement_media
        );
    }

    #[test]
    fn write_capable_staging_audit_stays_bound_after_parent_replacement() {
        let status =
            std::process::Command::new(std::env::current_exe().expect("recovery test executable"))
                .args([
                    "--exact",
                    "backup::recovery_cli::tests::write_capable_staging_audit_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("KOSH_BOUND_STAGING_AUDIT_WORKER", "1")
                .status()
                .expect("spawn isolated staging-audit worker");
        assert!(status.success(), "isolated staging audit must pass");
    }

    #[test]
    #[ignore = "invoked in an isolated process by the descriptor-binding test"]
    fn write_capable_staging_audit_worker() {
        if std::env::var_os("KOSH_BOUND_STAGING_AUDIT_WORKER").is_none() {
            return;
        }
        std::env::remove_var("KOSH_BOUND_STAGING_AUDIT_WORKER");
        let root = tempfile::tempdir().expect("staging audit parent");
        let staging_root = root.path().join("staging");
        let displaced = root.path().join("displaced-staging");
        let staged_paths = DatabasePaths::new(&staging_root);
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged database pair");
        let staging_directory =
            open_directory_no_follow(&staging_root).expect("retained staging directory descriptor");

        fs::rename(&staging_root, &displaced).expect("displace staged directory");
        let replacement_paths = DatabasePaths::new(&staging_root);
        let replacement =
            Database::initialize(replacement_paths.clone()).expect("replacement database pair");
        replacement.shutdown().expect("close replacement pair");
        let replacement_main = fs::read(&replacement_paths.main).expect("replacement main bytes");
        let replacement_media =
            fs::read(&replacement_paths.media).expect("replacement media bytes");
        let replacement_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&replacement_paths.ownership_lock)
            .expect("replacement ownership lock");
        replacement_lock
            .try_lock()
            .expect("exclude a path-based replacement reopen");

        let evidence = audit_staged_database(&staging_directory)
            .expect("descriptor-bound write-capable audit");
        assert_eq!(evidence.active_tidbits, 0);
        assert_eq!(evidence.search_documents_rebuilt, 0);
        assert_eq!(
            fs::read(&replacement_paths.main).expect("preserved replacement main"),
            replacement_main
        );
        assert_eq!(
            fs::read(&replacement_paths.media).expect("preserved replacement media"),
            replacement_media
        );
        drop(replacement_lock);
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_bound_audit_refuses_a_raced_reopen_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let displaced = root.path().join("displaced");
        let staged_root = tempfile::tempdir().expect("staged pair");
        let staged_paths = DatabasePaths::new(staged_root.path());
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let staged_pair =
            StagedDatabasePair::open_for_test(&staged_paths).expect("bind staged pair");
        let outside_parent = tempfile::tempdir().expect("outside parent");
        let outside_root = outside_parent.path().join("existing-library");
        let outside_paths = DatabasePaths::new(&outside_root);
        let outside = Database::initialize(outside_paths.clone()).expect("outside database pair");
        outside.shutdown().expect("close outside pair");
        let outside_main = fs::read(&outside_paths.main).expect("outside main bytes");
        let outside_media = fs::read(&outside_paths.media).expect("outside media bytes");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");
        reservation
            .install_validated_pair(&staged_pair)
            .expect("descriptor-bound install");

        fs::rename(&target, &displaced).expect("displace installed reservation");
        symlink(&outside_root, &target).expect("raced target symlink");
        assert_eq!(
            reservation.verify_installed_pair_matches(&staged_pair),
            Err("the recovery target was replaced after installation".into())
        );

        drop(reservation);
        assert!(fs::symlink_metadata(&target)
            .expect("preserved target symlink")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(&outside_paths.main).expect("preserved outside main"),
            outside_main
        );
        assert_eq!(
            fs::read(&outside_paths.media).expect("preserved outside media"),
            outside_media
        );
        assert_eq!(
            fs::read_dir(&displaced)
                .expect("descriptor-owned displaced directory")
                .count(),
            0,
            "descriptor-bound audit cleanup must remove only owned installed children"
        );
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_bound_install_rolls_back_a_raced_directory_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let displaced = root.path().join("displaced");
        let staged_root = tempfile::tempdir().expect("staged pair");
        let staged_paths = DatabasePaths::new(staged_root.path());
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let staged_pair =
            StagedDatabasePair::open_for_test(&staged_paths).expect("bind staged pair");
        let outside_parent = tempfile::tempdir().expect("outside parent");
        let outside_root = outside_parent.path().join("existing-library");
        let outside_paths = DatabasePaths::new(&outside_root);
        let outside = Database::initialize(outside_paths.clone()).expect("outside database pair");
        outside.shutdown().expect("close outside pair");
        let outside_main = fs::read(&outside_paths.main).expect("outside main bytes");
        let outside_media = fs::read(&outside_paths.media).expect("outside media bytes");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");

        let result = reservation.install_validated_pair_with_hook(&staged_pair, || {
            fs::rename(&target, &displaced).expect("displace reservation");
            symlink(&outside_root, &target).expect("raced target symlink");
        });
        assert_eq!(
            result,
            Err(
                "the clean recovery target changed during installation; no existing data was modified"
                    .into()
            )
        );
        drop(reservation);
        assert!(fs::symlink_metadata(&target)
            .expect("preserved target symlink")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(&outside_paths.main).expect("preserved outside main"),
            outside_main
        );
        assert_eq!(
            fs::read(&outside_paths.media).expect("preserved outside media"),
            outside_media
        );
        assert_eq!(
            fs::read_dir(&displaced)
                .expect("descriptor-owned displaced directory")
                .count(),
            0,
            "descriptor-relative rollback must remove only its own staged children"
        );
    }

    #[test]
    fn selector_uses_newest_or_an_exact_checkpoint_only() {
        assert!(select_checkpoint(Vec::new(), LATEST).is_err());
        assert!(select_checkpoint(Vec::new(), "not-a-checkpoint").is_err());
    }

    #[test]
    fn recovery_usage_is_stable_and_does_not_name_credentials() {
        let text = usage();
        assert!(text.contains("remote-restore"));
        assert!(!text.to_ascii_lowercase().contains("secret"));
        assert!(!text.to_ascii_lowercase().contains("access key"));
    }

    #[test]
    fn ephemeral_runtime_uses_a_short_socket_path_and_removes_only_its_owned_root() {
        let mut ephemeral = EphemeralLitestreamRuntime::create().expect("ephemeral runtime");
        let root = ephemeral
            .paths()
            .directory()
            .parent()
            .and_then(Path::parent)
            .expect("ephemeral root")
            .to_owned();
        ephemeral.paths().prepare().expect("prepare runtime");
        assert!(ephemeral.paths().socket().as_os_str().len() < 100);
        assert!(root.exists());
        ephemeral.cleanup().expect("cleanup runtime");
        ephemeral.cleanup().expect("idempotent cleanup");
        assert!(!root.exists());
    }
}
