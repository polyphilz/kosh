use std::{
    ffi::{CStr, CString, OsString},
    fs::{self, File, OpenOptions},
    io::Seek,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStringExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use zeroize::Zeroize;

use crate::database::{validate_restored_pair, Database, DatabasePaths};

use super::{
    credentials::R2Credentials,
    domain::{BackupSetId, CheckpointId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target},
    litestream::{CommandLitestreamRestore, EphemeralLitestreamRuntime, VerifiedLitestreamBinary},
    object_store::R2ObjectStore,
    restore::{discover_checkpoints, remove_staged_checkpoint, stage_checkpoint, RemoteCheckpoint},
};

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
const MAIN_TEMP_FILENAME: &str = ".kosh-recovery-main-v1.tmp";
const MEDIA_TEMP_FILENAME: &str = ".kosh-recovery-media-v1.tmp";
const MAIN_FILENAME: &str = "kosh.sqlite3";
const MEDIA_FILENAME: &str = "media.sqlite3";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteRestoreReport {
    schema_version: u32,
    result: &'static str,
    backup_set_id: String,
    checkpoint_id: String,
    target_data_directory: String,
    restored_media_count: u64,
    restored_media_bytes: u64,
    active_tidbits: u64,
    revisions: u64,
    sources: u64,
    attachments: u64,
    media_blobs: u64,
    search_documents_rebuilt: u64,
    research_runs: u64,
    research_citations: u64,
    safety_snapshot_created: bool,
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
    validate_new_target(&target_root)?;
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
    let checkpoints = discover_checkpoints(&store, &keyspace, backup_set_id)
        .map_err(|_| "complete recovery points could not be discovered")?;
    let checkpoint = select_checkpoint(checkpoints, selector)?;

    let parent = target_root
        .parent()
        .ok_or_else(|| "the recovery directory needs a parent".to_owned())?;
    let staging_root = parent.join(format!(".kosh-restore-{}", uuid::Uuid::now_v7()));
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
        stage_checkpoint(
            &store,
            &keyspace,
            &checkpoint,
            &engine,
            ephemeral.source_database_path(),
            &staging_root,
        )
        .map_err(|_| "the selected recovery point failed exact database or media validation")?
    };
    ephemeral
        .cleanup()
        .map_err(|_| "the isolated recovery runtime could not be removed")?;
    let restored_media_count = staged.restored_media_count;
    let restored_media_bytes = staged.restored_media_bytes;
    reservation.install_validated_pair(&staged.paths)?;
    remove_staged_checkpoint(&staged)
        .map_err(|_| "the validated recovery staging pair could not be removed")?;

    let database = Database::initialize(DatabasePaths::new(&target_root))
        .map_err(|_| "the restored Kosh library did not reopen normally")?;
    let search_documents_rebuilt = database
        .client()
        .rebuild_search()
        .map_err(|_| "the restored lexical search projection could not be rebuilt")?;
    database
        .client()
        .full_integrity_check()
        .map_err(|_| "the reopened Kosh library failed its full integrity check")?;
    let main = database
        .open_main_read_only()
        .map_err(|_| "the restored Kosh evidence could not be inspected")?;
    let media = database
        .open_media_read_only()
        .map_err(|_| "the restored Kosh media could not be inspected")?;
    let report = RemoteRestoreReport {
        schema_version: 1,
        result: "PASSED",
        backup_set_id: backup_set_id.to_string(),
        checkpoint_id: checkpoint.checkpoint_id().to_string(),
        target_data_directory: target_root.to_string_lossy().into_owned(),
        restored_media_count,
        restored_media_bytes,
        active_tidbits: count(
            &main,
            "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL",
        )?,
        revisions: count(&main, "SELECT count(*) FROM tidbit_revision")?,
        sources: count(&main, "SELECT count(*) FROM source")?,
        attachments: count(&main, "SELECT count(*) FROM attachment")?,
        media_blobs: count(&media, "SELECT count(*) FROM media_blob")?,
        search_documents_rebuilt,
        research_runs: count(&main, "SELECT count(*) FROM research_run")?,
        research_citations: count(
            &main,
            "SELECT coalesce(sum(json_array_length(final_answer_json, '$.citations')), 0)
             FROM research_run
             WHERE final_answer_json IS NOT NULL",
        )?,
        safety_snapshot_created: false,
    };
    drop(media);
    drop(main);
    database
        .shutdown()
        .map_err(|_| "the restored Kosh library did not close cleanly")?;
    Ok(report)
}

#[derive(Debug)]
struct RecoveryTargetReservation {
    root: PathBuf,
    directory: File,
    marker: File,
    active: bool,
}

impl RecoveryTargetReservation {
    fn reserve(path: &Path) -> Result<Self, String> {
        validate_new_target(path)?;
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
        let directory_is_empty =
            matches!(directory_entries(&directory), Ok(entries) if entries.is_empty());
        if !path_matches_open_file(path, &directory) || !directory_is_empty {
            let _ = remove_empty_open_directory(path, &directory);
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        let marker = match create_private_child(&directory, RESERVATION_FILENAME) {
            Ok(marker) => marker,
            Err(_) => {
                let _ = remove_empty_open_directory(path, &directory);
                return Err("the recovery target reservation could not be created".into());
            }
        };
        if marker.sync_all().is_err() || directory.sync_all().is_err() {
            let _ = unlink_owned_child(&directory, RESERVATION_FILENAME, &marker);
            let _ = remove_empty_open_directory(path, &directory);
            return Err("the recovery target reservation could not be persisted".into());
        }
        Ok(Self {
            root: path.to_owned(),
            directory,
            marker,
            active: true,
        })
    }

    fn path(&self) -> &Path {
        &self.root
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

    fn install_validated_pair(&mut self, staged: &DatabasePaths) -> Result<(), String> {
        self.install_validated_pair_with_hook(staged, || {})
    }

    fn install_validated_pair_with_hook(
        &mut self,
        staged: &DatabasePaths,
        before_publish: impl FnOnce(),
    ) -> Result<(), String> {
        validate_restored_pair(staged)
            .map_err(|_| "the independently staged recovery pair is no longer valid")?;
        self.verify_install_ready()?;

        let main_temporary =
            copy_regular_into_child(&staged.main, &self.directory, MAIN_TEMP_FILENAME)
                .map_err(|_| "the recovered main database could not be privately staged")?;
        let media_temporary =
            match copy_regular_into_child(&staged.media, &self.directory, MEDIA_TEMP_FILENAME) {
                Ok(file) => file,
                Err(_) => {
                    let _ =
                        unlink_owned_child(&self.directory, MAIN_TEMP_FILENAME, &main_temporary);
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
            unlink_owned_child(&self.directory, RESERVATION_FILENAME, &self.marker)?;
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
        self.active = false;
        Ok(())
    }
}

impl Drop for RecoveryTargetReservation {
    fn drop(&mut self) {
        if self.active {
            let _ = unlink_owned_child(&self.directory, RESERVATION_FILENAME, &self.marker);
            let _ = self.directory.sync_all();
            let _ = remove_empty_open_directory(&self.root, &self.directory);
        }
    }
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

fn open_regular_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recovery source is not a regular file",
        ));
    }
    Ok(file)
}

fn create_private_child(directory: &File, name: &str) -> std::io::Result<File> {
    let name = child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
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

fn copy_regular_into_child(source: &Path, directory: &File, name: &str) -> std::io::Result<File> {
    let mut source = open_regular_no_follow(source)?;
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
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("the recovery target must be an absolute, new data directory".into());
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("the recovery target already exists; no data was changed".into()),
        Err(_) => return Err("the recovery target could not be inspected".into()),
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

fn usage() -> String {
    "usage: kosh recovery remote-restore <backup-set-id> <latest|checkpoint-id> <new-absolute-data-directory>".into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn descriptor_bound_install_publishes_a_valid_pair_without_control_residue() {
        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let staged_root = tempfile::tempdir().expect("staged pair");
        let staged_paths = DatabasePaths::new(staged_root.path());
        let staged = Database::initialize(staged_paths.clone()).expect("staged database pair");
        staged.shutdown().expect("close staged pair");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");

        reservation
            .install_validated_pair(&staged_paths)
            .expect("descriptor-bound install");

        assert!(target.join(MAIN_FILENAME).is_file());
        assert!(target.join(MEDIA_FILENAME).is_file());
        for control in [
            RESERVATION_FILENAME,
            MAIN_TEMP_FILENAME,
            MEDIA_TEMP_FILENAME,
        ] {
            assert!(!target.join(control).exists(), "{control} must be removed");
        }
        let restored =
            Database::initialize(DatabasePaths::new(&target)).expect("reopen installed pair");
        restored.shutdown().expect("close installed pair");
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
        let outside_parent = tempfile::tempdir().expect("outside parent");
        let outside_root = outside_parent.path().join("existing-library");
        let outside_paths = DatabasePaths::new(&outside_root);
        let outside = Database::initialize(outside_paths.clone()).expect("outside database pair");
        outside.shutdown().expect("close outside pair");
        let outside_main = fs::read(&outside_paths.main).expect("outside main bytes");
        let outside_media = fs::read(&outside_paths.media).expect("outside media bytes");
        let mut reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");

        let result = reservation.install_validated_pair_with_hook(&staged_paths, || {
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
        assert!(!root.exists());
    }
}
