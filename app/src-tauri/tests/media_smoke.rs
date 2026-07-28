#![cfg(feature = "test-support")]

use kosh_lib::{
    test_support::{mock_app, TestDataRoot},
    Draft,
};

fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: tauri::ipc::InvokeBody,
) -> tauri::ipc::InvokeResponseBody {
    tauri::test::get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("test IPC URL"),
            body,
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        },
    )
    .expect("successful IPC response")
}

#[test]
fn attachment_bytes_cross_the_bounded_raw_ipc_boundary() {
    let data_root = TestDataRoot::new();
    let app = mock_app(
        &data_root,
        1_785_201_600_000,
        [
            "019f547b-6200-7000-8000-000000008001".to_owned(),
            "019f547b-6200-7000-8000-000000008002".to_owned(),
            "019f547b-6200-7000-8000-000000008003".to_owned(),
            "019f547b-6200-7000-8000-000000008004".to_owned(),
        ],
    );
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window");
    let draft = invoke(
        &window,
        "save_draft",
        tauri::ipc::InvokeBody::Json(serde_json::json!({
            "input": {
                "contextKey": "capture",
                "tidbitId": null,
                "baseRevisionId": null,
                "title": null,
                "bodyMarkdown": "",
                "sources": []
            }
        })),
    )
    .deserialize::<Draft>()
    .expect("saved media draft");
    let metadata = serde_json::to_vec(&serde_json::json!({
        "draftId": draft.id,
        "displayFilename": "ipc.txt",
        "mediaType": "text/plain"
    }))
    .expect("upload metadata");
    let mut payload = u32::try_from(metadata.len())
        .expect("metadata length")
        .to_be_bytes()
        .to_vec();
    payload.extend(metadata);
    payload.extend(b"raw attachment");

    let attachment = invoke(
        &window,
        "ingest_attachment",
        tauri::ipc::InvokeBody::Raw(payload),
    )
    .deserialize::<serde_json::Value>()
    .expect("attachment response");
    assert_eq!(attachment["displayFilename"], "ipc.txt");
    assert_eq!(attachment["mediaType"], "text/plain");
    assert_eq!(attachment["byteLength"], 14);
    assert_eq!(attachment["kind"], "TEXT");

    let report = invoke(
        &window,
        "media_integrity_scan",
        tauri::ipc::InvokeBody::default(),
    )
    .deserialize::<serde_json::Value>()
    .expect("media integrity response");
    assert_eq!(report["missingBlobAttachmentIds"], serde_json::json!([]));
    assert_eq!(report["corruptBlobSha256"], serde_json::json!([]));
}
