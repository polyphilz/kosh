mod http;
mod library;
mod mcp;

#[cfg(test)]
mod tests;

use std::{collections::HashMap, fmt::Write as _, path::Path, sync::Arc};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::database::{
    self, CitationLocator, CitationResolution, DatabaseError, DatabasePaths,
    SearchPassagesResponse, SemanticSearchReadiness, TidbitSource,
};

use library::ResearchLibrary;

pub use http::EphemeralResearchMcpServer;
pub use mcp::{
    research_tool_definitions, ClaudeMcpBridge, ResearchMcpReply, ResearchMcpSession,
    MCP_PROTOCOL_VERSION,
};

pub const HYBRID_SEARCH_TOOL: &str = "kosh_v1_hybrid_search";
pub const EXACT_SEARCH_TOOL: &str = "kosh_v1_exact_search";
pub const READ_PASSAGE_CONTEXT_TOOL: &str = "kosh_v1_read_passage_context";
pub const READ_CURRENT_TIDBIT_TOOL: &str = "kosh_v1_read_current_tidbit";
pub const INSPECT_SOURCES_TOOL: &str = "kosh_v1_inspect_sources";
pub const INSPECT_ATTACHMENT_SEGMENTS_TOOL: &str = "kosh_v1_inspect_attachment_segments";
pub const RESEARCH_TOOL_NAMES: [&str; 6] = [
    HYBRID_SEARCH_TOOL,
    EXACT_SEARCH_TOOL,
    READ_PASSAGE_CONTEXT_TOOL,
    READ_CURRENT_TIDBIT_TOOL,
    INSPECT_SOURCES_TOOL,
    INSPECT_ATTACHMENT_SEGMENTS_TOOL,
];

const RESEARCH_OUTPUT_VERSION: &str = "v1";
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_SEARCH_CANDIDATES_CEILING: u32 = 100;
const MAX_RESEARCH_ERROR_MESSAGE_BYTES: usize = 240;

pub trait ResearchQueryEmbedder: Send + Sync {
    fn embed_query(&self, query: &str) -> Result<Vec<f32>, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResearchLimits {
    pub max_tool_calls: u32,
    pub max_results_per_page: u32,
    pub max_passages_per_response: u32,
    pub max_search_candidates: u32,
    pub max_response_bytes: usize,
    pub max_run_response_bytes: usize,
    pub max_request_bytes: usize,
}

impl Default for ResearchLimits {
    fn default() -> Self {
        Self {
            max_tool_calls: 64,
            max_results_per_page: 10,
            max_passages_per_response: 12,
            max_search_candidates: 64,
            max_response_bytes: 64 * 1024,
            max_run_response_bytes: 2 * 1024 * 1024,
            max_request_bytes: 64 * 1024,
        }
    }
}

impl ResearchLimits {
    pub fn validate(self) -> Result<Self, ResearchError> {
        if self.max_tool_calls == 0 || self.max_tool_calls > 256 {
            return Err(ResearchError::invalid(
                "maxToolCalls must be between 1 and 256",
            ));
        }
        if self.max_results_per_page == 0 || self.max_results_per_page > 25 {
            return Err(ResearchError::invalid(
                "maxResultsPerPage must be between 1 and 25",
            ));
        }
        if self.max_passages_per_response == 0 || self.max_passages_per_response > 32 {
            return Err(ResearchError::invalid(
                "maxPassagesPerResponse must be between 1 and 32",
            ));
        }
        if self.max_search_candidates < self.max_results_per_page
            || self.max_search_candidates > MAX_SEARCH_CANDIDATES_CEILING
        {
            return Err(ResearchError::invalid(
                "maxSearchCandidates must cover one page and be at most 100",
            ));
        }
        if self.max_response_bytes < 1_024 || self.max_response_bytes > 256 * 1024 {
            return Err(ResearchError::invalid(
                "maxResponseBytes must be between 1024 and 262144",
            ));
        }
        if self.max_run_response_bytes < self.max_response_bytes
            || self.max_run_response_bytes > 16 * 1024 * 1024
        {
            return Err(ResearchError::invalid(
                "maxRunResponseBytes must cover one response and be at most 16777216",
            ));
        }
        if self.max_request_bytes < 1_024 || self.max_request_bytes > 256 * 1024 {
            return Err(ResearchError::invalid(
                "maxRequestBytes must be between 1024 and 262144",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchErrorCode {
    Unauthorized,
    UnknownTool,
    MalformedRequest,
    InvalidInput,
    LimitExceeded,
    HandleNotFound,
    WrongHandleKind,
    CursorNotFound,
    CursorWrongTool,
    StaleContent,
    ContentDeleted,
    ContentUnavailable,
    DatabaseUnavailable,
    EmbeddingUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchError {
    pub code: ResearchErrorCode,
    pub message: String,
}

impl std::fmt::Display for ResearchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResearchError {}

impl ResearchError {
    fn new(code: ResearchErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: truncate_utf8_bytes(&message.into(), MAX_RESEARCH_ERROR_MESSAGE_BYTES),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(ResearchErrorCode::InvalidInput, message)
    }

    fn malformed(message: impl Into<String>) -> Self {
        Self::new(ResearchErrorCode::MalformedRequest, message)
    }

    fn from_database(error: DatabaseError) -> Self {
        match error {
            DatabaseError::InvalidInput(message) => Self::invalid(message),
            DatabaseError::NotFound { .. } => Self::new(
                ResearchErrorCode::ContentUnavailable,
                "the requested Kosh content is unavailable",
            ),
            DatabaseError::TidbitDeleted { .. } => Self::new(
                ResearchErrorCode::ContentDeleted,
                "the requested tidbit is deleted",
            ),
            DatabaseError::StaleTidbit { .. } => Self::new(
                ResearchErrorCode::StaleContent,
                "the requested tidbit revision is stale",
            ),
            _ => Self::new(
                ResearchErrorCode::DatabaseUnavailable,
                "Kosh could not read the research library",
            ),
        }
    }

    fn from_sqlite(_error: rusqlite::Error) -> Self {
        Self::new(
            ResearchErrorCode::DatabaseUnavailable,
            "Kosh could not read the research library",
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchEventKind {
    ToolRequest,
    ToolResult,
    ToolError,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchEvent {
    pub ordinal: u32,
    pub kind: ResearchEventKind,
    pub tool: String,
    pub call_number: u32,
    pub argument_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ResearchErrorCode>,
}

#[derive(Clone)]
enum ResearchHandleRecord {
    Citation(Box<CitationResolution>),
    Resource(ResearchResourceSnapshot),
}

#[derive(Clone)]
enum ResearchResourceSnapshot {
    Tidbit {
        id: String,
        revision_id: String,
        sources: Vec<TidbitSource>,
    },
    Attachment {
        id: String,
        extraction_id: String,
        provenance_passage_id: String,
        display_filename: String,
        media_type: String,
        sources: Vec<TidbitSource>,
    },
}

#[derive(Clone)]
enum ResearchCursor {
    Search {
        exact: bool,
        response: SearchPassagesResponse,
        offset: usize,
    },
    Tidbit {
        owner_handle: String,
        offset: usize,
    },
    Attachment {
        owner_handle: String,
        display_filename: String,
        media_type: String,
        offset: usize,
    },
    Sources {
        citation_handle: Option<String>,
        owner_handle: Option<String>,
        sources: Vec<TidbitSource>,
        offset: usize,
    },
}

struct OpaqueIds {
    seed: String,
    counter: u64,
}

impl OpaqueIds {
    fn new() -> Self {
        Self {
            seed: uuid::Uuid::now_v7().to_string(),
            counter: 0,
        }
    }

    fn next(&mut self, prefix: &str) -> String {
        self.counter = self.counter.saturating_add(1);
        let digest = Sha256::digest(format!("{}:{prefix}:{}", self.seed, self.counter));
        let mut encoded = String::with_capacity(prefix.len() + 40);
        encoded.push_str(prefix);
        for byte in &digest[..20] {
            write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
        }
        encoded
    }
}

pub struct ResearchRun {
    library: ResearchLibrary,
    embedder: Option<Arc<dyn ResearchQueryEmbedder>>,
    limits: ResearchLimits,
    ids: OpaqueIds,
    handles: HashMap<String, ResearchHandleRecord>,
    citation_handles: HashMap<String, String>,
    resource_handles: HashMap<String, String>,
    cursors: HashMap<String, ResearchCursor>,
    events: Vec<ResearchEvent>,
    tool_calls: u32,
    response_bytes: usize,
}

impl ResearchRun {
    pub fn open(
        paths: &DatabasePaths,
        embedder: Option<Arc<dyn ResearchQueryEmbedder>>,
        limits: ResearchLimits,
    ) -> Result<Self, ResearchError> {
        let connection = database::connection::open_read_only(
            Path::new(&paths.main),
            database::connection::DatabaseKind::Main,
        )
        .map_err(ResearchError::from_database)?;
        Self::from_read_only_connection(connection, embedder, limits)
    }

    pub fn from_read_only_connection(
        connection: Connection,
        embedder: Option<Arc<dyn ResearchQueryEmbedder>>,
        limits: ResearchLimits,
    ) -> Result<Self, ResearchError> {
        let limits = limits.validate()?;
        let query_only = connection
            .pragma_query_value(None, "query_only", |row| row.get::<_, i64>(0))
            .map_err(ResearchError::from_sqlite)?;
        if query_only != 1 {
            return Err(ResearchError::new(
                ResearchErrorCode::Unauthorized,
                "research requires a query-only SQLite connection",
            ));
        }
        Ok(Self {
            library: ResearchLibrary::new(connection),
            embedder,
            limits,
            ids: OpaqueIds::new(),
            handles: HashMap::new(),
            citation_handles: HashMap::new(),
            resource_handles: HashMap::new(),
            cursors: HashMap::new(),
            events: Vec::new(),
            tool_calls: 0,
            response_bytes: 0,
        })
    }

    pub fn limits(&self) -> ResearchLimits {
        self.limits
    }

    pub fn call_tool(&mut self, tool: &str, arguments: Value) -> Result<Value, ResearchError> {
        self.call_tool_with_envelope(tool, arguments, |output| Ok(output.clone()))
    }

    pub(super) fn call_tool_for_mcp(
        &mut self,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, ResearchError> {
        if self.tool_calls >= self.limits.max_tool_calls {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the research run exhausted its tool-call budget",
            ));
        }
        match self.call_tool_with_envelope(tool, arguments, |output| {
            Ok(mcp_tool_response(output, false))
        }) {
            Ok(response) => Ok(response),
            Err(error) => self.account_mcp_error_response(error),
        }
    }

    fn call_tool_with_envelope<F>(
        &mut self,
        tool: &str,
        arguments: Value,
        envelope: F,
    ) -> Result<Value, ResearchError>
    where
        F: FnOnce(&Value) -> Result<Value, ResearchError>,
    {
        if self.tool_calls >= self.limits.max_tool_calls {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the research run exhausted its tool-call budget",
            ));
        }
        if tool.len() > MAX_TOOL_NAME_BYTES {
            return Err(ResearchError::new(
                ResearchErrorCode::UnknownTool,
                "the requested research tool is not authorized",
            ));
        }
        self.tool_calls = self.tool_calls.saturating_add(1);
        let call_number = self.tool_calls;
        let argument_bytes = serde_json::to_vec(&arguments)
            .map_err(|error| ResearchError::malformed(error.to_string()))?
            .len();
        self.push_event(ResearchEvent {
            ordinal: 0,
            kind: ResearchEventKind::ToolRequest,
            tool: tool.to_owned(),
            call_number,
            argument_bytes,
            response_bytes: None,
            item_count: None,
            error_code: None,
        });
        if argument_bytes > self.limits.max_request_bytes {
            let error = ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the research tool request exceeded its byte limit",
            );
            self.record_error(tool, call_number, argument_bytes, &error);
            return Err(error);
        }
        if !RESEARCH_TOOL_NAMES.contains(&tool) {
            let error = ResearchError::new(
                ResearchErrorCode::UnknownTool,
                "the requested research tool is not authorized",
            );
            self.record_error(tool, call_number, argument_bytes, &error);
            return Err(error);
        }

        let result = self.dispatch(tool, arguments);
        match result {
            Ok((output, item_count)) => {
                let response = envelope(&output)?;
                let bytes = serde_json::to_vec(&response)
                    .map_err(|error| ResearchError::malformed(error.to_string()))?
                    .len();
                if bytes > self.limits.max_response_bytes {
                    let error = ResearchError::new(
                        ResearchErrorCode::LimitExceeded,
                        "the research tool response exceeded its byte limit",
                    );
                    self.record_error(tool, call_number, argument_bytes, &error);
                    return Err(error);
                }
                if self.response_bytes.saturating_add(bytes) > self.limits.max_run_response_bytes {
                    let error = ResearchError::new(
                        ResearchErrorCode::LimitExceeded,
                        "the research run exhausted its response-byte budget",
                    );
                    self.record_error(tool, call_number, argument_bytes, &error);
                    return Err(error);
                }
                self.response_bytes += bytes;
                self.push_event(ResearchEvent {
                    ordinal: 0,
                    kind: ResearchEventKind::ToolResult,
                    tool: tool.to_owned(),
                    call_number,
                    argument_bytes,
                    response_bytes: Some(bytes),
                    item_count: Some(item_count),
                    error_code: None,
                });
                Ok(response)
            }
            Err(error) => {
                self.record_error(tool, call_number, argument_bytes, &error);
                Err(error)
            }
        }
    }

    fn account_mcp_error_response(&mut self, error: ResearchError) -> Result<Value, ResearchError> {
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "error": error,
        });
        let response = mcp_tool_response(&output, true);
        let bytes = serde_json::to_vec(&response)
            .map_err(|error| ResearchError::malformed(error.to_string()))?
            .len();
        if bytes > self.limits.max_response_bytes {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the research tool error exceeded its byte limit",
            ));
        }
        if self.response_bytes.saturating_add(bytes) > self.limits.max_run_response_bytes {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the research run exhausted its response-byte budget",
            ));
        }
        self.response_bytes += bytes;
        if let Some(event) = self
            .events
            .last_mut()
            .filter(|event| event.kind == ResearchEventKind::ToolError)
        {
            event.response_bytes = Some(bytes);
        }
        Ok(response)
    }

    pub fn events(&self) -> &[ResearchEvent] {
        &self.events
    }

    pub fn drain_events(&mut self) -> Vec<ResearchEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn resolve_citation_handle(
        &self,
        handle: &str,
    ) -> Result<&CitationResolution, ResearchError> {
        match self.handles.get(handle) {
            Some(ResearchHandleRecord::Citation(citation)) => Ok(citation),
            Some(ResearchHandleRecord::Resource(_)) => Err(ResearchError::new(
                ResearchErrorCode::WrongHandleKind,
                "this handle identifies an owner, not citation evidence",
            )),
            None => Err(ResearchError::new(
                ResearchErrorCode::HandleNotFound,
                "the citation handle is not valid for this research run",
            )),
        }
    }

    fn dispatch(&mut self, tool: &str, arguments: Value) -> Result<(Value, usize), ResearchError> {
        match tool {
            HYBRID_SEARCH_TOOL => self.search(arguments, false),
            EXACT_SEARCH_TOOL => self.search(arguments, true),
            READ_PASSAGE_CONTEXT_TOOL => self.read_passage_context(arguments),
            READ_CURRENT_TIDBIT_TOOL => self.read_current_tidbit(arguments),
            INSPECT_SOURCES_TOOL => self.inspect_sources(arguments),
            INSPECT_ATTACHMENT_SEGMENTS_TOOL => self.inspect_attachment_segments(arguments),
            _ => Err(ResearchError::new(
                ResearchErrorCode::UnknownTool,
                "the requested research tool is not authorized",
            )),
        }
    }

    fn search(&mut self, arguments: Value, exact: bool) -> Result<(Value, usize), ResearchError> {
        let input: SearchInput = parse_arguments(arguments)?;
        let limit = self.validate_page_limit(input.limit)?;
        let response = match (input.query, input.cursor) {
            (Some(query), None) => {
                let (query_embedding, fallback) = if exact {
                    (None, SemanticSearchReadiness::NotRequested)
                } else if let Some(embedder) = &self.embedder {
                    match embedder.embed_query(&query) {
                        Ok(embedding) => (Some(embedding), SemanticSearchReadiness::Ready),
                        Err(_) => (None, SemanticSearchReadiness::Failed),
                    }
                } else {
                    (None, SemanticSearchReadiness::WaitingForRuntime)
                };
                self.library.search(
                    query,
                    exact,
                    self.limits.max_search_candidates,
                    query_embedding.as_deref(),
                    fallback,
                )?
            }
            (None, Some(cursor)) => {
                let ResearchCursor::Search {
                    exact: cursor_exact,
                    response,
                    offset,
                } = self.take_cursor(
                    &cursor,
                    if exact {
                        "exact search"
                    } else {
                        "hybrid search"
                    },
                )?
                else {
                    return Err(ResearchError::new(
                        ResearchErrorCode::CursorWrongTool,
                        "the pagination cursor belongs to a different research tool",
                    ));
                };
                if cursor_exact != exact {
                    return Err(ResearchError::new(
                        ResearchErrorCode::CursorWrongTool,
                        "the pagination cursor belongs to a different search mode",
                    ));
                }
                return self.search_page(response, offset, exact, limit);
            }
            _ => {
                return Err(ResearchError::invalid(
                    "provide exactly one of query or cursor",
                ));
            }
        };
        self.search_page(response, 0, exact, limit)
    }

    fn search_page(
        &mut self,
        response: SearchPassagesResponse,
        offset: usize,
        exact: bool,
        limit: usize,
    ) -> Result<(Value, usize), ResearchError> {
        let end = (offset + limit).min(response.results.len());
        let page = response.results[offset..end].to_vec();
        for result in &page {
            self.library.validate_current_citation(&result.citation)?;
        }
        let items = page
            .iter()
            .map(|result| self.evidence(&result.citation, Some(result.score)))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = (end < response.results.len()).then(|| {
            self.store_cursor(ResearchCursor::Search {
                exact,
                response: response.clone(),
                offset: end,
            })
        });
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "executionMode": response.execution_mode,
            "semanticReadiness": response.semantic_readiness,
            "items": items,
            "nextCursor": next_cursor,
        });
        Ok((output, page.len()))
    }

    fn read_passage_context(&mut self, arguments: Value) -> Result<(Value, usize), ResearchError> {
        let input: PassageContextInput = parse_arguments(arguments)?;
        let before = input.before.unwrap_or(1);
        let after = input.after.unwrap_or(1);
        let passage_count = before.saturating_add(after).saturating_add(1);
        if passage_count > self.limits.max_passages_per_response {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the requested context exceeds the passage limit",
            ));
        }
        let citation = self
            .resolve_citation_handle(&input.citation_handle)?
            .clone();
        let passages = self
            .library
            .passage_context(&citation, before as usize, after as usize)?;
        let items = passages
            .iter()
            .map(|passage| self.evidence(passage, None))
            .collect::<Result<Vec<_>, _>>()?;
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "centerCitationHandle": input.citation_handle,
            "items": items,
        });
        Ok((output, passages.len()))
    }

    fn read_current_tidbit(&mut self, arguments: Value) -> Result<(Value, usize), ResearchError> {
        let input: OwnerPageInput = parse_arguments(arguments)?;
        let limit = self.validate_passage_limit(input.limit)?;
        match (input.owner_handle, input.cursor) {
            (Some(owner_handle), None) => {
                let snapshot = self.resolve_resource_handle(&owner_handle)?.clone();
                let (tidbit, passages, has_more) = self
                    .library
                    .current_tidbit_passage_page(&snapshot, 0, limit)?;
                self.tidbit_page(
                    owner_handle,
                    tidbit.title,
                    tidbit.display_title,
                    tidbit.revision_number,
                    passages,
                    0,
                    has_more,
                )
            }
            (None, Some(cursor)) => {
                let ResearchCursor::Tidbit {
                    owner_handle,
                    offset,
                } = self.take_cursor(&cursor, "read current tidbit")?
                else {
                    return Err(ResearchError::new(
                        ResearchErrorCode::CursorWrongTool,
                        "the pagination cursor belongs to a different research tool",
                    ));
                };
                let snapshot = self.resolve_resource_handle(&owner_handle)?.clone();
                let (tidbit, passages, has_more) = self
                    .library
                    .current_tidbit_passage_page(&snapshot, offset, limit)?;
                self.tidbit_page(
                    owner_handle,
                    tidbit.title,
                    tidbit.display_title,
                    tidbit.revision_number,
                    passages,
                    offset,
                    has_more,
                )
            }
            _ => Err(ResearchError::invalid(
                "provide exactly one of ownerHandle or cursor",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tidbit_page(
        &mut self,
        owner_handle: String,
        title: Option<String>,
        display_title: String,
        revision_number: i64,
        passages: Vec<CitationResolution>,
        offset: usize,
        has_more: bool,
    ) -> Result<(Value, usize), ResearchError> {
        let item_count = passages.len();
        let page = passages
            .iter()
            .map(|passage| self.evidence(passage, None))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = has_more.then(|| {
            self.store_cursor(ResearchCursor::Tidbit {
                owner_handle: owner_handle.clone(),
                offset: offset.saturating_add(item_count),
            })
        });
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "ownerHandle": owner_handle,
            "title": title,
            "displayTitle": display_title,
            "revisionNumber": revision_number,
            "items": page,
            "nextCursor": next_cursor,
        });
        Ok((output, item_count))
    }

    fn inspect_sources(&mut self, arguments: Value) -> Result<(Value, usize), ResearchError> {
        let input: InspectSourcesInput = parse_arguments(arguments)?;
        let limit = self.validate_page_limit(input.limit)?;
        let (citation_handle, owner_handle, sources, offset) = match (
            input.citation_handle.as_deref(),
            input.owner_handle.as_deref(),
            input.cursor.as_deref(),
        ) {
            (Some(citation_handle), None, None) => (
                Some(citation_handle.to_owned()),
                None,
                self.resolve_citation_handle(citation_handle)?
                    .sources
                    .clone(),
                0,
            ),
            (None, Some(owner_handle), None) => (
                None,
                Some(owner_handle.to_owned()),
                self.resolve_resource_handle(owner_handle)?
                    .sources()
                    .to_vec(),
                0,
            ),
            (None, None, Some(cursor)) => {
                let ResearchCursor::Sources {
                    citation_handle,
                    owner_handle,
                    sources,
                    offset,
                } = self.take_cursor(cursor, "inspect sources")?
                else {
                    return Err(ResearchError::new(
                        ResearchErrorCode::CursorWrongTool,
                        "the pagination cursor belongs to a different research tool",
                    ));
                };
                (citation_handle, owner_handle, sources, offset)
            }
            _ => {
                return Err(ResearchError::invalid(
                    "provide exactly one of citationHandle, ownerHandle, or cursor",
                ));
            }
        };
        let end = (offset + limit).min(sources.len());
        let items = sources[offset..end]
            .iter()
            .map(|source| {
                json!({
                    "label": source.label,
                    "url": source.url,
                })
            })
            .collect::<Vec<_>>();
        let next_cursor = (end < sources.len()).then(|| {
            self.store_cursor(ResearchCursor::Sources {
                citation_handle: citation_handle.clone(),
                owner_handle: owner_handle.clone(),
                sources,
                offset: end,
            })
        });
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "citationHandle": citation_handle,
            "ownerHandle": owner_handle,
            "items": items,
            "nextCursor": next_cursor,
        });
        Ok((output, end - offset))
    }

    fn inspect_attachment_segments(
        &mut self,
        arguments: Value,
    ) -> Result<(Value, usize), ResearchError> {
        let input: OwnerPageInput = parse_arguments(arguments)?;
        let limit = self.validate_passage_limit(input.limit)?;
        match (input.owner_handle, input.cursor) {
            (Some(owner_handle), None) => {
                let snapshot = self.resolve_resource_handle(&owner_handle)?.clone();
                let (passages, has_more) = self
                    .library
                    .current_attachment_passage_page(&snapshot, 0, limit)?;
                let ResearchResourceSnapshot::Attachment {
                    display_filename,
                    media_type,
                    ..
                } = snapshot
                else {
                    return Err(ResearchError::new(
                        ResearchErrorCode::WrongHandleKind,
                        "this tool requires an attachment owner handle",
                    ));
                };
                self.attachment_page(
                    owner_handle,
                    display_filename,
                    media_type,
                    passages,
                    0,
                    has_more,
                )
            }
            (None, Some(cursor)) => {
                let ResearchCursor::Attachment {
                    owner_handle,
                    display_filename,
                    media_type,
                    offset,
                } = self.take_cursor(&cursor, "inspect attachment segments")?
                else {
                    return Err(ResearchError::new(
                        ResearchErrorCode::CursorWrongTool,
                        "the pagination cursor belongs to a different research tool",
                    ));
                };
                let snapshot = self.resolve_resource_handle(&owner_handle)?.clone();
                let (passages, has_more) = self
                    .library
                    .current_attachment_passage_page(&snapshot, offset, limit)?;
                self.attachment_page(
                    owner_handle,
                    display_filename,
                    media_type,
                    passages,
                    offset,
                    has_more,
                )
            }
            _ => Err(ResearchError::invalid(
                "provide exactly one of ownerHandle or cursor",
            )),
        }
    }

    fn attachment_page(
        &mut self,
        owner_handle: String,
        display_filename: String,
        media_type: String,
        passages: Vec<CitationResolution>,
        offset: usize,
        has_more: bool,
    ) -> Result<(Value, usize), ResearchError> {
        let item_count = passages.len();
        let page = passages
            .iter()
            .map(|passage| self.evidence(passage, None))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = has_more.then(|| {
            self.store_cursor(ResearchCursor::Attachment {
                owner_handle: owner_handle.clone(),
                display_filename: display_filename.clone(),
                media_type: media_type.clone(),
                offset: offset.saturating_add(item_count),
            })
        });
        let output = json!({
            "version": RESEARCH_OUTPUT_VERSION,
            "ownerHandle": owner_handle,
            "displayFilename": display_filename,
            "mediaType": media_type,
            "items": page,
            "nextCursor": next_cursor,
        });
        Ok((output, item_count))
    }

    fn evidence(
        &mut self,
        citation: &CitationResolution,
        score: Option<f64>,
    ) -> Result<Value, ResearchError> {
        let citation_handle = self.register_citation(citation.clone());
        let (owner_handle, owner_kind, display_title, display_filename) =
            if let Some(tidbit) = &citation.tidbit {
                let owner_handle = self.register_resource(ResearchResourceSnapshot::Tidbit {
                    id: tidbit.id.clone(),
                    revision_id: tidbit.revision_id.clone(),
                    sources: citation.sources.clone(),
                });
                (
                    owner_handle,
                    "TIDBIT",
                    Some(tidbit.display_title.clone()),
                    None,
                )
            } else if let Some(attachment) = &citation.attachment {
                let owner_handle = self.register_resource(ResearchResourceSnapshot::Attachment {
                    id: attachment.id.clone(),
                    extraction_id: attachment.extraction_id.clone(),
                    provenance_passage_id: citation.passage_id.clone(),
                    display_filename: attachment.display_filename.clone(),
                    media_type: attachment.media_type.clone(),
                    sources: citation.sources.clone(),
                });
                (
                    owner_handle,
                    "ATTACHMENT",
                    None,
                    Some(attachment.display_filename.clone()),
                )
            } else {
                return Err(ResearchError::new(
                    ResearchErrorCode::ContentUnavailable,
                    "the evidence has no readable Kosh owner",
                ));
            };
        Ok(json!({
            "citationHandle": citation_handle,
            "ownerHandle": owner_handle,
            "ownerKind": owner_kind,
            "evidenceKind": evidence_kind(&citation.locator),
            "displayTitle": display_title,
            "displayFilename": display_filename,
            "excerpt": citation.excerpt,
            "headingContext": citation.heading_context,
            "locator": citation.locator,
            "sourceCount": citation.sources.len(),
            "score": score,
        }))
    }

    fn register_citation(&mut self, citation: CitationResolution) -> String {
        if let Some(handle) = self.citation_handles.get(&citation.passage_id) {
            return handle.clone();
        }
        let passage_id = citation.passage_id.clone();
        let handle = self.ids.next("cit_");
        self.handles.insert(
            handle.clone(),
            ResearchHandleRecord::Citation(Box::new(citation)),
        );
        self.citation_handles.insert(passage_id, handle.clone());
        handle
    }

    fn register_resource(&mut self, snapshot: ResearchResourceSnapshot) -> String {
        let key = snapshot.key();
        if let Some(handle) = self.resource_handles.get(&key) {
            return handle.clone();
        }
        let handle = self.ids.next("own_");
        self.handles
            .insert(handle.clone(), ResearchHandleRecord::Resource(snapshot));
        self.resource_handles.insert(key, handle.clone());
        handle
    }

    fn resolve_resource_handle(
        &self,
        handle: &str,
    ) -> Result<&ResearchResourceSnapshot, ResearchError> {
        match self.handles.get(handle) {
            Some(ResearchHandleRecord::Resource(resource)) => Ok(resource),
            Some(ResearchHandleRecord::Citation(_)) => Err(ResearchError::new(
                ResearchErrorCode::WrongHandleKind,
                "this handle identifies citation evidence, not an owner",
            )),
            None => Err(ResearchError::new(
                ResearchErrorCode::HandleNotFound,
                "the owner handle is not valid for this research run",
            )),
        }
    }

    fn store_cursor(&mut self, cursor: ResearchCursor) -> String {
        let handle = self.ids.next("cur_");
        self.cursors.insert(handle.clone(), cursor);
        handle
    }

    fn take_cursor(&mut self, cursor: &str, _tool: &str) -> Result<ResearchCursor, ResearchError> {
        self.cursors.remove(cursor).ok_or_else(|| {
            ResearchError::new(
                ResearchErrorCode::CursorNotFound,
                "the pagination cursor is invalid, expired, or belongs to another research run",
            )
        })
    }

    fn validate_page_limit(&self, requested: Option<u32>) -> Result<usize, ResearchError> {
        let limit = requested.unwrap_or(self.limits.max_results_per_page);
        if limit == 0 || limit > self.limits.max_results_per_page {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                format!(
                    "limit must be between 1 and {}",
                    self.limits.max_results_per_page
                ),
            ));
        }
        Ok(limit as usize)
    }

    fn validate_passage_limit(&self, requested: Option<u32>) -> Result<usize, ResearchError> {
        let default = self
            .limits
            .max_results_per_page
            .min(self.limits.max_passages_per_response);
        let limit = requested.unwrap_or(default);
        if limit == 0 || limit > self.limits.max_passages_per_response {
            return Err(ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                format!(
                    "limit must be between 1 and {}",
                    self.limits.max_passages_per_response
                ),
            ));
        }
        Ok(limit as usize)
    }

    fn push_event(&mut self, mut event: ResearchEvent) {
        event.ordinal = self.events.len() as u32;
        self.events.push(event);
    }

    fn record_error(
        &mut self,
        tool: &str,
        call_number: u32,
        argument_bytes: usize,
        error: &ResearchError,
    ) {
        self.push_event(ResearchEvent {
            ordinal: 0,
            kind: ResearchEventKind::ToolError,
            tool: tool.to_owned(),
            call_number,
            argument_bytes,
            response_bytes: None,
            item_count: None,
            error_code: Some(error.code),
        });
    }
}

impl ResearchResourceSnapshot {
    fn key(&self) -> String {
        match self {
            Self::Tidbit {
                id, revision_id, ..
            } => format!("tidbit:{id}:{revision_id}"),
            Self::Attachment {
                id,
                extraction_id,
                provenance_passage_id,
                ..
            } => format!("attachment:{id}:{extraction_id}:{provenance_passage_id}"),
        }
    }

    fn sources(&self) -> &[TidbitSource] {
        match self {
            Self::Tidbit { sources, .. } | Self::Attachment { sources, .. } => sources,
        }
    }
}

fn evidence_kind(locator: &CitationLocator) -> &'static str {
    match locator {
        CitationLocator::MarkdownBlocks { .. } => "AUTHORED",
        CitationLocator::PdfPage { .. } => "PDF_PAGE",
        CitationLocator::OcrRegion { .. } => "IMAGE_OCR",
        CitationLocator::TextLines { .. } => "TEXT_LINES",
    }
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, ResearchError> {
    serde_json::from_value(arguments)
        .map_err(|error| ResearchError::malformed(format!("invalid tool arguments: {error}")))
}

fn mcp_tool_response(output: &Value, is_error: bool) -> Value {
    let text =
        serde_json::to_string(output).expect("serializing a research MCP output value cannot fail");
    json!({
        "content": [{
            "type": "text",
            "text": text,
        }],
        "structuredContent": output,
        "isError": is_error,
    })
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchInput {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PassageContextInput {
    citation_handle: String,
    #[serde(default)]
    before: Option<u32>,
    #[serde(default)]
    after: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnerPageInput {
    #[serde(default)]
    owner_handle: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InspectSourcesInput {
    #[serde(default)]
    citation_handle: Option<String>,
    #[serde(default)]
    owner_handle: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}
