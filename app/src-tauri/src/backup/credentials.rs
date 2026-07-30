#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::domain::BackupSetId;

const KEYCHAIN_SERVICE: &str = "com.rohan.kosh.offsite-backup.r2";
const CREDENTIAL_FORMAT_VERSION: u32 = 1;
const MAX_CREDENTIAL_PAYLOAD_BYTES: usize = 4 * 1024;

pub(crate) struct R2Credentials {
    access_key_id: Zeroizing<String>,
    secret_access_key: Zeroizing<String>,
}

impl R2Credentials {
    pub(crate) fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Result<Self, CredentialError> {
        let mut access_key_id = access_key_id.into();
        let mut secret_access_key = secret_access_key.into();
        if access_key_id.len() != 32 || !is_lower_hex(&access_key_id) {
            access_key_id.zeroize();
            secret_access_key.zeroize();
            return Err(CredentialError::InvalidCredential("accessKeyId"));
        }
        if secret_access_key.len() != 64 || !is_lower_hex(&secret_access_key) {
            access_key_id.zeroize();
            secret_access_key.zeroize();
            return Err(CredentialError::InvalidCredential("secretAccessKey"));
        }
        Ok(Self {
            access_key_id: Zeroizing::new(access_key_id),
            secret_access_key: Zeroizing::new(secret_access_key),
        })
    }

    pub(crate) fn access_key_id(&self) -> &str {
        self.access_key_id.as_str()
    }

    pub(crate) fn secret_access_key(&self) -> &str {
        self.secret_access_key.as_str()
    }

    fn encode(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        let payload = CredentialPayloadRef {
            format_version: CREDENTIAL_FORMAT_VERSION,
            access_key_id: self.access_key_id(),
            secret_access_key: self.secret_access_key(),
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| CredentialError::CorruptPayload)?;
        if bytes.len() > MAX_CREDENTIAL_PAYLOAD_BYTES {
            return Err(CredentialError::CorruptPayload);
        }
        Ok(Zeroizing::new(bytes))
    }

    fn decode(bytes: &[u8]) -> Result<Self, CredentialError> {
        if bytes.len() > MAX_CREDENTIAL_PAYLOAD_BYTES {
            return Err(CredentialError::CorruptPayload);
        }
        let mut payload: CredentialPayload =
            serde_json::from_slice(bytes).map_err(|_| CredentialError::CorruptPayload)?;
        if payload.format_version != CREDENTIAL_FORMAT_VERSION {
            payload.access_key_id.zeroize();
            payload.secret_access_key.zeroize();
            return Err(CredentialError::UnsupportedPayloadVersion);
        }
        let credentials = Self::new(
            std::mem::take(&mut payload.access_key_id),
            std::mem::take(&mut payload.secret_access_key),
        );
        payload.access_key_id.zeroize();
        payload.secret_access_key.zeroize();
        credentials
    }
}

impl fmt::Debug for R2Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("R2Credentials")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .finish()
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialPayloadRef<'a> {
    format_version: u32,
    access_key_id: &'a str,
    secret_access_key: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialPayload {
    format_version: u32,
    access_key_id: String,
    secret_access_key: String,
}

pub(crate) trait CredentialStore: Send + Sync {
    fn save(
        &self,
        backup_set_id: &BackupSetId,
        credentials: &R2Credentials,
    ) -> Result<(), CredentialError>;
    fn load(&self, backup_set_id: &BackupSetId) -> Result<R2Credentials, CredentialError>;
    fn remove(&self, backup_set_id: &BackupSetId) -> Result<(), CredentialError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MacOsKeychainCredentialStore;

#[cfg(target_os = "macos")]
trait KeychainBackend {
    fn load(&self, service: &str, account: &str) -> Result<Vec<u8>, CredentialError>;
    fn save(&self, service: &str, account: &str, payload: &[u8]) -> Result<(), CredentialError>;
    fn remove(&self, service: &str, account: &str) -> Result<(), CredentialError>;
}

#[cfg(target_os = "macos")]
static KEYCHAIN_OPERATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default)]
struct SystemKeychainBackend;

#[cfg(target_os = "macos")]
impl SystemKeychainBackend {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(service, account).map_err(|_| CredentialError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
impl KeychainBackend for SystemKeychainBackend {
    fn load(&self, service: &str, account: &str) -> Result<Vec<u8>, CredentialError> {
        Self::entry(service, account)?
            .get_secret()
            .map_err(map_keyring_load_error)
    }

    fn save(&self, service: &str, account: &str, payload: &[u8]) -> Result<(), CredentialError> {
        Self::entry(service, account)?
            .set_secret(payload)
            .map_err(|_| CredentialError::Unavailable)
    }

    fn remove(&self, service: &str, account: &str) -> Result<(), CredentialError> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredentialError::Unavailable),
        }
    }
}

#[cfg(target_os = "macos")]
fn save_verified(
    backend: &impl KeychainBackend,
    backup_set_id: &BackupSetId,
    credentials: &R2Credentials,
) -> Result<(), CredentialError> {
    let previous = match backend.load(KEYCHAIN_SERVICE, backup_set_id.as_str()) {
        Ok(payload) => Some(Zeroizing::new(payload)),
        Err(CredentialError::Missing) => None,
        Err(error) => return Err(error),
    };
    let payload = credentials.encode()?;
    if let Err(error) = backend.save(KEYCHAIN_SERVICE, backup_set_id.as_str(), payload.as_slice()) {
        return rollback_and_return(
            backend,
            backup_set_id,
            previous.as_ref().map(|payload| payload.as_slice()),
            error,
        );
    }
    let verified = match backend.load(KEYCHAIN_SERVICE, backup_set_id.as_str()) {
        Ok(payload) => Zeroizing::new(payload),
        Err(error) => {
            return rollback_and_return(
                backend,
                backup_set_id,
                previous.as_ref().map(|payload| payload.as_slice()),
                error,
            );
        }
    };
    if verified.as_slice() != payload.as_slice() {
        return rollback_and_return(
            backend,
            backup_set_id,
            previous.as_ref().map(|payload| payload.as_slice()),
            CredentialError::Unavailable,
        );
    }
    if let Err(error) = R2Credentials::decode(verified.as_slice()) {
        return rollback_and_return(
            backend,
            backup_set_id,
            previous.as_ref().map(|payload| payload.as_slice()),
            error,
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn rollback_and_return(
    backend: &impl KeychainBackend,
    backup_set_id: &BackupSetId,
    previous: Option<&[u8]>,
    original_error: CredentialError,
) -> Result<(), CredentialError> {
    restore_previous(backend, backup_set_id, previous).map_err(|_| CredentialError::Unavailable)?;
    Err(original_error)
}

#[cfg(target_os = "macos")]
fn restore_previous(
    backend: &impl KeychainBackend,
    backup_set_id: &BackupSetId,
    previous: Option<&[u8]>,
) -> Result<(), CredentialError> {
    let account = backup_set_id.as_str();
    match previous {
        Some(previous) => {
            backend.save(KEYCHAIN_SERVICE, account, previous)?;
            let restored = Zeroizing::new(backend.load(KEYCHAIN_SERVICE, account)?);
            if restored.as_slice() != previous {
                return Err(CredentialError::Unavailable);
            }
        }
        None => {
            backend.remove(KEYCHAIN_SERVICE, account)?;
            match backend.load(KEYCHAIN_SERVICE, account) {
                Err(CredentialError::Missing) => {}
                Ok(_) | Err(_) => return Err(CredentialError::Unavailable),
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
impl CredentialStore for MacOsKeychainCredentialStore {
    fn save(
        &self,
        backup_set_id: &BackupSetId,
        credentials: &R2Credentials,
    ) -> Result<(), CredentialError> {
        let _guard = KEYCHAIN_OPERATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        save_verified(&SystemKeychainBackend, backup_set_id, credentials)
    }

    fn load(&self, backup_set_id: &BackupSetId) -> Result<R2Credentials, CredentialError> {
        let _guard = KEYCHAIN_OPERATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let payload =
            Zeroizing::new(SystemKeychainBackend.load(KEYCHAIN_SERVICE, backup_set_id.as_str())?);
        R2Credentials::decode(payload.as_slice())
    }

    fn remove(&self, backup_set_id: &BackupSetId) -> Result<(), CredentialError> {
        let _guard = KEYCHAIN_OPERATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        SystemKeychainBackend.remove(KEYCHAIN_SERVICE, backup_set_id.as_str())
    }
}

#[cfg(target_os = "macos")]
fn map_keyring_load_error(error: keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::Missing,
        _ => CredentialError::Unavailable,
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialStore for MacOsKeychainCredentialStore {
    fn save(
        &self,
        _backup_set_id: &BackupSetId,
        _credentials: &R2Credentials,
    ) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable)
    }

    fn load(&self, _backup_set_id: &BackupSetId) -> Result<R2Credentials, CredentialError> {
        Err(CredentialError::Unavailable)
    }

    fn remove(&self, _backup_set_id: &BackupSetId) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialError {
    #[error("invalid R2 credential field: {0}")]
    InvalidCredential(&'static str),
    #[error("off-site backup credentials are missing")]
    Missing,
    #[error("macOS Keychain is unavailable for off-site backup")]
    Unavailable,
    #[error("saved off-site backup credentials use an unsupported version")]
    UnsupportedPayloadVersion,
    #[error("saved off-site backup credentials are invalid")]
    CorruptPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::{
        collections::HashMap,
        sync::{Mutex, MutexGuard},
    };

    const ACCESS_KEY: &str = "0123456789abcdef0123456789abcdef";
    const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn payload_is_versioned_and_debug_is_always_redacted() {
        let credentials = R2Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials");
        let payload = credentials.encode().expect("payload");
        let decoded = R2Credentials::decode(payload.as_slice()).expect("decoded");
        assert_eq!(decoded.access_key_id(), ACCESS_KEY);
        assert_eq!(decoded.secret_access_key(), SECRET_KEY);
        let debug = format!("{credentials:?}");
        assert!(!debug.contains(ACCESS_KEY));
        assert!(!debug.contains(SECRET_KEY));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn malformed_credentials_and_payloads_fail_closed() {
        assert!(R2Credentials::new("short", SECRET_KEY).is_err());
        assert!(R2Credentials::new(ACCESS_KEY, "short").is_err());
        assert!(R2Credentials::decode(
            br#"{"formatVersion":2,"accessKeyId":"0123456789abcdef0123456789abcdef","secretAccessKey":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#
        )
        .is_err());
        assert!(R2Credentials::decode(
            br#"{"formatVersion":1,"accessKeyId":"0123456789abcdef0123456789abcdef","secretAccessKey":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","extra":true}"#
        )
        .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn save_is_verified_and_failed_verification_rolls_back_keychain_state() {
        let backend = FakeKeychainBackend::default();
        let backup_set_id = BackupSetId::new();
        let credentials = R2Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials");
        save_verified(&backend, &backup_set_id, &credentials).expect("verified save");
        assert!(backend.contains(KEYCHAIN_SERVICE, backup_set_id.as_str()));

        let broken = FakeKeychainBackend::default();
        broken.corrupt_verification_after_next_save();
        assert!(matches!(
            save_verified(&broken, &backup_set_id, &credentials),
            Err(CredentialError::Unavailable)
        ));
        assert!(!broken.contains(KEYCHAIN_SERVICE, backup_set_id.as_str()));

        let replacement = R2Credentials::new(
            "fedcba9876543210fedcba9876543210",
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
        )
        .expect("replacement");
        backend.corrupt_verification_after_next_save();
        assert!(matches!(
            save_verified(&backend, &backup_set_id, &replacement),
            Err(CredentialError::Unavailable)
        ));
        let restored = R2Credentials::decode(
            &backend
                .entry(KEYCHAIN_SERVICE, backup_set_id.as_str())
                .expect("restored previous payload"),
        )
        .expect("restored credentials");
        assert_eq!(restored.access_key_id(), ACCESS_KEY);
        assert_eq!(restored.secret_access_key(), SECRET_KEY);
    }

    #[cfg(target_os = "macos")]
    #[derive(Default)]
    struct FakeKeychainBackend {
        entries: Mutex<HashMap<(String, String), Vec<u8>>>,
        corrupt_next_read: Mutex<bool>,
        corrupt_after_next_save: Mutex<bool>,
    }

    #[cfg(target_os = "macos")]
    impl FakeKeychainBackend {
        fn entries(&self) -> MutexGuard<'_, HashMap<(String, String), Vec<u8>>> {
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        fn contains(&self, service: &str, account: &str) -> bool {
            self.entries()
                .contains_key(&(service.to_owned(), account.to_owned()))
        }

        fn entry(&self, service: &str, account: &str) -> Option<Vec<u8>> {
            self.entries()
                .get(&(service.to_owned(), account.to_owned()))
                .cloned()
        }

        fn corrupt_verification_after_next_save(&self) {
            *self
                .corrupt_after_next_save
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        }
    }

    #[cfg(target_os = "macos")]
    impl KeychainBackend for FakeKeychainBackend {
        fn load(&self, service: &str, account: &str) -> Result<Vec<u8>, CredentialError> {
            let mut payload = self
                .entries()
                .get(&(service.to_owned(), account.to_owned()))
                .cloned()
                .ok_or(CredentialError::Missing)?;
            if std::mem::take(
                &mut *self
                    .corrupt_next_read
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ) {
                payload.push(0);
            }
            Ok(payload)
        }

        fn save(
            &self,
            service: &str,
            account: &str,
            payload: &[u8],
        ) -> Result<(), CredentialError> {
            self.entries()
                .insert((service.to_owned(), account.to_owned()), payload.to_vec());
            if std::mem::take(
                &mut *self
                    .corrupt_after_next_save
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ) {
                *self
                    .corrupt_next_read
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            }
            Ok(())
        }

        fn remove(&self, service: &str, account: &str) -> Result<(), CredentialError> {
            self.entries()
                .remove(&(service.to_owned(), account.to_owned()));
            Ok(())
        }
    }
}
