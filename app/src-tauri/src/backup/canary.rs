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
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{
    drafts::SaveDraftWrite,
    passages,
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    AppendResearchEventWrite, AttachmentIngestInput, CreateResearchRunWrite, Database,
    DatabasePaths, EditTidbitInput, LexicalSearchMode, MediaLimits, PrepareOffsiteCheckpointInput,
    SaveDraftInput, SaveOffsiteBackupConfigInput, SearchPassagesInput, SourceDraft, TidbitDraft,
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
        ImmutableLitestreamBinary, LitestreamConfig, LitestreamControl, SystemCommandExecutor,
        VerifiedLitestreamBinary,
    },
    object_store::{
        ObjectContentType, ObjectStore, PutCondition, PutMediaRequest, PutObjectOutcome,
        PutObjectRequest, R2ObjectStore,
    },
    owner::claim_remote_owner,
    restore::{
        discover_checkpoints, drill_checkpoint, install_checkpoint, stage_checkpoint,
        RemoteCheckpoint,
    },
};

const STARTUP_CANARY: &str = "koshstartupcanaryv1";
const STARTUP_CANARY_TITLE: &str = "Kosh progressive startup canary";
const STARTUP_CANARY_SOURCE: &str = "https://example.invalid/kosh-progressive-operability";
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
    research_citation_resolution: CanaryOutcome,
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
    research_runs: u64,
    research_citations: u64,
    exact_result_count: u64,
    resolved_source_url: String,
    historical_research_citations: u64,
}

struct SourceFixture {
    _source_root: tempfile::TempDir,
    paths: DatabasePaths,
    backup_set_id: BackupSetId,
    epoch: ReplicaEpochId,
    checkpoint: crate::database::PreparedOffsiteCheckpoint,
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
        let mut command = Command::new(binary.path());
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
    let source = create_source_fixture(&target);
    let keyspace = target.keyspace(&source.backup_set_id);
    let store = R2ObjectStore::new(target.clone(), keyspace.clone(), &credentials)
        .expect("canary object store");
    let mut cleanup = CanaryCleanup::new(store, keyspace.clone());

    let manifest = replicate_and_publish(
        &source,
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
        run_library_restore(
            &cleanup.store,
            &keyspace,
            &checkpoint,
            &target,
            &credentials,
            &evidence_root,
            &restored_root,
        );
        "LIBRARY"
    };
    let restored = verify_restored_library(&restored_root);
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
        research_citation_resolution: CanaryOutcome::Passed,
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

    let draft_id = Uuid::now_v7().to_string();
    database
        .client()
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: "capture".into(),
                tidbit_id: None,
                base_revision_id: None,
                title: None,
                body_markdown: String::new(),
                sources: Vec::new(),
            },
            now_ms: 20,
            draft_id: draft_id.clone(),
            media_limits: MediaLimits::default(),
        })
        .expect("canary draft");
    let attachment = database
        .ingest_attachment(
            AttachmentIngestInput {
                draft_id,
                display_filename: "recovery-evidence.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 21,
                limits: MediaLimits::default(),
            },
            Cursor::new(b"immutable attachment evidence".to_vec()),
        )
        .expect("canary attachment");
    let tidbit_id = Uuid::now_v7().to_string();
    let original_revision_id = Uuid::now_v7().to_string();
    let attachment_token = format!("{{{{kosh:attachment:{}}}}}", attachment.id);
    let original = database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some("Historical recovery evidence".into()),
                body_markdown: format!(
                    "The durable historical fact is forty-two.\n\n{attachment_token}"
                ),
                sources: vec![SourceDraft {
                    label: Some("Historical source".into()),
                    url: Some("https://example.com/historical-recovery".into()),
                }],
            },
            now_ms: 30,
            tidbit_id: tidbit_id.clone(),
            revision_id: original_revision_id.clone(),
            source_ids: vec![Uuid::now_v7().to_string()],
        })
        .expect("canary original tidbit");
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
    let citation =
        passages::resolve_citation(&connection, &passage_id).expect("canary citation resolution");
    drop(connection);
    let run_id = Uuid::now_v7().to_string();
    database
        .client()
        .create_research_run(CreateResearchRunWrite {
            id: run_id.clone(),
            rerun_of_id: None,
            query: "What is the durable historical fact?".into(),
            requested_model: Some("sonnet".into()),
            requested_effort: Some("high".into()),
            now_ms: 40,
        })
        .expect("canary research run");
    append_research_event(&database, &run_id, 1, "STARTED", json!({}), 41);
    let answer = grounded_answer(&citation);
    append_research_event(
        &database,
        &run_id,
        2,
        "GROUNDED_FINAL_OUTPUT",
        json!({"answer": answer}),
        42,
    );
    append_research_event(
        &database,
        &run_id,
        3,
        "FINISHED",
        json!({"outcome": "SUCCEEDED", "stderrTruncated": false}),
        43,
    );
    database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: tidbit_id,
                expected_revision_id: original.current_revision_id,
                title: Some("Current recovery evidence".into()),
                body_markdown: format!("Exact citrine recovery evidence.\n\n{attachment_token}"),
                sources: vec![SourceDraft {
                    label: Some("Current source".into()),
                    url: Some("https://example.com/current-recovery".into()),
                }],
            },
            now_ms: 50,
            revision_id: Uuid::now_v7().to_string(),
            source_ids: vec![Uuid::now_v7().to_string()],
        })
        .expect("canary current revision");
    database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some(STARTUP_CANARY_TITLE.into()),
                body_markdown: STARTUP_CANARY.into(),
                sources: vec![SourceDraft {
                    label: Some("Startup canary source".into()),
                    url: Some(STARTUP_CANARY_SOURCE.into()),
                }],
            },
            now_ms: 60,
            tidbit_id: Uuid::now_v7().to_string(),
            revision_id: Uuid::now_v7().to_string(),
            source_ids: vec![Uuid::now_v7().to_string()],
        })
        .expect("canary startup tidbit");

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
    let checkpoint_id = super::domain::CheckpointId::new();
    let created_at = UtcTimestamp::now().expect("canary checkpoint timestamp");
    let created_at_ms = i64::try_from(
        created_at
            .unix_timestamp_nanos()
            .expect("canary checkpoint epoch")
            / 1_000_000,
    )
    .expect("canary checkpoint milliseconds");
    let checkpoint = database
        .client()
        .prepare_offsite_checkpoint(PrepareOffsiteCheckpointInput {
            checkpoint_id,
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: epoch.clone(),
            created_at_ms,
            kosh_version: env!("CARGO_PKG_VERSION").into(),
        })
        .expect("prepare canary checkpoint");
    database.shutdown().expect("close canary source");
    SourceFixture {
        _source_root: source_root,
        paths,
        backup_set_id,
        epoch,
        checkpoint,
    }
}

fn append_research_event(
    database: &Database,
    run_id: &str,
    sequence: u32,
    kind: &str,
    fields: Value,
    now_ms: i64,
) {
    let mut payload = fields.as_object().cloned().expect("research event object");
    payload.insert("runId".into(), json!(run_id));
    payload.insert("sequence".into(), json!(sequence));
    payload.insert("kind".into(), json!(kind));
    database
        .client()
        .append_research_event(AppendResearchEventWrite {
            run_id: run_id.into(),
            sequence,
            kind: kind.into(),
            payload: Value::Object(payload),
            now_ms,
        })
        .expect("append canary research event");
}

fn grounded_answer(citation: &crate::database::CitationResolution) -> Value {
    let markdown = "The durable historical fact is forty-two.【1】";
    let start = markdown.find('【').expect("citation marker");
    json!({
        "markdown": markdown,
        "citations": [{
            "number": 1,
            "label": "Historical recovery evidence",
            "evidenceKind": "AUTHORED_TIDBIT",
            "evidence": citation,
        }],
        "mentions": [{
            "citationNumber": 1,
            "startByte": start,
            "endByte": markdown.len(),
        }],
        "issues": [],
    })
}

fn replicate_and_publish(
    source: &SourceFixture,
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

    let mut interrupted = CanaryChild::spawn(&binary, runtime.paths().config(), credentials);
    wait_for_socket(interrupted.child_mut(), runtime.paths().socket());
    wait_for_replication_progress(
        interrupted.child_mut(),
        store,
        keyspace,
        replica_path.as_str(),
    );
    interrupted.kill_and_wait();
    remove_socket(runtime.paths().socket());

    let mut daemon = CanaryChild::spawn(&binary, runtime.paths().config(), credentials);
    wait_for_socket(daemon.child_mut(), runtime.paths().socket());
    let control = CommandLitestreamControl::new(
        binary.path().to_owned(),
        runtime.paths().socket().to_owned(),
        60,
        SystemCommandExecutor,
    );
    let sync = control
        .sync_remote(&source.paths.main)
        .expect("real R2 remote sync");
    assert_eq!(sync.replica_txid, Some(sync.txid));
    daemon.terminate_and_wait();

    claim_remote_owner(
        store,
        keyspace,
        &source.backup_set_id,
        &source.epoch,
        &BackupWriterId::new(),
    )
    .expect("claim canary remote owner");
    upload_referenced_media(&source.paths, store, keyspace);
    let created_at = UtcTimestamp::from_unix_millis(source.checkpoint.created_at_ms)
        .expect("manifest timestamp");
    let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
        backup_set_id: source.checkpoint.backup_set_id.clone(),
        replica_epoch_id: source.checkpoint.replica_epoch_id.clone(),
        checkpoint_id: source.checkpoint.checkpoint_id.clone(),
        created_at,
        kosh_version: source.checkpoint.kosh_version.clone(),
        content_revision: source.checkpoint.content_revision,
        main_migration_head: source.checkpoint.main_migration_head,
        litestream_path: replica_path,
        txid: sync.txid.to_string(),
        media_migration_head: source.checkpoint.media_migration_head,
        referenced_hash_count: source.checkpoint.referenced_hash_count,
        referenced_total_bytes: source.checkpoint.referenced_total_bytes,
        referenced_hash_set_sha256: source.checkpoint.referenced_hash_set_sha256,
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

fn run_library_restore(
    store: &R2ObjectStore,
    keyspace: &R2Keyspace,
    checkpoint: &RemoteCheckpoint,
    target: &R2Target,
    credentials: &R2Credentials,
    evidence_root: &Path,
    restored_root: &Path,
) {
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
    let install = install_checkpoint(&DatabasePaths::new(restored_root), &staged)
        .expect("install canary recovery");
    assert!(install.safety_snapshot_id.is_none());
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

fn verify_restored_library(root: &Path) -> RestoredEvidence {
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
        research_runs: query_count(&main, "SELECT count(*) FROM research_run"),
        research_citations: query_count(
            &main,
            "SELECT coalesce(sum(json_array_length(final_answer_json, '$.citations')), 0)
             FROM research_run WHERE final_answer_json IS NOT NULL",
        ),
        exact_result_count: u64::try_from(after.len()).expect("exact result count"),
        resolved_source_url: after[0]
            .citation
            .sources
            .iter()
            .find_map(|source| source.url.clone())
            .expect("restored result source"),
        historical_research_citations: query_count(
            &main,
            "SELECT count(*)
             FROM research_run, json_each(research_run.final_answer_json, '$.citations') AS citation
             JOIN tidbit
               ON tidbit.id = json_extract(citation.value, '$.evidence.tidbit.id')
             WHERE research_run.status = 'COMPLETED'
               AND tidbit.current_revision_id
                   <> json_extract(citation.value, '$.evidence.tidbit.revisionId')",
        ),
    };
    assert!(evidence.active_tidbits >= 2);
    assert!(evidence.revisions >= 3);
    assert!(evidence.sources >= 3);
    assert!(evidence.attachments >= 1);
    assert!(evidence.media_blobs >= 1);
    assert!(evidence.search_documents >= 2);
    assert_eq!(evidence.research_runs, 1);
    assert_eq!(evidence.research_citations, 1);
    assert_eq!(evidence.historical_research_citations, 1);
    drop(media);
    drop(main);
    database.shutdown().expect("close restored library");
    evidence
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
