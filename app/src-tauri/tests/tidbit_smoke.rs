#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    Draft, SearchExecutionMode, SearchPassagesResponse, SemanticSearchReadiness, Tidbit,
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
            "019f547b-6200-7000-8000-000000002004".to_owned(),
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

    let search_results = invoke(
        &window,
        "search_passages",
        serde_json::json!({
            "input": {
                "query": "\"Exact body\"",
                "mode": "DEFAULT",
                "limit": 10
            }
        }),
    )
    .deserialize::<SearchPassagesResponse>()
    .expect("search result payload");
    assert_eq!(
        search_results.execution_mode,
        SearchExecutionMode::LexicalOnly
    );
    assert_eq!(
        search_results.semantic_readiness,
        SemanticSearchReadiness::WaitingForRuntime
    );
    assert_eq!(search_results.results.len(), 1);
    assert_eq!(
        search_results.results[0]
            .citation
            .tidbit
            .as_ref()
            .map(|tidbit| tidbit.id.as_str()),
        Some(created.id.as_str())
    );

    let draft = invoke(
        &window,
        "save_draft",
        serde_json::json!({
            "input": {
                "contextKey": "capture",
                "tidbitId": null,
                "baseRevisionId": null,
                "title": "Recovered",
                "bodyMarkdown": "",
                "sources": [{ "label": null, "url": "" }]
            }
        }),
    )
    .deserialize::<Draft>()
    .expect("saved draft payload");
    let loaded_draft = invoke(
        &window,
        "load_draft",
        serde_json::json!({ "contextKey": "capture" }),
    )
    .deserialize::<Option<Draft>>()
    .expect("loaded draft payload");
    assert_eq!(loaded_draft, Some(draft.clone()));
    let cleared = invoke(
        &window,
        "clear_draft",
        serde_json::json!({
            "input": {
                "contextKey": "capture",
                "expectedUpdatedAtMs": draft.updated_at_ms
            }
        }),
    )
    .deserialize::<bool>()
    .expect("cleared draft payload");
    assert!(cleared);
    assert!(!data_root
        .path()
        .to_string_lossy()
        .contains("Application Support"));
}
