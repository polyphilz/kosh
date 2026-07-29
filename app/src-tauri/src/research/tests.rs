use std::io::Cursor;

use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::TempDir;

use crate::database::{
    drafts::{SaveDraftInput, SaveDraftWrite},
    media::{
        IngestAttachmentMetadata, IngestGenericAttachmentWrite, StagedAttachment, TextFileSegment,
    },
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    Database, DatabasePaths, DeleteTidbitInput, EditTidbitInput, MediaLimits, SourceDraft,
    TidbitDraft,
};

use super::{
    mcp::ResearchMcpReply, EphemeralResearchMcpServer, ResearchErrorCode, ResearchEventKind,
    ResearchLimits, ResearchMcpSession, ResearchRun, EXACT_SEARCH_TOOL, HYBRID_SEARCH_TOOL,
    INSPECT_ATTACHMENT_SEGMENTS_TOOL, INSPECT_SOURCES_TOOL, MCP_PROTOCOL_VERSION,
    READ_CURRENT_TIDBIT_TOOL, READ_PASSAGE_CONTEXT_TOOL, RESEARCH_TOOL_NAMES,
};

const CAPTURE_DRAFT_ID: u64 = 0x100;

struct TestLibrary {
    _root: TempDir,
    database: Database,
    staging: std::path::PathBuf,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary research library");
        let staging = root.path().join("staging");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("research database");
        let library = Self {
            _root: root,
            database,
            staging,
        };
        library.save_capture("", 1);
        library
    }

    fn create_tidbit(
        &self,
        suffix: u64,
        body: &str,
        source_label: &str,
    ) -> crate::database::Tidbit {
        self.database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    title: Some(format!("Knowledge {suffix}")),
                    body_markdown: body.to_owned(),
                    sources: vec![SourceDraft {
                        label: Some(source_label.to_owned()),
                        url: Some(format!("https://example.com/source/{suffix}")),
                    }],
                },
                now_ms: suffix as i64,
                tidbit_id: id(suffix),
                revision_id: id(suffix + 1),
                source_ids: vec![id(suffix + 2)],
            })
            .expect("create research tidbit")
    }

    fn edit_tidbit(
        &self,
        tidbit: &crate::database::Tidbit,
        revision_suffix: u64,
        body: &str,
    ) -> crate::database::Tidbit {
        self.database
            .client()
            .edit_tidbit(EditTidbitWrite {
                input: EditTidbitInput {
                    id: tidbit.id.clone(),
                    expected_revision_id: tidbit.current_revision_id.clone(),
                    title: Some("Revised knowledge".into()),
                    body_markdown: body.into(),
                    sources: vec![SourceDraft {
                        label: Some("Revised source".into()),
                        url: Some("https://example.com/revised".into()),
                    }],
                },
                now_ms: revision_suffix as i64,
                revision_id: id(revision_suffix),
                source_ids: vec![id(revision_suffix + 1)],
            })
            .expect("edit research tidbit")
    }

    fn run(&self) -> ResearchRun {
        ResearchRun::from_read_only_connection(
            self.database
                .open_main_read_only()
                .expect("read-only research connection"),
            None,
            ResearchLimits::default(),
        )
        .expect("research run")
    }

    fn run_with_limits(&self, limits: ResearchLimits) -> ResearchRun {
        ResearchRun::from_read_only_connection(
            self.database
                .open_main_read_only()
                .expect("read-only research connection"),
            None,
            limits,
        )
        .expect("bounded research run")
    }

    fn save_capture(&self, body: &str, now_ms: i64) {
        self.database
            .client()
            .save_draft(SaveDraftWrite {
                input: SaveDraftInput {
                    context_key: "capture".into(),
                    tidbit_id: None,
                    base_revision_id: None,
                    title: None,
                    body_markdown: body.into(),
                    sources: Vec::new(),
                },
                now_ms,
                draft_id: id(CAPTURE_DRAFT_ID),
                media_limits: MediaLimits::default(),
            })
            .expect("save capture draft");
    }

    fn create_text_attachment(&self) -> String {
        let segments = (0..5)
            .map(|index| TextFileSegment {
                start_line: index * 2 + 1,
                end_line: index * 2 + 2,
                content: format!(
                    "attachment segment {index}\nsegment_{index}_exact_research_evidence"
                ),
            })
            .collect::<Vec<_>>();
        let bytes = segments
            .iter()
            .map(|segment| segment.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        let staged = StagedAttachment::from_reader(
            Cursor::new(bytes),
            &self.staging,
            &id(0x203),
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("stage research attachment");
        let attachment = self
            .database
            .client()
            .ingest_generic_attachment(IngestGenericAttachmentWrite {
                attachment: staged.write(IngestAttachmentMetadata {
                    attachment_id: id(0x200),
                    ingest_lease_id: id(0x201),
                    draft_id: id(CAPTURE_DRAFT_ID),
                    display_filename: "research-notes.txt".into(),
                    media_type: "text/plain".into(),
                    now_ms: 20,
                    limits: MediaLimits::default(),
                }),
                extraction_id: id(0x202),
                extraction: Some(Ok(segments)),
            })
            .expect("ingest research attachment");
        let body = format!(
            "Attachment owner.\n\n{{{{kosh:attachment:{}}}}}",
            attachment.attachment.id
        );
        self.save_capture(&body, 21);
        self.database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    title: Some("Attachment owner".into()),
                    body_markdown: body,
                    sources: vec![SourceDraft {
                        label: Some("Attachment source".into()),
                        url: Some("https://example.com/attachment".into()),
                    }],
                },
                now_ms: 22,
                tidbit_id: id(0x204),
                revision_id: id(0x205),
                source_ids: vec![id(0x206)],
            })
            .expect("create attachment owner");
        attachment.attachment.id
    }
}

fn id(suffix: u64) -> String {
    format!("019f547b-6200-7000-8000-{suffix:012x}")
}

fn item_string(output: &Value, index: usize, field: &str) -> String {
    output["items"][index][field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string items[{index}].{field}"))
        .to_owned()
}

fn response_value(reply: ResearchMcpReply) -> Value {
    match reply {
        ResearchMcpReply::Response(value) => value,
        ResearchMcpReply::AcceptedNotification => panic!("expected MCP response"),
    }
}

#[test]
fn exact_search_paginates_with_opaque_run_scoped_handles() {
    let library = TestLibrary::new();
    for suffix in [0x300, 0x310, 0x320] {
        library.create_tidbit(
            suffix,
            &format!(
                "# Shared\n\nshared_exact_research_term appears in note {suffix}.\n\nA second passage."
            ),
            "Notebook",
        );
    }
    let mut run = library.run();
    let first = run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "shared_exact_research_term", "limit": 1 }),
        )
        .expect("first exact page");
    assert_eq!(first["executionMode"], "EXACT");
    assert_eq!(first["items"].as_array().map(Vec::len), Some(1));
    let citation_handle = item_string(&first, 0, "citationHandle");
    let owner_handle = item_string(&first, 0, "ownerHandle");
    let cursor = first["nextCursor"]
        .as_str()
        .expect("search continuation")
        .to_owned();
    assert!(citation_handle.starts_with("cit_"));
    assert!(owner_handle.starts_with("own_"));
    assert!(!first.to_string().contains("019f547b"));
    assert_eq!(
        run.resolve_citation_handle(&citation_handle)
            .expect("trusted citation")
            .excerpt,
        first["items"][0]["excerpt"]
    );

    let second = run
        .call_tool(EXACT_SEARCH_TOOL, json!({ "cursor": cursor, "limit": 1 }))
        .expect("second exact page");
    assert_eq!(second["items"].as_array().map(Vec::len), Some(1));
    assert_ne!(citation_handle, item_string(&second, 0, "citationHandle"));

    let other_run = library.run();
    assert_eq!(
        other_run
            .resolve_citation_handle(&citation_handle)
            .expect_err("handle is scoped to one run")
            .code,
        ResearchErrorCode::HandleNotFound
    );
}

#[test]
fn hybrid_search_falls_back_to_lexical_without_a_model() {
    let library = TestLibrary::new();
    library.create_tidbit(
        0x400,
        "hybrid_fallback_research_evidence remains locally searchable.",
        "Local notes",
    );
    let output = library
        .run()
        .call_tool(
            HYBRID_SEARCH_TOOL,
            json!({ "query": "hybrid_fallback_research_evidence" }),
        )
        .expect("hybrid fallback");
    assert_eq!(output["executionMode"], "LEXICAL_ONLY");
    assert_eq!(output["semanticReadiness"], "WAITING_FOR_RUNTIME");
    assert_eq!(output["items"].as_array().map(Vec::len), Some(1));
}

#[test]
fn context_tidbit_and_sources_expand_only_trusted_handles() {
    let library = TestLibrary::new();
    let long_neighbors = (0..10)
        .map(|index| {
            format!(
                "## Neighbor {index}\n\nneighbor_{index}_research_context {}",
                "bounded authored context. ".repeat(40)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    library.create_tidbit(
        0x500,
        &format!("# Context\n\ncontext_anchor_research_evidence.\n\n{long_neighbors}"),
        "Context notebook",
    );
    let mut run = library.run();
    let search = run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "context_anchor_research_evidence" }),
        )
        .expect("context search");
    let citation_handle = item_string(&search, 0, "citationHandle");
    let owner_handle = item_string(&search, 0, "ownerHandle");

    let context = run
        .call_tool(
            READ_PASSAGE_CONTEXT_TOOL,
            json!({
                "citationHandle": citation_handle,
                "before": 1,
                "after": 2,
            }),
        )
        .expect("passage context");
    assert!(context["items"]
        .as_array()
        .is_some_and(|items| items.len() >= 2));
    assert!(context["items"]
        .as_array()
        .expect("context items")
        .iter()
        .all(|item| item["citationHandle"]
            .as_str()
            .is_some_and(|handle| handle.starts_with("cit_"))));

    let tidbit = run
        .call_tool(
            READ_CURRENT_TIDBIT_TOOL,
            json!({ "ownerHandle": owner_handle, "limit": 2 }),
        )
        .expect("current tidbit");
    assert_eq!(tidbit["displayTitle"], "Knowledge 1280");
    assert_eq!(tidbit["items"].as_array().map(Vec::len), Some(2));
    let mut tidbit_page = tidbit;
    let mut passage_count = 2;
    while let Some(cursor) = tidbit_page["nextCursor"].as_str() {
        tidbit_page = run
            .call_tool(
                READ_CURRENT_TIDBIT_TOOL,
                json!({ "cursor": cursor, "limit": 2 }),
            )
            .expect("continued current tidbit");
        passage_count += tidbit_page["items"]
            .as_array()
            .expect("tidbit passage page")
            .len();
    }
    assert!(passage_count > 2);

    let sources = run
        .call_tool(
            INSPECT_SOURCES_TOOL,
            json!({ "citationHandle": item_string(&search, 0, "citationHandle") }),
        )
        .expect("trusted sources");
    assert_eq!(sources["items"][0]["label"], "Context notebook");
    assert_eq!(
        sources["items"][0]["url"],
        "https://example.com/source/1280"
    );
}

#[test]
fn valid_source_lists_are_cursor_paginated() {
    let library = TestLibrary::new();
    let sources = (0..15)
        .map(|index| SourceDraft {
            label: Some(format!("Source {index:02}")),
            url: Some(format!("https://example.com/many/{index}")),
        })
        .collect::<Vec<_>>();
    library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: Some("Many sources".into()),
                body_markdown: "many_sources_research_evidence.".into(),
                sources,
            },
            now_ms: 0xa00,
            tidbit_id: id(0xa00),
            revision_id: id(0xa01),
            source_ids: (0..15).map(|index| id(0xa10 + index)).collect(),
        })
        .expect("create many-source tidbit");
    let mut run = library.run();
    let search = run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "many_sources_research_evidence" }),
        )
        .expect("many-source search");
    let owner_handle = item_string(&search, 0, "ownerHandle");
    let mut page = run
        .call_tool(
            INSPECT_SOURCES_TOOL,
            json!({ "ownerHandle": owner_handle, "limit": 4 }),
        )
        .expect("first source page");
    let mut labels = Vec::new();
    loop {
        labels.extend(
            page["items"]
                .as_array()
                .expect("source page")
                .iter()
                .map(|source| source["label"].as_str().expect("source label").to_owned()),
        );
        let Some(cursor) = page["nextCursor"].as_str() else {
            break;
        };
        page = run
            .call_tool(
                INSPECT_SOURCES_TOOL,
                json!({ "cursor": cursor, "limit": 4 }),
            )
            .expect("continued source page");
    }
    assert_eq!(labels.len(), 15);
    assert_eq!(labels.first().map(String::as_str), Some("Source 00"));
    assert_eq!(labels.last().map(String::as_str), Some("Source 14"));
}

#[test]
fn stale_and_deleted_content_never_silently_retargets_handles() {
    let library = TestLibrary::new();
    let created = library.create_tidbit(
        0x600,
        "stale_handle_research_evidence in the first revision.",
        "First source",
    );
    let mut stale_run = library.run();
    let search = stale_run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "stale_handle_research_evidence" }),
        )
        .expect("stale fixture search");
    let citation_handle = item_string(&search, 0, "citationHandle");
    let owner_handle = item_string(&search, 0, "ownerHandle");
    library.edit_tidbit(
        &created,
        0x610,
        "A replacement revision with different evidence.",
    );
    assert_eq!(
        stale_run
            .call_tool(
                READ_CURRENT_TIDBIT_TOOL,
                json!({ "ownerHandle": owner_handle }),
            )
            .expect_err("stale owner rejected")
            .code,
        ResearchErrorCode::StaleContent
    );
    assert_eq!(
        stale_run
            .call_tool(
                READ_PASSAGE_CONTEXT_TOOL,
                json!({ "citationHandle": citation_handle }),
            )
            .expect_err("historical context rejected")
            .code,
        ResearchErrorCode::StaleContent
    );

    let deletable = library.create_tidbit(
        0x620,
        "deleted_handle_research_evidence before deletion.",
        "Disposable source",
    );
    let mut deleted_run = library.run();
    let search = deleted_run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "deleted_handle_research_evidence" }),
        )
        .expect("deleted fixture search");
    let citation_handle = item_string(&search, 0, "citationHandle");
    let owner_handle = item_string(&search, 0, "ownerHandle");
    library
        .database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: deletable.id,
                expected_revision_id: deletable.current_revision_id,
            },
            0x630,
        )
        .expect("delete research tidbit");
    for error in [
        deleted_run
            .call_tool(
                READ_CURRENT_TIDBIT_TOOL,
                json!({ "ownerHandle": owner_handle }),
            )
            .expect_err("deleted owner rejected"),
        deleted_run
            .call_tool(
                READ_PASSAGE_CONTEXT_TOOL,
                json!({ "citationHandle": citation_handle }),
            )
            .expect_err("deleted context rejected"),
    ] {
        assert_eq!(error.code, ResearchErrorCode::ContentDeleted);
    }
}

#[test]
fn attachment_segments_are_bounded_paginated_and_citable() {
    let library = TestLibrary::new();
    let attachment_id = library.create_text_attachment();
    let mut run = library.run();
    let search = run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "segment_2_exact_research_evidence" }),
        )
        .expect("attachment search");
    let owner_handle = item_string(&search, 0, "ownerHandle");
    assert_eq!(search["items"][0]["evidenceKind"], "TEXT_LINES");
    assert!(!search.to_string().contains(&attachment_id));
    let other_passage = run
        .call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "segment_3_exact_research_evidence" }),
        )
        .expect("other attachment passage");
    assert_ne!(
        owner_handle,
        item_string(&other_passage, 0, "ownerHandle"),
        "attachment owner capabilities retain passage-specific provenance"
    );

    let first = run
        .call_tool(
            INSPECT_ATTACHMENT_SEGMENTS_TOOL,
            json!({ "ownerHandle": owner_handle, "limit": 2 }),
        )
        .expect("first attachment page");
    assert_eq!(first["displayFilename"], "research-notes.txt");
    assert_eq!(first["items"].as_array().map(Vec::len), Some(2));
    let cursor = first["nextCursor"]
        .as_str()
        .expect("attachment continuation")
        .to_owned();
    let second = run
        .call_tool(
            INSPECT_ATTACHMENT_SEGMENTS_TOOL,
            json!({ "cursor": cursor, "limit": 2 }),
        )
        .expect("second attachment page");
    assert_eq!(second["items"].as_array().map(Vec::len), Some(2));
    assert!(second["items"]
        .as_array()
        .expect("attachment evidence")
        .iter()
        .all(|item| item["citationHandle"].as_str().is_some()));
    let third = run
        .call_tool(
            INSPECT_ATTACHMENT_SEGMENTS_TOOL,
            json!({
                "cursor": second["nextCursor"]
                    .as_str()
                    .expect("final attachment continuation"),
                "limit": 2,
            }),
        )
        .expect("final attachment page");
    assert_eq!(third["items"].as_array().map(Vec::len), Some(1));
    assert!(third["nextCursor"].is_null());
}

#[test]
fn malformed_calls_and_budgets_fail_with_compact_events() {
    let library = TestLibrary::new();
    library.create_tidbit(
        0x700,
        &format!(
            "large_response_research_evidence {}",
            "bounded ".repeat(300)
        ),
        "Bounded source",
    );
    let limits = ResearchLimits {
        max_tool_calls: 4,
        max_response_bytes: 1_024,
        max_run_response_bytes: 2_048,
        ..ResearchLimits::default()
    };
    let mut run = library.run_with_limits(limits);

    assert_eq!(
        run.call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "large_response_research_evidence", "extra": true }),
        )
        .expect_err("unknown argument")
        .code,
        ResearchErrorCode::MalformedRequest
    );
    assert_eq!(
        run.call_tool(
            READ_PASSAGE_CONTEXT_TOOL,
            json!({ "citationHandle": "cit_missing", "before": 20 }),
        )
        .expect_err("passage limit")
        .code,
        ResearchErrorCode::LimitExceeded
    );
    assert_eq!(
        run.call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "large_response_research_evidence", "limit": 11 }),
        )
        .expect_err("result limit")
        .code,
        ResearchErrorCode::LimitExceeded
    );
    assert_eq!(
        run.call_tool(
            EXACT_SEARCH_TOOL,
            json!({ "query": "large_response_research_evidence" }),
        )
        .expect_err("response byte limit")
        .code,
        ResearchErrorCode::LimitExceeded
    );
    assert_eq!(
        run.call_tool(EXACT_SEARCH_TOOL, json!({ "query": "fifth call" }))
            .expect_err("tool call budget")
            .code,
        ResearchErrorCode::LimitExceeded
    );
    assert_eq!(
        run.call_tool(EXACT_SEARCH_TOOL, json!({ "query": "sixth call" }))
            .expect_err("exhausted tool call budget remains terminal")
            .code,
        ResearchErrorCode::LimitExceeded
    );

    assert_eq!(run.events().len(), 8);
    assert!(run
        .events()
        .chunks_exact(2)
        .all(|events| events[0].kind == ResearchEventKind::ToolRequest
            && events[1].kind == ResearchEventKind::ToolError));
    let serialized = serde_json::to_string(run.events()).expect("serialize compact events");
    assert!(!serialized.contains("large_response_research_evidence"));
    assert!(!serialized.contains("bounded bounded"));
}

#[test]
fn mcp_byte_budget_counts_text_and_structured_copies() {
    let library = TestLibrary::new();
    library.create_tidbit(
        0xb00,
        &format!(
            "mcp_envelope_budget_evidence {}",
            "duplicated response content. ".repeat(40)
        ),
        "Envelope source",
    );
    let arguments = json!({ "query": "mcp_envelope_budget_evidence" });
    let raw = library
        .run()
        .call_tool(EXACT_SEARCH_TOOL, arguments.clone())
        .expect("raw research output");
    let raw_bytes = serde_json::to_vec(&raw)
        .expect("serialize raw output")
        .len();
    let wrapped = json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&raw).expect("serialize text copy"),
        }],
        "structuredContent": raw,
        "isError": false,
    });
    let wrapped_bytes = serde_json::to_vec(&wrapped)
        .expect("serialize wrapped output")
        .len();
    assert!(raw_bytes >= 1_024);
    assert!(wrapped_bytes > raw_bytes);
    let limit = raw_bytes + (wrapped_bytes - raw_bytes) / 2;
    let limits = ResearchLimits {
        max_response_bytes: limit,
        max_run_response_bytes: limit * 2,
        ..ResearchLimits::default()
    };
    library
        .run_with_limits(limits)
        .call_tool(EXACT_SEARCH_TOOL, arguments.clone())
        .expect("raw output remains within the selected limit");
    let error = library
        .run_with_limits(limits)
        .call_tool_for_mcp(EXACT_SEARCH_TOOL, arguments)
        .expect("bounded MCP error result");
    assert_eq!(error["isError"], true);
    assert_eq!(
        error["structuredContent"]["error"]["code"],
        "LIMIT_EXCEEDED"
    );
}

#[test]
fn mcp_error_responses_are_bounded_and_counted() {
    let library = TestLibrary::new();
    let limits = ResearchLimits {
        max_tool_calls: 8,
        max_response_bytes: 1_024,
        max_run_response_bytes: 1_024,
        ..ResearchLimits::default()
    };
    let mut run = library.run_with_limits(limits);
    let oversized_field = "untrusted_argument_name_".repeat(800);
    let arguments = Value::Object(
        [(oversized_field.clone(), Value::Bool(true))]
            .into_iter()
            .collect(),
    );
    let response = run
        .call_tool_for_mcp(EXACT_SEARCH_TOOL, arguments.clone())
        .expect("first bounded MCP tool error");
    assert_eq!(response["isError"], true);
    assert!(
        serde_json::to_vec(&response)
            .expect("serialize MCP error")
            .len()
            <= limits.max_response_bytes
    );
    assert!(!response.to_string().contains(&oversized_field));
    assert!(run.events().last().is_some_and(
        |event| event.kind == ResearchEventKind::ToolError && event.response_bytes.is_some()
    ));
    assert!(run.response_bytes <= limits.max_run_response_bytes);

    assert_eq!(
        run.call_tool_for_mcp(EXACT_SEARCH_TOOL, arguments)
            .expect_err("cumulative error bytes are enforced")
            .code,
        ResearchErrorCode::LimitExceeded
    );
    assert!(run.response_bytes <= limits.max_run_response_bytes);
}

#[test]
fn research_requires_query_only_sqlite() {
    let connection = Connection::open_in_memory().expect("writable SQLite");
    let error =
        match ResearchRun::from_read_only_connection(connection, None, ResearchLimits::default()) {
            Ok(_) => panic!("writable connection accepted"),
            Err(error) => error,
        };
    assert_eq!(error.code, ResearchErrorCode::Unauthorized);
}

#[test]
fn mcp_requires_auth_initialization_and_an_allowlisted_tool() {
    let library = TestLibrary::new();
    library.create_tidbit(0x800, "mcp_authorized_research_evidence.", "MCP source");
    let mut session = ResearchMcpSession::new(library.run());
    let authorization = session.authorization_header();

    let unauthorized = response_value(session.handle_json(
        Some("Bearer wrong"),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    ));
    assert_eq!(unauthorized["error"]["code"], -32001);

    let before_initialize = response_value(session.handle_json(
        Some(&authorization),
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ));
    assert_eq!(before_initialize["error"]["code"], -32002);

    let initialize = response_value(
        session.handle_json(
            Some(&authorization),
            serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "initialize",
                "params": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" },
                },
            }))
            .expect("initialize JSON")
            .as_bytes(),
        ),
    );
    assert_eq!(
        initialize["result"]["protocolVersion"],
        MCP_PROTOCOL_VERSION
    );
    assert_eq!(
        initialize["result"]["capabilities"]["tools"]["listChanged"],
        false
    );

    let tools = response_value(session.handle_json(
        Some(&authorization),
        br#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}"#,
    ));
    assert_eq!(
        tools["result"]["tools"].as_array().map(Vec::len),
        Some(RESEARCH_TOOL_NAMES.len())
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .expect("tool catalog")
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true
            && tool["annotations"]["openWorldHint"] == false));

    let mutation = response_value(session.handle_json(
        Some(&authorization),
        br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"write_file","arguments":{"path":"/tmp/x"}}}"#,
    ));
    assert_eq!(mutation["error"]["code"], -32602);

    let search = response_value(
        session.handle_json(
            Some(&authorization),
            serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": EXACT_SEARCH_TOOL,
                    "arguments": { "query": "mcp_authorized_research_evidence" },
                },
            }))
            .expect("tool JSON")
            .as_bytes(),
        ),
    );
    assert_eq!(search["result"]["isError"], false);
    assert_eq!(
        search["result"]["structuredContent"]["items"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        search["result"]["content"][0]["text"],
        serde_json::to_string(&search["result"]["structuredContent"])
            .expect("backwards-compatible text content")
    );
}

#[test]
fn claude_bridge_uses_ephemeral_loopback_http_and_exact_tool_names() {
    let library = TestLibrary::new();
    let session = ResearchMcpSession::new(library.run());
    let bridge = session
        .bridge("http://127.0.0.1:43127/mcp")
        .expect("loopback bridge");
    assert_eq!(bridge.mcp_config()["mcpServers"]["kosh"]["type"], "http");
    assert_eq!(
        bridge.mcp_config()["mcpServers"]["kosh"]["headers"]["Authorization"],
        "Bearer ${KOSH_RESEARCH_MCP_TOKEN}"
    );
    assert_eq!(
        bridge.allowed_tools(),
        RESEARCH_TOOL_NAMES
            .iter()
            .map(|name| format!("mcp__kosh__{name}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(bridge.environment().0, "KOSH_RESEARCH_MCP_TOKEN");
    assert!(!format!("{bridge:?}").contains(bridge.environment().1));
    let arguments = bridge.claude_cli_arguments();
    assert!(arguments
        .iter()
        .any(|argument| argument == "--strict-mcp-config"));
    let exact_tools = bridge.allowed_tools().join(",");
    assert!(arguments
        .windows(2)
        .any(|arguments| arguments == ["--tools", exact_tools.as_str()]));
    assert!(arguments
        .windows(2)
        .any(|arguments| arguments == ["--allowed-tools", exact_tools.as_str()]));
    assert!(!exact_tools.contains("WebSearch"));
    assert!(!exact_tools.contains("WebFetch"));
    assert!(!arguments.join(" ").contains(bridge.environment().1));

    let error = session
        .bridge("https://example.com/mcp")
        .expect_err("remote bridge rejected");
    assert_eq!(error.code, ResearchErrorCode::Unauthorized);
}

#[test]
fn ephemeral_http_bridge_serves_authenticated_mcp_and_retains_citations() {
    let library = TestLibrary::new();
    library.create_tidbit(
        0x900,
        "http_bridge_research_evidence stays inside Kosh.",
        "HTTP bridge source",
    );
    let server = EphemeralResearchMcpServer::start(library.run()).expect("ephemeral MCP server");
    let bridge = server.bridge().expect("Claude bridge config");
    let authorization = format!("Bearer {}", bridge.environment().1);
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .expect("loopback HTTP client");

    let unauthorized = client
        .post(server.endpoint())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        .send()
        .expect("unauthorized request");
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);

    let wrong_origin = client
        .post(server.endpoint())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Authorization", &authorization)
        .header("Origin", "https://attacker.example")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}"#)
        .send()
        .expect("cross-origin request");
    assert_eq!(wrong_origin.status(), reqwest::StatusCode::FORBIDDEN);

    let initialize = client
        .post(server.endpoint())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Authorization", &authorization)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1" },
            },
        }))
        .send()
        .expect("initialize over HTTP");
    assert_eq!(initialize.status(), reqwest::StatusCode::OK);
    assert_eq!(
        initialize.json::<Value>().expect("initialize response")["result"]["protocolVersion"],
        MCP_PROTOCOL_VERSION
    );

    let missing_protocol = client
        .post(server.endpoint())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Authorization", &authorization)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/list",
            "params": {},
        }))
        .send()
        .expect("request without negotiated protocol");
    assert_eq!(missing_protocol.status(), reqwest::StatusCode::BAD_REQUEST);

    let search = client
        .post(server.endpoint())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Authorization", &authorization)
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": EXACT_SEARCH_TOOL,
                "arguments": { "query": "http_bridge_research_evidence" },
            },
        }))
        .send()
        .expect("search over HTTP");
    assert_eq!(search.status(), reqwest::StatusCode::OK);
    let search = search.json::<Value>().expect("search response");
    let citation_handle = search["result"]["structuredContent"]["items"][0]["citationHandle"]
        .as_str()
        .expect("HTTP citation handle");
    assert_eq!(
        server
            .resolve_citation_handle(citation_handle)
            .expect("server-side trusted citation")
            .excerpt,
        "http_bridge_research_evidence stays inside Kosh."
    );
}
