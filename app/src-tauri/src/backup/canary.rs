use std::{
    fs::{self, File},
    io::{Cursor, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg(test)]
use crate::database::CitationLocator;
use crate::database::{
    passages, AttachmentIngestInput, CitationResolution, CitationState, Database, DatabasePaths,
    LexicalSearchMode, MediaLimits, PrepareOffsiteCheckpointInput, SaveOffsiteBackupConfigInput,
    SearchPassagesInput, SourceDraft,
};

use super::{
    credentials::R2Credentials,
    domain::{
        BackupSetId, BackupWriterId, CheckpointManifestInput, CheckpointManifestV1, ContentSha256,
        R2AccountId, R2BucketName, R2Jurisdiction, R2Keyspace, R2Target, ReplicaEpochId,
        UtcTimestamp,
    },
    litestream::{
        configure_credential_pipe_environment, write_aws_shared_credentials,
        CommandLitestreamControl, CommandLitestreamRestore, EphemeralLitestreamRuntime,
        ImmutableLitestreamBinary, LitestreamConfig, LitestreamControl, RelationalRestoreEngine,
        SystemCommandExecutor, VerifiedLitestreamBinary,
    },
    object_store::{
        ObjectContentType, ObjectStore, PutCondition, PutMediaRequest, PutObjectOutcome,
        PutObjectRequest, R2ObjectStore,
    },
    owner::claim_remote_owner,
    recovery_cli::install_staged_for_test,
    restore::{discover_checkpoints, drill_checkpoint, stage_checkpoint, RemoteCheckpoint},
};

const STARTUP_CANARY: &str = "koshstartupcanaryv1";
const STARTUP_CANARY_SOURCE: &str = "https://example.invalid/kosh-progressive-operability";
const INTERRUPTED_REPLICATION_WORKING_COPY_BYTES: usize = 4 * 1024 * 1024;
const RESTORE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CanaryOutcome {
    Passed,
    NotRun,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CanaryReport {
    schema_version: u32,
    result: CanaryOutcome,
    execution_mode: &'static str,
    source_head: String,
    source_tree_state: String,
    run_id: String,
    backup_set_id: String,
    checkpoint_id: String,
    checkpoint_manifest_sha256: String,
    interrupted_replication_retry: CanaryOutcome,
    immutable_manifest_published_last: CanaryOutcome,
    non_mutating_drill: CanaryOutcome,
    clean_directory_restore: CanaryOutcome,
    packaged_recovery_command: CanaryOutcome,
    normal_database_reopen: CanaryOutcome,
    search_rebuild: CanaryOutcome,
    citation_resolution: CanaryOutcome,
    historical_citation_resolution: CanaryOutcome,
    restored: RestoredEvidence,
    removed_remote_objects: u64,
    remote_residue_objects: u64,
    packaged_target_data_directory: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoredEvidence {
    active_tidbits: u64,
    revisions: u64,
    sources: u64,
    attachments: u64,
    media_blobs: u64,
    search_documents: u64,
    exact_result_count: u64,
    resolved_source_url: String,
    historical_citations: u64,
    interrupted_replication_working_copies: u64,
}

struct SourceFixture {
    _source_root: tempfile::TempDir,
    paths: DatabasePaths,
    backup_set_id: BackupSetId,
    epoch: ReplicaEpochId,
    historical_citation: CitationResolution,
    database: Option<Database>,
}

struct CanaryCleanup {
    store: R2ObjectStore,
    keyspace: R2Keyspace,
    armed: bool,
}

impl CanaryCleanup {
    fn new(store: R2ObjectStore, keyspace: R2Keyspace) -> Self {
        Self {
            store,
            keyspace,
            armed: true,
        }
    }

    fn cleanup(&mut self) -> Result<u64, &'static str> {
        let mut continuation = None;
        let mut keys = Vec::new();
        for _ in 0..100 {
            let page = self
                .store
                .list(&self.keyspace.root_prefix(), continuation.as_ref())
                .map_err(|_| "could not list the unique canary backup-set prefix")?;
            keys.extend(page.objects.into_iter().map(|object| object.key));
            match page.next {
                Some(next) => continuation = Some(next),
                None => break,
            }
        }
        for key in &keys {
            self.store
                .delete_canary_object(key)
                .map_err(|_| "could not delete an object from the unique canary backup set")?;
        }
        let residue = self
            .store
            .list(&self.keyspace.root_prefix(), None)
            .map_err(|_| "could not verify canary cleanup")?
            .objects
            .len();
        if residue != 0 {
            return Err("the unique canary backup set still contains objects");
        }
        self.armed = false;
        u64::try_from(keys.len()).map_err(|_| "canary object count overflow")
    }
}

impl Drop for CanaryCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.cleanup();
        }
    }
}

struct CanaryChild {
    child: Option<Child>,
}

impl CanaryChild {
    fn spawn(
        binary: &ImmutableLitestreamBinary,
        config: &Path,
        credentials: &R2Credentials,
    ) -> Self {
        binary
            .reverify_before_spawn()
            .expect("reverify canary Litestream");
        let mut command = Command::new(
            binary
                .resolved_command_path()
                .expect("resolve canary Litestream descriptor"),
        );
        command
            .args(["replicate", "-config"])
            .arg(config)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_credential_pipe_environment(&mut command);
        let mut child = command.spawn().expect("start canary Litestream");
        let mut stdin = child.stdin.take().expect("canary credential pipe");
        write_aws_shared_credentials(&mut stdin, credentials).expect("write canary credentials");
        drop(stdin);
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("canary child")
    }

    fn kill_and_wait(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn terminate_and_wait(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = i32::try_from(child.id()).expect("canary PID");
            // SAFETY: `pid` belongs to the live child owned by this guard.
            let result = unsafe { libc::kill(pid, libc::SIGTERM) };
            assert_eq!(result, 0, "terminate canary Litestream");
            let status = child.wait().expect("reap canary Litestream");
            assert!(
                status.success() || status.signal() == Some(libc::SIGTERM),
                "canary Litestream did not stop cleanly: {status}"
            );
        }
    }
}

impl Drop for CanaryChild {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

#[test]
#[ignore = "requires bucket-scoped R2 credentials and the pinned Litestream binary"]
fn live_r2_canary_restores_complete_kosh_library_and_cleans_unique_backup_set() {
    assert_eq!(
        required_environment("KOSH_RUN_R2_CANARY"),
        "1",
        "set KOSH_RUN_R2_CANARY=1 explicitly"
    );
    let run_id = Uuid::now_v7();
    let evidence_root = PathBuf::from(required_environment("KOSH_R2_CANARY_DATA_DIR"));
    assert!(!evidence_root.exists(), "canary evidence root must be new");
    assert!(
        evidence_root.parent().is_some_and(Path::is_dir),
        "canary evidence parent must exist"
    );
    create_private_directory(&evidence_root).expect("create canary evidence root");
    let source_head = required_environment("KOSH_R2_CANARY_HEAD");
    assert!(
        source_head.len() == 40 && source_head.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "canary source head must be a full Git commit"
    );
    let source_tree_state = required_environment("KOSH_R2_CANARY_TREE_STATE");
    assert!(
        matches!(source_tree_state.as_str(), "CLEAN" | "DIRTY"),
        "canary source tree state must be CLEAN or DIRTY"
    );
    let target = canary_target();
    let credentials = canary_credentials();
    let mut source = create_source_fixture(&target);
    let keyspace = target.keyspace(&source.backup_set_id);
    let store = R2ObjectStore::new(target.clone(), keyspace.clone(), &credentials)
        .expect("canary object store");
    let mut cleanup = CanaryCleanup::new(store, keyspace.clone());

    let manifest = replicate_and_publish(
        &mut source,
        &target,
        &credentials,
        &keyspace,
        &cleanup.store,
        &evidence_root,
    );
    let checkpoint = discover_checkpoints(&cleanup.store, &keyspace, &source.backup_set_id)
        .expect("discover real R2 checkpoint")
        .into_iter()
        .next()
        .expect("published real R2 checkpoint");
    assert_eq!(checkpoint.checkpoint_id(), manifest.checkpoint_id());

    let mut drill_runtime =
        EphemeralLitestreamRuntime::create().expect("isolated drill runtime paths");
    let verified = VerifiedLitestreamBinary::resolve_staged_for_test(Path::new(
        &required_environment("KOSH_LITESTREAM_PATH"),
    ))
    .expect("verified canary Litestream");
    let drill_binary = verified
        .stage_immutable(drill_runtime.paths())
        .expect("immutable drill Litestream");
    let drill_engine = CommandLitestreamRestore::new(
        &drill_binary,
        drill_runtime.paths(),
        &target,
        &keyspace.litestream(checkpoint.replica_epoch_id()),
        drill_runtime.source_database_path(),
        &credentials,
        RESTORE_TIMEOUT,
    )
    .expect("real R2 drill engine");
    let drill_root = evidence_root.join("drill");
    let drill = drill_checkpoint(
        &cleanup.store,
        &keyspace,
        &checkpoint,
        &drill_engine,
        drill_runtime.source_database_path(),
        &drill_root,
    )
    .expect("non-mutating real R2 drill");
    assert_eq!(drill.checkpoint_id, *checkpoint.checkpoint_id());
    assert!(drill.restored_media_count >= 1);
    assert!(!drill_root.exists());
    drop(drill_engine);
    drop(drill_binary);
    drill_runtime.cleanup().expect("clean drill runtime");

    let packaged = std::env::var_os("KOSH_R2_CANARY_PACKAGED_EXECUTABLE");
    let restored_root = packaged.as_ref().map_or_else(
        || evidence_root.join("restored"),
        |_| PathBuf::from(required_environment("KOSH_R2_CANARY_PACKAGED_DATA_DIR")),
    );
    if packaged.is_some() {
        let expected = evidence_root
            .join("packaged-home")
            .join("Library")
            .join("Application Support")
            .join("com.rohan.kosh");
        assert!(
            restored_root == expected && !restored_root.exists(),
            "packaged canary data target must be the exact new isolated location"
        );
        let parent = restored_root.parent().expect("packaged data parent");
        fs::create_dir_all(parent).expect("create packaged data parent");
        secure_owned_directories(parent, &evidence_root).expect("secure packaged directories");
    }
    let execution_mode = if let Some(executable) = packaged.as_deref() {
        run_packaged_restore(
            Path::new(executable),
            &source.backup_set_id,
            &target,
            &credentials,
            &restored_root,
        );
        "PACKAGED"
    } else {
        run_library_restore(LibraryRestoreRequest {
            store: &cleanup.store,
            keyspace: &keyspace,
            checkpoint: &checkpoint,
            backup_set_id: &source.backup_set_id,
            target: &target,
            credentials: &credentials,
            evidence_root: &evidence_root,
            restored_root: &restored_root,
        });
        "LIBRARY"
    };
    let restored = verify_restored_library(&restored_root, &source.historical_citation);
    let removed_remote_objects = cleanup.cleanup().expect("clean unique backup set");
    let report = CanaryReport {
        schema_version: 1,
        result: CanaryOutcome::Passed,
        execution_mode,
        source_head,
        source_tree_state,
        run_id: run_id.to_string(),
        backup_set_id: source.backup_set_id.to_string(),
        checkpoint_id: manifest.checkpoint_id().to_string(),
        checkpoint_manifest_sha256: hex_sha256(
            &manifest.to_json().expect("checkpoint manifest JSON"),
        ),
        interrupted_replication_retry: CanaryOutcome::Passed,
        immutable_manifest_published_last: CanaryOutcome::Passed,
        non_mutating_drill: CanaryOutcome::Passed,
        clean_directory_restore: CanaryOutcome::Passed,
        packaged_recovery_command: if packaged.is_some() {
            CanaryOutcome::Passed
        } else {
            CanaryOutcome::NotRun
        },
        normal_database_reopen: CanaryOutcome::Passed,
        search_rebuild: CanaryOutcome::Passed,
        citation_resolution: CanaryOutcome::Passed,
        historical_citation_resolution: CanaryOutcome::Passed,
        restored,
        removed_remote_objects,
        remote_residue_objects: 0,
        packaged_target_data_directory: packaged.map(|_| restored_root.to_string_lossy().into()),
    };
    write_json(&evidence_root.join("canary-report-v1.json"), &report).expect("write canary report");
}

fn create_source_fixture(target: &R2Target) -> SourceFixture {
    let source_root = tempfile::tempdir().expect("canary source root");
    let paths = DatabasePaths::new(source_root.path());
    let database = Database::initialize(paths.clone()).expect("canary source database");
    let backup_set_id = BackupSetId::new();
    let epoch = ReplicaEpochId::new();
    database
        .client()
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: 0,
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: epoch.clone(),
            enabled: true,
            target: target.clone(),
            now_ms: 10,
        })
        .expect("configure canary backup");

    let note_id = Uuid::now_v7().to_string();
    database
        .client()
        .save_working_copy_for_test(note_id.clone(), None, 1, String::new(), Vec::new(), 20)
        .expect("reserve canary working copy");
    let attachment = database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id: note_id.clone(),
                display_filename: "recovery-evidence.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 21,
                limits: MediaLimits::default(),
            },
            Cursor::new(b"immutable attachment evidence".to_vec()),
        )
        .expect("canary attachment");
    let original_revision_id = Uuid::now_v7().to_string();
    let attachment_token = format!("{{{{kosh:attachment:{}}}}}", attachment.id);
    database
        .client()
        .save_working_copy_for_test(
            note_id.clone(),
            None,
            2,
            format!("The durable historical fact is forty-two.\n\n{attachment_token}"),
            vec![SourceDraft {
                label: Some("Historical source".into()),
                url: Some("https://example.com/historical-recovery".into()),
            }],
            22,
        )
        .expect("save original canary working copy");
    let original = database
        .client()
        .checkpoint_working_copy_for_test(
            note_id.clone(),
            2,
            30,
            original_revision_id.clone(),
            vec![Uuid::now_v7().to_string()],
        )
        .expect("checkpoint original canary note")
        .note
        .expect("checkpointed original canary note");
    let connection = database
        .open_main_read_only()
        .expect("canary citation read");
    let passage_id = connection
        .query_row(
            "SELECT id FROM passage WHERE tidbit_revision_id = ?1 ORDER BY ordinal LIMIT 1",
            [&original_revision_id],
            |row| row.get::<_, String>(0),
        )
        .expect("canary historical passage");
    let historical_citation =
        passages::resolve_citation(&connection, &passage_id).expect("canary citation resolution");
    drop(connection);
    database
        .client()
        .save_working_copy_for_test(
            note_id.clone(),
            Some(original.current_revision_id),
            3,
            format!("Exact citrine recovery evidence.\n\n{attachment_token}"),
            vec![SourceDraft {
                label: Some("Current source".into()),
                url: Some("https://example.com/current-recovery".into()),
            }],
            50,
        )
        .expect("save current canary working copy");
    database
        .client()
        .checkpoint_working_copy_for_test(
            note_id,
            3,
            51,
            Uuid::now_v7().to_string(),
            vec![Uuid::now_v7().to_string()],
        )
        .expect("checkpoint current canary note");
    let startup_note_id = Uuid::now_v7().to_string();
    database
        .client()
        .save_working_copy_for_test(
            startup_note_id.clone(),
            None,
            1,
            STARTUP_CANARY.into(),
            vec![SourceDraft {
                label: Some("Startup canary source".into()),
                url: Some(STARTUP_CANARY_SOURCE.into()),
            }],
            60,
        )
        .expect("save startup canary working copy");
    database
        .client()
        .checkpoint_working_copy_for_test(
            startup_note_id,
            1,
            61,
            Uuid::now_v7().to_string(),
            vec![Uuid::now_v7().to_string()],
        )
        .expect("checkpoint startup canary note");

    let mut lease = 0_u64;
    while let Some(upload) = database
        .client()
        .claim_next_offsite_media_upload(
            70 + i64::try_from(lease).expect("lease timestamp"),
            Uuid::now_v7().to_string(),
        )
        .expect("claim canary media")
    {
        assert!(database
            .client()
            .complete_offsite_media_upload(
                upload,
                format!("\"canary-{lease}\""),
                80 + i64::try_from(lease).expect("completion timestamp"),
            )
            .expect("complete canary media"));
        lease += 1;
    }
    SourceFixture {
        _source_root: source_root,
        paths,
        backup_set_id,
        epoch,
        historical_citation,
        database: Some(database),
    }
}

fn interrupted_replication_payload() -> String {
    let salt = Uuid::now_v7();
    let mut payload = String::with_capacity(INTERRUPTED_REPLICATION_WORKING_COPY_BYTES);
    let mut block = 0_u64;
    while payload.len() < INTERRUPTED_REPLICATION_WORKING_COPY_BYTES {
        let mut hasher = Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(block.to_le_bytes());
        payload.push_str(&format!("{:x}", hasher.finalize()));
        block = block.checked_add(1).expect("canary payload block overflow");
    }
    payload.truncate(INTERRUPTED_REPLICATION_WORKING_COPY_BYTES);
    payload
}

fn prepare_canary_checkpoint(
    database: &Database,
    source: &SourceFixture,
) -> crate::database::PreparedOffsiteCheckpoint {
    let created_at = UtcTimestamp::now().expect("canary checkpoint timestamp");
    let created_at_ms = i64::try_from(
        created_at
            .unix_timestamp_nanos()
            .expect("canary checkpoint epoch")
            / 1_000_000,
    )
    .expect("canary checkpoint milliseconds");
    database
        .client()
        .prepare_offsite_checkpoint(PrepareOffsiteCheckpointInput {
            checkpoint_id: super::domain::CheckpointId::new(),
            backup_set_id: source.backup_set_id.clone(),
            replica_epoch_id: source.epoch.clone(),
            created_at_ms,
            kosh_version: env!("CARGO_PKG_VERSION").into(),
        })
        .expect("prepare canary checkpoint")
}

fn replicate_and_publish(
    source: &mut SourceFixture,
    target: &R2Target,
    credentials: &R2Credentials,
    keyspace: &R2Keyspace,
    store: &R2ObjectStore,
    evidence_root: &Path,
) -> CheckpointManifestV1 {
    let mut runtime =
        EphemeralLitestreamRuntime::create().expect("isolated canary replication runtime");
    let verified = VerifiedLitestreamBinary::resolve_staged_for_test(Path::new(
        &required_environment("KOSH_LITESTREAM_PATH"),
    ))
    .expect("verified replication binary");
    let binary = verified
        .stage_immutable(runtime.paths())
        .expect("immutable replication binary");
    let endpoint = target.endpoint();
    let replica_path = keyspace.litestream(&source.epoch);
    let config = LitestreamConfig {
        database_path: &source.paths.main,
        runtime: runtime.paths(),
        bucket: target.bucket.as_str(),
        replica_path: replica_path.as_str(),
        endpoint: &endpoint,
    }
    .render()
    .expect("canary Litestream config");
    runtime
        .paths()
        .write_config(&config)
        .expect("write canary config");
    let database = source
        .database
        .take()
        .expect("canary source database must be available exactly once");

    let runtime_config = runtime
        .paths()
        .config_command_path()
        .expect("bound canary config descriptor");
    let mut interrupted = CanaryChild::spawn(&binary, &runtime_config, credentials);
    wait_for_socket(interrupted.child_mut(), runtime.paths().socket());
    wait_for_replication_progress(
        interrupted.child_mut(),
        store,
        keyspace,
        replica_path.as_str(),
    );
    let interrupted_control = CommandLitestreamControl::new(
        binary
            .resolved_command_path()
            .expect("resolve interrupted control binary"),
        runtime.paths().socket().to_owned(),
        60,
        SystemCommandExecutor,
    );
    let baseline_txid = interrupted_control
        .sync_remote(&source.paths.main)
        .expect("establish remotely restorable canary baseline")
        .txid;
    let interrupted_working_copy = database
        .client()
        .save_working_copy_for_test(
            Uuid::now_v7().to_string(),
            None,
            1,
            interrupted_replication_payload(),
            Vec::new(),
            90,
        )
        .expect("write interrupted replication fence");
    assert_eq!(
        interrupted_working_copy.body_markdown.len(),
        INTERRUPTED_REPLICATION_WORKING_COPY_BYTES
    );
    let interrupted_txid = interrupted_control
        .sync_local(&source.paths.main)
        .expect("capture interrupted canary transaction")
        .txid;
    assert!(interrupted_txid > baseline_txid);
    interrupted.kill_and_wait();
    remove_socket(runtime.paths().socket());

    let mut proof_runtime =
        EphemeralLitestreamRuntime::create().expect("isolated interruption proof runtime");
    let proof_binary = verified
        .stage_immutable(proof_runtime.paths())
        .expect("immutable interruption proof Litestream");
    let proof_engine = CommandLitestreamRestore::new(
        &proof_binary,
        proof_runtime.paths(),
        target,
        &replica_path,
        proof_runtime.source_database_path(),
        credentials,
        RESTORE_TIMEOUT,
    )
    .expect("interruption proof restore engine");
    let proof_target = evidence_root.join("interrupted-replication-proof.sqlite3");
    let incomplete_plan = proof_engine
        .preview(
            proof_runtime.source_database_path(),
            &proof_target,
            baseline_txid,
        )
        .expect("observe the remotely restorable baseline after interruption");
    assert!(
        incomplete_plan.max_txid < interrupted_txid,
        "the interrupted transaction was already remotely restorable before Litestream was killed"
    );

    let mut daemon = CanaryChild::spawn(&binary, &runtime_config, credentials);
    wait_for_socket(daemon.child_mut(), runtime.paths().socket());
    let checkpoint = prepare_canary_checkpoint(&database, source);
    let control = CommandLitestreamControl::new(
        binary
            .resolved_command_path()
            .expect("resolve canary control binary"),
        runtime.paths().socket().to_owned(),
        60,
        SystemCommandExecutor,
    );
    let sync = control
        .sync_remote(&source.paths.main)
        .expect("real R2 remote sync");
    assert_eq!(sync.replica_txid, Some(sync.txid));
    assert!(
        sync.txid >= interrupted_txid,
        "resumed replication did not cover the interrupted transaction"
    );
    proof_engine
        .preview(
            proof_runtime.source_database_path(),
            &proof_target,
            interrupted_txid,
        )
        .expect("interrupted transaction must become exactly restorable after retry");
    daemon.terminate_and_wait();
    database.shutdown().expect("close canary source");
    drop(proof_engine);
    drop(proof_binary);
    proof_runtime
        .cleanup()
        .expect("clean interruption proof runtime");

    claim_remote_owner(
        store,
        keyspace,
        &source.backup_set_id,
        &source.epoch,
        &BackupWriterId::new(),
    )
    .expect("claim canary remote owner");
    upload_referenced_media(&source.paths, store, keyspace);
    let created_at =
        UtcTimestamp::from_unix_millis(checkpoint.created_at_ms).expect("manifest timestamp");
    let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
        backup_set_id: checkpoint.backup_set_id.clone(),
        replica_epoch_id: checkpoint.replica_epoch_id.clone(),
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        created_at,
        kosh_version: checkpoint.kosh_version.clone(),
        content_revision: checkpoint.content_revision,
        main_migration_head: checkpoint.main_migration_head,
        litestream_path: replica_path,
        txid: sync.txid.to_string(),
        media_migration_head: checkpoint.media_migration_head,
        referenced_hash_count: checkpoint.referenced_hash_count,
        referenced_total_bytes: checkpoint.referenced_total_bytes,
        referenced_hash_set_sha256: checkpoint.referenced_hash_set_sha256,
    })
    .expect("canary checkpoint manifest");
    let manifest_bytes = manifest.to_json().expect("canary manifest JSON");
    assert_eq!(
        store
            .put(PutObjectRequest {
                key: manifest.object_key(keyspace).expect("canary manifest key"),
                bytes: manifest_bytes.clone(),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .expect("publish canary manifest"),
        PutObjectOutcome::Stored
    );
    let readback = store
        .get(&manifest.object_key(keyspace).expect("canary readback key"))
        .expect("read back canary manifest");
    assert_eq!(readback.bytes, manifest_bytes);
    fs::write(
        evidence_root.join("checkpoint-manifest-v1.json"),
        &readback.bytes,
    )
    .expect("write checkpoint evidence");
    drop(control);
    drop(binary);
    runtime.cleanup().expect("clean replication runtime");
    manifest
}

fn upload_referenced_media(paths: &DatabasePaths, store: &R2ObjectStore, keyspace: &R2Keyspace) {
    let main = rusqlite::Connection::open(&paths.main).expect("canary main media references");
    let media = rusqlite::Connection::open(&paths.media).expect("canary media blobs");
    let mut statement = main
        .prepare(
            "SELECT DISTINCT sha256
             FROM (
                SELECT sha256 FROM attachment WHERE deleted_at IS NULL
                UNION ALL
                SELECT preview_sha256 FROM attachment_image
             )
             ORDER BY sha256",
        )
        .expect("canary media reference query");
    let hashes = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("canary media references")
        .collect::<Result<Vec<_>, _>>()
        .expect("canary media hashes");
    for bytes in hashes {
        let sha256 = ContentSha256::from_bytes(bytes.try_into().expect("32-byte media SHA"));
        let blob = media
            .query_row(
                "SELECT bytes FROM media_blob WHERE sha256 = ?1",
                [sha256.as_bytes().as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .expect("canary media bytes");
        assert_eq!(
            store
                .put_media(PutMediaRequest::new(keyspace, sha256, blob).expect("verified media"))
                .expect("upload canary media"),
            PutObjectOutcome::Stored
        );
    }
}

struct LibraryRestoreRequest<'a> {
    store: &'a R2ObjectStore,
    keyspace: &'a R2Keyspace,
    checkpoint: &'a RemoteCheckpoint,
    backup_set_id: &'a BackupSetId,
    target: &'a R2Target,
    credentials: &'a R2Credentials,
    evidence_root: &'a Path,
    restored_root: &'a Path,
}

fn run_library_restore(request: LibraryRestoreRequest<'_>) {
    let LibraryRestoreRequest {
        store,
        keyspace,
        checkpoint,
        backup_set_id,
        target,
        credentials,
        evidence_root,
        restored_root,
    } = request;
    let mut runtime =
        EphemeralLitestreamRuntime::create().expect("isolated canary restore runtime");
    let verified = VerifiedLitestreamBinary::resolve_staged_for_test(Path::new(
        &required_environment("KOSH_LITESTREAM_PATH"),
    ))
    .expect("verified restore Litestream");
    let binary = verified
        .stage_immutable(runtime.paths())
        .expect("immutable restore Litestream");
    let engine = CommandLitestreamRestore::new(
        &binary,
        runtime.paths(),
        target,
        &keyspace.litestream(checkpoint.replica_epoch_id()),
        runtime.source_database_path(),
        credentials,
        RESTORE_TIMEOUT,
    )
    .expect("canary restore engine");
    let staging_root = evidence_root.join("staged");
    let staged = stage_checkpoint(
        store,
        keyspace,
        checkpoint,
        &engine,
        runtime.source_database_path(),
        &staging_root,
    )
    .expect("stage canary recovery");
    install_staged_for_test(restored_root, &staged, backup_set_id, checkpoint)
        .expect("install canary recovery");
    drop(engine);
    drop(binary);
    runtime.cleanup().expect("clean restore runtime");
}

fn run_packaged_restore(
    executable: &Path,
    backup_set_id: &BackupSetId,
    target: &R2Target,
    credentials: &R2Credentials,
    restored_root: &Path,
) {
    assert!(
        executable.is_absolute() && executable.is_file(),
        "packaged recovery executable must be an absolute regular file"
    );
    let output = Command::new(executable)
        .args([
            "recovery",
            "remote-restore",
            backup_set_id.as_str(),
            "latest",
        ])
        .arg(restored_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("KOSH_LITESTREAM_R2_ACCOUNT_ID", target.account_id.as_str())
        .env(
            "KOSH_LITESTREAM_R2_JURISDICTION",
            target.jurisdiction.as_db_str(),
        )
        .env("KOSH_LITESTREAM_R2_BUCKET", target.bucket.as_str())
        .env(
            "KOSH_LITESTREAM_R2_ACCESS_KEY_ID",
            credentials.access_key_id(),
        )
        .env(
            "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY",
            credentials.secret_access_key(),
        )
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .expect("execute packaged recovery command");
    assert!(
        output.status.success(),
        "packaged recovery command failed with redacted stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 64 * 1024);
    let report: Value =
        serde_json::from_slice(&output.stdout).expect("packaged recovery receipt JSON");
    assert_eq!(report.get("result").and_then(Value::as_str), Some("PASSED"));
    assert_eq!(
        report.get("backupSetId").and_then(Value::as_str),
        Some(backup_set_id.as_str())
    );
    assert_eq!(
        report.get("safetySnapshotCreated").and_then(Value::as_bool),
        Some(false)
    );
}

fn verify_restored_library(
    root: &Path,
    historical_citation: &CitationResolution,
) -> RestoredEvidence {
    let database = Database::initialize(DatabasePaths::new(root)).expect("reopen restored library");
    let client = database.client();
    let before = client
        .search_passages(SearchPassagesInput {
            query: "citrine".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("restored exact search before rebuild");
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0]
            .citation
            .sources
            .iter()
            .find_map(|source| source.url.as_deref()),
        Some("https://example.com/current-recovery")
    );
    let rebuilt = client.rebuild_search().expect("rebuild restored search");
    assert!(rebuilt >= 2);
    let after = client
        .search_passages(SearchPassagesInput {
            query: "citrine".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("restored exact search after rebuild");
    assert_eq!(after.len(), 1);
    let main = database
        .open_main_read_only()
        .expect("inspect restored main");
    let media = database
        .open_media_read_only()
        .expect("inspect restored media");
    let evidence = RestoredEvidence {
        active_tidbits: query_count(
            &main,
            "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL",
        ),
        revisions: query_count(&main, "SELECT count(*) FROM tidbit_revision"),
        sources: query_count(&main, "SELECT count(*) FROM source"),
        attachments: query_count(&main, "SELECT count(*) FROM attachment"),
        media_blobs: query_count(&media, "SELECT count(*) FROM media_blob"),
        search_documents: query_count(&main, "SELECT count(*) FROM passage_search_document"),
        exact_result_count: u64::try_from(after.len()).expect("exact result count"),
        resolved_source_url: after[0]
            .citation
            .sources
            .iter()
            .find_map(|source| source.url.clone())
            .expect("restored result source"),
        historical_citations: verify_historical_citation(&main, historical_citation),
        interrupted_replication_working_copies: interrupted_replication_working_copy_count(&main),
    };
    assert!(evidence.active_tidbits >= 2);
    assert!(evidence.revisions >= 3);
    assert!(evidence.sources >= 3);
    assert!(evidence.attachments >= 1);
    assert!(evidence.media_blobs >= 1);
    assert!(evidence.search_documents >= 2);
    assert_eq!(evidence.historical_citations, 1);
    assert_eq!(evidence.interrupted_replication_working_copies, 1);
    drop(media);
    drop(main);
    database.shutdown().expect("close restored library");
    evidence
}

fn verify_historical_citation(
    connection: &rusqlite::Connection,
    stored: &CitationResolution,
) -> u64 {
    assert_eq!(stored.state, CitationState::Current);
    let resolved = passages::resolve_citation(connection, &stored.passage_id)
        .expect("resolve restored historical passage");
    assert_eq!(resolved.state, CitationState::Historical);
    assert!(
        same_citation_provenance(stored, &resolved),
        "restored citation changed its passage, revision, locator, or source provenance"
    );
    let tidbit = resolved
        .tidbit
        .as_ref()
        .expect("historical authored citation tidbit");
    let current_revision_id = connection
        .query_row(
            "SELECT current_revision_id FROM tidbit WHERE id = ?1",
            [tidbit.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("current revision for restored historical citation");
    assert_ne!(current_revision_id, tidbit.revision_id);
    1
}

fn same_citation_provenance(stored: &CitationResolution, resolved: &CitationResolution) -> bool {
    stored.passage_id == resolved.passage_id
        && stored.excerpt == resolved.excerpt
        && stored.heading_context == resolved.heading_context
        && stored.construction_version == resolved.construction_version
        && stored.locator == resolved.locator
        && stored.tidbit == resolved.tidbit
        && stored.attachment == resolved.attachment
        && stored.sources == resolved.sources
}

fn interrupted_replication_working_copy_count(connection: &rusqlite::Connection) -> u64 {
    let expected_bytes = i64::try_from(INTERRUPTED_REPLICATION_WORKING_COPY_BYTES)
        .expect("interrupted working-copy byte count");
    let count = connection
        .query_row(
            "SELECT count(*)
             FROM draft
             WHERE length(CAST(draft.body_markdown AS BLOB)) = ?1",
            [expected_bytes],
            |row| row.get::<_, i64>(0),
        )
        .expect("restored interrupted replication working copy");
    u64::try_from(count).expect("non-negative interrupted working-copy count")
}

#[test]
fn historical_canary_evidence_requires_exact_locator_and_source_provenance() {
    let target = R2Target {
        account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef")
            .expect("test account ID"),
        jurisdiction: R2Jurisdiction::from_db("DEFAULT").expect("test jurisdiction"),
        bucket: R2BucketName::parse("kosh-canary-test").expect("test bucket"),
    };
    let mut source = create_source_fixture(&target);
    let database = source.database.as_ref().expect("test canary database");
    let connection = database.open_main_read_only().expect("test canary read");
    let stored = &source.historical_citation;
    assert_eq!(verify_historical_citation(&connection, stored), 1);
    let resolved = passages::resolve_citation(&connection, &stored.passage_id)
        .expect("test historical passage resolution");
    assert!(same_citation_provenance(stored, &resolved));

    let mut wrong_locator = resolved.clone();
    wrong_locator.locator = CitationLocator::OcrRegion {
        region: serde_json::json!({"x": 0, "y": 0, "width": 1, "height": 1}),
    };
    assert!(!same_citation_provenance(stored, &wrong_locator));
    let mut wrong_sources = resolved;
    wrong_sources.sources.clear();
    assert!(!same_citation_provenance(stored, &wrong_sources));

    drop(connection);
    source
        .database
        .take()
        .expect("test canary database shutdown")
        .shutdown()
        .expect("close test canary database");
}

fn wait_for_socket(child: &mut Child, socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(metadata) = fs::metadata(socket) {
            use std::os::unix::fs::FileTypeExt;
            if metadata.file_type().is_socket() {
                assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
                return;
            }
        }
        assert!(
            child.try_wait().expect("canary child status").is_none(),
            "Litestream exited before creating the canary socket"
        );
        thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for the canary Litestream socket");
}

fn wait_for_replication_progress(
    child: &mut Child,
    store: &R2ObjectStore,
    keyspace: &R2Keyspace,
    replica_path: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        assert!(
            child.try_wait().expect("canary child status").is_none(),
            "Litestream exited before canary replication progress"
        );
        if store.list(&keyspace.root_prefix(), None).is_ok_and(|page| {
            page.objects
                .iter()
                .any(|object| object.key.as_str().starts_with(replica_path))
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("timed out waiting for interrupted canary replication progress");
}

fn remove_socket(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("remove interrupted canary socket: {error}"),
    }
}

fn canary_target() -> R2Target {
    R2Target {
        account_id: R2AccountId::parse(required_environment("KOSH_LITESTREAM_R2_ACCOUNT_ID"))
            .expect("canary account ID"),
        jurisdiction: R2Jurisdiction::from_db(&required_environment(
            "KOSH_LITESTREAM_R2_JURISDICTION",
        ))
        .expect("canary jurisdiction"),
        bucket: R2BucketName::parse(required_environment("KOSH_LITESTREAM_R2_BUCKET"))
            .expect("canary bucket"),
    }
}

fn canary_credentials() -> R2Credentials {
    R2Credentials::new(
        required_environment("KOSH_LITESTREAM_R2_ACCESS_KEY_ID"),
        required_environment("KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY"),
    )
    .expect("canary credentials")
}

fn required_environment(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} is required for the real-R2 canary"))
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    File::open(path)?.sync_all()
}

fn secure_owned_directories(mut path: &Path, boundary: &Path) -> std::io::Result<()> {
    loop {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() || !path.starts_with(boundary) {
            return Err(std::io::Error::other(
                "canary directory is outside the isolated evidence root",
            ));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        if path == boundary {
            return Ok(());
        }
        path = path
            .parent()
            .ok_or_else(|| std::io::Error::other("canary directory has no boundary"))?;
    }
}

fn query_count(connection: &rusqlite::Connection, sql: &str) -> u64 {
    let count = connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .expect("canary evidence count");
    u64::try_from(count).expect("non-negative canary count")
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("canary report has no parent"))?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::now_v7()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
