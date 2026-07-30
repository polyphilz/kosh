#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use uuid::{Uuid, Variant};

pub(crate) const OBJECT_FORMAT_VERSION: u32 = 1;
pub(crate) const FIXED_R2_PREFIX: &str = "kosh/v1/backup-sets";
const MAX_OBJECT_KEY_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BackupProvider {
    R2,
}

impl BackupProvider {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::R2 => "R2",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, BackupDomainError> {
        match value {
            "R2" => Ok(Self::R2),
            _ => Err(BackupDomainError::InvalidStoredValue("provider")),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum R2Jurisdiction {
    Default,
    Eu,
    Fedramp,
}

impl R2Jurisdiction {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::Eu => "EU",
            Self::Fedramp => "FEDRAMP",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, BackupDomainError> {
        match value {
            "DEFAULT" => Ok(Self::Default),
            "EU" => Ok(Self::Eu),
            "FEDRAMP" => Ok(Self::Fedramp),
            _ => Err(BackupDomainError::InvalidStoredValue("jurisdiction")),
        }
    }

    const fn endpoint_suffix(self) -> &'static str {
        match self {
            Self::Default => "r2.cloudflarestorage.com",
            Self::Eu => "eu.r2.cloudflarestorage.com",
            Self::Fedramp => "fedramp.r2.cloudflarestorage.com",
        }
    }
}

macro_rules! uuid_v7_id {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub(crate) struct $name(String);

        impl $name {
            pub(crate) fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }

            pub(crate) fn parse(value: impl Into<String>) -> Result<Self, BackupDomainError> {
                let value = value.into();
                let parsed =
                    Uuid::parse_str(&value).map_err(|_| BackupDomainError::InvalidField($field))?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(BackupDomainError::InvalidField($field));
                }
                Ok(Self(value))
            }

            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = BackupDomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(de::Error::custom)
            }
        }
    };
}

uuid_v7_id!(BackupSetId, "backupSetId");
uuid_v7_id!(ReplicaEpochId, "replicaEpochId");
uuid_v7_id!(ProbeRunId, "probeRunId");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2AccountId(String);

impl R2AccountId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, BackupDomainError> {
        let value = value.into();
        if value.len() != 32 || !is_lower_hex(&value) {
            return Err(BackupDomainError::InvalidField("accountId"));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2BucketName(String);

impl R2BucketName {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, BackupDomainError> {
        let value = value.into();
        let valid_character =
            |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !(3..=63).contains(&value.len())
            || !value.bytes().all(valid_character)
            || !value
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !value
                .as_bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            return Err(BackupDomainError::InvalidField("bucket"));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2Target {
    pub(crate) account_id: R2AccountId,
    pub(crate) jurisdiction: R2Jurisdiction,
    pub(crate) bucket: R2BucketName,
}

impl R2Target {
    pub(crate) fn endpoint(&self) -> String {
        format!(
            "https://{}.{}",
            self.account_id.as_str(),
            self.jurisdiction.endpoint_suffix()
        )
    }

    pub(crate) fn keyspace(&self, backup_set_id: &BackupSetId) -> R2Keyspace {
        R2Keyspace::new(backup_set_id)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct ContentSha256([u8; 32]);

impl ContentSha256 {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn parse_hex(value: &str) -> Result<Self, BackupDomainError> {
        if value.len() != 64 || !is_lower_hex(value) {
            return Err(BackupDomainError::InvalidField("sha256"));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }

    pub(crate) fn to_hex(self) -> String {
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            use std::fmt::Write;
            let _ = write!(output, "{byte:02x}");
        }
        output
    }
}

impl fmt::Debug for ContentSha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ContentSha256")
            .field(&self.to_hex())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2ObjectKey(String);

impl R2ObjectKey {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2ListPrefix(String);

impl R2ListPrefix {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct R2Keyspace {
    root: String,
}

impl R2Keyspace {
    fn new(backup_set_id: &BackupSetId) -> Self {
        Self {
            root: format!("{FIXED_R2_PREFIX}/{}", backup_set_id.as_str()),
        }
    }

    pub(crate) fn root_prefix(&self) -> R2ListPrefix {
        R2ListPrefix(format!("{}/", self.root))
    }

    pub(crate) fn identity(&self) -> R2ObjectKey {
        self.fixed_key("identity/v1.json")
    }

    pub(crate) fn owner(&self) -> R2ObjectKey {
        self.fixed_key("owner/v1.json")
    }

    pub(crate) fn media(&self, sha256: ContentSha256) -> R2ObjectKey {
        let hex = sha256.to_hex();
        self.fixed_key(&format!("media/v1/sha256/{}/{}.blob", &hex[..2], hex))
    }

    pub(crate) fn is_media_key(&self, key: &R2ObjectKey) -> bool {
        key.as_str()
            .starts_with(&format!("{}/media/v1/sha256/", self.root))
    }

    pub(crate) fn litestream(&self, epoch: &ReplicaEpochId) -> R2ObjectKey {
        self.fixed_key(&format!("litestream/v1/{}/kosh.sqlite3", epoch.as_str()))
    }

    pub(crate) fn probe_prefix(&self, run_id: &ProbeRunId) -> R2ListPrefix {
        R2ListPrefix(format!("{}/probes/{}/", self.root, run_id.as_str()))
    }

    pub(crate) fn probe_object(&self, run_id: &ProbeRunId) -> R2ObjectKey {
        self.fixed_key(&format!("probes/{}/object.bin", run_id.as_str()))
    }

    pub(crate) fn validate_returned_key(
        &self,
        value: impl Into<String>,
    ) -> Result<R2ObjectKey, BackupDomainError> {
        let value = value.into();
        if !value.starts_with(&format!("{}/", self.root)) {
            return Err(BackupDomainError::KeyOutsidePrefix);
        }
        validate_object_key(&value)?;
        Ok(R2ObjectKey(value))
    }

    pub(crate) fn validate_list_prefix(
        &self,
        prefix: &R2ListPrefix,
    ) -> Result<(), BackupDomainError> {
        let root = self.root_prefix();
        if !prefix.as_str().starts_with(root.as_str()) || !prefix.as_str().ends_with('/') {
            return Err(BackupDomainError::KeyOutsidePrefix);
        }
        validate_object_key(prefix.as_str().trim_end_matches('/'))
    }

    fn fixed_key(&self, suffix: &str) -> R2ObjectKey {
        let value = format!("{}/{suffix}", self.root);
        debug_assert!(validate_object_key(&value).is_ok());
        R2ObjectKey(value)
    }
}

fn validate_object_key(value: &str) -> Result<(), BackupDomainError> {
    if value.is_empty()
        || value.len() > MAX_OBJECT_KEY_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.contains("..")
        || value.contains('\\')
        || value.contains('?')
        || value.contains('#')
        || value.chars().any(char::is_control)
    {
        return Err(BackupDomainError::InvalidObjectKey);
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_nibble(byte: u8) -> Result<u8, BackupDomainError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(BackupDomainError::InvalidField("sha256")),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackupDomainError {
    #[error("invalid off-site backup field: {0}")]
    InvalidField(&'static str),
    #[error("invalid stored off-site backup value: {0}")]
    InvalidStoredValue(&'static str),
    #[error("R2 object key is invalid")]
    InvalidObjectKey,
    #[error("R2 object key is outside Kosh's fixed backup prefix")]
    KeyOutsidePrefix,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(jurisdiction: R2Jurisdiction) -> R2Target {
        R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef").expect("account"),
            jurisdiction,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        }
    }

    #[test]
    fn validates_targets_and_derives_only_cloudflare_endpoints() {
        assert_eq!(
            target(R2Jurisdiction::Default).endpoint(),
            "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com"
        );
        assert_eq!(
            target(R2Jurisdiction::Eu).endpoint(),
            "https://0123456789abcdef0123456789abcdef.eu.r2.cloudflarestorage.com"
        );
        assert_eq!(
            target(R2Jurisdiction::Fedramp).endpoint(),
            "https://0123456789abcdef0123456789abcdef.fedramp.r2.cloudflarestorage.com"
        );
        for invalid in ["", "abc", "ABCDEF0123456789abcdef0123456789", "example.com"] {
            assert!(R2AccountId::parse(invalid).is_err(), "{invalid}");
        }
        for invalid in ["ab", "Kosh", "-kosh", "kosh_", "kosh-"] {
            assert!(R2BucketName::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn every_key_is_derived_under_the_fixed_backup_set_prefix() {
        let backup_set_id = BackupSetId::new();
        let keyspace = target(R2Jurisdiction::Default).keyspace(&backup_set_id);
        let epoch = ReplicaEpochId::new();
        let run = ProbeRunId::new();
        let hash = ContentSha256::from_bytes([0xab; 32]);
        let expected_root = format!("{FIXED_R2_PREFIX}/{backup_set_id}/");
        for key in [
            keyspace.identity(),
            keyspace.owner(),
            keyspace.media(hash),
            keyspace.litestream(&epoch),
            keyspace.probe_object(&run),
        ] {
            assert!(key.as_str().starts_with(&expected_root), "{key:?}");
            keyspace
                .validate_returned_key(key.as_str())
                .expect("derived key");
        }
        assert!(matches!(
            keyspace.validate_returned_key("kosh/v1/backup-sets/other/object"),
            Err(BackupDomainError::KeyOutsidePrefix)
        ));
    }

    #[test]
    fn identifiers_are_canonical_uuid_v7_values() {
        let backup_set = BackupSetId::new();
        assert_eq!(
            BackupSetId::parse(backup_set.as_str()).expect("round trip"),
            backup_set
        );
        assert!(BackupSetId::parse("550e8400-e29b-41d4-a716-446655440000").is_err());
        assert!(ReplicaEpochId::parse("not-an-id").is_err());
    }

    #[test]
    fn identifiers_reject_every_non_rfc_uuid_variant() {
        let canonical = BackupSetId::new().to_string();
        for &variant_nibble in b"07cdef" {
            let mut bytes = canonical.as_bytes().to_vec();
            bytes[19] = variant_nibble;
            let invalid = String::from_utf8(bytes).expect("ASCII UUID");

            assert!(BackupSetId::parse(invalid.clone()).is_err());
            assert!(ReplicaEpochId::parse(invalid.clone()).is_err());
            assert!(ProbeRunId::parse(invalid).is_err());
        }
    }
}
