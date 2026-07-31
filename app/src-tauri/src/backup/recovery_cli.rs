use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use zeroize::Zeroize;

use crate::database::{Database, DatabasePaths};

use super::{
    credentials::R2Credentials,
    domain::{BackupSetId, CheckpointId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target},
    litestream::{CommandLitestreamRestore, EphemeralLitestreamRuntime, VerifiedLitestreamBinary},
    object_store::R2ObjectStore,
    restore::{
        discover_checkpoints, install_checkpoint_into_empty, stage_checkpoint, RemoteCheckpoint,
    },
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
const MAX_RESERVATION_BYTES: u64 = 128;

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
    reservation.verify_install_ready()?;
    let install = install_checkpoint_into_empty(&DatabasePaths::new(&target_root), &staged)
        .map_err(|_| "the validated recovery point could not be installed")?;
    if install.safety_snapshot_id.is_some() {
        return Err("a clean-directory recovery unexpectedly replaced existing data".into());
    }
    reservation.finish()?;

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
    marker: PathBuf,
    token: String,
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

        let marker = path.join(RESERVATION_FILENAME);
        let token = uuid::Uuid::now_v7().to_string();
        let result = (|| {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&marker)
                .map_err(|_| "the recovery target reservation could not be created")?;
            file.write_all(token.as_bytes())
                .map_err(|_| "the recovery target reservation could not be persisted")?;
            file.sync_all()
                .map_err(|_| "the recovery target reservation could not be persisted")?;
            sync_directory(path)
                .map_err(|_| "the recovery target reservation could not be persisted")?;
            Ok(Self {
                root: path.to_owned(),
                marker,
                token,
                active: true,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(path.join(RESERVATION_FILENAME));
            let _ = fs::remove_dir(path);
        }
        result
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn verify_install_ready(&self) -> Result<(), String> {
        let root_metadata = fs::symlink_metadata(&self.root)
            .map_err(|_| "the recovery target reservation disappeared")?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        let marker_metadata = fs::symlink_metadata(&self.marker)
            .map_err(|_| "the recovery target reservation was replaced; no data was changed")?;
        if marker_metadata.file_type().is_symlink()
            || !marker_metadata.is_file()
            || marker_metadata.len() > MAX_RESERVATION_BYTES
        {
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        let marker = open_marker_no_follow(&self.marker)
            .map_err(|_| "the recovery target reservation was replaced; no data was changed")?;
        let mut token = String::new();
        marker
            .take(MAX_RESERVATION_BYTES + 1)
            .read_to_string(&mut token)
            .map_err(|_| "the recovery target reservation was replaced; no data was changed")?;
        if token != self.token {
            return Err("the recovery target reservation was replaced; no data was changed".into());
        }
        let entries = fs::read_dir(&self.root)
            .map_err(|_| "the recovery target reservation could not be inspected")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "the recovery target reservation could not be inspected")?;
        if entries.len() != 1
            || entries[0].file_name() != RESERVATION_FILENAME
            || !entries[0]
                .file_type()
                .is_ok_and(|file_type| file_type.is_file() && !file_type.is_symlink())
        {
            return Err("the recovery target stopped being empty; no data was changed".into());
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        verify_marker(&self.marker, &self.token)?;
        fs::remove_file(&self.marker)
            .map_err(|_| "the recovery target reservation could not be removed")?;
        sync_directory(&self.root)
            .map_err(|_| "the recovery target reservation removal was not durable")?;
        self.active = false;
        Ok(())
    }
}

impl Drop for RecoveryTargetReservation {
    fn drop(&mut self) {
        if self.active && self.verify_install_ready().is_ok() {
            let _ = fs::remove_file(&self.marker);
            let _ = fs::remove_dir(&self.root);
        }
    }
}

fn verify_marker(path: &Path, expected: &str) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "the recovery target reservation disappeared")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_RESERVATION_BYTES
    {
        return Err("the recovery target reservation was replaced".into());
    }
    let file =
        open_marker_no_follow(path).map_err(|_| "the recovery target reservation was replaced")?;
    let mut token = String::new();
    file.take(MAX_RESERVATION_BYTES + 1)
        .read_to_string(&mut token)
        .map_err(|_| "the recovery target reservation could not be read")?;
    if token != expected {
        return Err("the recovery target reservation was replaced".into());
    }
    Ok(())
}

fn open_marker_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
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

    #[cfg(unix)]
    #[test]
    fn target_reservation_rejects_a_raced_directory_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("recovery parent");
        let target = root.path().join("restored");
        let displaced = root.path().join("displaced");
        let outside = tempfile::tempdir().expect("outside target");
        let reservation = RecoveryTargetReservation::reserve(&target).expect("reserved target");
        fs::rename(&target, &displaced).expect("displace reservation");
        symlink(outside.path(), &target).expect("raced target symlink");

        assert_eq!(
            reservation.verify_install_ready(),
            Err("the recovery target reservation was replaced; no data was changed".into())
        );
        drop(reservation);
        assert!(fs::symlink_metadata(&target)
            .expect("preserved target symlink")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_dir(outside.path())
                .expect("outside entries")
                .count(),
            0,
            "a raced symlink target must remain untouched"
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
