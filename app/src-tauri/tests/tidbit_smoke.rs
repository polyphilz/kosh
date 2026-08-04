#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    SearchExecutionMode, SearchPassagesResponse, SemanticSearchReadiness, Tidbit,
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
fn note_autosave_checkpoint_and_search_cross_the_typed_ipc_boundary() {
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

    let note_id = "019f547b-6200-7000-8000-000000002010";
    let saved = invoke(
        &window,
        "save_working_copy",
        serde_json::json!({
            "input": {
                "noteId": note_id,
                "baseRevisionId": null,
                "editGeneration": 1,
                "documentJson": r#"{"schemaVersion":1,"blocks":[{"id":"019f547b-6200-7000-8000-000000002011","type":"heading","props":{"level":1},"content":[{"type":"text","text":"IPC thought","styles":{}}],"children":[]},{"id":"019f547b-6200-7000-8000-000000002012","type":"paragraph","content":[{"type":"text","text":"Exact body.","styles":{}}],"children":[]}]}"#,
                "bodyMarkdown": "# IPC thought\n\nExact body.",
                "sources": [{
                    "label": "Reference",
                    "url": "https://example.com/page#fragment"
                }]
            }
        }),
    )
    .deserialize::<serde_json::Value>()
    .expect("saved working-copy payload");
    assert_eq!(saved["status"], "SAVED");

    let checkpointed = invoke(
        &window,
        "checkpoint_working_copy",
        serde_json::json!({
            "input": {
                "noteId": note_id,
                "expectedEditGeneration": 1
            }
        }),
    )
    .deserialize::<serde_json::Value>()
    .expect("checkpointed working-copy payload");
    assert_eq!(checkpointed["status"], "CHECKPOINTED");

    let created = invoke(&window, "load_tidbit", serde_json::json!({ "id": note_id }))
        .deserialize::<Tidbit>()
        .expect("loaded tidbit payload");
    assert_eq!(created.display_title, "IPC thought");
    assert!(created
        .document_json
        .contains("019f547b-6200-7000-8000-000000002012"));
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

    assert!(!data_root
        .path()
        .to_string_lossy()
        .contains("Application Support"));
}
