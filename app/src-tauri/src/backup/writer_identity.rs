#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use super::domain::BackupWriterId;

const IOREG_PATH: &str = "/usr/sbin/ioreg";
const IO_PLATFORM_UUID_PROPERTY: &str = "IOPlatformUUID";
const MAX_IOREG_OUTPUT_BYTES: usize = 64 * 1024;
const IOREG_TIMEOUT: Duration = Duration::from_secs(2);
const IOREG_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) trait WriterIdentityProvider: Send + Sync {
    fn load(&self) -> Result<BackupWriterId, WriterIdentityError>;
}

#[derive(Debug)]
pub(crate) struct MacOsInstallationWriterIdentity {
    data_root: PathBuf,
    identity: OnceLock<BackupWriterId>,
}

impl MacOsInstallationWriterIdentity {
    pub(crate) fn new(data_root: PathBuf) -> Self {
        Self {
            data_root,
            identity: OnceLock::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum WriterIdentityError {
    #[error("device-local backup writer identity is unavailable")]
    Unavailable,
    #[error("device-local backup writer identity is invalid")]
    Invalid,
}

#[cfg(target_os = "macos")]
static PLATFORM_UUID: OnceLock<String> = OnceLock::new();

#[cfg(target_os = "macos")]
impl WriterIdentityProvider for MacOsInstallationWriterIdentity {
    fn load(&self) -> Result<BackupWriterId, WriterIdentityError> {
        if let Some(identity) = self.identity.get() {
            return Ok(identity.clone());
        }
        let loaded = load_macos_installation_writer_identity(&self.data_root)?;
        let _ = self.identity.set(loaded);
        Ok(self
            .identity
            .get()
            .expect("writer identity was initialized")
            .clone())
    }
}

#[cfg(not(target_os = "macos"))]
impl WriterIdentityProvider for MacOsInstallationWriterIdentity {
    fn load(&self) -> Result<BackupWriterId, WriterIdentityError> {
        Err(WriterIdentityError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
fn load_macos_installation_writer_identity(
    data_root: &Path,
) -> Result<BackupWriterId, WriterIdentityError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(data_root).map_err(|_| WriterIdentityError::Unavailable)?;
    if !metadata.is_dir() {
        return Err(WriterIdentityError::Invalid);
    }
    let platform_uuid = load_platform_uuid()?;
    Ok(derive_scoped_writer_identity(
        platform_uuid.as_bytes(),
        metadata.dev(),
        metadata.ino(),
    ))
}

#[cfg(target_os = "macos")]
fn load_platform_uuid() -> Result<String, WriterIdentityError> {
    if let Some(platform_uuid) = PLATFORM_UUID.get() {
        return Ok(platform_uuid.clone());
    }
    let loaded = read_platform_uuid()?;
    let _ = PLATFORM_UUID.set(loaded);
    Ok(PLATFORM_UUID
        .get()
        .expect("platform UUID was initialized")
        .clone())
}

#[cfg(target_os = "macos")]
fn read_platform_uuid() -> Result<String, WriterIdentityError> {
    let mut child = Command::new(IOREG_PATH)
        .args([
            "-rd1",
            "-c",
            "IOPlatformExpertDevice",
            "-k",
            IO_PLATFORM_UUID_PROPERTY,
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| WriterIdentityError::Unavailable)?;
    let deadline = Instant::now() + IOREG_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(IOREG_POLL_INTERVAL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WriterIdentityError::Unavailable);
            }
        }
    };
    if !status.success() {
        return Err(WriterIdentityError::Unavailable);
    }
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or(WriterIdentityError::Unavailable)?
        .take(MAX_IOREG_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| WriterIdentityError::Unavailable)?;
    if output.len() > MAX_IOREG_OUTPUT_BYTES {
        return Err(WriterIdentityError::Unavailable);
    }
    parse_platform_uuid_output(&output)
}

fn derive_scoped_writer_identity(
    platform_uuid: &[u8],
    filesystem_device: u64,
    directory_inode: u64,
) -> BackupWriterId {
    let mut material = Vec::with_capacity(platform_uuid.len() + 17);
    material.extend_from_slice(platform_uuid);
    material.push(0);
    material.extend_from_slice(&filesystem_device.to_be_bytes());
    material.extend_from_slice(&directory_inode.to_be_bytes());
    BackupWriterId::derive_from_device_identifier(&material)
}

fn parse_platform_uuid_output(output: &[u8]) -> Result<String, WriterIdentityError> {
    let output = std::str::from_utf8(output).map_err(|_| WriterIdentityError::Invalid)?;
    let prefix = format!("\"{IO_PLATFORM_UUID_PROPERTY}\" = \"");
    let mut matches = output.lines().filter_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?.strip_suffix('"')?;
        Some(value)
    });
    let value = matches.next().ok_or(WriterIdentityError::Invalid)?;
    if matches.next().is_some() {
        return Err(WriterIdentityError::Invalid);
    }
    let parsed = Uuid::parse_str(value).map_err(|_| WriterIdentityError::Invalid)?;
    if parsed.hyphenated().to_string().to_ascii_uppercase() != value {
        return Err(WriterIdentityError::Invalid);
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_PLATFORM_UUID: &str = "046EE8A6-AE65-5DB2-9194-24B9C73DC61A";
    const SECOND_PLATFORM_UUID: &str = "C8D20AE7-CFA4-5E77-BF7B-4F853769B935";

    #[test]
    fn platform_uuid_parser_is_exact_bounded_and_canonical() {
        let output = format!(
            "+-o IOPlatformExpertDevice\n    {{\n      \"IOPlatformUUID\" = \"{FIRST_PLATFORM_UUID}\"\n    }}\n"
        );
        assert_eq!(
            parse_platform_uuid_output(output.as_bytes()).expect("platform UUID"),
            FIRST_PLATFORM_UUID
        );
        assert_eq!(
            parse_platform_uuid_output(
                format!(
                    "\"IOPlatformUUID\" = \"{FIRST_PLATFORM_UUID}\"\n\"IOPlatformUUID\" = \"{SECOND_PLATFORM_UUID}\"\n"
                )
                .as_bytes()
            ),
            Err(WriterIdentityError::Invalid)
        );
        assert_eq!(
            parse_platform_uuid_output(
                format!(
                    "\"IOPlatformUUID\" = \"{}\"\n",
                    FIRST_PLATFORM_UUID.to_ascii_lowercase()
                )
                .as_bytes()
            ),
            Err(WriterIdentityError::Invalid)
        );
        assert_eq!(
            parse_platform_uuid_output(b"no platform identifier"),
            Err(WriterIdentityError::Invalid)
        );
    }

    #[test]
    fn installation_binding_is_stable_and_distinguishes_devices_and_profiles() {
        let first = derive_scoped_writer_identity(FIRST_PLATFORM_UUID.as_bytes(), 7, 11);
        assert_eq!(
            derive_scoped_writer_identity(FIRST_PLATFORM_UUID.as_bytes(), 7, 11),
            first
        );
        assert_ne!(
            derive_scoped_writer_identity(SECOND_PLATFORM_UUID.as_bytes(), 7, 11),
            first
        );
        assert_ne!(
            derive_scoped_writer_identity(FIRST_PLATFORM_UUID.as_bytes(), 7, 12),
            first
        );
        assert_ne!(
            derive_scoped_writer_identity(FIRST_PLATFORM_UUID.as_bytes(), 8, 11),
            first
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copied_profile_gets_a_distinct_live_identity_without_keychain_or_ui() {
        let first = tempfile::tempdir().expect("first profile");
        let second = tempfile::tempdir().expect("second profile");
        let identity = load_macos_installation_writer_identity(first.path())
            .expect("first installation writer identity");
        assert_eq!(
            BackupWriterId::parse(identity.as_str()).expect("canonical writer identity"),
            identity
        );
        assert_ne!(
            load_macos_installation_writer_identity(second.path())
                .expect("second installation writer identity"),
            identity
        );
    }
}
