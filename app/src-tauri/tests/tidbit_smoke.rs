#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    Tidbit,
};

fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
) -> tauri::ipc::InvokeResponseBody {
    tauri::test::get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("test IPC URL"),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("successful IPC response")
}

#[test]
fn tidbit_create_and_load_cross_the_typed_ipc_boundary() {
    let data_root = TestDataRoot::new();
    let app = mock_app(
        &data_root,
        1_785_201_600_000,
        [
            "019f547b-6200-7000-8000-000000002001".to_owned(),
            "019f547b-6200-7000-8000-000000002002".to_owned(),
            "019f547b-6200-7000-8000-000000002003".to_owned(),
        ],
    );
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");

    let created = invoke(
        &window,
        "create_tidbit",
        serde_json::json!({
            "input": {
                "title": null,
                "bodyMarkdown": "# IPC thought\n\nExact body.",
                "sources": [{
                    "label": "Reference",
                    "url": "https://example.com/page#fragment"
                }]
            }
        }),
    )
    .deserialize::<Tidbit>()
    .expect("created tidbit payload");
    assert_eq!(created.display_title, "IPC thought");
    assert_eq!(
        created.sources[0].url.as_deref(),
        Some("https://example.com/page")
    );

    let loaded = invoke(
        &window,
        "load_tidbit",
        serde_json::json!({ "id": created.id }),
    )
    .deserialize::<Tidbit>()
    .expect("loaded tidbit payload");
    assert_eq!(loaded, created);
    assert!(!data_root
        .path()
        .to_string_lossy()
        .contains("Application Support"));
}
