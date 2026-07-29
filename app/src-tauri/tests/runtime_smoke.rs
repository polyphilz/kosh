#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    PassageEmbeddingIndexPhase, PassageEmbeddingIndexStatus, RuntimeProbe, SemanticRuntimePhase,
    SemanticRuntimeStatus,
};

#[test]
fn main_window_invokes_runtime_probe_with_temporary_state() {
    let data_root = TestDataRoot::new();
    let app = mock_app(
        &data_root,
        1_785_201_600_000,
        ["fixture-request-1".to_owned()],
    );
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");

    assert_eq!(window.label(), "main");

    let response = tauri::test::get_ipc_response(
        &window,
        tauri::webview::InvokeRequest {
            cmd: "runtime_probe".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test IPC URL"),
            body: tauri::ipc::InvokeBody::default(),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("runtime_probe IPC response")
    .deserialize::<RuntimeProbe>()
    .expect("runtime probe payload");

    assert_eq!(
        response,
        RuntimeProbe {
            data_dir: data_root.path().to_string_lossy().into_owned(),
            now_ms: 1_785_201_600_000,
            request_id: "fixture-request-1".to_owned(),
            startup_smoke_canary: None,
        }
    );
    assert!(!response.data_dir.contains("Application Support"));
}

#[test]
fn passage_embedding_progress_is_available_before_the_model_is_downloaded() {
    let data_root = TestDataRoot::new();
    let app = mock_app(&data_root, 1_785_201_600_000, std::iter::empty::<String>());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");

    let response = tauri::test::get_ipc_response(
        &window,
        tauri::webview::InvokeRequest {
            cmd: "passage_embedding_index_status".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test IPC URL"),
            body: tauri::ipc::InvokeBody::default(),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("passage embedding status IPC response")
    .deserialize::<PassageEmbeddingIndexStatus>()
    .expect("passage embedding status payload");

    assert_eq!(
        response.phase,
        PassageEmbeddingIndexPhase::WaitingForRuntime
    );
    assert_eq!(response.index_key, "jina_v1");
    assert_eq!(response.indexed_passages, 0);
    assert_eq!(response.total_passages, 0);
    assert!(!response.active);
}

#[test]
fn semantic_status_is_available_without_starting_or_downloading_the_model() {
    let data_root = TestDataRoot::new();
    let app = mock_app(&data_root, 1_785_201_600_000, std::iter::empty::<String>());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");

    let response = tauri::test::get_ipc_response(
        &window,
        tauri::webview::InvokeRequest {
            cmd: "semantic_runtime_status".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test IPC URL"),
            body: tauri::ipc::InvokeBody::default(),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("semantic status IPC response")
    .deserialize::<SemanticRuntimeStatus>()
    .expect("semantic status payload");

    assert_eq!(response.phase, SemanticRuntimePhase::Unavailable);
    assert_eq!(response.downloaded_bytes, 0);
    assert_eq!(response.model_bytes, 232_883_776);
    assert!(!response.runtime_running);
    assert!(!response.verified);
    assert!(!data_root.path().join("models").exists());
}

#[test]
fn diagnostics_and_empty_maintenance_are_available_through_typed_ipc() {
    let data_root = TestDataRoot::new();
    let app = mock_app(&data_root, 1_785_201_600_000, std::iter::empty::<String>());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");

    let diagnostics = invoke_json(&window, "load_maintenance_diagnostics");
    assert_eq!(diagnostics["applicationVersion"], "0.1.0");
    assert_eq!(diagnostics["library"]["activeTidbits"], 0);
    assert_eq!(diagnostics["backupPhase"], "COMING_LATER");
    assert_eq!(
        diagnostics["storage"]["dataRoot"],
        data_root.path().to_string_lossy().as_ref()
    );
    assert_eq!(diagnostics["mediaLimits"]["maxAttachmentsPerDraft"], 32);

    let integrity = invoke_json(&window, "run_integrity_check");
    assert_eq!(integrity["databaseOk"], true);
    assert_eq!(
        integrity["media"]["missingBlobAttachmentIds"],
        serde_json::json!([])
    );

    let first = invoke_json(&window, "rebuild_search_indexes");
    let second = invoke_json(&window, "rebuild_search_indexes");
    assert_eq!(first["operation"], "REBUILD_SEARCH");
    assert_eq!(first["changedItems"], 0);
    assert_eq!(second["changedItems"], 0);

    let reclaim = invoke_json(&window, "reclaim_eligible_media");
    assert_eq!(reclaim["operation"], "RECLAIM_MEDIA");
    assert_eq!(reclaim["changedItems"], 0);
    assert!(reclaim["safetySnapshotId"].is_null());
    assert!(!data_root.path().join("safety-snapshots").exists());
}

fn invoke_json(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
) -> serde_json::Value {
    tauri::test::get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test IPC URL"),
            body: tauri::ipc::InvokeBody::default(),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("maintenance IPC response")
    .deserialize()
    .expect("maintenance JSON payload")
}
