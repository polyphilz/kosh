//! Restored-pair validation and startup cleanup for the retired in-place
//! installer. Production recovery publishes into a newly reserved directory;
//! the test-only installer remains to exercise legacy transaction recovery.

use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::{fs::TryLockError, io::Write, os::unix::fs::FileExt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backup::domain::CheckpointId;

use super::{
    connection::{self, DatabaseKind, FileState},
    migrations, safety_snapshot, validation, DatabaseError, DatabasePaths, Result,
};

const TRANSACTION_DIRECTORY: &str = ".restore-install-v1";
const JOURNAL_FILENAME: &str = "journal.json";
const RECEIPT_FILENAME: &str = "restore-install-v1.json";
const FORMAT_VERSION: u32 = 1;
const MAX_CONTROL_BYTES: u64 = 64 * 1024;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum RestoreInstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreInstallReport {
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) outcome: RestoreInstallOutcome,
    pub(crate) safety_snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallControl {
    format_version: u32,
    checkpoint_id: CheckpointId,
    had_existing_pair: bool,
    main_sha256: String,
    media_sha256: String,
}

pub(super) fn recover_interrupted(paths: &DatabasePaths) -> Result<()> {
    let transaction = paths.root.join(TRANSACTION_DIRECTORY);
    let metadata = match fs::symlink_metadata(&transaction) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid("restore transaction path is not a real directory"));
    }
    let journal = transaction.join(JOURNAL_FILENAME);
    if !journal.exists() {
        remove_owned_transaction(&transaction)?;
        return Ok(());
    }
    let control = read_control(&journal)?;
    let receipt = read_optional_control(&paths.root.join(RECEIPT_FILENAME))?;
    if receipt.as_ref() == Some(&control) && live_pair_matches(paths, &control)? {
        remove_owned_transaction(&transaction)?;
        return Ok(());
    }
    rollback(paths, &transaction, control.had_existing_pair)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn install(
    paths: &DatabasePaths,
    staged_main: &File,
    staged_media: &File,
    checkpoint_id: CheckpointId,
) -> Result<RestoreInstallReport> {
    let _lock = acquire_restore_lock(paths)?;
    recover_interrupted(paths)?;

    if let Some(report) = completed_install(paths, &checkpoint_id)? {
        return Ok(report);
    }

    validate_pair_at(staged_main, staged_media)?;
    let main_sha256 = hash_regular_descriptor(staged_main)?;
    let media_sha256 = hash_regular_descriptor(staged_media)?;
    let (main_state, media_state) = (
        connection::inspect_file(&paths.main)?,
        connection::inspect_file(&paths.media)?,
    );
    if main_state != media_state {
        return Err(DatabaseError::IncompletePair {
            main_state: main_state.label(),
            media_state: media_state.label(),
        });
    }
    let had_existing_pair = main_state == FileState::Existing;
    let safety_snapshot_id = if had_existing_pair {
        Some(safety_snapshot::create_pre_restore(paths)?.id)
    } else {
        None
    };
    let control = InstallControl {
        format_version: FORMAT_VERSION,
        checkpoint_id: checkpoint_id.clone(),
        had_existing_pair,
        main_sha256,
        media_sha256,
    };
    let transaction = paths.root.join(TRANSACTION_DIRECTORY);
    create_private_transaction(&transaction)?;
    write_control(&transaction.join(JOURNAL_FILENAME), &control)?;
    copy_regular_descriptor_synced(staged_main, &transaction.join("new-main.sqlite3"))?;
    copy_regular_descriptor_synced(staged_media, &transaction.join("new-media.sqlite3"))?;
    sync_directory(&transaction)?;

    let installation = install_transaction(paths, &transaction, &control);
    if let Err(error) = installation {
        let rollback_error = rollback(paths, &transaction, had_existing_pair).err();
        return Err(rollback_error.unwrap_or(error));
    }
    finish_install(
        paths,
        &transaction,
        &control,
        had_existing_pair,
        |receipt, control| {
            write_control(receipt, control)?;
            sync_directory(&paths.root)
        },
    )?;
    Ok(RestoreInstallReport {
        checkpoint_id,
        outcome: RestoreInstallOutcome::Installed,
        safety_snapshot_id,
    })
}

#[cfg(test)]
pub(crate) fn inspect_completed_install(
    paths: &DatabasePaths,
    checkpoint_id: &CheckpointId,
) -> Result<Option<RestoreInstallReport>> {
    let _lock = acquire_restore_lock(paths)?;
    recover_interrupted(paths)?;
    completed_install(paths, checkpoint_id)
}

#[cfg(test)]
fn acquire_restore_lock(paths: &DatabasePaths) -> Result<File> {
    fs::create_dir_all(&paths.root)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&paths.ownership_lock)?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(DatabaseError::DatabaseInUse {
            path: paths.root.clone(),
        }),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
fn completed_install(
    paths: &DatabasePaths,
    checkpoint_id: &CheckpointId,
) -> Result<Option<RestoreInstallReport>> {
    let Some(receipt) = read_optional_control(&paths.root.join(RECEIPT_FILENAME))? else {
        return Ok(None);
    };
    if receipt.checkpoint_id != *checkpoint_id || !live_pair_matches(paths, &receipt)? {
        return Ok(None);
    }
    Ok(Some(RestoreInstallReport {
        checkpoint_id: checkpoint_id.clone(),
        outcome: RestoreInstallOutcome::AlreadyInstalled,
        safety_snapshot_id: None,
    }))
}

#[cfg(test)]
pub(crate) fn validate_pair(paths: &DatabasePaths) -> Result<()> {
    if connection::inspect_file(&paths.main)? != FileState::Existing
        || connection::inspect_file(&paths.media)? != FileState::Existing
    {
        return Err(invalid("restored database pair is incomplete"));
    }
    let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)?;
    let mut media =
        connection::open_writer(&paths.media, DatabaseKind::Media, FileState::Existing)?;
    let mut main_status = migrations::inspect_main(&mut main)?;
    let mut media_status = migrations::inspect_media(&mut media)?;
    if media_status.pending {
        migrations::run_media(&mut media)?;
        media_status = migrations::inspect_media(&mut media)?;
    }
    if main_status.pending {
        migrations::run_main(&mut main)?;
        main_status = migrations::inspect_main(&mut main)?;
    }
    let expected = migrations::expected_heads();
    if main_status.pending
        || media_status.pending
        || main_status.head != expected.main
        || media_status.head != expected.media
    {
        return Err(DatabaseError::Validation {
            kind: "restore",
            reason: format!(
                "migration heads are ({:?}, {:?}), expected ({:?}, {:?})",
                main_status.head, media_status.head, expected.main, expected.media
            ),
        });
    }
    validation::validate_migrated_pair(&mut main, &mut media, &paths.main, &paths.media)?;
    main.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    media.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    drop(main);
    drop(media);
    safety_snapshot::verify_restore_pair(paths)
}

pub(crate) fn validate_pair_at(main_file: &File, media_file: &File) -> Result<()> {
    let mut main =
        connection::open_bound_writer(main_file, DatabaseKind::Main, FileState::Existing)?;
    let mut media =
        connection::open_bound_writer(media_file, DatabaseKind::Media, FileState::Existing)?;
    let mut main_status = migrations::inspect_main(&mut main)?;
    let mut media_status = migrations::inspect_media(&mut media)?;
    if media_status.pending {
        migrations::run_media(&mut media)?;
        media_status = migrations::inspect_media(&mut media)?;
    }
    if main_status.pending {
        migrations::run_main(&mut main)?;
        main_status = migrations::inspect_main(&mut main)?;
    }
    let expected = migrations::expected_heads();
    if main_status.pending
        || media_status.pending
        || main_status.head != expected.main
        || media_status.head != expected.media
    {
        return Err(DatabaseError::Validation {
            kind: "restore",
            reason: format!(
                "migration heads are ({:?}, {:?}), expected ({:?}, {:?})",
                main_status.head, media_status.head, expected.main, expected.media
            ),
        });
    }
    let main_path = PathBuf::from(format!("/dev/fd/{}", main_file.as_raw_fd()));
    let media_path = PathBuf::from(format!("/dev/fd/{}", media_file.as_raw_fd()));
    validation::validate_migrated_pair(&mut main, &mut media, &main_path, &media_path)?;
    drop(main);
    drop(media);
    main_file.sync_all()?;
    media_file.sync_all()?;
    let main = connection::open_bound_read_only(main_file, DatabaseKind::Main)?;
    let media = connection::open_bound_read_only(media_file, DatabaseKind::Media)?;
    safety_snapshot::verify_restore_pair_connections(&main, &media)
}

pub(crate) fn create_empty_media_at(file: &File) -> Result<rusqlite::Connection> {
    if file.metadata()?.len() != 0 {
        return Err(invalid("restore media destination is not empty"));
    }
    let mut media = connection::open_bound_writer(file, DatabaseKind::Media, FileState::Fresh)?;
    migrations::run_media(&mut media)?;
    Ok(media)
}

pub(crate) fn open_main_read_only_at(file: &File) -> Result<rusqlite::Connection> {
    connection::open_bound_read_only(file, DatabaseKind::Main)
}

#[cfg(test)]
fn install_transaction(
    paths: &DatabasePaths,
    transaction: &Path,
    control: &InstallControl,
) -> Result<()> {
    remove_sqlite_sidecars(paths)?;
    if control.had_existing_pair {
        backup_existing_pair(paths, transaction)?;
    }
    fs::rename(transaction.join("new-main.sqlite3"), &paths.main)?;
    fs::rename(transaction.join("new-media.sqlite3"), &paths.media)?;
    sync_directory(&paths.root)?;
    let main = open_regular_read_write_no_follow(&paths.main)?;
    let media = open_regular_read_write_no_follow(&paths.media)?;
    validate_pair_at(&main, &media)?;
    if hash_regular_descriptor(&main)? != control.main_sha256
        || hash_regular_descriptor(&media)? != control.media_sha256
    {
        return Err(invalid("installed restore bytes changed during validation"));
    }
    Ok(())
}

#[cfg(test)]
fn backup_existing_pair(paths: &DatabasePaths, transaction: &Path) -> Result<()> {
    copy_regular_synced(&paths.main, &transaction.join("old-main.sqlite3.tmp"))?;
    rename_regular(
        &transaction.join("old-main.sqlite3.tmp"),
        &transaction.join("old-main.sqlite3"),
    )?;
    sync_directory(transaction)?;

    copy_regular_synced(&paths.media, &transaction.join("old-media.sqlite3.tmp"))?;
    rename_regular(
        &transaction.join("old-media.sqlite3.tmp"),
        &transaction.join("old-media.sqlite3"),
    )?;
    sync_directory(transaction)?;

    remove_regular_if_present(&paths.main)?;
    remove_regular_if_present(&paths.media)?;
    sync_directory(&paths.root)
}

#[cfg(test)]
fn finish_install(
    paths: &DatabasePaths,
    transaction: &Path,
    control: &InstallControl,
    had_existing_pair: bool,
    persist_receipt: impl FnOnce(&Path, &InstallControl) -> Result<()>,
) -> Result<()> {
    let receipt = paths.root.join(RECEIPT_FILENAME);
    if let Err(error) = persist_receipt(&receipt, control) {
        let cleanup_error = remove_receipt_artifacts(&receipt).err();
        let rollback_error = rollback(paths, transaction, had_existing_pair).err();
        return Err(rollback_error.or(cleanup_error).unwrap_or(error));
    }
    remove_owned_transaction(transaction)
}

#[cfg(test)]
fn remove_receipt_artifacts(receipt: &Path) -> Result<()> {
    let temporary = receipt.with_extension("json.tmp");
    let mut first_error = None;
    for path in [receipt, temporary.as_path()] {
        if let Err(error) = remove_regular_if_present(path) {
            first_error.get_or_insert(error);
        }
    }
    if let Some(parent) = receipt.parent() {
        if let Err(error) = sync_directory(parent) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn rollback(paths: &DatabasePaths, transaction: &Path, had_existing_pair: bool) -> Result<()> {
    remove_sqlite_sidecars(paths)?;
    if had_existing_pair {
        restore_old_if_backed_up(&transaction.join("old-main.sqlite3"), &paths.main)?;
        restore_old_if_backed_up(&transaction.join("old-media.sqlite3"), &paths.media)?;
    } else {
        remove_regular_if_present(&paths.main)?;
        remove_regular_if_present(&paths.media)?;
    }
    sync_directory(&paths.root)?;
    remove_owned_transaction(transaction)
}

fn restore_old_if_backed_up(old: &Path, live: &Path) -> Result<()> {
    match fs::symlink_metadata(old) {
        Ok(metadata) if metadata.file_type().is_file() => {
            remove_regular_if_present(live)?;
            copy_regular_synced(old, live)
        }
        Ok(_) => Err(invalid("restore rollback file is not regular")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let live_metadata = fs::symlink_metadata(live)?;
            if live_metadata.file_type().is_file() {
                Ok(())
            } else {
                Err(invalid("restore rollback lost the live database"))
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn live_pair_matches(paths: &DatabasePaths, control: &InstallControl) -> Result<bool> {
    if control.format_version != FORMAT_VERSION
        || connection::inspect_file(&paths.main)? != FileState::Existing
        || connection::inspect_file(&paths.media)? != FileState::Existing
    {
        return Ok(false);
    }
    Ok(hash_regular_file(&paths.main)? == control.main_sha256
        && hash_regular_file(&paths.media)? == control.media_sha256)
}

#[cfg(test)]
fn create_private_transaction(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(invalid("another restore installation is incomplete"));
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    sync_directory(
        path.parent()
            .ok_or_else(|| invalid("restore transaction has no parent"))?,
    )
}

fn read_optional_control(path: &Path) -> Result<Option<InstallControl>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => read_control(path).map(Some),
        Ok(_) => Err(invalid("restore receipt is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_control(path: &Path) -> Result<InstallControl> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_CONTROL_BYTES {
        return Err(invalid("restore control file is invalid"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONTROL_BYTES {
        return Err(invalid("restore control file is oversized"));
    }
    let control: InstallControl = serde_json::from_slice(&bytes)?;
    if control.format_version != FORMAT_VERSION
        || !is_sha256(&control.main_sha256)
        || !is_sha256(&control.media_sha256)
    {
        return Err(invalid("restore control file is malformed"));
    }
    Ok(control)
}

#[cfg(test)]
fn write_control(path: &Path, control: &InstallControl) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    remove_regular_if_present(&temporary)?;
    let bytes = serde_json::to_vec(control)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| invalid("restore control file has no parent"))?,
    )
}

fn copy_regular_synced(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.file_type().is_file() {
        return Err(invalid("restore source is not a regular file"));
    }
    let mut input = File::open(source)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut output = options.open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

#[cfg(test)]
fn copy_regular_descriptor_synced(source: &File, destination: &Path) -> Result<()> {
    if !source.metadata()?.file_type().is_file() {
        return Err(invalid("restore source descriptor is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut output = options.open(destination)?;
    let mut buffer = [0_u8; 128 * 1024];
    let mut offset = 0_u64;
    loop {
        let read = source.read_at(&mut buffer, offset)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        offset = offset
            .checked_add(u64::try_from(read).map_err(|_| invalid("restore source is too large"))?)
            .ok_or_else(|| invalid("restore source is too large"))?;
    }
    output.sync_all()?;
    Ok(())
}

#[cfg(test)]
fn open_regular_read_write_no_follow(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(invalid("installed restore database is not a regular file"));
    }
    Ok(file)
}

#[cfg(test)]
fn rename_regular(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.file_type().is_file() {
        return Err(invalid("restore transaction file is not regular"));
    }
    fs::rename(source, destination)?;
    Ok(())
}

fn hash_regular_file(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(invalid("restore database is not a regular file"));
    }
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
fn hash_regular_descriptor(file: &File) -> Result<String> {
    if !file.metadata()?.file_type().is_file() {
        return Err(invalid("restore database descriptor is not a regular file"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut offset = 0_u64;
    loop {
        let read = file.read_at(&mut buffer, offset)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        offset = offset
            .checked_add(u64::try_from(read).map_err(|_| invalid("restore source is too large"))?)
            .ok_or_else(|| invalid("restore source is too large"))?;
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn remove_sqlite_sidecars(paths: &DatabasePaths) -> Result<()> {
    for database in [&paths.main, &paths.media] {
        let base = database.as_os_str().to_string_lossy();
        for suffix in ["-wal", "-shm"] {
            remove_regular_if_present(&PathBuf::from(format!("{base}{suffix}")))?;
        }
    }
    Ok(())
}

fn remove_regular_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(invalid("restore-owned path is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_owned_transaction(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(invalid("restore transaction path is not owned")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let allowed = [
            JOURNAL_FILENAME,
            "journal.json.tmp",
            "new-main.sqlite3",
            "new-media.sqlite3",
            "old-main.sqlite3",
            "old-main.sqlite3.tmp",
            "old-media.sqlite3",
            "old-media.sqlite3.tmp",
        ];
        if !entry.file_type()?.is_file()
            || !name.to_str().is_some_and(|name| allowed.contains(&name))
        {
            return Err(invalid("restore transaction contains an unexpected path"));
        }
        fs::remove_file(entry.path())?;
    }
    fs::remove_dir(path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(reason: &str) -> DatabaseError {
    DatabaseError::Validation {
        kind: "restore",
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use refinery::Target;

    use super::*;

    fn write_bytes(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("fixture bytes");
    }

    fn control(
        checkpoint_id: CheckpointId,
        had_existing_pair: bool,
        main: &[u8],
        media: &[u8],
    ) -> InstallControl {
        InstallControl {
            format_version: FORMAT_VERSION,
            checkpoint_id,
            had_existing_pair,
            main_sha256: format!("{:x}", Sha256::digest(main)),
            media_sha256: format!("{:x}", Sha256::digest(media)),
        }
    }

    #[test]
    fn recovery_before_a_durable_journal_keeps_the_original_pair() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        write_bytes(&paths.main, b"original main");
        write_bytes(&paths.media, b"original media");
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        write_bytes(&transaction.join("journal.json.tmp"), b"partial");

        recover_interrupted(&paths).expect("safe recovery");

        assert_eq!(fs::read(&paths.main).expect("main"), b"original main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"original media");
        assert!(!transaction.exists());
    }

    #[test]
    fn recovery_rolls_back_a_partially_installed_pair_without_a_receipt() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        let checkpoint_id = CheckpointId::new();
        let control = control(checkpoint_id, true, b"new main", b"new media");
        write_control(&transaction.join(JOURNAL_FILENAME), &control).expect("journal");
        write_bytes(&transaction.join("old-main.sqlite3"), b"original main");
        write_bytes(&transaction.join("old-media.sqlite3"), b"original media");
        write_bytes(&paths.main, b"new main");

        recover_interrupted(&paths).expect("rollback recovery");

        assert_eq!(fs::read(&paths.main).expect("main"), b"original main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"original media");
        assert!(!transaction.exists());
    }

    #[test]
    fn recovery_rolls_back_a_complete_installed_pair_without_a_receipt() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        let checkpoint_id = CheckpointId::new();
        let control = control(checkpoint_id, true, b"new main", b"new media");
        write_control(&transaction.join(JOURNAL_FILENAME), &control).expect("journal");
        write_bytes(&transaction.join("old-main.sqlite3"), b"original main");
        write_bytes(&transaction.join("old-media.sqlite3"), b"original media");
        write_bytes(&paths.main, b"new main");
        write_bytes(&paths.media, b"new media");

        recover_interrupted(&paths).expect("unreceipted rollback");

        assert_eq!(fs::read(&paths.main).expect("main"), b"original main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"original media");
        assert!(!transaction.exists());
    }

    #[test]
    fn recovery_during_original_backup_keeps_the_live_pair() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        let checkpoint_id = CheckpointId::new();
        let control = control(checkpoint_id, true, b"new main", b"new media");
        write_control(&transaction.join(JOURNAL_FILENAME), &control).expect("journal");
        write_bytes(&transaction.join("old-main.sqlite3.tmp"), b"partial backup");
        write_bytes(&paths.main, b"original main");
        write_bytes(&paths.media, b"original media");

        recover_interrupted(&paths).expect("backup recovery");

        assert_eq!(fs::read(&paths.main).expect("main"), b"original main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"original media");
        assert!(!transaction.exists());
    }

    #[test]
    fn recovery_keeps_a_receipted_pair_and_only_cleans_transaction_debris() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        let checkpoint_id = CheckpointId::new();
        let control = control(checkpoint_id, true, b"new main", b"new media");
        write_control(&transaction.join(JOURNAL_FILENAME), &control).expect("journal");
        write_control(&root.path().join(RECEIPT_FILENAME), &control).expect("receipt");
        write_bytes(&transaction.join("old-main.sqlite3"), b"original main");
        write_bytes(&transaction.join("old-media.sqlite3"), b"original media");
        write_bytes(&paths.main, b"new main");
        write_bytes(&paths.media, b"new media");

        recover_interrupted(&paths).expect("commit recovery");

        assert_eq!(fs::read(&paths.main).expect("main"), b"new main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"new media");
        assert!(!transaction.exists());
    }

    #[test]
    fn receipt_persistence_failure_rolls_back_the_installed_pair() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let transaction = root.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir(&transaction).expect("transaction");
        let checkpoint_id = CheckpointId::new();
        let control = control(checkpoint_id, true, b"new main", b"new media");
        write_control(&transaction.join(JOURNAL_FILENAME), &control).expect("journal");
        write_bytes(&transaction.join("old-main.sqlite3"), b"original main");
        write_bytes(&transaction.join("old-media.sqlite3"), b"original media");
        write_bytes(&paths.main, b"new main");
        write_bytes(&paths.media, b"new media");

        let result = finish_install(&paths, &transaction, &control, true, |receipt, control| {
            write_control(receipt, control)?;
            Err(invalid("injected receipt durability failure"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&paths.main).expect("main"), b"original main");
        assert_eq!(fs::read(&paths.media).expect("media"), b"original media");
        assert!(!transaction.exists());
        assert!(!root.path().join(RECEIPT_FILENAME).exists());
        assert!(!root.path().join("restore-install-v1.json.tmp").exists());
    }

    #[test]
    fn staged_pairs_from_the_previous_schema_are_migrated_before_validation() {
        let root = tempfile::tempdir().expect("restore root");
        let paths = DatabasePaths::new(root.path());
        let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Fresh)
            .expect("main writer");
        main.pragma_update(None, "foreign_keys", "OFF")
            .expect("disable foreign keys");
        migrations::main_runner()
            .set_target(Target::Version(20))
            .run(&mut main)
            .expect("previous main schema");
        main.pragma_update(None, "foreign_keys", "ON")
            .expect("restore foreign keys");
        let mut media =
            connection::open_writer(&paths.media, DatabaseKind::Media, FileState::Fresh)
                .expect("media writer");
        migrations::media_runner()
            .set_target(Target::Version(1))
            .run(&mut media)
            .expect("previous media schema");
        drop(main);
        drop(media);

        validate_pair(&paths).expect("compatible pending migrations");

        let main =
            connection::open_read_only(&paths.main, DatabaseKind::Main).expect("migrated main");
        let media =
            connection::open_read_only(&paths.media, DatabaseKind::Media).expect("migrated media");
        assert_eq!(
            main.query_row(
                "SELECT max(version) FROM refinery_schema_history",
                [],
                |row| row.get::<_, i32>(0),
            )
            .expect("main head"),
            22
        );
        assert_eq!(
            media
                .query_row(
                    "SELECT max(version) FROM refinery_schema_history",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .expect("media head"),
            2
        );
    }
}
