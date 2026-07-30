use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::credentials::R2Credentials;

const EMBEDDED_MANIFEST: &str = include_str!("../../resources/sidecars/litestream-v1.json");
const DEVELOPMENT_BINARY_OVERRIDE_ENV: &str = "KOSH_LITESTREAM_PATH";
const ACCESS_KEY_ID_ENV: &str = "KOSH_LITESTREAM_R2_ACCESS_KEY_ID";
const SECRET_ACCESS_KEY_ENV: &str = "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY";
const REQUIRED_L0_RETENTION: &str = "720h";
const MAX_CONTROL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_RESTORE_PLAN_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESTORE_FILES: usize = 100_000;
const MAX_MACOS_UNIX_SOCKET_PATH_BYTES: usize = 103;
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) fn configure_credentials_environment(
    command: &mut Command,
    credentials: &R2Credentials,
) {
    command
        .env_clear()
        .env(ACCESS_KEY_ID_ENV, credentials.access_key_id());
    command.env(SECRET_ACCESS_KEY_ENV, credentials.secret_access_key());
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct BinaryPin {
    sha256: String,
    size: u64,
    code_signature_identifier: String,
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
#[derive(Clone, Debug)]
pub struct VerifiedLitestreamBinary {
    path: PathBuf,
    sha256: String,
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

    pub fn resolve(resource_dir: &Path) -> Result<Self, LitestreamError> {
        let manifest = embedded_manifest()?;
        validate_protocol_manifest(&manifest)?;

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
        verify_binary(&path, &manifest.binary.universal)?;
        Ok(Self {
            path,
            sha256: manifest.binary.universal.sha256,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn resolve_staged_for_test(path: &Path) -> Result<Self, LitestreamError> {
        let manifest = embedded_manifest()?;
        validate_protocol_manifest(&manifest)?;
        verify_binary(path, &manifest.binary.universal)?;
        Ok(Self {
            path: path.to_owned(),
            sha256: manifest.binary.universal.sha256,
        })
    }
}

fn embedded_manifest() -> Result<SourceManifest, LitestreamError> {
    serde_json::from_str(EMBEDDED_MANIFEST).map_err(LitestreamError::InvalidEmbeddedManifest)
}

fn validate_protocol_manifest(manifest: &SourceManifest) -> Result<(), LitestreamError> {
    let universal = &manifest.binary.universal;
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

fn verify_binary(path: &Path, manifest: &BinaryPin) -> Result<(), LitestreamError> {
    let symlink_metadata =
        fs::symlink_metadata(path).map_err(|_| LitestreamError::MissingBinary(path.to_owned()))?;
    if symlink_metadata.file_type().is_symlink() || !symlink_metadata.is_file() {
        return Err(LitestreamError::BinaryNotRegular);
    }
    if symlink_metadata.len() != manifest.size {
        return Err(LitestreamError::BinarySizeMismatch);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if symlink_metadata.permissions().mode() & 0o111 == 0 {
            return Err(LitestreamError::BinaryNotExecutable);
        }
    }

    let mut file = File::open(path).map_err(|_| LitestreamError::MissingBinary(path.to_owned()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| LitestreamError::BinaryChecksumMismatch)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != manifest.sha256 {
        return Err(LitestreamError::BinaryChecksumMismatch);
    }
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
      access-key-id: ${{{ACCESS_KEY_ID_ENV}}}
      secret-access-key: ${{{SECRET_ACCESS_KEY_ENV}}}
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
        sync::{Arc, Mutex},
    };

    use super::*;

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
    fn daemon_environment_contains_only_the_two_zeroizing_credentials() {
        let credentials = R2Credentials::new(
            "0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("credentials");
        let mut command = Command::new("/app/bin/litestream");
        command.env("UNRELATED_SECRET", "must-not-survive");

        configure_credentials_environment(&mut command, &credentials);

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
                (
                    ACCESS_KEY_ID_ENV.into(),
                    Some(credentials.access_key_id().into()),
                ),
                (
                    SECRET_ACCESS_KEY_ENV.into(),
                    Some(credentials.secret_access_key().into()),
                ),
            ]
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
            "${KOSH_LITESTREAM_R2_ACCESS_KEY_ID}",
            "${KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY}",
        ] {
            assert!(config.contains(required), "missing {required}");
        }
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
