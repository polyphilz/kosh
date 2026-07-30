#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};
use uuid::{Uuid, Variant};

pub(crate) const OBJECT_FORMAT_VERSION: u32 = 1;
pub(crate) const FIXED_R2_PREFIX: &str = "kosh/v1/backup-sets";
pub(crate) const CHECKPOINT_MANIFEST_FORMAT_VERSION: u32 = 1;
const MAX_OBJECT_KEY_BYTES: usize = 1_024;
pub(crate) const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_KOSH_VERSION_BYTES: usize = 64;

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
pub(crate) enum CheckpointBackupPhase {
    Off,
    WaitingForMedia,
    Fencing,
    WaitingForReplica,
    Validating,
    Publishing,
    Idle,
    Degraded,
    Blocked,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CheckpointPhase {
    Prepared,
    Fenced,
    Replicated,
    Published,
    Failed,
}

impl CheckpointPhase {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::Prepared => "PREPARED",
            Self::Fenced => "FENCED",
            Self::Replicated => "REPLICATED",
            Self::Published => "PUBLISHED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckpointErrorCode {
    Network,
    NetworkTimeout,
    RateLimited,
    ServiceUnavailable,
    CredentialsMissing,
    KeychainUnavailable,
    InvalidConfiguration,
    AuthenticationRejected,
    AuthorizationRejected,
    OwnerConflict,
    OwnerInvalid,
    ImmutableObjectConflict,
    LocalMediaMissing,
    WorkerUnavailable,
    LitestreamUnavailable,
    FenceTimeout,
    ReplicaBehind,
    MalformedManifest,
    RemoteMediaMissing,
    RemoteMediaCorrupt,
}

impl CheckpointErrorCode {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::Network => "NETWORK",
            Self::NetworkTimeout => "NETWORK_TIMEOUT",
            Self::RateLimited => "RATE_LIMITED",
            Self::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            Self::CredentialsMissing => "CREDENTIALS_MISSING",
            Self::KeychainUnavailable => "KEYCHAIN_UNAVAILABLE",
            Self::InvalidConfiguration => "INVALID_CONFIGURATION",
            Self::AuthenticationRejected => "AUTHENTICATION_REJECTED",
            Self::AuthorizationRejected => "AUTHORIZATION_REJECTED",
            Self::OwnerConflict => "OWNER_CONFLICT",
            Self::OwnerInvalid => "OWNER_INVALID",
            Self::ImmutableObjectConflict => "IMMUTABLE_OBJECT_CONFLICT",
            Self::LocalMediaMissing => "LOCAL_MEDIA_MISSING",
            Self::WorkerUnavailable => "WORKER_UNAVAILABLE",
            Self::LitestreamUnavailable => "LITESTREAM_UNAVAILABLE",
            Self::FenceTimeout => "FENCE_TIMEOUT",
            Self::ReplicaBehind => "REPLICA_BEHIND",
            Self::MalformedManifest => "MALFORMED_MANIFEST",
            Self::RemoteMediaMissing => "REMOTE_MEDIA_MISSING",
            Self::RemoteMediaCorrupt => "REMOTE_MEDIA_CORRUPT",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, BackupDomainError> {
        match value {
            "NETWORK" => Ok(Self::Network),
            "NETWORK_TIMEOUT" => Ok(Self::NetworkTimeout),
            "RATE_LIMITED" => Ok(Self::RateLimited),
            "SERVICE_UNAVAILABLE" => Ok(Self::ServiceUnavailable),
            "CREDENTIALS_MISSING" => Ok(Self::CredentialsMissing),
            "KEYCHAIN_UNAVAILABLE" => Ok(Self::KeychainUnavailable),
            "INVALID_CONFIGURATION" => Ok(Self::InvalidConfiguration),
            "AUTHENTICATION_REJECTED" => Ok(Self::AuthenticationRejected),
            "AUTHORIZATION_REJECTED" => Ok(Self::AuthorizationRejected),
            "OWNER_CONFLICT" => Ok(Self::OwnerConflict),
            "OWNER_INVALID" => Ok(Self::OwnerInvalid),
            "IMMUTABLE_OBJECT_CONFLICT" => Ok(Self::ImmutableObjectConflict),
            "LOCAL_MEDIA_MISSING" => Ok(Self::LocalMediaMissing),
            "WORKER_UNAVAILABLE" => Ok(Self::WorkerUnavailable),
            "LITESTREAM_UNAVAILABLE" => Ok(Self::LitestreamUnavailable),
            "FENCE_TIMEOUT" => Ok(Self::FenceTimeout),
            "REPLICA_BEHIND" => Ok(Self::ReplicaBehind),
            "MALFORMED_MANIFEST" => Ok(Self::MalformedManifest),
            "REMOTE_MEDIA_MISSING" => Ok(Self::RemoteMediaMissing),
            "REMOTE_MEDIA_CORRUPT" => Ok(Self::RemoteMediaCorrupt),
            _ => Err(BackupDomainError::InvalidStoredValue(
                "checkpoint error code",
            )),
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
uuid_v7_id!(CheckpointId, "checkpointId");
uuid_v7_id!(ProbeRunId, "probeRunId");

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct BackupWriterId(String);

impl BackupWriterId {
    pub(crate) fn new() -> Self {
        Self::derive_from_device_identifier(Uuid::now_v7().as_bytes())
    }

    pub(crate) fn derive_from_device_identifier(identifier: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"com.rohan.kosh.backup-writer.v1\0");
        digest.update(identifier);
        Self(format!("{:x}", digest.finalize()))
    }

    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, BackupDomainError> {
        let value = value.into();
        if value.len() != 64 || !is_lower_hex(&value) {
            return Err(BackupDomainError::InvalidField("backupWriterId"));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BackupWriterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for BackupWriterId {
    type Err = BackupDomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for BackupWriterId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BackupWriterId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

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

impl Serialize for ContentSha256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentSha256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse_hex(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UtcTimestamp(String);

impl UtcTimestamp {
    pub(crate) fn now() -> Result<Self, BackupDomainError> {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| BackupDomainError::InvalidField("createdAt"))?;
        Self::parse(value)
    }

    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, BackupDomainError> {
        let value = value.into();
        let parsed = OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| BackupDomainError::InvalidField("createdAt"))?;
        if parsed.offset() != UtcOffset::UTC || !value.ends_with('Z') {
            return Err(BackupDomainError::InvalidField("createdAt"));
        }
        Ok(Self(value))
    }

    pub(crate) fn from_unix_millis(value: i64) -> Result<Self, BackupDomainError> {
        if value < 0 {
            return Err(BackupDomainError::InvalidField("createdAt"));
        }
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
            .map_err(|_| BackupDomainError::InvalidField("createdAt"))?;
        let formatted = timestamp
            .format(&Rfc3339)
            .map_err(|_| BackupDomainError::InvalidField("createdAt"))?;
        Self::parse(formatted)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn basic_utc(&self) -> Result<String, BackupDomainError> {
        let value = OffsetDateTime::parse(&self.0, &Rfc3339)
            .map_err(|_| BackupDomainError::InvalidField("createdAt"))?;
        Ok(format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            value.year(),
            u8::from(value.month()),
            value.day(),
            value.hour(),
            value.minute(),
            value.second()
        ))
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
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

    pub(crate) fn checkpoint(
        &self,
        epoch: &ReplicaEpochId,
        checkpoint: &CheckpointId,
        created_at: &UtcTimestamp,
    ) -> Result<R2ObjectKey, BackupDomainError> {
        Ok(self.fixed_key(&format!(
            "checkpoints/v1/{}/{}-{}.json",
            epoch.as_str(),
            created_at.basic_utc()?,
            checkpoint.as_str()
        )))
    }

    pub(crate) fn checkpoint_prefix(&self) -> R2ListPrefix {
        R2ListPrefix(format!("{}/checkpoints/v1/", self.root))
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckpointMainManifestV1 {
    migration_head: u32,
    litestream_path: String,
    txid: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckpointMediaManifestV1 {
    migration_head: u32,
    object_format_version: u32,
    referenced_hash_count: u64,
    referenced_total_bytes: u64,
    referenced_hash_set_sha256: ContentSha256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CheckpointManifestV1 {
    format_version: u32,
    backup_set_id: BackupSetId,
    replica_epoch_id: ReplicaEpochId,
    checkpoint_id: CheckpointId,
    created_at: UtcTimestamp,
    kosh_version: String,
    content_revision: u64,
    main: CheckpointMainManifestV1,
    media: CheckpointMediaManifestV1,
}

pub(crate) struct CheckpointManifestInput {
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) created_at: UtcTimestamp,
    pub(crate) kosh_version: String,
    pub(crate) content_revision: u64,
    pub(crate) main_migration_head: u32,
    pub(crate) litestream_path: R2ObjectKey,
    pub(crate) txid: String,
    pub(crate) media_migration_head: u32,
    pub(crate) referenced_hash_count: u64,
    pub(crate) referenced_total_bytes: u64,
    pub(crate) referenced_hash_set_sha256: ContentSha256,
}

pub(crate) struct PublishedCheckpointEvidence<'a> {
    pub(crate) checkpoint_id: &'a CheckpointId,
    pub(crate) backup_set_id: &'a BackupSetId,
    pub(crate) replica_epoch_id: &'a ReplicaEpochId,
    pub(crate) content_revision: u64,
    pub(crate) kosh_version: &'a str,
    pub(crate) main_migration_head: u32,
    pub(crate) media_migration_head: u32,
    pub(crate) referenced_hash_count: u64,
    pub(crate) referenced_total_bytes: u64,
    pub(crate) referenced_hash_set_sha256: ContentSha256,
    pub(crate) litestream_txid: &'a str,
}

impl CheckpointManifestV1 {
    pub(crate) fn new(input: CheckpointManifestInput) -> Result<Self, BackupDomainError> {
        if input.kosh_version.is_empty()
            || input.kosh_version.len() > MAX_KOSH_VERSION_BYTES
            || input.kosh_version.chars().any(char::is_control)
            || input.main_migration_head == 0
            || input.media_migration_head == 0
            || !is_canonical_txid(&input.txid)
        {
            return Err(BackupDomainError::InvalidManifest);
        }
        Ok(Self {
            format_version: CHECKPOINT_MANIFEST_FORMAT_VERSION,
            backup_set_id: input.backup_set_id,
            replica_epoch_id: input.replica_epoch_id,
            checkpoint_id: input.checkpoint_id,
            created_at: input.created_at,
            kosh_version: input.kosh_version,
            content_revision: input.content_revision,
            main: CheckpointMainManifestV1 {
                migration_head: input.main_migration_head,
                litestream_path: input.litestream_path.as_str().to_owned(),
                txid: input.txid,
            },
            media: CheckpointMediaManifestV1 {
                migration_head: input.media_migration_head,
                object_format_version: OBJECT_FORMAT_VERSION,
                referenced_hash_count: input.referenced_hash_count,
                referenced_total_bytes: input.referenced_total_bytes,
                referenced_hash_set_sha256: input.referenced_hash_set_sha256,
            },
        })
    }

    pub(crate) fn to_json(&self) -> Result<Vec<u8>, BackupDomainError> {
        encode_manifest(self)
    }

    pub(crate) fn from_json(
        bytes: &[u8],
        keyspace: &R2Keyspace,
    ) -> Result<Self, BackupDomainError> {
        let manifest: Self = decode_manifest(bytes)?;
        if manifest.format_version != CHECKPOINT_MANIFEST_FORMAT_VERSION
            || manifest.media.object_format_version != OBJECT_FORMAT_VERSION
            || manifest.kosh_version.is_empty()
            || manifest.kosh_version.len() > MAX_KOSH_VERSION_BYTES
            || manifest.kosh_version.chars().any(char::is_control)
            || manifest.main.migration_head == 0
            || manifest.media.migration_head == 0
            || !is_canonical_txid(&manifest.main.txid)
            || manifest.main.litestream_path
                != keyspace.litestream(&manifest.replica_epoch_id).as_str()
        {
            return Err(BackupDomainError::InvalidManifest);
        }
        Ok(manifest)
    }

    pub(crate) fn object_key(
        &self,
        keyspace: &R2Keyspace,
    ) -> Result<R2ObjectKey, BackupDomainError> {
        keyspace.checkpoint(
            &self.replica_epoch_id,
            &self.checkpoint_id,
            &self.created_at,
        )
    }

    pub(crate) fn matches_published_evidence(
        &self,
        evidence: &PublishedCheckpointEvidence<'_>,
    ) -> bool {
        self.checkpoint_id == *evidence.checkpoint_id
            && self.backup_set_id == *evidence.backup_set_id
            && self.replica_epoch_id == *evidence.replica_epoch_id
            && self.content_revision == evidence.content_revision
            && self.kosh_version == evidence.kosh_version
            && self.main.migration_head == evidence.main_migration_head
            && self.media.migration_head == evidence.media_migration_head
            && self.media.referenced_hash_count == evidence.referenced_hash_count
            && self.media.referenced_total_bytes == evidence.referenced_total_bytes
            && self.media.referenced_hash_set_sha256 == evidence.referenced_hash_set_sha256
            && self.main.txid == evidence.litestream_txid
    }

    pub(crate) fn backup_set_id(&self) -> &BackupSetId {
        &self.backup_set_id
    }

    pub(crate) fn replica_epoch_id(&self) -> &ReplicaEpochId {
        &self.replica_epoch_id
    }

    pub(crate) fn checkpoint_id(&self) -> &CheckpointId {
        &self.checkpoint_id
    }

    pub(crate) fn created_at(&self) -> &UtcTimestamp {
        &self.created_at
    }

    pub(crate) fn kosh_version(&self) -> &str {
        &self.kosh_version
    }

    pub(crate) const fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub(crate) const fn main_migration_head(&self) -> u32 {
        self.main.migration_head
    }

    pub(crate) fn litestream_path(&self) -> &str {
        &self.main.litestream_path
    }

    pub(crate) fn txid(&self) -> &str {
        &self.main.txid
    }

    pub(crate) const fn media_migration_head(&self) -> u32 {
        self.media.migration_head
    }

    pub(crate) const fn referenced_hash_count(&self) -> u64 {
        self.media.referenced_hash_count
    }

    pub(crate) const fn referenced_total_bytes(&self) -> u64 {
        self.media.referenced_total_bytes
    }

    pub(crate) const fn referenced_hash_set_sha256(&self) -> ContentSha256 {
        self.media.referenced_hash_set_sha256
    }
}

fn is_canonical_txid(value: &str) -> bool {
    value.len() == 16 && is_lower_hex(value)
}

fn encode_manifest<T: Serialize>(manifest: &T) -> Result<Vec<u8>, BackupDomainError> {
    let bytes = serde_json::to_vec(manifest).map_err(BackupDomainError::ManifestJson)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(BackupDomainError::ManifestTooLarge);
    }
    Ok(bytes)
}

fn decode_manifest<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, BackupDomainError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(BackupDomainError::ManifestTooLarge);
    }
    serde_json::from_slice(bytes).map_err(BackupDomainError::ManifestJson)
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
    #[error("off-site backup manifest is too large")]
    ManifestTooLarge,
    #[error("off-site backup manifest is invalid")]
    InvalidManifest,
    #[error("off-site backup manifest JSON is invalid")]
    ManifestJson(#[source] serde_json::Error),
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
        let checkpoint = CheckpointId::new();
        let created_at = UtcTimestamp::parse("2026-07-30T15:00:00Z").expect("timestamp");
        let hash = ContentSha256::from_bytes([0xab; 32]);
        let expected_root = format!("{FIXED_R2_PREFIX}/{backup_set_id}/");
        for key in [
            keyspace.identity(),
            keyspace.owner(),
            keyspace.media(hash),
            keyspace.litestream(&epoch),
            keyspace
                .checkpoint(&epoch, &checkpoint, &created_at)
                .expect("checkpoint"),
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
    fn event_identifiers_are_canonical_uuid_v7_values() {
        let backup_set = BackupSetId::new();
        assert_eq!(
            BackupSetId::parse(backup_set.as_str()).expect("round trip"),
            backup_set
        );
        assert!(BackupSetId::parse("550e8400-e29b-41d4-a716-446655440000").is_err());
        assert!(ReplicaEpochId::parse("not-an-id").is_err());
    }

    #[test]
    fn writer_identifiers_are_canonical_sha256_values() {
        let writer = BackupWriterId::new();
        assert_eq!(
            BackupWriterId::parse(writer.as_str()).expect("writer round trip"),
            writer
        );
        assert_eq!(writer.as_str().len(), 64);
        assert!(BackupWriterId::parse("not-an-id").is_err());
        assert!(BackupWriterId::parse("A".repeat(64)).is_err());
    }

    #[test]
    fn checkpoint_manifest_round_trips_only_for_its_derived_lineage() {
        let backup_set_id = BackupSetId::new();
        let keyspace = target(R2Jurisdiction::Default).keyspace(&backup_set_id);
        let replica_epoch_id = ReplicaEpochId::new();
        let checkpoint_id = CheckpointId::new();
        let created_at = UtcTimestamp::parse("2026-07-30T15:00:00Z").expect("timestamp");
        let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: replica_epoch_id.clone(),
            checkpoint_id: checkpoint_id.clone(),
            created_at: created_at.clone(),
            kosh_version: "0.1.0".into(),
            content_revision: 7,
            main_migration_head: 20,
            litestream_path: keyspace.litestream(&replica_epoch_id),
            txid: "000000000000002a".into(),
            media_migration_head: 2,
            referenced_hash_count: 1,
            referenced_total_bytes: 42,
            referenced_hash_set_sha256: ContentSha256::from_bytes([0xcd; 32]),
        })
        .expect("manifest");
        let bytes = manifest.to_json().expect("manifest JSON");
        let parsed =
            CheckpointManifestV1::from_json(&bytes, &keyspace).expect("manifest round trip");
        assert_eq!(parsed, manifest);
        assert_eq!(
            parsed.object_key(&keyspace).expect("object key"),
            keyspace
                .checkpoint(&replica_epoch_id, &checkpoint_id, &created_at)
                .expect("expected object key")
        );

        let other_keyspace = target(R2Jurisdiction::Default).keyspace(&BackupSetId::new());
        assert!(CheckpointManifestV1::from_json(&bytes, &other_keyspace).is_err());
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
