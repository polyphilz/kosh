#![cfg_attr(feature = "test-support", allow(dead_code))]

use sha2::{Digest, Sha256};

use super::{
    domain::{ContentSha256, ProbeRunId, R2Keyspace, OBJECT_FORMAT_VERSION},
    object_store::{
        ContinuationToken, ObjectContentType, ObjectStore, ObjectStoreErrorCode, PutCondition,
        PutObjectOutcome, PutObjectRequest,
    },
};

const MAX_CLEANUP_PAGES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProbeStage {
    Put,
    Head,
    Get,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProbeCleanupStatus {
    Complete,
    Failed(ObjectStoreErrorCode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObjectStoreProbeReport {
    pub(crate) run_id: ProbeRunId,
    pub(crate) cleanup: ProbeCleanupStatus,
}

#[derive(Debug, thiserror::Error)]
#[error("R2 connection probe failed during {stage:?}: {code:?}")]
pub(crate) struct ObjectStoreProbeError {
    pub(crate) stage: ProbeStage,
    pub(crate) code: ObjectStoreErrorCode,
    pub(crate) cleanup: ProbeCleanupStatus,
}

pub(crate) fn verify_object_store(
    object_store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
) -> Result<ObjectStoreProbeReport, ObjectStoreProbeError> {
    let run_id = ProbeRunId::new();
    let verification = verify_inner(object_store, keyspace, &run_id);
    let cleanup = cleanup_probe_prefix(object_store, keyspace, &run_id);
    match verification {
        Ok(()) if cleanup == ProbeCleanupStatus::Complete => {
            Ok(ObjectStoreProbeReport { run_id, cleanup })
        }
        Ok(()) => Err(ObjectStoreProbeError {
            stage: ProbeStage::List,
            code: match cleanup {
                ProbeCleanupStatus::Complete => unreachable!("matched above"),
                ProbeCleanupStatus::Failed(code) => code,
            },
            cleanup,
        }),
        Err((stage, code)) => Err(ObjectStoreProbeError {
            stage,
            code,
            cleanup,
        }),
    }
}

fn verify_inner(
    object_store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    run_id: &ProbeRunId,
) -> Result<(), (ProbeStage, ObjectStoreErrorCode)> {
    let key = keyspace.probe_object(run_id);
    let payload = format!("kosh-r2-probe-v1:{}", run_id.as_str()).into_bytes();
    let digest = ContentSha256::from_bytes(Sha256::digest(&payload).into());

    let outcome = object_store
        .put(PutObjectRequest {
            key: key.clone(),
            bytes: payload.clone(),
            content_type: ObjectContentType::Binary,
            kosh_sha256: Some(digest),
            condition: PutCondition::IfAbsent,
        })
        .map_err(|error| (ProbeStage::Put, error.code))?;
    if outcome != PutObjectOutcome::Stored {
        return Err((ProbeStage::Put, ObjectStoreErrorCode::Conflict));
    }

    let head = object_store
        .head(&key)
        .map_err(|error| (ProbeStage::Head, error.code))?
        .ok_or((ProbeStage::Head, ObjectStoreErrorCode::NotFound))?;
    if head.byte_length != payload.len() as u64
        || head.content_type != Some(ObjectContentType::Binary)
        || head.kosh_sha256 != Some(digest)
        || head.object_format_version != Some(OBJECT_FORMAT_VERSION)
    {
        return Err((ProbeStage::Head, ObjectStoreErrorCode::InvalidResponse));
    }

    let get = object_store
        .get(&key)
        .map_err(|error| (ProbeStage::Get, error.code))?;
    if get.bytes != payload
        || get.metadata != head
        || ContentSha256::from_bytes(Sha256::digest(&get.bytes).into()) != digest
    {
        return Err((ProbeStage::Get, ObjectStoreErrorCode::InvalidResponse));
    }

    let listed = object_store
        .list(&keyspace.probe_prefix(run_id), None)
        .map_err(|error| (ProbeStage::List, error.code))?;
    if listed.next.is_some()
        || listed.objects.len() != 1
        || listed.objects[0].key != key
        || listed.objects[0].byte_length != payload.len() as u64
    {
        return Err((ProbeStage::List, ObjectStoreErrorCode::InvalidResponse));
    }
    Ok(())
}

fn cleanup_probe_prefix(
    object_store: &dyn ObjectStore,
    keyspace: &R2Keyspace,
    run_id: &ProbeRunId,
) -> ProbeCleanupStatus {
    let prefix = keyspace.probe_prefix(run_id);
    let mut continuation: Option<ContinuationToken> = None;
    for _ in 0..MAX_CLEANUP_PAGES {
        let page = match object_store.list(&prefix, continuation.as_ref()) {
            Ok(page) => page,
            Err(error) => return ProbeCleanupStatus::Failed(error.code),
        };
        for object in page.objects {
            if let Err(error) = object_store.delete(&object.key) {
                return ProbeCleanupStatus::Failed(error.code);
            }
        }
        let Some(next) = page.next else {
            return match object_store.list(&prefix, None) {
                Ok(page) if page.objects.is_empty() && page.next.is_none() => {
                    ProbeCleanupStatus::Complete
                }
                Ok(_) => ProbeCleanupStatus::Failed(ObjectStoreErrorCode::InvalidResponse),
                Err(error) => ProbeCleanupStatus::Failed(error.code),
            };
        };
        continuation = Some(next);
    }
    ProbeCleanupStatus::Failed(ObjectStoreErrorCode::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{
        credentials::R2Credentials,
        domain::{BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target},
        object_store::{
            fake::{FakeObjectStore, ObjectOperation},
            R2ObjectStore,
        },
    };

    fn target() -> R2Target {
        R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef").expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        }
    }

    #[test]
    fn fake_probe_verifies_round_trip_and_always_cleans_its_unique_prefix() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());

        let report = verify_object_store(&store, &keyspace).expect("probe");

        assert_eq!(report.cleanup, ProbeCleanupStatus::Complete);
        assert_eq!(
            store.operations(),
            [
                ObjectOperation::Put,
                ObjectOperation::Head,
                ObjectOperation::Get,
                ObjectOperation::List,
                ObjectOperation::List,
                ObjectOperation::Delete,
                ObjectOperation::List,
            ]
        );
    }

    #[test]
    fn fake_probe_reports_the_failed_stage_and_still_attempts_cleanup() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());
        store.fail_next(ObjectOperation::Get, ObjectStoreErrorCode::RateLimited);

        let error = verify_object_store(&store, &keyspace).expect_err("probe failure");

        assert_eq!(error.stage, ProbeStage::Get);
        assert_eq!(error.code, ObjectStoreErrorCode::RateLimited);
        assert_eq!(error.cleanup, ProbeCleanupStatus::Complete);
        assert!(store.operations().contains(&ObjectOperation::Delete));
    }

    #[test]
    fn live_r2_object_store_probe_uses_an_isolated_fixed_prefix_and_cleans_it() {
        if std::env::var("KOSH_RUN_R2_OBJECT_PROBE").as_deref() != Ok("1") {
            return;
        }
        let account_id =
            required_env("KOSH_LITESTREAM_R2_ACCOUNT_ID").expect("R2 account ID environment");
        let bucket = required_env("KOSH_LITESTREAM_R2_BUCKET").expect("R2 bucket environment");
        let access_key =
            required_env("KOSH_LITESTREAM_R2_ACCESS_KEY_ID").expect("R2 access key environment");
        let secret_key = required_env("KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY")
            .expect("R2 secret key environment");
        let jurisdiction = match std::env::var("KOSH_LITESTREAM_R2_JURISDICTION")
            .unwrap_or_else(|_| "DEFAULT".into())
            .as_str()
        {
            "DEFAULT" => R2Jurisdiction::Default,
            "EU" => R2Jurisdiction::Eu,
            "FEDRAMP" => R2Jurisdiction::Fedramp,
            value => panic!("unsupported R2 jurisdiction {value}"),
        };
        let target = R2Target {
            account_id: R2AccountId::parse(account_id).expect("R2 account ID"),
            jurisdiction,
            bucket: R2BucketName::parse(bucket).expect("R2 bucket"),
        };
        let credentials = R2Credentials::new(access_key, secret_key).expect("R2 credentials");
        let keyspace = target.keyspace(&BackupSetId::new());
        let store =
            R2ObjectStore::new(target, keyspace.clone(), &credentials).expect("R2 object store");

        let report = verify_object_store(&store, &keyspace).expect("live R2 probe");

        assert_eq!(report.cleanup, ProbeCleanupStatus::Complete);
    }

    fn required_env(name: &str) -> Result<String, String> {
        std::env::var(name)
            .map_err(|_| format!("{name} is required"))
            .and_then(|value| {
                if value.is_empty() {
                    Err(format!("{name} is empty"))
                } else {
                    Ok(value)
                }
            })
    }
}
