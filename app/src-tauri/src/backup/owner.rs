#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::{
    domain::{BackupSetId, BackupWriterId, R2Keyspace, ReplicaEpochId, OBJECT_FORMAT_VERSION},
    object_store::{
        ObjectContentType, ObjectStore, ObjectStoreError, PutCondition, PutObjectOutcome,
        PutObjectRequest,
    },
};

const OWNER_FORMAT_VERSION: u32 = 1;
const MAX_OWNER_BYTES: usize = 4 * 1024;
const MAX_CLAIM_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteOwnerDocument {
    format_version: u32,
    backup_set_id: BackupSetId,
    replica_epoch_id: ReplicaEpochId,
    writer_id: BackupWriterId,
}

impl RemoteOwnerDocument {
    fn new(
        backup_set_id: &BackupSetId,
        replica_epoch_id: &ReplicaEpochId,
        writer_id: &BackupWriterId,
    ) -> Self {
        Self {
            format_version: OWNER_FORMAT_VERSION,
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: replica_epoch_id.clone(),
            writer_id: writer_id.clone(),
        }
    }

    fn encode(&self) -> Result<Vec<u8>, RemoteOwnerError> {
        let bytes = serde_json::to_vec(self).map_err(|_| RemoteOwnerError::Invalid)?;
        if bytes.len() > MAX_OWNER_BYTES {
            return Err(RemoteOwnerError::Invalid);
        }
        Ok(bytes)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteOwnerError {
    #[error("remote owner acquisition was cancelled")]
    Cancelled,
    #[error("another installation owns this backup set")]
    Conflict,
    #[error("the remote owner record is invalid")]
    Invalid,
    #[error("the remote owner record is unavailable")]
    Store(#[from] ObjectStoreError),
}

pub(crate) fn claim_remote_owner(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
    replica_epoch_id: &ReplicaEpochId,
    writer_id: &BackupWriterId,
) -> Result<(), RemoteOwnerError> {
    claim_remote_owner_cancellable(
        store,
        keyspace,
        backup_set_id,
        replica_epoch_id,
        writer_id,
        &AtomicBool::new(false),
    )
}

pub(crate) fn claim_remote_owner_cancellable(
    store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    backup_set_id: &BackupSetId,
    replica_epoch_id: &ReplicaEpochId,
    writer_id: &BackupWriterId,
    cancelled: &AtomicBool,
) -> Result<(), RemoteOwnerError> {
    ensure_not_cancelled(cancelled)?;
    let expected = RemoteOwnerDocument::new(backup_set_id, replica_epoch_id, writer_id);
    let expected_bytes = expected.encode()?;
    let key = keyspace.owner();

    match store.put(PutObjectRequest {
        key: key.clone(),
        bytes: expected_bytes.clone(),
        content_type: ObjectContentType::Json,
        kosh_sha256: None,
        condition: PutCondition::IfAbsent,
    })? {
        PutObjectOutcome::Stored => {
            ensure_not_cancelled(cancelled)?;
            return verify_exact_owner(store, &key, &expected, &expected_bytes).map(|_| ());
        }
        PutObjectOutcome::ConditionNotMet => {}
    }

    for _ in 0..MAX_CLAIM_ATTEMPTS {
        ensure_not_cancelled(cancelled)?;
        let current = read_owner(store, &key)?;
        ensure_not_cancelled(cancelled)?;
        if current.document == expected {
            return Ok(());
        }
        if current.document.writer_id != *writer_id
            || current.document.backup_set_id != *backup_set_id
        {
            return Err(RemoteOwnerError::Conflict);
        }
        match store.put(PutObjectRequest {
            key: key.clone(),
            bytes: expected_bytes.clone(),
            content_type: ObjectContentType::Json,
            kosh_sha256: None,
            condition: PutCondition::IfMatch(current.version),
        })? {
            PutObjectOutcome::Stored => {
                ensure_not_cancelled(cancelled)?;
                return verify_exact_owner(store, &key, &expected, &expected_bytes).map(|_| ());
            }
            PutObjectOutcome::ConditionNotMet => {}
        }
    }
    Err(RemoteOwnerError::Conflict)
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), RemoteOwnerError> {
    if cancelled.load(Ordering::Acquire) {
        Err(RemoteOwnerError::Cancelled)
    } else {
        Ok(())
    }
}

struct ReadOwner {
    document: RemoteOwnerDocument,
    version: super::object_store::ObjectVersion,
}

fn read_owner(
    store: &dyn ObjectStore,
    key: &super::domain::R2ObjectKey,
) -> Result<ReadOwner, RemoteOwnerError> {
    let result = store.get_bounded(key, MAX_OWNER_BYTES)?;
    if result.metadata.byte_length != result.bytes.len() as u64
        || result.metadata.content_type != Some(ObjectContentType::Json)
        || result.metadata.kosh_sha256.is_some()
        || result.metadata.object_format_version != Some(OBJECT_FORMAT_VERSION)
    {
        return Err(RemoteOwnerError::Invalid);
    }
    let document: RemoteOwnerDocument =
        serde_json::from_slice(&result.bytes).map_err(|_| RemoteOwnerError::Invalid)?;
    if document.format_version != OWNER_FORMAT_VERSION
        || document.encode()?.as_slice() != result.bytes.as_slice()
    {
        return Err(RemoteOwnerError::Invalid);
    }
    Ok(ReadOwner {
        document,
        version: result.metadata.version,
    })
}

fn verify_exact_owner(
    store: &dyn ObjectStore,
    key: &super::domain::R2ObjectKey,
    expected: &RemoteOwnerDocument,
    expected_bytes: &[u8],
) -> Result<ReadOwner, RemoteOwnerError> {
    let current = read_owner(store, key)?;
    if current.document != *expected || current.document.encode()?.as_slice() != expected_bytes {
        return Err(RemoteOwnerError::Conflict);
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{
        domain::{R2AccountId, R2BucketName, R2Jurisdiction, R2Target},
        object_store::{
            fake::FakeObjectStore, ObjectStoreErrorCode, PutCondition, PutObjectRequest,
        },
    };

    fn fixture() -> (
        BackupSetId,
        R2Keyspace,
        FakeObjectStore,
        ReplicaEpochId,
        BackupWriterId,
    ) {
        let backup_set_id = BackupSetId::new();
        let target = R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef").expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        };
        let keyspace = target.keyspace(&backup_set_id);
        (
            backup_set_id,
            keyspace.clone(),
            FakeObjectStore::new(keyspace),
            ReplicaEpochId::new(),
            BackupWriterId::new(),
        )
    }

    #[test]
    fn first_writer_claims_once_and_the_same_installation_reclaims_idempotently() {
        let (backup_set_id, keyspace, store, epoch, writer) = fixture();

        claim_remote_owner(&store, &keyspace, &backup_set_id, &epoch, &writer)
            .expect("first claim");
        claim_remote_owner(&store, &keyspace, &backup_set_id, &epoch, &writer)
            .expect("idempotent reclaim");

        let stored = read_owner(&store, &keyspace.owner()).expect("owner");
        assert_eq!(stored.document.backup_set_id, backup_set_id);
        assert_eq!(stored.document.replica_epoch_id, epoch);
        assert_eq!(stored.document.writer_id, writer);
    }

    #[test]
    fn copied_configuration_and_r2_keys_cannot_claim_from_another_installation() {
        let (backup_set_id, keyspace, store, epoch, first_writer) = fixture();
        claim_remote_owner(&store, &keyspace, &backup_set_id, &epoch, &first_writer)
            .expect("first owner");

        assert!(matches!(
            claim_remote_owner(
                &store,
                &keyspace,
                &backup_set_id,
                &epoch,
                &BackupWriterId::new(),
            ),
            Err(RemoteOwnerError::Conflict)
        ));
    }

    #[test]
    fn current_installation_can_advance_the_epoch_with_an_etag_guard() {
        let (backup_set_id, keyspace, store, first_epoch, writer) = fixture();
        claim_remote_owner(&store, &keyspace, &backup_set_id, &first_epoch, &writer)
            .expect("first epoch");
        let next_epoch = ReplicaEpochId::new();

        claim_remote_owner(&store, &keyspace, &backup_set_id, &next_epoch, &writer)
            .expect("guarded epoch advance");

        assert_eq!(
            read_owner(&store, &keyspace.owner())
                .expect("advanced owner")
                .document
                .replica_epoch_id,
            next_epoch
        );
    }

    #[test]
    fn malformed_or_oversized_owner_records_fail_closed() {
        let (backup_set_id, keyspace, store, epoch, writer) = fixture();
        store
            .put(PutObjectRequest {
                key: keyspace.owner(),
                bytes: b"{\"formatVersion\":1}".to_vec(),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("malformed owner");
        assert!(matches!(
            claim_remote_owner(&store, &keyspace, &backup_set_id, &epoch, &writer),
            Err(RemoteOwnerError::Invalid)
        ));

        let (other_backup_set_id, other_keyspace, other_store, other_epoch, other_writer) =
            fixture();
        let oversized = vec![b'x'; MAX_OWNER_BYTES + 1];
        other_store
            .put(PutObjectRequest {
                key: other_keyspace.owner(),
                bytes: oversized,
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("oversized owner");
        assert_eq!(
            claim_remote_owner(
                &other_store,
                &other_keyspace,
                &other_backup_set_id,
                &other_epoch,
                &other_writer,
            )
            .expect_err("oversized owner")
            .store_code(),
            Some(ObjectStoreErrorCode::ResponseTooLarge)
        );
    }

    #[test]
    fn a_cancelled_claim_performs_no_remote_operation() {
        let (backup_set_id, keyspace, store, epoch, writer) = fixture();
        let cancelled = AtomicBool::new(true);

        assert!(matches!(
            claim_remote_owner_cancellable(
                &store,
                &keyspace,
                &backup_set_id,
                &epoch,
                &writer,
                &cancelled,
            ),
            Err(RemoteOwnerError::Cancelled)
        ));
        assert!(store.operations().is_empty());
    }

    impl RemoteOwnerError {
        fn store_code(&self) -> Option<ObjectStoreErrorCode> {
            match self {
                Self::Store(error) => Some(error.code),
                Self::Cancelled | Self::Conflict | Self::Invalid => None,
            }
        }
    }
}
