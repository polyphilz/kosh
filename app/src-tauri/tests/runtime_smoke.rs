#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    RuntimeProbe,
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
        }
    );
    assert!(!response.data_dir.contains("Application Support"));
}
