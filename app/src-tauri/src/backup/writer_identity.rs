#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    io::Read,
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

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MacOsHardwareWriterIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum WriterIdentityError {
    #[error("device-local backup writer identity is unavailable")]
    Unavailable,
    #[error("device-local backup writer identity is invalid")]
    Invalid,
}

#[cfg(target_os = "macos")]
static DEVICE_WRITER_IDENTITY: OnceLock<BackupWriterId> = OnceLock::new();

#[cfg(target_os = "macos")]
impl WriterIdentityProvider for MacOsHardwareWriterIdentity {
    fn load(&self) -> Result<BackupWriterId, WriterIdentityError> {
        if let Some(identity) = DEVICE_WRITER_IDENTITY.get() {
            return Ok(identity.clone());
        }
        let loaded = load_macos_hardware_writer_identity()?;
        let _ = DEVICE_WRITER_IDENTITY.set(loaded);
        Ok(DEVICE_WRITER_IDENTITY
            .get()
            .expect("writer identity was initialized")
            .clone())
    }
}

#[cfg(not(target_os = "macos"))]
impl WriterIdentityProvider for MacOsHardwareWriterIdentity {
    fn load(&self) -> Result<BackupWriterId, WriterIdentityError> {
        Err(WriterIdentityError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
fn load_macos_hardware_writer_identity() -> Result<BackupWriterId, WriterIdentityError> {
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
    let platform_uuid = parse_platform_uuid_output(&output)?;
    Ok(BackupWriterId::derive_from_device_identifier(
        platform_uuid.as_bytes(),
    ))
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
    fn hardware_binding_is_stable_on_one_device_and_distinct_across_devices() {
        let first = BackupWriterId::derive_from_device_identifier(FIRST_PLATFORM_UUID.as_bytes());
        assert_eq!(
            BackupWriterId::derive_from_device_identifier(FIRST_PLATFORM_UUID.as_bytes()),
            first
        );
        assert_ne!(
            BackupWriterId::derive_from_device_identifier(SECOND_PLATFORM_UUID.as_bytes()),
            first
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_macos_hardware_identity_is_available_without_keychain_or_ui() {
        let identity =
            load_macos_hardware_writer_identity().expect("macOS hardware writer identity");
        assert_eq!(
            BackupWriterId::parse(identity.as_str()).expect("canonical writer identity"),
            identity
        );
    }
}
