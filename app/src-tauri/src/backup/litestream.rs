use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString, OsStr, OsString},
    fmt,
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStringExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    credentials::R2Credentials,
    domain::{R2ObjectKey, R2Target},
};

const EMBEDDED_MANIFEST: &str = include_str!("../../resources/sidecars/litestream-v1.json");
const DEVELOPMENT_BINARY_OVERRIDE_ENV: &str = "KOSH_LITESTREAM_PATH";
pub(crate) const AWS_SHARED_CREDENTIALS_FILE_ENV: &str = "AWS_SHARED_CREDENTIALS_FILE";
pub(crate) const AWS_SHARED_CREDENTIALS_FILE_FD: &str = "/dev/fd/0";
pub(crate) const AWS_EC2_METADATA_DISABLED_ENV: &str = "AWS_EC2_METADATA_DISABLED";
const REQUIRED_L0_RETENTION: &str = "720h";
const MAX_CONTROL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_RESTORE_CONFIG_BYTES: usize = 64 * 1024;
const MAX_RESTORE_PLAN_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESTORE_FILES: usize = 100_000;
const MAX_MACOS_UNIX_SOCKET_PATH_BYTES: usize = 103;
const MAX_TRUSTED_CLEANUP_PINS: usize = 32;
const EPHEMERAL_RUNTIME_CONTROL_FILENAME: &str = ".kosh-recovery-runtime-v1.json";
const EPHEMERAL_RUNTIME_CONTROL_SCHEMA_VERSION: u32 = 1;
const MAX_EPHEMERAL_RUNTIME_CONTROL_BYTES: u64 = 4 * 1024;
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(target_os = "macos")]
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";
#[cfg(target_os = "macos")]
const PROCESS_IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(test)]
pub(crate) fn configure_credential_pipe_environment(command: &mut Command) {
    command
        .env_clear()
        .env(
            AWS_SHARED_CREDENTIALS_FILE_ENV,
            AWS_SHARED_CREDENTIALS_FILE_FD,
        )
        .env(AWS_EC2_METADATA_DISABLED_ENV, "true");
}

pub(crate) fn write_aws_shared_credentials(
    writer: &mut impl Write,
    credentials: &R2Credentials,
) -> std::io::Result<()> {
    writer.write_all(b"[default]\naws_access_key_id = ")?;
    writer.write_all(credentials.access_key_id().as_bytes())?;
    writer.write_all(b"\naws_secret_access_key = ")?;
    writer.write_all(credentials.secret_access_key().as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[derive(Debug, Error)]
pub enum LitestreamError {
    #[error("the Litestream source manifest is invalid")]
    InvalidEmbeddedManifest(#[source] serde_json::Error),
    #[error("the bundled Litestream release manifest is unavailable")]
    MissingReleaseManifest(#[source] std::io::Error),
    #[error("the bundled Litestream release manifest is invalid")]
    InvalidReleaseManifest(#[source] serde_json::Error),
    #[error("the bundled Litestream release manifest does not match the application pin")]
    ReleaseManifestMismatch,
    #[error("the Litestream executable is unavailable: {0}")]
    MissingBinary(PathBuf),
    #[error("the Litestream executable is not a regular file")]
    BinaryNotRegular,
    #[error("the Litestream executable is not executable")]
    BinaryNotExecutable,
    #[error("the Litestream executable size does not match the application pin")]
    BinarySizeMismatch,
    #[error("the Litestream executable checksum does not match the application pin")]
    BinaryChecksumMismatch,
    #[error("the immutable Litestream launch copy could not be staged")]
    StageBinary(#[source] std::io::Error),
    #[error("the immutable Litestream launch copy is invalid")]
    InvalidStagedBinary,
    #[error("the immutable Litestream restore configuration could not be staged")]
    StageRestoreConfig(#[source] std::io::Error),
    #[error("the immutable Litestream restore configuration is invalid")]
    InvalidRestoreConfig,
    #[error("the running Litestream process does not match the pinned code signature")]
    ProcessCodeSignatureMismatch,
    #[error("the running Litestream process identity could not be inspected")]
    ProcessIdentityUnavailable(#[source] std::io::Error),
    #[error("the Litestream manifest does not preserve exact TXIDs for 30 days")]
    UnsafeProtocolPin,
    #[error("the Litestream runtime path is not valid UTF-8")]
    NonUtf8RuntimePath,
    #[error("the Litestream control socket path is too long for macOS")]
    ControlSocketPathTooLong,
    #[error("invalid Litestream configuration field: {0}")]
    InvalidConfigField(&'static str),
    #[error("could not prepare the private Litestream runtime directory")]
    PrepareRuntime(#[source] std::io::Error),
    #[error("could not write the private Litestream configuration")]
    WriteConfig(#[source] std::io::Error),
    #[error("Litestream command execution failed")]
    Execute(#[source] std::io::Error),
    #[error("Litestream command failed with exit code {exit_code:?}")]
    CommandFailed { exit_code: Option<i32> },
    #[error("Litestream returned an oversized control response")]
    OversizedControlResponse,
    #[error("Litestream returned invalid JSON")]
    InvalidJson(#[source] serde_json::Error),
    #[error("Litestream returned a malformed transaction ID")]
    InvalidTxid,
    #[error("Litestream sync did not return the expected remote transaction ID")]
    InvalidSyncContract,
    #[error("Litestream sync returned a different database path")]
    UnexpectedDatabasePath,
    #[error("Litestream returned too many restore files")]
    RestorePlanTooLarge,
    #[error("Litestream restore response does not match the requested exact transaction")]
    InvalidRestoreContract,
    #[error("Litestream restore destination already exists")]
    RestoreDestinationExists,
    #[error("the private Litestream restore destination could not be prepared")]
    PrepareRestoreDestination(#[source] std::io::Error),
    #[error("Litestream did not produce a private regular restore file")]
    InvalidRestoreDestination,
    #[error("the verified Litestream restore could not be published")]
    PublishRestoreDestination(#[source] std::io::Error),
    #[error("Litestream commands require an absolute database path")]
    RelativeDatabasePath,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceManifest {
    component: String,
    binary: BinaryManifest,
    resource_destinations: ResourceDestinations,
    verification: ProtocolVerification,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinaryManifest {
    bundle_path: String,
    universal: BinaryPin,
    trusted_cleanup_sha256s: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct BinaryPin {
    sha256: String,
    size: u64,
    code_signature_identifier: String,
    code_signature_cdhash_by_architecture: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDestinations {
    release_manifest: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolVerification {
    exact_txid_fence_passed: bool,
    ordinary_compaction_exact_restore_passed: bool,
    default_l0_expiry_interior_txid_failure_observed: bool,
    required_l0_retention: String,
    graceful_shutdown_final_sync_passed: bool,
    orphan_process_check_passed: bool,
    real_r2_protocol_passed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseManifest {
    staged_binary: StagedBinary,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedBinary {
    sha256: String,
    size: u64,
}

/// A regular, executable Litestream binary whose bytes match the embedded pin.
#[derive(Debug)]
pub struct VerifiedLitestreamBinary {
    path: PathBuf,
    sha256: String,
    size: u64,
    code_signature_cdhash: String,
    file: File,
}

#[derive(Clone, Debug)]
pub struct ImmutableLitestreamBinary {
    path: PathBuf,
    sha256: String,
    size: u64,
    code_signature_cdhash: String,
}

#[derive(Clone, Debug)]
#[allow(
    dead_code,
    reason = "replica binding is read by the chunk 29g production restore adapter"
)]
struct ImmutableLitestreamRestoreConfig {
    path: PathBuf,
    sha256: String,
    size: u64,
    replica_path: R2ObjectKey,
}

impl VerifiedLitestreamBinary {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn trusted_cleanup_sha256s() -> Result<Vec<String>, LitestreamError> {
        let manifest = embedded_manifest()?;
        validate_protocol_manifest(&manifest)?;
        Ok(manifest.binary.trusted_cleanup_sha256s)
    }

    pub fn resolve(resource_dir: &Path) -> Result<Self, LitestreamError> {
        let manifest = embedded_manifest()?;
        validate_protocol_manifest(&manifest)?;
        let code_signature_cdhash = current_code_signature_cdhash(&manifest.binary.universal)?;

        #[cfg(debug_assertions)]
        let development_override =
            std::env::var_os(DEVELOPMENT_BINARY_OVERRIDE_ENV).map(PathBuf::from);
        #[cfg(not(debug_assertions))]
        let development_override: Option<PathBuf> = None;

        let path = if let Some(path) = development_override {
            path
        } else {
            verify_release_manifest(resource_dir, &manifest)?;
            resource_dir.join(&manifest.binary.bundle_path)
        };
        let file = verify_binary(&path, &manifest.binary.universal)?;
        Ok(Self {
            path,
            sha256: manifest.binary.universal.sha256,
            size: manifest.binary.universal.size,
            code_signature_cdhash,
            file,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn resolve_staged_for_test(path: &Path) -> Result<Self, LitestreamError> {
        let manifest = embedded_manifest()?;
        validate_protocol_manifest(&manifest)?;
        let code_signature_cdhash = current_code_signature_cdhash(&manifest.binary.universal)?;
        let file = verify_binary(path, &manifest.binary.universal)?;
        Ok(Self {
            path: path.to_owned(),
            sha256: manifest.binary.universal.sha256,
            size: manifest.binary.universal.size,
            code_signature_cdhash,
            file,
        })
    }

    pub(crate) fn stage_immutable(
        &self,
        runtime: &LitestreamRuntimePaths,
    ) -> Result<ImmutableLitestreamBinary, LitestreamError> {
        stage_immutable_binary(self, runtime)
    }
}

impl ImmutableLitestreamBinary {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn reverify_before_spawn(&self) -> Result<(), LitestreamError> {
        let file = verify_binary(
            &self.path,
            &BinaryPin {
                sha256: self.sha256.clone(),
                size: self.size,
                code_signature_identifier: String::new(),
                code_signature_cdhash_by_architecture: BTreeMap::new(),
            },
        )?;
        validate_immutable_file(&file)?;
        let parent = self
            .path
            .parent()
            .ok_or(LitestreamError::InvalidStagedBinary)?;
        validate_immutable_directory(parent)?;
        Ok(())
    }

    pub(crate) fn verify_running_process(&self, pid: u32) -> Result<(), LitestreamError> {
        let actual =
            running_process_cdhash(pid).map_err(LitestreamError::ProcessIdentityUnavailable)?;
        if actual != self.code_signature_cdhash {
            return Err(LitestreamError::ProcessCodeSignatureMismatch);
        }
        Ok(())
    }
}

impl ImmutableLitestreamRestoreConfig {
    fn reverify_before_spawn(&self) -> Result<(), LitestreamError> {
        let parent = self
            .path
            .parent()
            .ok_or(LitestreamError::InvalidRestoreConfig)?;
        if validate_immutable_directory(parent).is_err() {
            return Err(LitestreamError::InvalidRestoreConfig);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options
            .open(&self.path)
            .map_err(LitestreamError::StageRestoreConfig)?;
        let metadata = file
            .metadata()
            .map_err(LitestreamError::StageRestoreConfig)?;
        if !metadata.is_file() || metadata.len() != self.size {
            return Err(LitestreamError::InvalidRestoreConfig);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o400 {
                return Err(LitestreamError::InvalidRestoreConfig);
            }
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            if metadata.st_flags() & libc::UF_IMMUTABLE == 0 {
                return Err(LitestreamError::InvalidRestoreConfig);
            }
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_RESTORE_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(LitestreamError::StageRestoreConfig)?;
        if bytes.len() as u64 != self.size || format!("{:x}", Sha256::digest(&bytes)) != self.sha256
        {
            return Err(LitestreamError::InvalidRestoreConfig);
        }
        Ok(())
    }
}

fn stage_immutable_restore_config(
    runtime: &LitestreamRuntimePaths,
    rendered: &str,
    replica_path: &R2ObjectKey,
) -> Result<ImmutableLitestreamRestoreConfig, LitestreamError> {
    if rendered.is_empty() || rendered.len() > MAX_RESTORE_CONFIG_BYTES {
        return Err(LitestreamError::InvalidRestoreConfig);
    }
    runtime.prepare()?;
    let bytes = rendered.as_bytes();
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let size = bytes.len() as u64;
    let stage_root = runtime.directory().join("restore-configs");
    prepare_private_runtime_directory(&stage_root).map_err(LitestreamError::StageRestoreConfig)?;
    let _stage_lock = lock_restore_config_stage(&stage_root, &sha256)
        .map_err(LitestreamError::StageRestoreConfig)?;
    remove_stale_restore_config_temporaries(&stage_root, &sha256)
        .map_err(LitestreamError::StageRestoreConfig)?;
    let final_directory = stage_root.join(&sha256);
    let final_path = final_directory.join("ls.yml");
    let config = ImmutableLitestreamRestoreConfig {
        path: final_path.clone(),
        sha256: sha256.clone(),
        size,
        replica_path: replica_path.clone(),
    };
    match fs::symlink_metadata(&final_directory) {
        Ok(_) => match config.reverify_before_spawn() {
            Ok(()) => return Ok(config),
            Err(_) => remove_partial_restore_config(&final_directory)
                .map_err(LitestreamError::StageRestoreConfig)?,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(LitestreamError::StageRestoreConfig(error)),
    }

    let temporary_directory = stage_root.join(format!(".{sha256}.{}.tmp", uuid::Uuid::now_v7()));
    fs::create_dir(&temporary_directory).map_err(LitestreamError::StageRestoreConfig)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary_directory, fs::Permissions::from_mode(0o700))
            .map_err(LitestreamError::StageRestoreConfig)?;
    }
    let temporary_path = temporary_directory.join("ls.yml");
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o400)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(&temporary_path)
        .map_err(LitestreamError::StageRestoreConfig)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(LitestreamError::StageRestoreConfig)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o400))
            .map_err(LitestreamError::StageRestoreConfig)?;
    }
    drop(file);
    File::open(&temporary_directory)
        .and_then(|directory| directory.sync_all())
        .map_err(LitestreamError::StageRestoreConfig)?;
    fs::rename(&temporary_directory, &final_directory)
        .map_err(LitestreamError::StageRestoreConfig)?;
    let staged = File::open(&final_path).map_err(LitestreamError::StageRestoreConfig)?;
    set_user_immutable(&staged).map_err(LitestreamError::StageRestoreConfig)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&final_directory, fs::Permissions::from_mode(0o500))
            .map_err(LitestreamError::StageRestoreConfig)?;
    }
    let directory =
        open_directory_no_follow(&final_directory).map_err(LitestreamError::StageRestoreConfig)?;
    set_user_immutable(&directory).map_err(LitestreamError::StageRestoreConfig)?;
    directory
        .sync_all()
        .map_err(LitestreamError::StageRestoreConfig)?;
    File::open(&stage_root)
        .and_then(|root| root.sync_all())
        .map_err(LitestreamError::StageRestoreConfig)?;
    config.reverify_before_spawn()?;
    Ok(config)
}

fn lock_restore_config_stage(stage_root: &Path, sha256: &str) -> std::io::Result<File> {
    let lock_path = stage_root.join(format!(".{sha256}.lock"));
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let lock = options.open(lock_path)?;
    if !lock.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "Litestream restore config lock is not regular",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    lock.lock()?;
    Ok(lock)
}

fn remove_stale_restore_config_temporaries(stage_root: &Path, sha256: &str) -> std::io::Result<()> {
    let prefix = format!(".{sha256}.");
    for entry in fs::read_dir(stage_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(unique) = name
            .strip_prefix(&prefix)
            .and_then(|name| name.strip_suffix(".tmp"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(unique).is_ok() {
            remove_partial_restore_config(&entry.path())?;
        }
    }
    Ok(())
}

fn embedded_manifest() -> Result<SourceManifest, LitestreamError> {
    serde_json::from_str(EMBEDDED_MANIFEST).map_err(LitestreamError::InvalidEmbeddedManifest)
}

fn current_code_signature_cdhash(pin: &BinaryPin) -> Result<String, LitestreamError> {
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return Err(LitestreamError::UnsafeProtocolPin),
    };
    pin.code_signature_cdhash_by_architecture
        .get(architecture)
        .cloned()
        .ok_or(LitestreamError::UnsafeProtocolPin)
}

fn validate_protocol_manifest(manifest: &SourceManifest) -> Result<(), LitestreamError> {
    let universal = &manifest.binary.universal;
    let trusted_cleanup_sha256s = &manifest.binary.trusted_cleanup_sha256s;
    let unique_cleanup_sha256s = trusted_cleanup_sha256s.iter().collect::<BTreeSet<_>>();
    let expected_cdhash_architectures = BTreeSet::from(["arm64", "x86_64"]);
    let actual_cdhash_architectures = universal
        .code_signature_cdhash_by_architecture
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if manifest.component != "litestream"
        || !manifest.verification.exact_txid_fence_passed
        || !manifest
            .verification
            .ordinary_compaction_exact_restore_passed
        || !manifest
            .verification
            .default_l0_expiry_interior_txid_failure_observed
        || !manifest.verification.graceful_shutdown_final_sync_passed
        || !manifest.verification.orphan_process_check_passed
        || !manifest.verification.real_r2_protocol_passed
        || manifest.verification.required_l0_retention != REQUIRED_L0_RETENTION
        || universal.sha256.len() != 64
        || !universal
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || universal.size == 0
        || universal.code_signature_identifier != "com.rohan.kosh.litestream"
        || actual_cdhash_architectures != expected_cdhash_architectures
        || universal
            .code_signature_cdhash_by_architecture
            .values()
            .any(|cdhash| {
                cdhash.len() != 40
                    || !cdhash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        || trusted_cleanup_sha256s.is_empty()
        || trusted_cleanup_sha256s.len() > MAX_TRUSTED_CLEANUP_PINS
        || unique_cleanup_sha256s.len() != trusted_cleanup_sha256s.len()
        || !trusted_cleanup_sha256s.contains(&universal.sha256)
        || trusted_cleanup_sha256s.iter().any(|sha256| {
            sha256.len() != 64
                || !sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Err(LitestreamError::UnsafeProtocolPin);
    }
    Ok(())
}

fn verify_release_manifest(
    resource_dir: &Path,
    manifest: &SourceManifest,
) -> Result<(), LitestreamError> {
    let release_path = resource_dir.join(&manifest.resource_destinations.release_manifest);
    let release =
        fs::read_to_string(release_path).map_err(LitestreamError::MissingReleaseManifest)?;
    let release_manifest: ReleaseManifest =
        serde_json::from_str(&release).map_err(LitestreamError::InvalidReleaseManifest)?;
    if release_manifest.staged_binary.sha256 != manifest.binary.universal.sha256
        || release_manifest.staged_binary.size != manifest.binary.universal.size
    {
        return Err(LitestreamError::ReleaseManifestMismatch);
    }

    let embedded_value: serde_json::Value = serde_json::from_str(EMBEDDED_MANIFEST)
        .map_err(LitestreamError::InvalidEmbeddedManifest)?;
    let mut release_value: serde_json::Value =
        serde_json::from_str(&release).map_err(LitestreamError::InvalidReleaseManifest)?;
    let release_object = release_value
        .as_object_mut()
        .ok_or(LitestreamError::ReleaseManifestMismatch)?;
    release_object.remove("stagedBinary");
    let verification = release_object
        .get_mut("verification")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(LitestreamError::ReleaseManifestMismatch)?;
    verification.remove("architectureChecks");
    if release_value != embedded_value {
        return Err(LitestreamError::ReleaseManifestMismatch);
    }
    Ok(())
}

fn verify_binary(path: &Path, manifest: &BinaryPin) -> Result<File, LitestreamError> {
    #[cfg(not(unix))]
    {
        let path_metadata = fs::symlink_metadata(path)
            .map_err(|_| LitestreamError::MissingBinary(path.to_owned()))?;
        if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
            return Err(LitestreamError::BinaryNotRegular);
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            LitestreamError::BinaryNotRegular
        } else {
            LitestreamError::MissingBinary(path.to_owned())
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| LitestreamError::BinaryNotRegular)?;
    if !metadata.is_file() {
        return Err(LitestreamError::BinaryNotRegular);
    }
    if metadata.len() != manifest.size {
        return Err(LitestreamError::BinarySizeMismatch);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(LitestreamError::BinaryNotExecutable);
        }
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    let mut bounded = (&mut file).take(manifest.size + 1);
    loop {
        let read = bounded
            .read(&mut buffer)
            .map_err(|_| LitestreamError::BinaryChecksumMismatch)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    if total != manifest.size {
        return Err(LitestreamError::BinarySizeMismatch);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != manifest.sha256 {
        return Err(LitestreamError::BinaryChecksumMismatch);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| LitestreamError::BinaryChecksumMismatch)?;
    Ok(file)
}

#[cfg(target_os = "macos")]
fn running_process_cdhash(pid: u32) -> std::io::Result<String> {
    const CS_OPS_CDHASH: u32 = 5;
    const CS_CDHASH_LEN: usize = 20;

    unsafe extern "C" {
        fn csops(
            pid: libc::pid_t,
            operations: u32,
            user_address: *mut libc::c_void,
            user_size: libc::size_t,
        ) -> libc::c_int;
    }

    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| std::io::Error::other("Litestream PID is out of range"))?;
    let mut cdhash = [0_u8; CS_CDHASH_LEN];
    // SAFETY: `cdhash` is writable for exactly the byte count passed to `csops`.
    let result = unsafe { csops(pid, CS_OPS_CDHASH, cdhash.as_mut_ptr().cast(), cdhash.len()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cdhash.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(target_os = "macos"))]
fn running_process_cdhash(_pid: u32) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "process code-signature inspection requires macOS",
    ))
}

fn stage_immutable_binary(
    binary: &VerifiedLitestreamBinary,
    runtime: &LitestreamRuntimePaths,
) -> Result<ImmutableLitestreamBinary, LitestreamError> {
    runtime.prepare()?;
    let stage_root = runtime.directory().join("verified-litestream");
    prepare_private_runtime_directory(&stage_root).map_err(LitestreamError::StageBinary)?;
    let final_directory = stage_root.join(&binary.sha256);
    let final_path = final_directory.join("litestream");
    match fs::symlink_metadata(&final_directory) {
        Ok(_) => match validate_immutable_stage(&final_directory, &final_path, binary) {
            Ok(()) => {
                return Ok(ImmutableLitestreamBinary {
                    path: final_path,
                    sha256: binary.sha256.clone(),
                    size: binary.size,
                    code_signature_cdhash: binary.code_signature_cdhash.clone(),
                });
            }
            Err(_) => {
                remove_partial_stage(&final_directory).map_err(LitestreamError::StageBinary)?;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(LitestreamError::StageBinary(error)),
    }

    let temporary_directory = stage_root.join(format!(".{}.tmp", binary.sha256));
    remove_partial_stage(&temporary_directory).map_err(LitestreamError::StageBinary)?;
    fs::create_dir(&temporary_directory).map_err(LitestreamError::StageBinary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary_directory, fs::Permissions::from_mode(0o700))
            .map_err(LitestreamError::StageBinary)?;
    }
    let temporary_path = temporary_directory.join("litestream");
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o500)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut destination = options
        .open(&temporary_path)
        .map_err(LitestreamError::StageBinary)?;
    let mut source = binary
        .file
        .try_clone()
        .map_err(LitestreamError::StageBinary)?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(LitestreamError::StageBinary)?;
    let copied = std::io::copy(&mut source.take(binary.size + 1), &mut destination)
        .map_err(LitestreamError::StageBinary)?;
    if copied != binary.size {
        return Err(LitestreamError::InvalidStagedBinary);
    }
    destination
        .sync_all()
        .map_err(LitestreamError::StageBinary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        destination
            .set_permissions(fs::Permissions::from_mode(0o500))
            .map_err(LitestreamError::StageBinary)?;
    }
    drop(destination);
    File::open(&temporary_directory)
        .and_then(|directory| directory.sync_all())
        .map_err(LitestreamError::StageBinary)?;
    fs::rename(&temporary_directory, &final_directory).map_err(LitestreamError::StageBinary)?;
    let staged = verify_binary(
        &final_path,
        &BinaryPin {
            sha256: binary.sha256.clone(),
            size: binary.size,
            code_signature_identifier: String::new(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        },
    )?;
    set_user_immutable(&staged).map_err(LitestreamError::StageBinary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&final_directory, fs::Permissions::from_mode(0o500))
            .map_err(LitestreamError::StageBinary)?;
    }
    let directory =
        open_directory_no_follow(&final_directory).map_err(LitestreamError::StageBinary)?;
    set_user_immutable(&directory).map_err(LitestreamError::StageBinary)?;
    directory.sync_all().map_err(LitestreamError::StageBinary)?;
    File::open(&stage_root)
        .and_then(|root| root.sync_all())
        .map_err(LitestreamError::StageBinary)?;
    validate_immutable_stage(&final_directory, &final_path, binary)?;
    Ok(ImmutableLitestreamBinary {
        path: final_path,
        sha256: binary.sha256.clone(),
        size: binary.size,
        code_signature_cdhash: binary.code_signature_cdhash.clone(),
    })
}

fn validate_immutable_stage(
    directory_path: &Path,
    binary_path: &Path,
    binary: &VerifiedLitestreamBinary,
) -> Result<(), LitestreamError> {
    let directory =
        open_directory_no_follow(directory_path).map_err(LitestreamError::StageBinary)?;
    validate_immutable_directory_file(&directory)?;
    let staged = verify_binary(
        binary_path,
        &BinaryPin {
            sha256: binary.sha256.clone(),
            size: binary.size,
            code_signature_identifier: String::new(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        },
    )?;
    validate_immutable_file(&staged)
}

fn validate_immutable_file(file: &File) -> Result<(), LitestreamError> {
    let metadata = file.metadata().map_err(LitestreamError::StageBinary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o500 {
            return Err(LitestreamError::InvalidStagedBinary);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        if metadata.st_flags() & libc::UF_IMMUTABLE == 0 {
            return Err(LitestreamError::InvalidStagedBinary);
        }
    }
    Ok(())
}

fn validate_immutable_directory(path: &Path) -> Result<(), LitestreamError> {
    let directory = open_directory_no_follow(path).map_err(LitestreamError::StageBinary)?;
    validate_immutable_directory_file(&directory)
}

fn validate_immutable_directory_file(directory: &File) -> Result<(), LitestreamError> {
    let metadata = directory.metadata().map_err(LitestreamError::StageBinary)?;
    if !metadata.is_dir() {
        return Err(LitestreamError::InvalidStagedBinary);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o500 {
            return Err(LitestreamError::InvalidStagedBinary);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        if metadata.st_flags() & libc::UF_IMMUTABLE == 0 {
            return Err(LitestreamError::InvalidStagedBinary);
        }
    }
    Ok(())
}

fn open_directory_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

#[cfg(target_os = "macos")]
fn set_user_immutable(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `file` owns a live descriptor and `fchflags` does not retain it.
    let result = unsafe { libc::fchflags(file.as_raw_fd(), libc::UF_IMMUTABLE) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn set_user_immutable(_file: &File) -> std::io::Result<()> {
    Ok(())
}

fn remove_partial_stage(path: &Path) -> std::io::Result<()> {
    remove_partial_stage_entry(path, "litestream")
}

fn remove_partial_restore_config(path: &Path) -> std::io::Result<()> {
    remove_partial_stage_entry(path, "ls.yml")
}

fn remove_partial_stage_entry(path: &Path, expected_filename: &str) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "Litestream staging temporary is not a real directory",
        ));
    }
    let directory = open_directory_no_follow(path)?;
    clear_user_immutable(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    let entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() > 1
        || entries
            .first()
            .is_some_and(|entry| entry.file_name() != expected_filename)
    {
        return Err(std::io::Error::other(
            "Litestream staging temporary has unexpected contents",
        ));
    }
    if let Some(entry) = entries.first() {
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::other(
                "Litestream staging temporary file is not regular",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let file = options.open(entry.path())?;
        clear_user_immutable(&file)?;
        drop(file);
        fs::remove_file(entry.path())?;
    }
    drop(directory);
    fs::remove_dir(path)
}

#[cfg(target_os = "macos")]
pub(crate) fn clear_user_immutable(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `file` owns a live descriptor and `fchflags` does not retain it.
    let result = unsafe { libc::fchflags(file.as_raw_fd(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn clear_user_immutable(_file: &File) -> std::io::Result<()> {
    Ok(())
}

/// Private configuration, socket, and future PID-record paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitestreamRuntimePaths {
    directory: PathBuf,
    config: PathBuf,
    socket: PathBuf,
    pid: PathBuf,
    ownership_lock: PathBuf,
}

pub(crate) struct EphemeralLitestreamRuntime {
    #[cfg(test)]
    root: PathBuf,
    source_database_path: PathBuf,
    runtime: LitestreamRuntimePaths,
    ownership: EphemeralRuntimeOwnership,
    cleaned: bool,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EphemeralRuntimeControl {
    schema_version: u32,
    token: String,
}

struct EphemeralRuntimeOwnership {
    root: PathBuf,
    token: String,
    directory: File,
    marker: File,
}

impl EphemeralLitestreamRuntime {
    pub(crate) fn create() -> Result<Self, LitestreamError> {
        #[cfg(unix)]
        let parent = Path::new("/tmp");
        #[cfg(not(unix))]
        let parent = std::env::temp_dir().as_path();
        reclaim_abandoned_ephemeral_runtimes(parent).map_err(LitestreamError::PrepareRuntime)?;
        let mut ownership =
            EphemeralRuntimeOwnership::create(parent).map_err(LitestreamError::PrepareRuntime)?;
        let root = ownership.root.clone();
        let runtime = match LitestreamRuntimePaths::new(&root) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = ownership.remove();
                return Err(error);
            }
        };
        let source_database_path = root.join("remote-kosh.sqlite3");
        Ok(Self {
            #[cfg(test)]
            root,
            source_database_path,
            runtime,
            ownership,
            cleaned: false,
        })
    }

    pub(crate) fn paths(&self) -> &LitestreamRuntimePaths {
        &self.runtime
    }

    pub(crate) fn source_database_path(&self) -> &Path {
        &self.source_database_path
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), LitestreamError> {
        if self.cleaned {
            return Ok(());
        }
        self.ownership
            .remove()
            .map_err(LitestreamError::PrepareRuntime)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for EphemeralLitestreamRuntime {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.ownership.remove();
        }
    }
}

impl EphemeralRuntimeOwnership {
    fn create(parent: &Path) -> std::io::Result<Self> {
        let token = uuid::Uuid::now_v7().to_string();
        let root = parent.join(format!("kosh-r-{token}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&root)?;
        let directory = match open_directory_no_follow(&root) {
            Ok(directory) => directory,
            Err(error) => {
                let _ = fs::remove_dir(&root);
                return Err(error);
            }
        };
        if !path_matches_open_directory(&root, &directory)
            || !is_private_current_user_directory(&directory)?
            || !runtime_directory_entries(&directory)?.is_empty()
        {
            let _ = remove_empty_owned_runtime_directory(&root, &directory);
            return Err(std::io::Error::other(
                "ephemeral recovery root was replaced",
            ));
        }
        let mut marker =
            match create_private_runtime_child(&directory, EPHEMERAL_RUNTIME_CONTROL_FILENAME) {
                Ok(marker) => marker,
                Err(error) => {
                    let _ = remove_empty_owned_runtime_directory(&root, &directory);
                    return Err(error);
                }
            };
        if let Err(error) = marker.lock() {
            let _ =
                unlink_owned_runtime_file(&directory, EPHEMERAL_RUNTIME_CONTROL_FILENAME, &marker);
            let _ = remove_empty_owned_runtime_directory(&root, &directory);
            return Err(error);
        }
        let control = EphemeralRuntimeControl {
            schema_version: EPHEMERAL_RUNTIME_CONTROL_SCHEMA_VERSION,
            token: token.clone(),
        };
        if let Err(error) = write_ephemeral_runtime_control(&mut marker, &control)
            .and_then(|()| directory.sync_all())
            .and_then(|()| File::open(parent)?.sync_all())
        {
            let _ =
                unlink_owned_runtime_file(&directory, EPHEMERAL_RUNTIME_CONTROL_FILENAME, &marker);
            let _ = remove_empty_owned_runtime_directory(&root, &directory);
            return Err(error);
        }
        Ok(Self {
            root,
            token,
            directory,
            marker,
        })
    }

    fn open(path: &Path) -> std::io::Result<Option<Self>> {
        let Some(token) = ephemeral_runtime_token(path) else {
            return Ok(None);
        };
        let directory = match open_directory_no_follow(path) {
            Ok(directory) => directory,
            Err(_) => return Ok(None),
        };
        if !path_matches_open_directory(path, &directory)
            || !is_private_current_user_directory(&directory)?
        {
            return Ok(None);
        }
        let mut marker =
            match open_regular_runtime_child(&directory, EPHEMERAL_RUNTIME_CONTROL_FILENAME) {
                Ok(marker) => marker,
                Err(_) => return Ok(None),
            };
        if !is_private_current_user_file(&marker)? || runtime_link_count(&marker)? != 1 {
            return Ok(None);
        }
        let control = match read_ephemeral_runtime_control(&mut marker) {
            Ok(control) => control,
            Err(_) => return Ok(None),
        };
        if control.schema_version != EPHEMERAL_RUNTIME_CONTROL_SCHEMA_VERSION
            || control.token != token
        {
            return Ok(None);
        }
        Ok(Some(Self {
            root: path.to_owned(),
            token,
            directory,
            marker,
        }))
    }

    fn remove(&mut self) -> std::io::Result<()> {
        if !path_matches_open_directory(&self.root, &self.directory) {
            return Err(std::io::Error::other(
                "ephemeral recovery root identity changed",
            ));
        }
        let parent = self
            .root
            .parent()
            .ok_or_else(|| std::io::Error::other("ephemeral recovery root has no parent"))?;
        let quarantine = parent.join(format!("kosh-c-{}", self.token));
        let already_quarantined = self.root == quarantine;
        if !already_quarantined {
            if quarantine.exists() {
                return Err(std::io::Error::other(
                    "ephemeral recovery quarantine already exists",
                ));
            }
            fs::rename(&self.root, &quarantine)?;
            if !path_matches_open_directory(&quarantine, &self.directory) {
                if !self.root.exists() {
                    let _ = fs::rename(&quarantine, &self.root);
                }
                return Err(std::io::Error::other(
                    "ephemeral recovery quarantine identity changed",
                ));
            }
        }
        let quarantined_runtime = LitestreamRuntimePaths::new(&quarantine)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let cleanup = quarantined_runtime.remove_ephemeral_recovery_runtime_bound(
            &quarantine,
            &self.directory,
            Some(&self.marker),
        );
        if cleanup.is_err() && !already_quarantined && !self.root.exists() {
            let _ = fs::rename(&quarantine, &self.root);
        }
        if cleanup.is_ok() {
            self.root = quarantine;
        }
        cleanup
    }
}

impl LitestreamRuntimePaths {
    pub fn new(data_root: &Path) -> Result<Self, LitestreamError> {
        let directory = data_root.join("run").join("backup");
        let paths = Self {
            config: directory.join("ls.yml"),
            socket: directory.join("ls.sock"),
            pid: directory.join("ls.pid.json"),
            ownership_lock: directory.join("ownership.lock"),
            directory,
        };
        ensure_socket_path_fits(&paths.socket)?;
        Ok(paths)
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    #[must_use]
    pub fn pid(&self) -> &Path {
        &self.pid
    }

    #[must_use]
    pub fn ownership_lock(&self) -> &Path {
        &self.ownership_lock
    }

    pub fn prepare(&self) -> Result<(), LitestreamError> {
        let run_directory = self
            .directory
            .parent()
            .ok_or_else(|| {
                LitestreamError::PrepareRuntime(std::io::Error::other(
                    "Litestream runtime has no run directory",
                ))
            })?
            .to_owned();
        let data_root = run_directory.parent().ok_or_else(|| {
            LitestreamError::PrepareRuntime(std::io::Error::other(
                "Litestream runtime has no data root",
            ))
        })?;
        fs::create_dir_all(data_root).map_err(LitestreamError::PrepareRuntime)?;
        prepare_private_runtime_directory(&run_directory)
            .and_then(|()| prepare_private_runtime_directory(&self.directory))
            .map_err(LitestreamError::PrepareRuntime)?;
        Ok(())
    }

    pub fn write_config(&self, config: &str) -> Result<(), LitestreamError> {
        self.prepare()?;
        reject_symlink_or_non_file(&self.config).map_err(LitestreamError::WriteConfig)?;
        let temporary = self.directory.join("ls.yml.tmp");
        remove_regular_temporary(&temporary).map_err(LitestreamError::WriteConfig)?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&temporary)
            .map_err(LitestreamError::WriteConfig)?;
        file.write_all(config.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(LitestreamError::WriteConfig)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(LitestreamError::WriteConfig)?;
        }
        fs::rename(&temporary, &self.config).map_err(LitestreamError::WriteConfig)?;
        File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(LitestreamError::WriteConfig)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn remove_ephemeral_recovery_runtime(
        &self,
        data_root: &Path,
    ) -> std::io::Result<()> {
        let directory = open_directory_no_follow(data_root)?;
        self.remove_ephemeral_recovery_runtime_bound(data_root, &directory, None)
    }

    fn remove_ephemeral_recovery_runtime_bound(
        &self,
        data_root: &Path,
        data_root_directory: &File,
        ownership_marker: Option<&File>,
    ) -> std::io::Result<()> {
        self.remove_ephemeral_recovery_runtime_bound_with_hook(
            data_root,
            data_root_directory,
            ownership_marker,
            || {},
        )
    }

    fn remove_ephemeral_recovery_runtime_bound_with_hook(
        &self,
        data_root: &Path,
        data_root_directory: &File,
        ownership_marker: Option<&File>,
        before_cleanup: impl FnOnce(),
    ) -> std::io::Result<()> {
        let expected_directory = data_root.join("run").join("backup");
        if !data_root.is_absolute()
            || self.directory != expected_directory
            || !is_ephemeral_recovery_root(data_root)
            || !path_matches_open_directory(data_root, data_root_directory)
            || !is_private_current_user_directory(data_root_directory)?
        {
            return Err(std::io::Error::other(
                "refusing to remove an unrecognized recovery runtime",
            ));
        }
        let parent = data_root
            .parent()
            .ok_or_else(|| std::io::Error::other("ephemeral recovery root has no parent"))?;
        let root_name = data_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::other("ephemeral recovery root name is invalid"))?;
        // macOS exposes `/tmp` through the trusted `/private/tmp` symlink.
        // Follow only this already-selected parent; the UUID child itself is
        // still reopened with `O_NOFOLLOW` and identity-matched before removal.
        let parent_directory = File::open(parent)?;
        if !parent_directory.metadata()?.is_dir() {
            return Err(std::io::Error::other(
                "ephemeral recovery parent is not a directory",
            ));
        }

        before_cleanup();
        remove_ephemeral_runtime_tree_bound(data_root_directory)?;
        if let Some(marker) = ownership_marker {
            unlink_owned_runtime_file(
                data_root_directory,
                EPHEMERAL_RUNTIME_CONTROL_FILENAME,
                marker,
            )?;
            data_root_directory.sync_all()?;
        }
        remove_owned_runtime_directory_child(&parent_directory, root_name, data_root_directory)
    }
}

fn is_ephemeral_recovery_root(path: &Path) -> bool {
    ephemeral_runtime_token(path).is_some()
}

fn ephemeral_runtime_token(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let token = name
        .strip_prefix("kosh-r-")
        .or_else(|| name.strip_prefix("kosh-c-"))?;
    uuid::Uuid::parse_str(token)
        .ok()
        .filter(|parsed| parsed.to_string() == token)
        .map(|_| token.to_owned())
}

fn reclaim_abandoned_ephemeral_runtimes(parent: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(parent)? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if ephemeral_runtime_token(&path).is_none() {
            continue;
        }
        let Ok(Some(mut ownership)) = EphemeralRuntimeOwnership::open(&path) else {
            continue;
        };
        match ownership.marker.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(_)) => continue,
        }
        // Unknown contents or a raced identity are preserved. One damaged
        // temporary must not prevent a fresh disaster-recovery attempt.
        let _ = ownership.remove();
    }
    Ok(())
}

fn write_ephemeral_runtime_control(
    file: &mut File,
    control: &EphemeralRuntimeControl,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(control)
        .map_err(|_| std::io::Error::other("invalid ephemeral runtime control"))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_EPHEMERAL_RUNTIME_CONTROL_BYTES {
        return Err(std::io::Error::other(
            "invalid ephemeral runtime control length",
        ));
    }
    file.rewind()?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn read_ephemeral_runtime_control(file: &mut File) -> std::io::Result<EphemeralRuntimeControl> {
    let length = file.metadata()?.len();
    if length == 0 || length > MAX_EPHEMERAL_RUNTIME_CONTROL_BYTES {
        return Err(std::io::Error::other(
            "invalid ephemeral runtime control length",
        ));
    }
    file.rewind()?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_EPHEMERAL_RUNTIME_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length {
        return Err(std::io::Error::other(
            "unstable ephemeral runtime control length",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| std::io::Error::other("invalid ephemeral runtime control"))
}

fn create_private_runtime_child(directory: &File, name: &str) -> std::io::Result<File> {
    let name = runtime_child_name(name)?;
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

fn open_regular_runtime_child(directory: &File, name: &str) -> std::io::Result<File> {
    let name = runtime_child_name(name)?;
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
        return Err(std::io::Error::other(
            "ephemeral runtime child is not regular",
        ));
    }
    Ok(file)
}

fn unlink_owned_runtime_file(directory: &File, name: &str, owned: &File) -> std::io::Result<()> {
    let current = open_regular_runtime_child(directory, name)?;
    if !same_runtime_file(&current, owned)
        || !is_private_current_user_file(&current)?
        || runtime_link_count(&current)? != 1
    {
        return Err(std::io::Error::other(
            "ephemeral runtime child identity changed",
        ));
    }
    clear_user_immutable(&current)?;
    let name = runtime_child_name(name)?;
    let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn remove_empty_owned_runtime_directory(path: &Path, directory: &File) -> std::io::Result<()> {
    if !path_matches_open_directory(path, directory)
        || !runtime_directory_entries(directory)?.is_empty()
    {
        return Err(std::io::Error::other(
            "ephemeral runtime directory identity changed",
        ));
    }
    clear_user_immutable(directory)?;
    fs::remove_dir(path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn runtime_directory_entries(directory: &File) -> std::io::Result<Vec<OsString>> {
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

fn runtime_child_name(name: &str) -> std::io::Result<CString> {
    if name.is_empty() || name.as_bytes().contains(&b'/') {
        return Err(std::io::Error::other(
            "invalid ephemeral runtime child name",
        ));
    }
    CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::other("invalid ephemeral runtime child name"))
}

fn remove_ephemeral_runtime_tree_bound(data_root: &File) -> std::io::Result<()> {
    make_runtime_directory_removable(data_root)?;
    if let Some(run) = open_owned_runtime_directory_child(data_root, "run")? {
        make_runtime_directory_removable(&run)?;
        if let Some(backup) = open_owned_runtime_directory_child(&run, "backup")? {
            remove_ephemeral_backup_tree_bound(&backup)?;
            remove_owned_runtime_directory_child(&run, "backup", &backup)?;
        }
        if !runtime_directory_entries(&run)?.is_empty() {
            return Err(std::io::Error::other(
                "ephemeral recovery run directory has unexpected contents",
            ));
        }
        remove_owned_runtime_directory_child(data_root, "run", &run)?;
    }

    for name in [
        "remote-kosh.sqlite3",
        "remote-kosh.sqlite3-wal",
        "remote-kosh.sqlite3-shm",
    ] {
        remove_optional_owned_runtime_file(data_root, name)?;
    }
    let remaining = runtime_directory_entries(data_root)?;
    if remaining
        .iter()
        .any(|name| name != &OsString::from(EPHEMERAL_RUNTIME_CONTROL_FILENAME))
    {
        return Err(std::io::Error::other(
            "ephemeral recovery root has unexpected contents",
        ));
    }
    Ok(())
}

fn remove_ephemeral_backup_tree_bound(backup: &File) -> std::io::Result<()> {
    make_runtime_directory_removable(backup)?;
    if let Some(stages) = open_owned_runtime_directory_child(backup, "verified-litestream")? {
        remove_ephemeral_binary_stages_bound(&stages)?;
        remove_owned_runtime_directory_child(backup, "verified-litestream", &stages)?;
    }
    if let Some(stages) = open_owned_runtime_directory_child(backup, "restore-configs")? {
        remove_ephemeral_restore_config_stages_bound(&stages)?;
        remove_owned_runtime_directory_child(backup, "restore-configs", &stages)?;
    }
    for name in ["ls.yml", "ls.pid.json", "ownership.lock", "ls.yml.tmp"] {
        remove_optional_owned_runtime_file(backup, name)?;
    }
    remove_optional_owned_runtime_socket(backup, "ls.sock")?;
    if !runtime_directory_entries(backup)?.is_empty() {
        return Err(std::io::Error::other(
            "ephemeral recovery runtime has unexpected contents",
        ));
    }
    Ok(())
}

fn remove_ephemeral_binary_stages_bound(stages: &File) -> std::io::Result<()> {
    make_runtime_directory_removable(stages)?;
    for entry in runtime_directory_entries(stages)? {
        let name = entry
            .to_str()
            .ok_or_else(|| std::io::Error::other("invalid recovery binary stage name"))?;
        let checksum = name
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".tmp"))
            .unwrap_or(name);
        if !is_sha256(checksum) {
            return Err(std::io::Error::other("unexpected recovery binary stage"));
        }
        remove_single_file_stage_bound(stages, name, "litestream")?;
    }
    Ok(())
}

fn remove_ephemeral_restore_config_stages_bound(stages: &File) -> std::io::Result<()> {
    make_runtime_directory_removable(stages)?;
    for entry in runtime_directory_entries(stages)? {
        let name = entry
            .to_str()
            .ok_or_else(|| std::io::Error::other("invalid recovery config stage name"))?;
        if let Some(checksum) = name
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".lock"))
        {
            if !is_sha256(checksum) {
                return Err(std::io::Error::other("unexpected recovery config lock"));
            }
            remove_optional_owned_runtime_file(stages, name)?;
            continue;
        }
        let checksum = name.strip_prefix('.').unwrap_or(name);
        let checksum = checksum
            .split_once('.')
            .map_or(checksum, |(checksum, _)| checksum);
        if !is_sha256(checksum) {
            return Err(std::io::Error::other("unexpected recovery config stage"));
        }
        remove_single_file_stage_bound(stages, name, "ls.yml")?;
    }
    Ok(())
}

fn remove_single_file_stage_bound(
    parent: &File,
    stage_name: &str,
    filename: &str,
) -> std::io::Result<()> {
    let stage = open_owned_runtime_directory_child(parent, stage_name)?.ok_or_else(|| {
        std::io::Error::other("ephemeral recovery stage disappeared during cleanup")
    })?;
    make_runtime_directory_removable(&stage)?;
    remove_optional_owned_runtime_file(&stage, filename)?;
    if !runtime_directory_entries(&stage)?.is_empty() {
        return Err(std::io::Error::other(
            "ephemeral recovery stage has unexpected contents",
        ));
    }
    remove_owned_runtime_directory_child(parent, stage_name, &stage)
}

fn open_owned_runtime_directory_child(parent: &File, name: &str) -> std::io::Result<Option<File>> {
    let name = runtime_child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error);
    }
    let directory = unsafe { File::from_raw_fd(descriptor) };
    use std::os::unix::fs::MetadataExt;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(
            "ephemeral runtime directory is not privately owned",
        ));
    }
    Ok(Some(directory))
}

fn open_owned_runtime_file_child(parent: &File, name: &str) -> std::io::Result<Option<File>> {
    let name = runtime_child_name(name)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error);
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(std::io::Error::other(
            "ephemeral runtime file is not privately owned",
        ));
    }
    Ok(Some(file))
}

fn make_runtime_directory_removable(directory: &File) -> std::io::Result<()> {
    clear_user_immutable(directory)?;
    use std::os::unix::fs::PermissionsExt;
    directory.set_permissions(fs::Permissions::from_mode(0o700))
}

fn remove_optional_owned_runtime_file(parent: &File, name: &str) -> std::io::Result<()> {
    let Some(owned) = open_owned_runtime_file_child(parent, name)? else {
        return Ok(());
    };
    clear_user_immutable(&owned)?;
    let current = open_owned_runtime_file_child(parent, name)?.ok_or_else(|| {
        std::io::Error::other("ephemeral runtime file disappeared during cleanup")
    })?;
    if !same_runtime_file(&current, &owned) {
        return Err(std::io::Error::other(
            "ephemeral runtime file identity changed",
        ));
    }
    let name = runtime_child_name(name)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn remove_optional_owned_runtime_socket(parent: &File, name: &str) -> std::io::Result<()> {
    let name = runtime_child_name(name)?;
    let Some(identity) = runtime_socket_identity(parent, &name)? else {
        return Ok(());
    };
    if runtime_socket_identity(parent, &name)? != Some(identity) {
        return Err(std::io::Error::other(
            "ephemeral runtime socket identity changed",
        ));
    }
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn runtime_socket_identity(parent: &File, name: &CString) -> std::io::Result<Option<(u64, u64)>> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error);
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK || stat.st_uid != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(
            "ephemeral runtime socket is not privately owned",
        ));
    }
    let device = stat
        .st_dev
        .try_into()
        .map_err(|_| std::io::Error::other("ephemeral runtime socket device is invalid"))?;
    let inode = stat.st_ino;
    Ok(Some((device, inode)))
}

fn remove_owned_runtime_directory_child(
    parent: &File,
    name: &str,
    owned: &File,
) -> std::io::Result<()> {
    make_runtime_directory_removable(owned)?;
    if !runtime_directory_entries(owned)?.is_empty() {
        return Err(std::io::Error::other(
            "ephemeral runtime directory is not empty",
        ));
    }
    let current = open_owned_runtime_directory_child(parent, name)?.ok_or_else(|| {
        std::io::Error::other("ephemeral runtime directory disappeared during cleanup")
    })?;
    if !same_runtime_file(&current, owned) {
        return Err(std::io::Error::other(
            "ephemeral runtime directory identity changed",
        ));
    }
    clear_user_immutable(&current)?;
    owned.sync_all()?;
    let name = runtime_child_name(name)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if result == 0 {
        parent.sync_all()
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn path_matches_open_directory(path: &Path, directory: &File) -> bool {
    let Ok(path_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_dir() {
        return false;
    }
    let Ok(open_metadata) = directory.metadata() else {
        return false;
    };
    same_runtime_metadata(&path_metadata, &open_metadata)
}

fn same_runtime_file(left: &File, right: &File) -> bool {
    left.metadata()
        .ok()
        .zip(right.metadata().ok())
        .is_some_and(|(left, right)| same_runtime_metadata(&left, &right))
}

fn same_runtime_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn is_private_current_user_directory(directory: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = directory.metadata()?;
    Ok(metadata.is_dir()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o777 == 0o700)
}

fn is_private_current_user_file(file: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o777 == 0o600)
}

fn runtime_link_count(file: &File) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn prepare_private_runtime_directory(path: &Path) -> std::io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "Litestream runtime directory is not a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn reject_symlink_or_non_file(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(std::io::Error::other(
            "Litestream runtime file is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_regular_temporary(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
            fs::remove_file(path)
        }
        Ok(_) => Err(std::io::Error::other(
            "Litestream temporary file is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn ensure_socket_path_fits(path: &Path) -> Result<(), LitestreamError> {
    use std::os::unix::ffi::OsStrExt;
    if path.as_os_str().as_bytes().len() > MAX_MACOS_UNIX_SOCKET_PATH_BYTES {
        return Err(LitestreamError::ControlSocketPathTooLong);
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_socket_path_fits(_path: &Path) -> Result<(), LitestreamError> {
    Ok(())
}

/// Non-secret inputs for the fixed Litestream replication protocol.
#[derive(Clone, Debug)]
pub struct LitestreamConfig<'a> {
    pub database_path: &'a Path,
    pub runtime: &'a LitestreamRuntimePaths,
    pub bucket: &'a str,
    pub replica_path: &'a str,
    pub endpoint: &'a str,
}

impl LitestreamConfig<'_> {
    pub fn render(&self) -> Result<String, LitestreamError> {
        if !self.database_path.is_absolute() || !self.runtime.socket().is_absolute() {
            return Err(LitestreamError::RelativeDatabasePath);
        }
        let database_path = path_scalar("database_path", self.database_path)?;
        let socket_path = path_scalar("socket_path", self.runtime.socket())?;
        let bucket = scalar("bucket", self.bucket)?;
        let replica_path = scalar("replica_path", self.replica_path)?;
        let endpoint = scalar("endpoint", self.endpoint)?;
        if !self.endpoint.starts_with("https://") {
            return Err(LitestreamError::InvalidConfigField("endpoint"));
        }

        Ok(format!(
            r#"logging:
  level: info
  type: json
  stderr: true

socket:
  enabled: true
  path: {socket_path}
  permissions: 0600

sync-interval: 5s
verify-compaction: true
auto-recover: false
l0-retention: 720h
l0-retention-check-interval: 1m
shutdown-sync-timeout: 30s
shutdown-sync-interval: 500ms

snapshot:
  interval: 6h
  retention: 720h

validation:
  interval: 6h

dbs:
  - path: {database_path}
    monitor-interval: 1s
    checkpoint-interval: 1m
    replica:
      type: s3
      bucket: {bucket}
      path: {replica_path}
      endpoint: {endpoint}
      region: auto
      force-path-style: false
      sync-interval: 5s
"#
        ))
    }
}

fn path_scalar(name: &'static str, value: &Path) -> Result<String, LitestreamError> {
    let value = value.to_str().ok_or(LitestreamError::NonUtf8RuntimePath)?;
    scalar(name, value)
}

fn scalar(name: &'static str, value: &str) -> Result<String, LitestreamError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(LitestreamError::InvalidConfigField(name));
    }
    Ok(format!("'{}'", value.replace('\'', "''")))
}

/// Litestream's canonical 16-character lowercase hexadecimal transaction ID.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LitestreamTxid(u64);

impl LitestreamTxid {
    #[must_use]
    pub fn from_local(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LitestreamTxid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

impl FromStr for LitestreamTxid {
    type Err = LitestreamError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 16
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(LitestreamError::InvalidTxid);
        }
        u64::from_str_radix(value, 16)
            .map(Self)
            .map_err(|_| LitestreamError::InvalidTxid)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncResult {
    pub database_path: PathBuf,
    pub txid: LitestreamTxid,
    pub replica_txid: Option<LitestreamTxid>,
    pub duration_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncWire {
    db_path: String,
    txid: u64,
    #[serde(default)]
    replica_txid: Option<u64>,
    duration_ms: u64,
}

pub fn parse_sync_json(bytes: &[u8], expect_remote: bool) -> Result<SyncResult, LitestreamError> {
    ensure_bounded_output(bytes, MAX_CONTROL_OUTPUT_BYTES)?;
    let wire: SyncWire = serde_json::from_slice(bytes).map_err(LitestreamError::InvalidJson)?;
    if expect_remote {
        if wire.replica_txid != Some(wire.txid) {
            return Err(LitestreamError::InvalidSyncContract);
        }
    } else if wire.replica_txid.is_some() {
        return Err(LitestreamError::InvalidSyncContract);
    }
    Ok(SyncResult {
        database_path: PathBuf::from(wire.db_path),
        txid: LitestreamTxid::from_local(wire.txid),
        replica_txid: wire.replica_txid.map(LitestreamTxid::from_local),
        duration_ms: wire.duration_ms,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReplicaKind {
    S3,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreFile {
    pub level: u8,
    pub name: String,
    pub min_txid: LitestreamTxid,
    pub max_txid: LitestreamTxid,
    pub size: u64,
    pub timestamp: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlan {
    pub source: String,
    pub target_path: PathBuf,
    pub replica: ReplicaKind,
    pub min_txid: LitestreamTxid,
    pub max_txid: LitestreamTxid,
    pub files: Vec<RestoreFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestorePlanWire {
    source: String,
    target_path: String,
    replica: ReplicaKind,
    min_txid: String,
    max_txid: String,
    files: Vec<RestoreFileWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreFileWire {
    level: u8,
    name: String,
    min_txid: String,
    max_txid: String,
    size: u64,
    timestamp: String,
}

pub fn parse_restore_plan_json(bytes: &[u8]) -> Result<RestorePlan, LitestreamError> {
    ensure_bounded_output(bytes, MAX_RESTORE_PLAN_OUTPUT_BYTES)?;
    let wire: RestorePlanWire =
        serde_json::from_slice(bytes).map_err(LitestreamError::InvalidJson)?;
    if wire.files.len() > MAX_RESTORE_FILES {
        return Err(LitestreamError::RestorePlanTooLarge);
    }
    let files = wire
        .files
        .into_iter()
        .map(|file| {
            Ok(RestoreFile {
                level: file.level,
                name: file.name,
                min_txid: file.min_txid.parse()?,
                max_txid: file.max_txid.parse()?,
                size: file.size,
                timestamp: file.timestamp,
            })
        })
        .collect::<Result<Vec<_>, LitestreamError>>()?;
    Ok(RestorePlan {
        source: wire.source,
        target_path: PathBuf::from(wire.target_path),
        replica: wire.replica,
        min_txid: wire.min_txid.parse()?,
        max_txid: wire.max_txid.parse()?,
        files,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum IntegrityCheck {
    Full,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreResult {
    pub database_path: PathBuf,
    pub replica: ReplicaKind,
    pub txid: LitestreamTxid,
    pub duration_ms: u64,
    pub integrity_check: IntegrityCheck,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreResultWire {
    db_path: String,
    replica: ReplicaKind,
    txid: String,
    duration_ms: u64,
    integrity_check: IntegrityCheck,
}

pub fn parse_restore_result_json(bytes: &[u8]) -> Result<RestoreResult, LitestreamError> {
    ensure_bounded_output(bytes, MAX_CONTROL_OUTPUT_BYTES)?;
    let wire: RestoreResultWire =
        serde_json::from_slice(bytes).map_err(LitestreamError::InvalidJson)?;
    Ok(RestoreResult {
        database_path: PathBuf::from(wire.db_path),
        replica: wire.replica,
        txid: wire.txid.parse()?,
        duration_ms: wire.duration_ms,
        integrity_check: wire.integrity_check,
    })
}

pub(crate) trait RelationalRestoreEngine: Send + Sync {
    fn replica_path(&self) -> &str;

    fn preview(
        &self,
        source_database_path: &Path,
        target_path: &Path,
        txid: LitestreamTxid,
    ) -> Result<RestorePlan, LitestreamError>;

    fn restore(
        &self,
        source_database_path: &Path,
        target_path: &Path,
        txid: LitestreamTxid,
    ) -> Result<RestoreResult, LitestreamError>;
}

/// Runs the pinned offline restore command with credentials supplied only
/// through the child's inherited stdin descriptor. No credential value is
/// written to arguments, environment values, configuration, or logs.
#[allow(
    dead_code,
    reason = "constructed by the Settings restore IPC in chunk 29g"
)]
pub(crate) struct CommandLitestreamRestore<'a> {
    binary: &'a ImmutableLitestreamBinary,
    config: ImmutableLitestreamRestoreConfig,
    credentials: &'a R2Credentials,
    timeout: Duration,
}

#[allow(
    dead_code,
    reason = "constructed by the Settings restore IPC in chunk 29g"
)]
impl<'a> CommandLitestreamRestore<'a> {
    pub(crate) fn new(
        binary: &'a ImmutableLitestreamBinary,
        runtime: &LitestreamRuntimePaths,
        target: &R2Target,
        replica_path: &R2ObjectKey,
        source_database_path: &Path,
        credentials: &'a R2Credentials,
        timeout: Duration,
    ) -> Result<Self, LitestreamError> {
        let endpoint = target.endpoint();
        let rendered = LitestreamConfig {
            database_path: source_database_path,
            runtime,
            bucket: target.bucket.as_str(),
            replica_path: replica_path.as_str(),
            endpoint: &endpoint,
        }
        .render()?;
        let config = stage_immutable_restore_config(runtime, &rendered, replica_path)?;
        Ok(Self {
            binary,
            config,
            credentials,
            timeout,
        })
    }

    fn execute(
        &self,
        source_database_path: &Path,
        target_path: &Path,
        txid: LitestreamTxid,
        dry_run: bool,
    ) -> Result<Vec<u8>, LitestreamError> {
        if !source_database_path.is_absolute() || !target_path.is_absolute() {
            return Err(LitestreamError::RelativeDatabasePath);
        }
        let arguments = restore_arguments(
            &self.config.path,
            source_database_path,
            target_path,
            txid,
            dry_run,
        );
        self.binary.reverify_before_spawn()?;
        self.config.reverify_before_spawn()?;
        execute_credentialed_command(
            self.binary,
            &arguments,
            self.credentials,
            self.timeout,
            if dry_run {
                MAX_RESTORE_PLAN_OUTPUT_BYTES
            } else {
                MAX_CONTROL_OUTPUT_BYTES
            },
            if dry_run { None } else { target_path.parent() },
        )
    }
}

impl RelationalRestoreEngine for CommandLitestreamRestore<'_> {
    fn replica_path(&self) -> &str {
        self.config.replica_path.as_str()
    }

    fn preview(
        &self,
        source_database_path: &Path,
        target_path: &Path,
        txid: LitestreamTxid,
    ) -> Result<RestorePlan, LitestreamError> {
        let bytes = self.execute(source_database_path, target_path, txid, true)?;
        let plan = parse_restore_plan_json(&bytes)?;
        if plan.source != source_database_path.to_string_lossy()
            || plan.target_path != target_path
            || plan.replica != ReplicaKind::S3
            || plan.min_txid > txid
            || plan.max_txid < txid
            || plan.files.is_empty()
            || !plan
                .files
                .iter()
                .any(|file| file.min_txid <= txid && file.max_txid >= txid)
        {
            return Err(LitestreamError::InvalidRestoreContract);
        }
        Ok(plan)
    }

    fn restore(
        &self,
        source_database_path: &Path,
        target_path: &Path,
        txid: LitestreamTxid,
    ) -> Result<RestoreResult, LitestreamError> {
        if !source_database_path.is_absolute() || !target_path.is_absolute() {
            return Err(LitestreamError::RelativeDatabasePath);
        }
        let destination = ExclusiveRestoreDestination::prepare(target_path)?;
        destination.verify_path_bindings()?;
        let bytes = self.execute(
            source_database_path,
            destination.private_path(),
            txid,
            false,
        )?;
        let mut result = parse_restore_result_json(&bytes)?;
        if result.database_path != destination.private_path()
            || result.replica != ReplicaKind::S3
            || result.txid != txid
            || result.integrity_check != IntegrityCheck::Full
        {
            return Err(LitestreamError::InvalidRestoreContract);
        }
        destination.publish()?;
        result.database_path = target_path.to_owned();
        Ok(result)
    }
}

/// Owns a one-shot restore path and publishes it without ever handing the
/// caller-selected destination to Litestream. `hard_link` creates the final
/// directory entry atomically and fails for every existing node, including a
/// dangling symlink, while the open no-follow descriptors bind validation to
/// the exact inode Litestream produced.
#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
struct ExclusiveRestoreDestination {
    requested_path: PathBuf,
    requested_parent: File,
    requested_name: OsString,
    private_directory: File,
    private_directory_name: String,
    private_path: PathBuf,
}

#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
impl ExclusiveRestoreDestination {
    fn prepare(requested_path: &Path) -> Result<Self, LitestreamError> {
        let parent = requested_path.parent().ok_or_else(|| {
            LitestreamError::PrepareRestoreDestination(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "restore destination has no parent",
            ))
        })?;
        let requested_name = requested_path.file_name().ok_or_else(|| {
            LitestreamError::PrepareRestoreDestination(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "restore destination has no filename",
            ))
        })?;
        restore_child_name(requested_name).map_err(LitestreamError::PrepareRestoreDestination)?;
        let requested_parent =
            open_directory_no_follow(parent).map_err(LitestreamError::PrepareRestoreDestination)?;
        let parent_metadata = requested_parent
            .metadata()
            .map_err(LitestreamError::PrepareRestoreDestination)?;
        if !parent_metadata.file_type().is_dir() {
            return Err(LitestreamError::InvalidRestoreDestination);
        }
        if restore_child_exists(&requested_parent, requested_name)
            .map_err(LitestreamError::PrepareRestoreDestination)?
        {
            return Err(LitestreamError::RestoreDestinationExists);
        }

        for _ in 0..8 {
            let private_directory_name =
                format!(".kosh-litestream-restore-{}.tmp", uuid::Uuid::now_v7());
            match create_private_restore_directory(&requested_parent, &private_directory_name) {
                Ok(()) => {
                    let private_directory = open_owned_runtime_directory_child(
                        &requested_parent,
                        &private_directory_name,
                    )
                    .map_err(LitestreamError::PrepareRestoreDestination)?
                    .ok_or_else(|| {
                        LitestreamError::PrepareRestoreDestination(std::io::Error::other(
                            "private restore directory disappeared",
                        ))
                    })?;
                    if !is_private_current_user_directory(&private_directory)
                        .map_err(LitestreamError::PrepareRestoreDestination)?
                    {
                        return Err(LitestreamError::InvalidRestoreDestination);
                    }
                    requested_parent
                        .sync_all()
                        .map_err(LitestreamError::PrepareRestoreDestination)?;
                    let private_path = parent
                        .join(&private_directory_name)
                        .join("database.sqlite3");
                    return Ok(Self {
                        requested_path: requested_path.to_owned(),
                        requested_parent,
                        requested_name: requested_name.to_owned(),
                        private_directory,
                        private_directory_name,
                        private_path,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(LitestreamError::PrepareRestoreDestination(error));
                }
            }
        }
        Err(LitestreamError::PrepareRestoreDestination(
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique private restore directory",
            ),
        ))
    }

    fn private_path(&self) -> &Path {
        &self.private_path
    }

    fn verify_path_bindings(&self) -> Result<(), LitestreamError> {
        let requested_parent_path = self
            .requested_path
            .parent()
            .ok_or(LitestreamError::InvalidRestoreDestination)?;
        if !path_matches_open_directory(requested_parent_path, &self.requested_parent) {
            return Err(LitestreamError::InvalidRestoreDestination);
        }
        let current_private = open_owned_runtime_directory_child(
            &self.requested_parent,
            &self.private_directory_name,
        )
        .map_err(LitestreamError::PublishRestoreDestination)?
        .ok_or(LitestreamError::InvalidRestoreDestination)?;
        if !same_runtime_file(&current_private, &self.private_directory) {
            return Err(LitestreamError::InvalidRestoreDestination);
        }
        Ok(())
    }

    fn publish(self) -> Result<(), LitestreamError> {
        self.verify_path_bindings()?;
        let private_file =
            open_owned_runtime_file_child(&self.private_directory, "database.sqlite3")
                .map_err(LitestreamError::PublishRestoreDestination)?
                .ok_or(LitestreamError::InvalidRestoreDestination)?;
        let private_metadata = private_file
            .metadata()
            .map_err(LitestreamError::PublishRestoreDestination)?;
        if !private_metadata.file_type().is_file() || private_metadata.len() == 0 {
            return Err(LitestreamError::InvalidRestoreDestination);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if private_metadata.nlink() != 1 {
                return Err(LitestreamError::InvalidRestoreDestination);
            }
        }
        private_file
            .sync_all()
            .map_err(LitestreamError::PublishRestoreDestination)?;

        match link_restore_file(
            &self.private_directory,
            OsStr::new("database.sqlite3"),
            &self.requested_parent,
            &self.requested_name,
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(LitestreamError::RestoreDestinationExists);
            }
            Err(error) => return Err(LitestreamError::PublishRestoreDestination(error)),
        }

        let publication = (|| {
            let published_file =
                open_regular_restore_child(&self.requested_parent, &self.requested_name)?;
            let published_metadata = published_file
                .metadata()
                .map_err(LitestreamError::PublishRestoreDestination)?;
            if !same_restore_file(&private_metadata, &published_metadata) {
                return Err(LitestreamError::InvalidRestoreDestination);
            }
            self.requested_parent
                .sync_all()
                .map_err(LitestreamError::PublishRestoreDestination)?;
            unlink_exact_restore_file(
                &self.private_directory,
                OsStr::new("database.sqlite3"),
                &private_file,
            )
            .map_err(LitestreamError::PublishRestoreDestination)?;
            remove_owned_runtime_directory_child(
                &self.requested_parent,
                &self.private_directory_name,
                &self.private_directory,
            )
            .map_err(LitestreamError::PublishRestoreDestination)?;
            self.verify_requested_parent_binding()?;
            Ok(())
        })();
        if let Err(error) = publication {
            let _ = unlink_exact_restore_file(
                &self.requested_parent,
                &self.requested_name,
                &private_file,
            );
            let _ = self.requested_parent.sync_all();
            return Err(error);
        }
        Ok(())
    }

    fn verify_requested_parent_binding(&self) -> Result<(), LitestreamError> {
        let requested_parent_path = self
            .requested_path
            .parent()
            .ok_or(LitestreamError::InvalidRestoreDestination)?;
        if path_matches_open_directory(requested_parent_path, &self.requested_parent) {
            Ok(())
        } else {
            Err(LitestreamError::InvalidRestoreDestination)
        }
    }
}

impl Drop for ExclusiveRestoreDestination {
    fn drop(&mut self) {
        let _ = remove_optional_private_restore_output(&self.private_directory);
        let _ = remove_owned_runtime_directory_child(
            &self.requested_parent,
            &self.private_directory_name,
            &self.private_directory,
        );
    }
}

fn restore_child_name(name: &OsStr) -> std::io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(std::io::Error::other("invalid restore child name"));
    }
    CString::new(bytes).map_err(|_| std::io::Error::other("invalid restore child name"))
}

fn restore_child_exists(parent: &File, name: &OsStr) -> std::io::Result<bool> {
    Ok(restore_child_stat(parent, name)?.is_some())
}

fn restore_child_stat(parent: &File, name: &OsStr) -> std::io::Result<Option<libc::stat>> {
    let name = restore_child_name(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(Some(unsafe { stat.assume_init() }));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(error)
    }
}

fn remove_optional_private_restore_output(parent: &File) -> std::io::Result<()> {
    let name = OsStr::new("database.sqlite3");
    let Some(identity) = restore_child_stat(parent, name)? else {
        return Ok(());
    };
    let kind = identity.st_mode & libc::S_IFMT;
    if kind == libc::S_IFREG {
        return remove_optional_owned_runtime_file(parent, "database.sqlite3");
    }
    if kind != libc::S_IFLNK
        || identity.st_uid != unsafe { libc::geteuid() }
        || identity.st_nlink != 1
    {
        return Err(std::io::Error::other(
            "private restore output has an unexpected type or owner",
        ));
    }
    let current = restore_child_stat(parent, name)?.ok_or_else(|| {
        std::io::Error::other("private restore output disappeared during cleanup")
    })?;
    if current.st_dev != identity.st_dev
        || current.st_ino != identity.st_ino
        || current.st_mode != identity.st_mode
        || current.st_uid != identity.st_uid
    {
        return Err(std::io::Error::other(
            "private restore output identity changed",
        ));
    }
    let name = restore_child_name(name)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn create_private_restore_directory(parent: &File, name: &str) -> std::io::Result<()> {
    let name = restore_child_name(OsStr::new(name))?;
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn link_restore_file(
    source_parent: &File,
    source_name: &OsStr,
    target_parent: &File,
    target_name: &OsStr,
) -> std::io::Result<()> {
    let source_name = restore_child_name(source_name)?;
    let target_name = restore_child_name(target_name)?;
    let result = unsafe {
        libc::linkat(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            target_parent.as_raw_fd(),
            target_name.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn open_regular_restore_child(parent: &File, name: &OsStr) -> Result<File, LitestreamError> {
    let name = restore_child_name(name).map_err(LitestreamError::PublishRestoreDestination)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(LitestreamError::PublishRestoreDestination(
            std::io::Error::last_os_error(),
        ));
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    if !file
        .metadata()
        .map_err(LitestreamError::PublishRestoreDestination)?
        .is_file()
    {
        return Err(LitestreamError::InvalidRestoreDestination);
    }
    Ok(file)
}

fn unlink_exact_restore_file(parent: &File, name: &OsStr, owned: &File) -> std::io::Result<()> {
    let current = open_regular_restore_child(parent, name)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if !same_runtime_file(&current, owned) {
        return Err(std::io::Error::other("restore file identity changed"));
    }
    let name = restore_child_name(name)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
fn same_restore_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    right.file_type().is_file() && left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
fn same_restore_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    right.file_type().is_file() && left.len() == right.len()
}

#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
fn restore_arguments(
    config: &Path,
    source_database_path: &Path,
    target_path: &Path,
    txid: LitestreamTxid,
    dry_run: bool,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("restore"),
        OsString::from("-config"),
        config.as_os_str().to_owned(),
        OsString::from("-txid"),
        OsString::from(txid.to_string()),
    ];
    if dry_run {
        arguments.push(OsString::from("-dry-run"));
    }
    arguments.push(OsString::from("-json"));
    if !dry_run {
        arguments.extend([OsString::from("-integrity-check"), OsString::from("full")]);
    }
    arguments.extend([
        OsString::from("-o"),
        target_path.as_os_str().to_owned(),
        source_database_path.as_os_str().to_owned(),
    ]);
    arguments
}

#[allow(dead_code, reason = "used by the chunk 29g production restore adapter")]
fn execute_credentialed_command(
    binary: &ImmutableLitestreamBinary,
    arguments: &[OsString],
    credentials: &R2Credentials,
    timeout: Duration,
    output_limit: usize,
    restore_write_root: Option<&Path>,
) -> Result<Vec<u8>, LitestreamError> {
    let mut command = credentialed_restore_command(binary, arguments, restore_write_root)?;
    command
        .env_clear()
        .env(
            AWS_SHARED_CREDENTIALS_FILE_ENV,
            AWS_SHARED_CREDENTIALS_FILE_FD,
        )
        .env(AWS_EC2_METADATA_DISABLED_ENV, "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(LitestreamError::Execute)?;
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = verify_spawned_restore_process(binary, &mut child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    let mut stdin = child.stdin.take().ok_or_else(|| {
        LitestreamError::Execute(std::io::Error::other(
            "Litestream restore credential pipe is unavailable",
        ))
    })?;
    if let Err(error) = write_aws_shared_credentials(&mut stdin, credentials) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(LitestreamError::Execute(error));
    }
    drop(stdin);
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(LitestreamError::Execute(std::io::Error::other(
            "Litestream restore stdout pipe is unavailable",
        )));
    };
    let reader = match thread::Builder::new()
        .name("kosh-litestream-restore-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take(output_limit as u64 + 1)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LitestreamError::Execute(error));
        }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(COMMAND_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(LitestreamError::Execute(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Litestream restore timed out",
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(LitestreamError::Execute(error));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| {
            LitestreamError::Execute(std::io::Error::other(
                "Litestream restore output reader panicked",
            ))
        })?
        .map_err(LitestreamError::Execute)?;
    if bytes.len() > output_limit {
        return Err(LitestreamError::OversizedControlResponse);
    }
    if !status.success() {
        return Err(LitestreamError::CommandFailed {
            exit_code: status.code(),
        });
    }
    Ok(bytes)
}

fn credentialed_restore_command(
    binary: &ImmutableLitestreamBinary,
    arguments: &[OsString],
    restore_write_root: Option<&Path>,
) -> Result<Command, LitestreamError> {
    #[cfg(target_os = "macos")]
    if let Some(write_root) = restore_write_root {
        let profile = restore_sandbox_profile(write_root)?;
        let mut command = Command::new(SANDBOX_EXEC_PATH);
        command
            .args(["-p", profile.as_str()])
            .arg(binary.path())
            .args(arguments);
        return Ok(command);
    }

    #[cfg(not(target_os = "macos"))]
    let _ = restore_write_root;

    let mut command = Command::new(binary.path());
    command.args(arguments);
    Ok(command)
}

#[cfg(target_os = "macos")]
fn restore_sandbox_profile(write_root: &Path) -> Result<String, LitestreamError> {
    let canonical_write_root =
        fs::canonicalize(write_root).map_err(LitestreamError::PrepareRestoreDestination)?;
    let write_root = canonical_write_root
        .to_str()
        .ok_or(LitestreamError::NonUtf8RuntimePath)?;
    if write_root.chars().any(|character| character.is_control()) {
        return Err(LitestreamError::InvalidConfigField("restore sandbox path"));
    }
    let escaped = write_root.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process*)\n\
         (allow sysctl-read)\n\
         (allow file-read*)\n\
         (allow file-write* (literal \"/dev/null\") (subpath \"{escaped}\"))\n\
         (allow network*)\n\
         (allow system-socket)\n\
         (allow mach-lookup)\n\
         (allow ipc-posix*)"
    ))
}

#[cfg(target_os = "macos")]
fn verify_spawned_restore_process(
    binary: &ImmutableLitestreamBinary,
    child: &mut std::process::Child,
) -> Result<(), LitestreamError> {
    let deadline = Instant::now() + PROCESS_IDENTITY_TIMEOUT;
    loop {
        let identity_error = match running_process_cdhash(child.id()) {
            Ok(actual) if actual == binary.code_signature_cdhash => return Ok(()),
            Ok(_) => LitestreamError::ProcessCodeSignatureMismatch,
            Err(error) => LitestreamError::ProcessIdentityUnavailable(error),
        };
        if child
            .try_wait()
            .map_err(LitestreamError::Execute)?
            .is_some()
            || Instant::now() >= deadline
        {
            return Err(identity_error);
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    }
}

fn ensure_bounded_output(bytes: &[u8], limit: usize) -> Result<(), LitestreamError> {
    if bytes.len() > limit {
        return Err(LitestreamError::OversizedControlResponse);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
}

pub trait CommandExecutor: Send + Sync {
    fn execute(&self, spec: &CommandSpec) -> Result<CommandResult, std::io::Error>;

    fn execute_with_timeout(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
    ) -> Result<CommandResult, std::io::Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCommandExecutor;

impl CommandExecutor for SystemCommandExecutor {
    fn execute(&self, spec: &CommandSpec) -> Result<CommandResult, std::io::Error> {
        let mut child = Command::new(&spec.program)
            .args(&spec.arguments)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::other("Litestream control stdout pipe is unavailable")
        })?;
        let mut bytes = Vec::new();
        stdout
            .by_ref()
            .take(MAX_CONTROL_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CONTROL_OUTPUT_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(
                "Litestream control output exceeded its safety bound",
            ));
        }
        let status = child.wait()?;
        Ok(CommandResult {
            exit_code: status.code(),
            stdout: bytes,
        })
    }

    fn execute_with_timeout(
        &self,
        spec: &CommandSpec,
        timeout: Duration,
    ) -> Result<CommandResult, std::io::Error> {
        execute_command_with_timeout(spec, timeout)
    }
}

fn execute_command_with_timeout(
    spec: &CommandSpec,
    timeout: Duration,
) -> Result<CommandResult, std::io::Error> {
    let mut child = Command::new(&spec.program)
        .args(&spec.arguments)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other(
            "Litestream control stdout pipe is unavailable",
        ));
    };
    let reader = match thread::Builder::new()
        .name("kosh-litestream-control-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take(MAX_CONTROL_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_CONTROL_OUTPUT_BYTES {
                return Err(std::io::Error::other(
                    "Litestream control output exceeded its safety bound",
                ));
            }
            Ok(bytes)
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Litestream control command timed out",
            ));
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    };
    let bytes = reader
        .join()
        .map_err(|_| std::io::Error::other("Litestream control output reader panicked"))??;
    Ok(CommandResult {
        exit_code: status.code(),
        stdout: bytes,
    })
}

pub trait LitestreamControl: Send + Sync {
    fn sync_local(&self, database_path: &Path) -> Result<SyncResult, LitestreamError>;
    fn sync_remote(&self, database_path: &Path) -> Result<SyncResult, LitestreamError>;
}

#[derive(Clone, Debug)]
pub struct CommandLitestreamControl<E> {
    binary: PathBuf,
    socket: PathBuf,
    remote_timeout_seconds: u64,
    executor: E,
}

impl<E> CommandLitestreamControl<E> {
    #[must_use]
    pub fn new(binary: PathBuf, socket: PathBuf, remote_timeout_seconds: u64, executor: E) -> Self {
        Self {
            binary,
            socket,
            remote_timeout_seconds,
            executor,
        }
    }
}

impl<E: CommandExecutor> CommandLitestreamControl<E> {
    fn sync(
        &self,
        database_path: &Path,
        wait: bool,
        execution_timeout: Option<Duration>,
    ) -> Result<SyncResult, LitestreamError> {
        if !database_path.is_absolute() {
            return Err(LitestreamError::RelativeDatabasePath);
        }
        let mut arguments = vec![OsString::from("sync")];
        if wait {
            arguments.extend([
                OsString::from("-wait"),
                OsString::from("-timeout"),
                OsString::from(self.remote_timeout_seconds.to_string()),
            ]);
        }
        arguments.extend([
            OsString::from("-json"),
            OsString::from("-socket"),
            self.socket.as_os_str().to_owned(),
            database_path.as_os_str().to_owned(),
        ]);
        let spec = CommandSpec {
            program: self.binary.clone(),
            arguments,
        };
        let result = match execution_timeout {
            Some(timeout) => self.executor.execute_with_timeout(&spec, timeout),
            None => self.executor.execute(&spec),
        }
        .map_err(LitestreamError::Execute)?;
        if result.exit_code != Some(0) {
            return Err(LitestreamError::CommandFailed {
                exit_code: result.exit_code,
            });
        }
        let sync = parse_sync_json(&result.stdout, wait)?;
        if sync.database_path != database_path {
            return Err(LitestreamError::UnexpectedDatabasePath);
        }
        Ok(sync)
    }

    pub fn sync_local_with_timeout(
        &self,
        database_path: &Path,
        timeout: Duration,
    ) -> Result<SyncResult, LitestreamError> {
        self.sync(database_path, false, Some(timeout))
    }

    pub fn sync_remote_with_timeout(
        &self,
        database_path: &Path,
        timeout: Duration,
    ) -> Result<SyncResult, LitestreamError> {
        self.sync(database_path, true, Some(timeout))
    }
}

impl<E: CommandExecutor> LitestreamControl for CommandLitestreamControl<E> {
    fn sync_local(&self, database_path: &Path) -> Result<SyncResult, LitestreamError> {
        self.sync(database_path, false, None)
    }

    fn sync_remote(&self, database_path: &Path) -> Result<SyncResult, LitestreamError> {
        self.sync(database_path, true, None)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::{symlink, PermissionsExt},
        sync::{Arc, Barrier, Mutex},
    };

    use super::*;
    use crate::backup::domain::{
        BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, ReplicaEpochId,
    };

    fn staged_restore_test_binary(root: &Path, bytes: &[u8]) -> ImmutableLitestreamBinary {
        let source_path = root.join("source-litestream");
        fs::write(&source_path, bytes).expect("restore test binary");
        fs::set_permissions(&source_path, fs::Permissions::from_mode(0o700))
            .expect("restore test binary permissions");
        let pin = BinaryPin {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            code_signature_identifier: "com.rohan.kosh.litestream".into(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        };
        let verified = VerifiedLitestreamBinary {
            path: source_path.clone(),
            sha256: pin.sha256.clone(),
            size: pin.size,
            code_signature_cdhash: restore_test_process_cdhash(),
            file: verify_binary(&source_path, &pin).expect("verified restore test binary"),
        };
        let runtime =
            LitestreamRuntimePaths::new(&root.join("runtime")).expect("restore test runtime");
        verified
            .stage_immutable(&runtime)
            .expect("immutable restore test binary")
    }

    #[cfg(target_os = "macos")]
    fn restore_test_process_cdhash() -> String {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read ignored"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("signed shell process");
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut previous = None;
        let cdhash = loop {
            let current = running_process_cdhash(child.id()).expect("shell process CDHash");
            if previous.as_ref() == Some(&current) {
                break current;
            }
            assert!(
                Instant::now() < deadline,
                "shell process CDHash did not stabilize"
            );
            previous = Some(current);
            thread::sleep(Duration::from_millis(10));
        };
        child.kill().expect("terminate shell process");
        child.wait().expect("reap shell process");
        cdhash
    }

    #[cfg(not(target_os = "macos"))]
    fn restore_test_process_cdhash() -> String {
        "0".repeat(40)
    }

    fn thaw_restore_test_binary(binary: &ImmutableLitestreamBinary) {
        let directory_path = binary.path().parent().expect("immutable directory");
        let directory = File::open(directory_path).expect("immutable directory descriptor");
        clear_user_immutable(&directory).expect("thaw immutable directory");
        fs::set_permissions(directory_path, fs::Permissions::from_mode(0o700))
            .expect("thawed directory permissions");
        let file = File::open(binary.path()).expect("immutable binary descriptor");
        clear_user_immutable(&file).expect("thaw immutable binary");
        fs::set_permissions(binary.path(), fs::Permissions::from_mode(0o700))
            .expect("thawed binary permissions");
    }

    fn restore_test_target() -> (R2Target, R2ObjectKey) {
        let target = R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef")
                .expect("test account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-restore-test").expect("test bucket"),
        };
        let replica_path = target
            .keyspace(&BackupSetId::new())
            .litestream(&ReplicaEpochId::new());
        (target, replica_path)
    }

    #[derive(Clone, Debug)]
    struct FakeExecutor {
        calls: Arc<Mutex<Vec<CommandSpec>>>,
        result: CommandResult,
    }

    impl CommandExecutor for FakeExecutor {
        fn execute(&self, spec: &CommandSpec) -> Result<CommandResult, std::io::Error> {
            self.calls.lock().expect("fake calls").push(spec.clone());
            Ok(self.result.clone())
        }

        fn execute_with_timeout(
            &self,
            spec: &CommandSpec,
            _timeout: Duration,
        ) -> Result<CommandResult, std::io::Error> {
            self.execute(spec)
        }
    }

    #[test]
    fn txids_normalize_to_canonical_lowercase_hex() {
        let txid = LitestreamTxid::from_local(66);
        assert_eq!(txid.to_string(), "0000000000000042");
        assert_eq!(
            "0000000000000042".parse::<LitestreamTxid>().expect("txid"),
            txid
        );
        for malformed in [
            "42",
            "000000000000004G",
            "000000000000004A",
            "00000000000000000",
        ] {
            assert!(malformed.parse::<LitestreamTxid>().is_err());
        }
    }

    #[test]
    fn daemon_environment_contains_only_nonsecret_credential_pipe_controls() {
        let credentials = R2Credentials::new(
            "0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("credentials");
        let mut command = Command::new("/app/bin/litestream");
        command.env("UNRELATED_SECRET", "must-not-survive");

        configure_credential_pipe_environment(&mut command);

        let environment = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            environment,
            [
                (AWS_EC2_METADATA_DISABLED_ENV.into(), Some("true".into()),),
                (
                    AWS_SHARED_CREDENTIALS_FILE_ENV.into(),
                    Some(AWS_SHARED_CREDENTIALS_FILE_FD.into()),
                ),
            ]
        );

        let mut shared_credentials = Vec::new();
        write_aws_shared_credentials(&mut shared_credentials, &credentials)
            .expect("shared credentials");
        assert_eq!(
            String::from_utf8(shared_credentials).expect("UTF-8 credentials"),
            "[default]\n\
             aws_access_key_id = 0123456789abcdef0123456789abcdef\n\
             aws_secret_access_key = \
             0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
        );
    }

    #[test]
    fn parses_pinned_sync_json_contracts() {
        let local = parse_sync_json(
            br#"{"db_path":"/tmp/kosh.sqlite3","txid":2,"duration_ms":8}"#,
            false,
        )
        .expect("local sync");
        assert_eq!(local.txid.to_string(), "0000000000000002");
        assert_eq!(local.replica_txid, None);

        let remote = parse_sync_json(
            br#"{"db_path":"/tmp/kosh.sqlite3","txid":3,"replica_txid":3,"duration_ms":244}"#,
            true,
        )
        .expect("remote sync");
        assert_eq!(
            remote.replica_txid.expect("replica txid"),
            LitestreamTxid::from_local(3)
        );
        assert!(parse_sync_json(
            br#"{"db_path":"/tmp/kosh.sqlite3","txid":3,"duration_ms":244}"#,
            true
        )
        .is_err());
        assert!(matches!(
            parse_sync_json(&vec![b' '; MAX_CONTROL_OUTPUT_BYTES + 1], false),
            Err(LitestreamError::OversizedControlResponse)
        ));
    }

    #[test]
    fn parses_pinned_restore_json_contracts() {
        let plan = parse_restore_plan_json(
            br#"{
              "source":"/tmp/kosh.sqlite3",
              "target_path":"/tmp/restore.sqlite3",
              "replica":"s3",
              "min_txid":"0000000000000001",
              "max_txid":"0000000000000002",
              "files":[{
                "level":0,
                "name":"0000000000000002-0000000000000002.ltx",
                "min_txid":"0000000000000002",
                "max_txid":"0000000000000002",
                "size":339,
                "timestamp":"2026-07-30T01:34:40Z"
              }]
            }"#,
        )
        .expect("restore plan");
        assert_eq!(plan.max_txid.to_string(), "0000000000000002");
        assert_eq!(plan.files.len(), 1);

        let result = parse_restore_result_json(
            br#"{
              "db_path":"/tmp/restore.sqlite3",
              "replica":"s3",
              "txid":"0000000000000002",
              "duration_ms":1051,
              "integrity_check":"full"
            }"#,
        )
        .expect("restore result");
        assert_eq!(result.txid, plan.max_txid);
        assert_eq!(result.integrity_check, IntegrityCheck::Full);
    }

    #[test]
    fn restore_commands_pin_exact_txid_and_full_integrity_without_secrets() {
        let source = Path::new("/data/kosh.sqlite3");
        let target = Path::new("/data/restore/kosh.sqlite3");
        let config = Path::new("/data/run/backup/ls.yml");
        let txid = LitestreamTxid::from_local(42);
        let preview = restore_arguments(config, source, target, txid, true);
        let restore = restore_arguments(config, source, target, txid, false);
        assert_eq!(
            preview,
            [
                "restore",
                "-config",
                "/data/run/backup/ls.yml",
                "-txid",
                "000000000000002a",
                "-dry-run",
                "-json",
                "-o",
                "/data/restore/kosh.sqlite3",
                "/data/kosh.sqlite3",
            ]
            .map(OsString::from)
        );
        assert_eq!(
            restore,
            [
                "restore",
                "-config",
                "/data/run/backup/ls.yml",
                "-txid",
                "000000000000002a",
                "-json",
                "-integrity-check",
                "full",
                "-o",
                "/data/restore/kosh.sqlite3",
                "/data/kosh.sqlite3",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn concurrent_restore_config_staging_converges_on_one_immutable_directory() {
        const BUILDERS: usize = 4;

        let root = tempfile::tempdir().expect("restore config root");
        let rendered = "dbs:\n  - path: /tmp/kosh.sqlite3\n".to_owned();
        let (_, replica_path) = restore_test_target();
        let barrier = Arc::new(Barrier::new(BUILDERS + 1));
        let configs = thread::scope(|scope| {
            let builders = (0..BUILDERS)
                .map(|_| {
                    let data_root = root.path().to_owned();
                    let rendered = rendered.clone();
                    let replica_path = replica_path.clone();
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        let runtime = LitestreamRuntimePaths::new(&data_root)
                            .expect("concurrent restore runtime");
                        barrier.wait();
                        stage_immutable_restore_config(&runtime, &rendered, &replica_path)
                            .expect("converged immutable restore config")
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            builders
                .into_iter()
                .map(|builder| builder.join().expect("restore config builder"))
                .collect::<Vec<_>>()
        });

        for config in &configs {
            assert_eq!(config.path, configs[0].path);
            assert_eq!(config.sha256, configs[0].sha256);
            assert_eq!(config.size, configs[0].size);
            config.reverify_before_spawn().expect("immutable config");
        }
        let stage_root = configs[0]
            .path
            .parent()
            .and_then(Path::parent)
            .expect("restore config stage root");
        let entries = fs::read_dir(stage_root)
            .expect("restore config entries")
            .map(|entry| entry.expect("restore config entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.to_str() == Some(configs[0].sha256.as_str()))
                .count(),
            1
        );
        assert!(!entries
            .iter()
            .any(|name| { name.to_str().is_some_and(|name| name.ends_with(".tmp")) }));

        remove_partial_restore_config(
            configs[0]
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore config");
    }

    #[test]
    fn ephemeral_recovery_cleanup_removes_immutable_binary_and_config_stages() {
        let source = tempfile::tempdir().expect("ephemeral recovery binary source");
        let bytes = b"ephemeral recovery binary";
        let source_path = source.path().join("litestream");
        fs::write(&source_path, bytes).expect("write recovery binary");
        fs::set_permissions(&source_path, fs::Permissions::from_mode(0o700))
            .expect("recovery binary permissions");
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let pin = BinaryPin {
            sha256: sha256.clone(),
            size: bytes.len() as u64,
            code_signature_identifier: "com.rohan.kosh.litestream".into(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        };
        let verified = VerifiedLitestreamBinary {
            path: source_path.clone(),
            sha256,
            size: bytes.len() as u64,
            code_signature_cdhash: restore_test_process_cdhash(),
            file: verify_binary(&source_path, &pin).expect("verify recovery binary"),
        };
        let root = PathBuf::from(format!("/tmp/kosh-r-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).expect("create ephemeral recovery root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("ephemeral recovery root permissions");
        let runtime = LitestreamRuntimePaths::new(&root).expect("ephemeral recovery paths");
        let binary = verified
            .stage_immutable(&runtime)
            .expect("stage recovery binary");
        let (_, replica_path) = restore_test_target();
        let config = stage_immutable_restore_config(
            &runtime,
            "dbs:\n  - path: /tmp/kosh.sqlite3\n",
            &replica_path,
        )
        .expect("stage recovery config");
        binary
            .reverify_before_spawn()
            .expect("immutable recovery binary");
        config
            .reverify_before_spawn()
            .expect("immutable recovery config");
        drop(config);
        drop(binary);
        runtime
            .remove_ephemeral_recovery_runtime(&root)
            .expect("remove ephemeral recovery runtime");
        assert!(!root.exists());
    }

    #[test]
    fn ephemeral_recovery_cleanup_refuses_unexpected_contents_without_removing_them() {
        let root = PathBuf::from(format!("/tmp/kosh-r-{}", uuid::Uuid::now_v7()));
        let runtime = LitestreamRuntimePaths::new(&root).expect("ephemeral recovery paths");
        runtime.prepare().expect("prepare recovery runtime");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("private recovery root");
        let unexpected = runtime.directory().join("operator-file");
        fs::write(&unexpected, b"keep").expect("unexpected recovery file");
        assert!(runtime.remove_ephemeral_recovery_runtime(&root).is_err());
        assert_eq!(
            fs::read(&unexpected).expect("preserved unexpected file"),
            b"keep"
        );
        fs::remove_file(unexpected).expect("remove fixture");
        runtime
            .remove_ephemeral_recovery_runtime(&root)
            .expect("cleanup fixture");
    }

    #[test]
    fn next_ephemeral_runtime_reclaims_an_authenticated_interrupted_runtime() {
        let mut interrupted =
            EphemeralLitestreamRuntime::create().expect("interrupted ephemeral runtime");
        let interrupted_root = interrupted.root.clone();
        interrupted
            .paths()
            .prepare()
            .expect("prepare interrupted runtime");
        fs::write(
            interrupted.source_database_path(),
            b"private recovery bytes",
        )
        .expect("write interrupted recovery database");
        interrupted.cleaned = true;
        drop(interrupted);
        assert!(
            interrupted_root.exists(),
            "fixture must model a process that exited without Drop cleanup"
        );

        let mut retry = EphemeralLitestreamRuntime::create().expect("retry ephemeral runtime");
        assert!(
            !interrupted_root.exists(),
            "the authenticated unlocked runtime and its database copy must be reclaimed"
        );
        retry.cleanup().expect("cleanup retry runtime");
    }

    #[test]
    fn next_ephemeral_runtime_finishes_an_interrupted_quarantine_cleanup() {
        let mut interrupted =
            EphemeralLitestreamRuntime::create().expect("interrupted ephemeral runtime");
        let original_root = interrupted.root.clone();
        let quarantine_root =
            original_root.with_file_name(format!("kosh-c-{}", interrupted.ownership.token));
        interrupted
            .paths()
            .prepare()
            .expect("prepare interrupted runtime");
        fs::write(
            interrupted.source_database_path(),
            b"private recovery bytes",
        )
        .expect("write interrupted recovery database");
        fs::rename(&original_root, &quarantine_root).expect("model interrupted quarantine rename");
        interrupted.root.clone_from(&quarantine_root);
        interrupted.ownership.root.clone_from(&quarantine_root);
        interrupted.cleaned = true;
        drop(interrupted);

        let mut retry = EphemeralLitestreamRuntime::create().expect("retry ephemeral runtime");
        assert!(
            !quarantine_root.exists(),
            "an authenticated cleanup quarantine must remain retryable"
        );
        retry.cleanup().expect("cleanup retry runtime");
    }

    #[test]
    fn quarantined_runtime_cleanup_never_unlinks_a_replacement_root() {
        let mut interrupted =
            EphemeralLitestreamRuntime::create().expect("interrupted ephemeral runtime");
        let original_root = interrupted.root.clone();
        let quarantine_root =
            original_root.with_file_name(format!("kosh-c-{}", interrupted.ownership.token));
        let displaced_root =
            original_root.with_file_name(format!("displaced-{}", interrupted.ownership.token));
        interrupted
            .paths()
            .prepare()
            .expect("prepare interrupted runtime");
        fs::write(
            interrupted.source_database_path(),
            b"private recovery bytes",
        )
        .expect("write interrupted recovery database");
        fs::rename(&original_root, &quarantine_root).expect("quarantine interrupted runtime");
        let quarantined_runtime =
            LitestreamRuntimePaths::new(&quarantine_root).expect("quarantined runtime paths");

        let replacement_bytes = b"preserve replacement database";
        let cleanup = quarantined_runtime.remove_ephemeral_recovery_runtime_bound_with_hook(
            &quarantine_root,
            &interrupted.ownership.directory,
            Some(&interrupted.ownership.marker),
            || {
                fs::rename(&quarantine_root, &displaced_root)
                    .expect("displace authenticated quarantine");
                let mut builder = fs::DirBuilder::new();
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
                builder
                    .create(&quarantine_root)
                    .expect("replacement quarantine root");
                fs::write(
                    quarantine_root.join("remote-kosh.sqlite3"),
                    replacement_bytes,
                )
                .expect("replacement database bytes");
            },
        );
        assert!(
            cleanup.is_err(),
            "the final parent-bound removal must reject the replacement name"
        );
        assert_eq!(
            fs::read(quarantine_root.join("remote-kosh.sqlite3"))
                .expect("preserved replacement database"),
            replacement_bytes
        );
        assert_eq!(
            fs::read_dir(&displaced_root)
                .expect("descriptor-owned displaced quarantine")
                .count(),
            0,
            "cleanup must operate only through the authenticated directory descriptor"
        );

        interrupted.cleaned = true;
        drop(interrupted);
        fs::remove_dir(displaced_root).expect("remove displaced fixture");
        fs::remove_file(quarantine_root.join("remote-kosh.sqlite3"))
            .expect("remove replacement fixture");
        fs::remove_dir(quarantine_root).expect("remove replacement root");
    }

    #[test]
    fn ephemeral_runtime_reclamation_skips_active_and_replacement_roots() {
        let mut active = EphemeralLitestreamRuntime::create().expect("active runtime");
        let active_root = active.root.clone();
        let mut peer = EphemeralLitestreamRuntime::create().expect("peer runtime");
        assert!(
            active_root.exists(),
            "the held ownership lock must protect an active runtime"
        );
        peer.cleanup().expect("cleanup peer runtime");

        let replacement_root = PathBuf::from(format!("/tmp/kosh-r-{}", uuid::Uuid::now_v7()));
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        builder.create(&replacement_root).expect("replacement root");
        let replacement_database = replacement_root.join("remote-kosh.sqlite3");
        fs::write(&replacement_database, b"preserve unauthenticated bytes")
            .expect("replacement bytes");
        let mut retry = EphemeralLitestreamRuntime::create().expect("retry runtime");
        assert_eq!(
            fs::read(&replacement_database).expect("preserved replacement"),
            b"preserve unauthenticated bytes"
        );

        retry.cleanup().expect("cleanup retry");
        active.cleanup().expect("cleanup active");
        fs::remove_file(replacement_database).expect("remove replacement fixture");
        fs::remove_dir(replacement_root).expect("remove replacement root");
    }

    #[cfg(unix)]
    #[test]
    fn private_restore_cleanup_never_follows_a_replacement_directory_symlink() {
        let root = tempfile::tempdir().expect("restore root");
        let requested = root.path().join("restored.sqlite3");
        let destination =
            ExclusiveRestoreDestination::prepare(&requested).expect("private destination");
        fs::write(destination.private_path(), b"owned partial restore")
            .expect("owned partial restore");
        let private_directory = destination
            .private_path()
            .parent()
            .expect("private restore parent")
            .to_owned();
        let displaced = root.path().join("displaced-private-restore");
        fs::rename(&private_directory, &displaced).expect("displace private restore");

        let replacement = root.path().join("replacement-private-restore");
        fs::create_dir(&replacement).expect("replacement directory");
        let replacement_database = replacement.join("database.sqlite3");
        fs::write(&replacement_database, b"replacement-must-not-change")
            .expect("replacement database");
        std::os::unix::fs::symlink(&replacement, &private_directory)
            .expect("replacement directory symlink");

        drop(destination);

        assert_eq!(
            fs::read(&replacement_database).expect("preserved replacement database"),
            b"replacement-must-not-change"
        );
        assert!(
            fs::symlink_metadata(&private_directory)
                .expect("replacement symlink remains")
                .file_type()
                .is_symlink(),
            "cleanup must not unlink a substituted path"
        );
        assert!(
            !displaced.join("database.sqlite3").exists(),
            "cleanup must remove the partial restore only through its retained directory descriptor"
        );
    }

    #[test]
    fn offline_restore_supplies_credentials_only_through_stdin_and_checks_exact_results() {
        let root = tempfile::tempdir().expect("restore command root");
        let binary = staged_restore_test_binary(
            root.path(),
            br#"#!/bin/sh
set -eu
test "${AWS_SHARED_CREDENTIALS_FILE:-}" = "/dev/fd/0"
test "${AWS_EC2_METADATA_DISABLED:-}" = "true"
test -z "${UNRELATED_SECRET:-}"
IFS= read -r profile
IFS= read -r access_key
IFS= read -r secret_key
test "$profile" = "[default]"
test "$access_key" = "aws_access_key_id = 00000000000000000000000000000000"
test "$secret_key" = "aws_secret_access_key = 0000000000000000000000000000000000000000000000000000000000000000"
dry=0
target=
txid=
source=
while test "$#" -gt 0; do
  case "$1" in
    -config) shift 2 ;;
    -txid) txid=$2; shift 2 ;;
    -dry-run) dry=1; shift ;;
    -integrity-check) test "$2" = "full"; shift 2 ;;
    -json) shift ;;
    -o) target=$2; shift 2 ;;
    restore) shift ;;
    -*) exit 92 ;;
    *) source=$1; shift ;;
  esac
done
if test "$dry" = 1; then
  printf '{"source":"%s","target_path":"%s","replica":"s3","min_txid":"%s","max_txid":"%s","files":[{"level":0,"name":"exact.ltx","min_txid":"%s","max_txid":"%s","size":12,"timestamp":"2026-07-30T19:00:00Z"}]}' "$source" "$target" "$txid" "$txid" "$txid" "$txid"
else
  printf 'restored' >"$target"
  printf '{"db_path":"%s","replica":"s3","txid":"%s","duration_ms":1,"integrity_check":"full"}' "$target" "$txid"
fi
"#,
        );
        let source = root.path().join("kosh.sqlite3");
        fs::write(&source, b"source").expect("source");
        let target = root.path().join("restored.sqlite3");
        let runtime =
            LitestreamRuntimePaths::new(&root.path().join("runtime")).expect("restore runtime");
        let (r2_target, replica_path) = restore_test_target();
        let credentials = R2Credentials::new(
            "00000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("credentials");
        let engine = CommandLitestreamRestore::new(
            &binary,
            &runtime,
            &r2_target,
            &replica_path,
            &source,
            &credentials,
            Duration::from_secs(5),
        )
        .expect("bound restore engine");
        let txid = LitestreamTxid::from_local(42);
        assert_ne!(engine.config.path, runtime.config());
        runtime
            .write_config("wrong shared restore config")
            .expect("rewrite shared runtime config");
        assert!(
            fs::write(&engine.config.path, "wrong private restore config").is_err(),
            "private restore config must be immutable"
        );

        let plan = engine
            .preview(&source, &target, txid)
            .expect("credentialed preview");
        assert_eq!(plan.max_txid, txid);
        let result = engine
            .restore(&source, &target, txid)
            .expect("credentialed restore");
        assert_eq!(result.txid, txid);
        assert_eq!(fs::read(target).expect("restore bytes"), b"restored");
        remove_partial_restore_config(
            engine
                .config
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore test config");
        remove_partial_stage(binary.path().parent().expect("immutable directory"))
            .expect("remove immutable restore test binary");
    }

    #[cfg(unix)]
    #[test]
    fn offline_restore_rejects_a_dangling_destination_symlink_without_spawning() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("restore command root");
        let marker = root.path().join("litestream-was-launched");
        let binary = staged_restore_test_binary(
            root.path(),
            format!("#!/bin/sh\n: > '{}'\n", marker.display()).as_bytes(),
        );
        let source = root.path().join("kosh.sqlite3");
        fs::write(&source, b"source").expect("source");
        let outside = root.path().join("outside.sqlite3");
        let target = root.path().join("restored.sqlite3");
        symlink(&outside, &target).expect("dangling destination symlink");
        let runtime =
            LitestreamRuntimePaths::new(&root.path().join("runtime")).expect("restore runtime");
        let (r2_target, replica_path) = restore_test_target();
        let credentials = R2Credentials::new(
            "00000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("credentials");
        let engine = CommandLitestreamRestore::new(
            &binary,
            &runtime,
            &r2_target,
            &replica_path,
            &source,
            &credentials,
            Duration::from_secs(5),
        )
        .expect("bound restore engine");

        assert!(matches!(
            engine.restore(&source, &target, LitestreamTxid::from_local(42)),
            Err(LitestreamError::RestoreDestinationExists)
        ));
        assert!(!marker.exists(), "Litestream must not be spawned");
        assert!(
            fs::symlink_metadata(&target)
                .expect("destination symlink")
                .file_type()
                .is_symlink(),
            "the caller-owned destination must remain untouched"
        );
        assert!(
            !outside.exists(),
            "the symlink target must remain untouched"
        );

        remove_partial_restore_config(
            engine
                .config
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore test config");
        remove_partial_stage(binary.path().parent().expect("immutable directory"))
            .expect("remove immutable restore test binary");
    }

    #[cfg(unix)]
    #[test]
    fn offline_restore_exclusively_publishes_after_a_destination_symlink_race() {
        let root = tempfile::tempdir().expect("restore command root");
        let source = root.path().join("kosh.sqlite3");
        fs::write(&source, b"source").expect("source");
        let outside = root.path().join("outside.sqlite3");
        fs::write(&outside, b"outside-must-not-change").expect("outside fixture");
        let target = root.path().join("restored.sqlite3");
        let binary = staged_restore_test_binary(
            root.path(),
            format!(
                r#"#!/bin/sh
set -eu
IFS= read -r _
IFS= read -r _
IFS= read -r _
target=
txid=
while test "$#" -gt 0; do
  case "$1" in
    -config) shift 2 ;;
    -txid) txid=$2; shift 2 ;;
    -integrity-check) test "$2" = "full"; shift 2 ;;
    -json) shift ;;
    -o) target=$2; shift 2 ;;
    restore) shift ;;
    -*) exit 92 ;;
    *) shift ;;
  esac
done
test "$target" != '{}'
printf 'restored' >"$target"
sleep 1
printf '{{"db_path":"%s","replica":"s3","txid":"%s","duration_ms":1,"integrity_check":"full"}}' "$target" "$txid"
"#,
                target.display()
            )
            .as_bytes(),
        );
        let runtime =
            LitestreamRuntimePaths::new(&root.path().join("runtime")).expect("restore runtime");
        let (r2_target, replica_path) = restore_test_target();
        let credentials = R2Credentials::new(
            "00000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("credentials");
        let engine = CommandLitestreamRestore::new(
            &binary,
            &runtime,
            &r2_target,
            &replica_path,
            &source,
            &credentials,
            Duration::from_secs(5),
        )
        .expect("bound restore engine");

        let raced_target = target.clone();
        let raced_outside = outside.clone();
        let restore_root = root.path().to_owned();
        let racer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let private_output_exists = fs::read_dir(&restore_root)
                    .expect("restore root entries")
                    .filter_map(Result::ok)
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".kosh-litestream-restore-")
                            && entry.path().join("database.sqlite3").is_file()
                    });
                if private_output_exists {
                    std::os::unix::fs::symlink(&raced_outside, &raced_target)
                        .expect("race destination symlink");
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "private restore output was not observed"
                );
                thread::sleep(Duration::from_millis(5));
            }
        });
        let restore_result = engine.restore(&source, &target, LitestreamTxid::from_local(42));
        racer.join().expect("destination racer");
        assert!(matches!(
            restore_result,
            Err(LitestreamError::RestoreDestinationExists)
        ));
        assert!(
            fs::symlink_metadata(&target)
                .expect("raced destination symlink")
                .file_type()
                .is_symlink(),
            "exclusive publication must not replace a raced-in node"
        );
        assert_eq!(
            fs::read(&outside).expect("outside bytes"),
            b"outside-must-not-change"
        );
        assert!(
            fs::read_dir(root.path())
                .expect("restore root entries")
                .all(|entry| {
                    !entry
                        .expect("restore root entry")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".kosh-litestream-restore-")
                }),
            "failed publication must clean its private restore directory"
        );

        fs::remove_file(&target).expect("remove raced symlink");
        remove_partial_restore_config(
            engine
                .config
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore test config");
        remove_partial_stage(binary.path().parent().expect("immutable directory"))
            .expect("remove immutable restore test binary");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn offline_restore_sandbox_blocks_a_private_output_symlink_escape() {
        let root = tempfile::tempdir().expect("restore command root");
        let source = root.path().join("kosh.sqlite3");
        fs::write(&source, b"source").expect("source");
        let outside = root.path().join("outside.sqlite3");
        fs::write(&outside, b"outside-must-not-change").expect("outside fixture");
        let target = root.path().join("restored.sqlite3");
        let binary = staged_restore_test_binary(
            root.path(),
            format!(
                r#"#!/bin/sh
set -eu
IFS= read -r _
IFS= read -r _
IFS= read -r _
target=
while test "$#" -gt 0; do
  case "$1" in
    -config|-txid|-integrity-check|-o)
      if test "$1" = "-o"; then target=$2; fi
      shift 2
      ;;
    -json|restore) shift ;;
    -*) exit 92 ;;
    *) shift ;;
  esac
done
ln -s '{}' "$target"
printf 'escaped' >"$target"
"#,
                outside.display()
            )
            .as_bytes(),
        );
        let runtime =
            LitestreamRuntimePaths::new(&root.path().join("runtime")).expect("restore runtime");
        let (r2_target, replica_path) = restore_test_target();
        let credentials = R2Credentials::new(
            "00000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("credentials");
        let engine = CommandLitestreamRestore::new(
            &binary,
            &runtime,
            &r2_target,
            &replica_path,
            &source,
            &credentials,
            Duration::from_secs(5),
        )
        .expect("bound restore engine");

        assert!(matches!(
            engine.restore(&source, &target, LitestreamTxid::from_local(42)),
            Err(LitestreamError::CommandFailed { .. })
        ));
        assert_eq!(
            fs::read(&outside).expect("outside bytes"),
            b"outside-must-not-change"
        );
        assert!(!target.exists(), "failed restore must not be published");
        assert!(
            fs::read_dir(root.path())
                .expect("restore root entries")
                .all(|entry| {
                    !entry
                        .expect("restore root entry")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".kosh-litestream-restore-")
                }),
            "sandboxed failure must clean its private restore directory"
        );

        remove_partial_restore_config(
            engine
                .config
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore test config");
        remove_partial_stage(binary.path().parent().expect("immutable directory"))
            .expect("remove immutable restore test binary");
    }

    #[test]
    fn offline_restore_reverifies_the_immutable_binary_before_credentials_are_sent() {
        let root = tempfile::tempdir().expect("restore command root");
        let binary =
            staged_restore_test_binary(root.path(), b"#!/bin/sh\ncat >/dev/null\nexit 0\n");
        let marker = root.path().join("replacement-was-launched");
        let source = root.path().join("kosh.sqlite3");
        fs::write(&source, b"source").expect("source");
        let target = root.path().join("restored.sqlite3");
        let runtime =
            LitestreamRuntimePaths::new(&root.path().join("runtime")).expect("restore runtime");
        let (r2_target, replica_path) = restore_test_target();
        let credentials = R2Credentials::new(
            "00000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("credentials");
        let engine = CommandLitestreamRestore::new(
            &binary,
            &runtime,
            &r2_target,
            &replica_path,
            &source,
            &credentials,
            Duration::from_secs(5),
        )
        .expect("bound restore engine");

        thaw_restore_test_binary(&binary);
        fs::write(
            binary.path(),
            format!("#!/bin/sh\n: > '{}'\n", marker.display()),
        )
        .expect("replace staged binary");
        assert!(matches!(
            engine.preview(&source, &target, LitestreamTxid::from_local(42)),
            Err(LitestreamError::BinarySizeMismatch
                | LitestreamError::BinaryChecksumMismatch
                | LitestreamError::InvalidStagedBinary)
        ));
        assert!(!marker.exists(), "replacement process must never launch");

        remove_partial_restore_config(
            engine
                .config
                .path
                .parent()
                .expect("immutable config directory"),
        )
        .expect("remove immutable restore test config");
        remove_partial_stage(binary.path().parent().expect("immutable directory"))
            .expect("remove modified restore test binary");
    }

    #[test]
    fn rendered_config_has_fixed_safe_protocol_settings_and_no_secret() {
        let directory = tempfile::tempdir().expect("data root");
        let runtime = LitestreamRuntimePaths::new(directory.path()).expect("runtime paths");
        let config = LitestreamConfig {
            database_path: &directory.path().join("kosh.sqlite3"),
            runtime: &runtime,
            bucket: "kosh-local",
            replica_path: "kosh/primary/litestream/v1/epoch/kosh.sqlite3",
            endpoint: "https://account.r2.cloudflarestorage.com",
        }
        .render()
        .expect("config");
        for required in [
            "l0-retention: 720h",
            "l0-retention-check-interval: 1m",
            "auto-recover: false",
            "verify-compaction: true",
            "permissions: 0600",
            "snapshot:\n  interval: 6h\n  retention: 720h",
        ] {
            assert!(config.contains(required), "missing {required}");
        }
        assert!(!config.contains("access-key-id"));
        assert!(!config.contains("secret-access-key"));
        assert!(!config.contains("secret-value"));
        assert!(!config.contains("access-key-value"));
    }

    #[test]
    fn config_rejects_relative_paths_control_characters_and_plain_http() {
        let directory = tempfile::tempdir().expect("data root");
        let runtime = LitestreamRuntimePaths::new(directory.path()).expect("runtime paths");
        let relative = LitestreamConfig {
            database_path: Path::new("kosh.sqlite3"),
            runtime: &runtime,
            bucket: "kosh-local",
            replica_path: "kosh/primary",
            endpoint: "https://account.r2.cloudflarestorage.com",
        };
        assert!(matches!(
            relative.render(),
            Err(LitestreamError::RelativeDatabasePath)
        ));

        let plain_http = LitestreamConfig {
            database_path: &directory.path().join("kosh.sqlite3"),
            runtime: &runtime,
            bucket: "kosh-local",
            replica_path: "kosh/primary",
            endpoint: "http://account.r2.cloudflarestorage.com",
        };
        assert!(matches!(
            plain_http.render(),
            Err(LitestreamError::InvalidConfigField("endpoint"))
        ));

        let control = LitestreamConfig {
            database_path: &directory.path().join("kosh.sqlite3"),
            runtime: &runtime,
            bucket: "kosh\nother",
            replica_path: "kosh/primary",
            endpoint: "https://account.r2.cloudflarestorage.com",
        };
        assert!(matches!(
            control.render(),
            Err(LitestreamError::InvalidConfigField("bucket"))
        ));
    }

    #[test]
    fn runtime_files_are_private_and_socket_paths_are_bounded() {
        let directory = tempfile::tempdir().expect("data root");
        let runtime = LitestreamRuntimePaths::new(directory.path()).expect("runtime paths");
        runtime.write_config("test: true\n").expect("write config");
        assert_eq!(
            fs::metadata(runtime.directory())
                .expect("runtime metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(runtime.config())
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let too_long = directory.path().join("x".repeat(200));
        assert!(matches!(
            LitestreamRuntimePaths::new(&too_long),
            Err(LitestreamError::ControlSocketPathTooLong)
        ));
    }

    #[test]
    fn config_writes_reject_symlinked_runtime_paths_without_touching_targets() {
        let directory = tempfile::tempdir().expect("data root");
        let runtime = LitestreamRuntimePaths::new(directory.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        let target = directory.path().join("must-not-change");
        fs::write(&target, b"retained").expect("symlink target");
        symlink(&target, runtime.directory().join("ls.yml.tmp")).expect("temporary symlink");

        assert!(matches!(
            runtime.write_config("unsafe: false\n"),
            Err(LitestreamError::WriteConfig(_))
        ));
        assert_eq!(fs::read(&target).expect("unchanged target"), b"retained");
        assert!(fs::symlink_metadata(runtime.directory().join("ls.yml.tmp"))
            .expect("rejected symlink")
            .file_type()
            .is_symlink());

        fs::remove_file(runtime.directory().join("ls.yml.tmp")).expect("remove test symlink");
        fs::remove_dir_all(directory.path().join("run")).expect("remove runtime");
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).expect("outside directory");
        symlink(&outside, directory.path().join("run")).expect("run-directory symlink");
        assert!(matches!(
            runtime.prepare(),
            Err(LitestreamError::PrepareRuntime(_))
        ));
        assert!(!outside.join("backup").exists());
    }

    #[test]
    fn control_commands_use_only_private_socket_and_absolute_database() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let executor = FakeExecutor {
            calls: calls.clone(),
            result: CommandResult {
                exit_code: Some(0),
                stdout: br#"{"db_path":"/tmp/kosh.sqlite3","txid":2,"duration_ms":8}"#.to_vec(),
            },
        };
        let control = CommandLitestreamControl::new(
            PathBuf::from("/app/bin/litestream"),
            PathBuf::from("/private/runtime/ls.sock"),
            60,
            executor,
        );
        control
            .sync_local(Path::new("/tmp/kosh.sqlite3"))
            .expect("local sync");
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].arguments,
            [
                "sync",
                "-json",
                "-socket",
                "/private/runtime/ls.sock",
                "/tmp/kosh.sqlite3",
            ]
            .map(OsString::from)
        );
        assert!(control.sync_local(Path::new("relative.sqlite3")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn timed_control_commands_are_killed_and_reaped() {
        let directory = tempfile::tempdir().expect("control runtime");
        let binary = directory.path().join("blocking-litestream");
        let pid_path = directory.path().join("control.pid");
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nwhile :; do :; done\n",
                pid_path.display()
            ),
        )
        .expect("blocking control script");
        let mut permissions = fs::metadata(&binary)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions).expect("script permissions");
        let control = CommandLitestreamControl::new(
            binary,
            directory.path().join("litestream.sock"),
            60,
            SystemCommandExecutor,
        );
        let database_path = directory.path().join("kosh.sqlite3");

        for remote in [false, true] {
            let error = if remote {
                control.sync_remote_with_timeout(&database_path, Duration::from_secs(1))
            } else {
                control.sync_local_with_timeout(&database_path, Duration::from_secs(1))
            }
            .expect_err("blocking command must time out");
            assert!(matches!(
                error,
                LitestreamError::Execute(ref source)
                    if source.kind() == std::io::ErrorKind::TimedOut
            ));

            let pid = fs::read_to_string(&pid_path)
                .expect("control pid")
                .trim()
                .parse::<i32>()
                .expect("numeric control pid");
            let result = unsafe { libc::kill(pid, 0) };
            assert_eq!(result, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    #[test]
    fn embedded_manifest_records_the_verified_protocol() {
        let manifest = embedded_manifest().expect("embedded manifest");
        validate_protocol_manifest(&manifest).expect("safe protocol manifest");
    }

    #[test]
    fn cleanup_pin_registry_accepts_prior_releases_and_rejects_unsafe_sets() {
        let manifest = embedded_manifest().expect("embedded manifest");
        let current_sha256 = manifest.binary.universal.sha256.clone();
        assert_eq!(
            manifest.binary.trusted_cleanup_sha256s,
            vec![current_sha256.clone()],
            "the first release starts the append-only cleanup registry"
        );
        assert_eq!(
            VerifiedLitestreamBinary::trusted_cleanup_sha256s().expect("embedded cleanup registry"),
            manifest.binary.trusted_cleanup_sha256s,
            "cleanup authentication must not require a staged launch binary"
        );

        let mut upgraded = manifest.clone();
        upgraded
            .binary
            .trusted_cleanup_sha256s
            .insert(0, "0".repeat(64));
        validate_protocol_manifest(&upgraded).expect("prior and current pins");

        let mut missing_current = manifest.clone();
        missing_current.binary.trusted_cleanup_sha256s = vec!["0".repeat(64)];
        assert!(matches!(
            validate_protocol_manifest(&missing_current),
            Err(LitestreamError::UnsafeProtocolPin)
        ));

        let mut duplicate = manifest.clone();
        duplicate
            .binary
            .trusted_cleanup_sha256s
            .push(current_sha256);
        assert!(matches!(
            validate_protocol_manifest(&duplicate),
            Err(LitestreamError::UnsafeProtocolPin)
        ));
    }

    #[test]
    fn release_manifest_must_match_the_embedded_source_and_binary_pin() {
        let directory = tempfile::tempdir().expect("resource root");
        let release_directory = directory.path().join("release");
        fs::create_dir(&release_directory).expect("release directory");
        let release_path = release_directory.join("litestream.json");
        let source = embedded_manifest().expect("source manifest");
        let mut release: serde_json::Value =
            serde_json::from_str(EMBEDDED_MANIFEST).expect("source value");
        release.as_object_mut().expect("manifest object").insert(
            "stagedBinary".into(),
            serde_json::json!({
                "bundlePath": "bin/litestream",
                "sha256": source.binary.universal.sha256.clone(),
                "size": source.binary.universal.size,
                "architectures": ["arm64", "x86_64"],
                "versionOutputByArchitecture": {
                    "arm64": "0.5.15",
                    "x86_64": "0.5.15"
                }
            }),
        );
        release["verification"]["architectureChecks"] = serde_json::json!([
            {
                "architecture": "arm64",
                "executable": true,
                "systemLibrariesOnly": true
            },
            {
                "architecture": "x86_64",
                "executable": true,
                "systemLibrariesOnly": true
            }
        ]);
        fs::write(
            &release_path,
            serde_json::to_vec_pretty(&release).expect("release JSON"),
        )
        .expect("release manifest");
        verify_release_manifest(directory.path(), &source).expect("matching release");

        release["component"] = serde_json::json!("other");
        fs::write(
            &release_path,
            serde_json::to_vec_pretty(&release).expect("mismatched JSON"),
        )
        .expect("mismatched release manifest");
        assert!(matches!(
            verify_release_manifest(directory.path(), &source),
            Err(LitestreamError::ReleaseManifestMismatch)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn binary_verification_rejects_permissions_bytes_and_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("binary root");
        let binary = directory.path().join("litestream");
        let bytes = b"pinned-litestream";
        fs::write(&binary, bytes).expect("binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("executable permissions");
        let manifest = BinaryPin {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            code_signature_identifier: "com.rohan.kosh.litestream".into(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        };
        verify_binary(&binary, &manifest).expect("matching binary");

        fs::set_permissions(&binary, fs::Permissions::from_mode(0o600))
            .expect("nonexecutable permissions");
        assert!(matches!(
            verify_binary(&binary, &manifest),
            Err(LitestreamError::BinaryNotExecutable)
        ));
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("restored permissions");

        fs::write(&binary, b"pinned-litestreaM").expect("changed binary");
        assert!(matches!(
            verify_binary(&binary, &manifest),
            Err(LitestreamError::BinaryChecksumMismatch)
        ));

        let link = directory.path().join("litestream-link");
        symlink(&binary, &link).expect("binary symlink");
        assert!(matches!(
            verify_binary(&link, &manifest),
            Err(LitestreamError::BinaryNotRegular)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn running_process_identity_is_bound_to_the_kernel_cdhash() {
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("signed test process");
        let actual = running_process_cdhash(child.id()).expect("running process CDHash");
        assert_eq!(actual.len(), 40);
        let matching = ImmutableLitestreamBinary {
            path: PathBuf::from("/bin/sleep"),
            sha256: "0".repeat(64),
            size: 1,
            code_signature_cdhash: actual,
        };
        matching
            .verify_running_process(child.id())
            .expect("matching kernel identity");
        let mismatched = ImmutableLitestreamBinary {
            code_signature_cdhash: "0".repeat(40),
            ..matching
        };
        assert!(matches!(
            mismatched.verify_running_process(child.id()),
            Err(LitestreamError::ProcessCodeSignatureMismatch)
        ));
        child.kill().expect("terminate test process");
        child.wait().expect("reap test process");
    }

    #[cfg(unix)]
    #[test]
    fn immutable_launch_stage_reuses_verified_bytes_and_rejects_path_replacement() {
        let directory = tempfile::tempdir().expect("binary root");
        let source_path = directory.path().join("source-litestream");
        let bytes = b"pinned-litestream";
        fs::write(&source_path, bytes).expect("source binary");
        fs::set_permissions(&source_path, fs::Permissions::from_mode(0o700))
            .expect("source permissions");
        let pin = BinaryPin {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            code_signature_identifier: "com.rohan.kosh.litestream".into(),
            code_signature_cdhash_by_architecture: BTreeMap::new(),
        };
        let verified = VerifiedLitestreamBinary {
            path: source_path,
            sha256: pin.sha256.clone(),
            size: pin.size,
            code_signature_cdhash: "0".repeat(40),
            file: verify_binary(&directory.path().join("source-litestream"), &pin)
                .expect("verified source descriptor"),
        };
        let runtime = LitestreamRuntimePaths::new(directory.path()).expect("runtime paths");
        runtime.prepare().expect("prepared runtime");
        let partial_directory = runtime
            .directory()
            .join("verified-litestream")
            .join(&pin.sha256);
        fs::create_dir_all(&partial_directory).expect("partial immutable directory");
        fs::write(partial_directory.join("litestream"), b"partial")
            .expect("partial immutable binary");

        let immutable = verified
            .stage_immutable(&runtime)
            .expect("recovered immutable launch stage");
        immutable
            .reverify_before_spawn()
            .expect("immutable stage verification");
        assert_eq!(
            verified
                .stage_immutable(&runtime)
                .expect("reused immutable launch stage")
                .path(),
            immutable.path()
        );

        let replacement = directory.path().join("replacement");
        fs::write(&replacement, bytes).expect("replacement binary");
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700))
            .expect("replacement permissions");
        assert!(
            fs::rename(&replacement, immutable.path()).is_err(),
            "immutable directory must reject executable replacement"
        );

        let immutable_directory = immutable.path().parent().expect("immutable directory");
        let displaced_directory = directory.path().join("displaced-immutable-stage");
        assert!(
            fs::rename(immutable_directory, &displaced_directory).is_err(),
            "immutable directory must reject parent-path displacement"
        );
        let directory_file =
            File::open(immutable_directory).expect("immutable directory descriptor");
        clear_user_immutable(&directory_file).expect("thaw immutable directory");
        fs::set_permissions(immutable_directory, fs::Permissions::from_mode(0o700))
            .expect("thawed directory permissions");
        let binary_file = File::open(immutable.path()).expect("immutable binary descriptor");
        clear_user_immutable(&binary_file).expect("thaw immutable binary");
        fs::set_permissions(immutable.path(), fs::Permissions::from_mode(0o700))
            .expect("thawed binary permissions");
    }

    #[cfg(unix)]
    #[test]
    fn system_executor_rejects_oversized_output_without_hanging() {
        let directory = tempfile::tempdir().expect("executor root");
        let binary = directory.path().join("oversized-control");
        fs::write(
            &binary,
            "#!/bin/sh\ndd if=/dev/zero bs=70000 count=1 2>/dev/null\n",
        )
        .expect("oversized script");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("script permissions");
        let error = SystemCommandExecutor
            .execute(&CommandSpec {
                program: binary,
                arguments: Vec::new(),
            })
            .expect_err("oversized output must fail");
        assert!(error
            .to_string()
            .contains("control output exceeded its safety bound"));
    }

    #[test]
    fn staged_universal_binary_matches_the_embedded_pin_when_available() {
        let staged =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/release/bin/litestream");
        if staged.exists() {
            VerifiedLitestreamBinary::resolve_staged_for_test(&staged)
                .expect("staged Litestream pin");
        }
    }
}
