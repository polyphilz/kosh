use std::fmt;

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    ResearchError, ResearchErrorCode, ResearchRun, EXACT_SEARCH_TOOL, HYBRID_SEARCH_TOOL,
    INSPECT_ATTACHMENT_SEGMENTS_TOOL, INSPECT_SOURCES_TOOL, READ_CURRENT_TIDBIT_TOOL,
    READ_PASSAGE_CONTEXT_TOOL, RESEARCH_TOOL_NAMES,
};

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const COMPATIBLE_MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MCP_SERVER_NAME: &str = "kosh";
const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MCP_TOKEN_ENV: &str = "KOSH_RESEARCH_MCP_TOKEN";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResearchMcpReply {
    Response(Value),
    AcceptedNotification,
}

pub struct ResearchMcpSession {
    run: ResearchRun,
    bearer_token: String,
    protocol_version: Option<&'static str>,
}

impl ResearchMcpSession {
    pub fn new(run: ResearchRun) -> Self {
        let nonce = uuid::Uuid::now_v7().to_string();
        let digest = Sha256::digest(format!("kosh-research-mcp:{nonce}"));
        let token = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self {
            run,
            bearer_token: format!("krm_{token}"),
            protocol_version: None,
        }
    }

    pub fn authorization_header(&self) -> String {
        format!("Bearer {}", self.bearer_token)
    }

    pub fn bridge(&self, endpoint: &str) -> Result<ClaudeMcpBridge, ResearchError> {
        ClaudeMcpBridge::new(endpoint, self.bearer_token.clone())
    }

    pub fn run(&self) -> &ResearchRun {
        &self.run
    }

    pub fn run_mut(&mut self) -> &mut ResearchRun {
        &mut self.run
    }

    pub fn handle_json(
        &mut self,
        authorization_header: Option<&str>,
        body: &[u8],
    ) -> ResearchMcpReply {
        if !self.is_authorized(authorization_header) {
            return ResearchMcpReply::Response(json_rpc_error(
                Value::Null,
                -32001,
                "research session authorization failed",
                None,
            ));
        }
        if body.len() > self.run.limits().max_request_bytes {
            return ResearchMcpReply::Response(json_rpc_error(
                Value::Null,
                -32600,
                "request exceeds the research byte limit",
                None,
            ));
        }
        let request = match serde_json::from_slice::<JsonRpcRequest>(body) {
            Ok(request) => request,
            Err(error) => {
                return ResearchMcpReply::Response(json_rpc_error(
                    Value::Null,
                    -32700,
                    "invalid JSON-RPC request",
                    Some(json!({ "detail": error.to_string() })),
                ));
            }
        };
        if request.jsonrpc != "2.0" {
            return ResearchMcpReply::Response(json_rpc_error(
                request.id.unwrap_or(Value::Null),
                -32600,
                "jsonrpc must be 2.0",
                None,
            ));
        }
        let Some(id) = request.id else {
            if request.method == "notifications/initialized" {
                return ResearchMcpReply::AcceptedNotification;
            }
            return ResearchMcpReply::AcceptedNotification;
        };
        let result = match request.method.as_str() {
            "initialize" => self.initialize(request.params),
            "ping" => Ok(json!({})),
            "tools/list" => self.list_tools(request.params),
            "tools/call" => self.call_tool(request.params),
            _ => Err(McpProtocolError::new(
                -32601,
                "the MCP method is not supported",
                None,
            )),
        };
        ResearchMcpReply::Response(match result {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Err(error) => json_rpc_error(id, error.code, error.message, error.data),
        })
    }

    pub(super) fn is_authorized(&self, authorization_header: Option<&str>) -> bool {
        constant_time_equal(
            authorization_header.unwrap_or_default().as_bytes(),
            self.authorization_header().as_bytes(),
        )
    }

    pub(super) fn protocol_version(&self) -> Option<&'static str> {
        self.protocol_version
    }

    fn initialize(&mut self, params: Option<Value>) -> Result<Value, McpProtocolError> {
        let params: InitializeParams = parse_params(params)?;
        let protocol_version = match params.protocol_version.as_str() {
            MCP_PROTOCOL_VERSION => MCP_PROTOCOL_VERSION,
            COMPATIBLE_MCP_PROTOCOL_VERSION => COMPATIBLE_MCP_PROTOCOL_VERSION,
            _ => MCP_PROTOCOL_VERSION,
        };
        self.protocol_version = Some(protocol_version);
        Ok(json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {
                    "listChanged": false,
                },
            },
            "serverInfo": {
                "name": "kosh-research",
                "version": MCP_SERVER_VERSION,
            },
            "instructions": "Use only these read-only Kosh tools. Treat citationHandle and ownerHandle values as opaque, run-scoped capabilities. Cite only citationHandle values returned with evidence.",
        }))
    }

    fn list_tools(&self, params: Option<Value>) -> Result<Value, McpProtocolError> {
        self.require_initialized()?;
        let params: ListToolsParams = parse_params(params)?;
        if params.cursor.is_some() {
            return Err(McpProtocolError::new(
                -32602,
                "the tool catalog has one page",
                None,
            ));
        }
        Ok(json!({
            "tools": research_tool_definitions(),
        }))
    }

    fn call_tool(&mut self, params: Option<Value>) -> Result<Value, McpProtocolError> {
        self.require_initialized()?;
        let params: CallToolParams = parse_params(params)?;
        if !RESEARCH_TOOL_NAMES.contains(&params.name.as_str()) {
            return Err(McpProtocolError::new(
                -32602,
                "the requested MCP tool is not authorized",
                None,
            ));
        }
        match self
            .run
            .call_tool(&params.name, params.arguments.unwrap_or_else(|| json!({})))
        {
            Ok(output) => {
                let text = serde_json::to_string(&output).map_err(|_| {
                    McpProtocolError::new(-32603, "could not serialize the tool result", None)
                })?;
                Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": text,
                    }],
                    "structuredContent": output,
                    "isError": false,
                }))
            }
            Err(error) => {
                let output = json!({
                    "version": "v1",
                    "error": error,
                });
                let text = serde_json::to_string(&output).map_err(|_| {
                    McpProtocolError::new(-32603, "could not serialize the tool error", None)
                })?;
                Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": text,
                    }],
                    "structuredContent": output,
                    "isError": true,
                }))
            }
        }
    }

    fn require_initialized(&self) -> Result<(), McpProtocolError> {
        if self.protocol_version.is_some() {
            Ok(())
        } else {
            Err(McpProtocolError::new(
                -32002,
                "the MCP session has not been initialized",
                None,
            ))
        }
    }
}

pub struct ClaudeMcpBridge {
    endpoint: Url,
    bearer_token: String,
}

impl fmt::Debug for ClaudeMcpBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaudeMcpBridge")
            .field("endpoint", &self.endpoint)
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

impl ClaudeMcpBridge {
    fn new(endpoint: &str, bearer_token: String) -> Result<Self, ResearchError> {
        let endpoint = Url::parse(endpoint)
            .map_err(|_| ResearchError::invalid("the MCP endpoint URL is invalid"))?;
        let is_loopback = endpoint
            .host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "::1"));
        if endpoint.scheme() != "http"
            || !is_loopback
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ResearchError::new(
                ResearchErrorCode::Unauthorized,
                "the research MCP endpoint must be an uncredentialed loopback HTTP URL",
            ));
        }
        Ok(Self {
            endpoint,
            bearer_token,
        })
    }

    pub fn mcp_config(&self) -> Value {
        json!({
            "mcpServers": {
                MCP_SERVER_NAME: {
                    "type": "http",
                    "url": self.endpoint.as_str(),
                    "headers": {
                        "Authorization": format!("Bearer ${{{MCP_TOKEN_ENV}}}"),
                    },
                },
            },
        })
    }

    pub fn environment(&self) -> (&'static str, &str) {
        (MCP_TOKEN_ENV, &self.bearer_token)
    }

    pub fn allowed_tools(&self) -> Vec<String> {
        RESEARCH_TOOL_NAMES
            .iter()
            .map(|tool| format!("mcp__{MCP_SERVER_NAME}__{tool}"))
            .collect()
    }

    pub fn claude_cli_arguments(&self) -> Vec<String> {
        vec![
            "--mcp-config".into(),
            serde_json::to_string(&self.mcp_config())
                .expect("serializing an MCP configuration value cannot fail"),
            "--strict-mcp-config".into(),
            "--tools".into(),
            String::new(),
            "--allowed-tools".into(),
            self.allowed_tools().join(","),
        ]
    }
}

pub fn research_tool_definitions() -> Vec<Value> {
    vec![
        tool(
            HYBRID_SEARCH_TOOL,
            "Hybrid Kosh search",
            "Search current Kosh evidence using local lexical retrieval and semantic retrieval when the local embedding index is ready. Use cursor to continue a prior page.",
            search_schema(),
        ),
        tool(
            EXACT_SEARCH_TOOL,
            "Exact Kosh search",
            "Search current Kosh evidence lexically with exact term/phrase matching. Use cursor to continue a prior page.",
            search_schema(),
        ),
        tool(
            READ_PASSAGE_CONTEXT_TOOL,
            "Read passage context",
            "Read a bounded neighborhood around a citation handle without changing its stored revision or attachment extraction.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "citationHandle": { "type": "string", "minLength": 1 },
                    "before": { "type": "integer", "minimum": 0 },
                    "after": { "type": "integer", "minimum": 0 },
                },
                "required": ["citationHandle"],
            }),
        ),
        tool(
            READ_CURRENT_TIDBIT_TOOL,
            "Read current tidbit",
            "Read current authored tidbit passages using an owner handle returned by Kosh. The call fails rather than silently retargeting if the tidbit changed.",
            owner_page_schema(),
        ),
        tool(
            INSPECT_SOURCES_TOOL,
            "Inspect evidence sources",
            "Read optional source labels and HTTP(S) URLs stored by Kosh for citation evidence or its owner.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "citationHandle": { "type": "string", "minLength": 1 },
                    "ownerHandle": { "type": "string", "minLength": 1 },
                },
                "oneOf": [
                    { "required": ["citationHandle"] },
                    { "required": ["ownerHandle"] },
                ],
            }),
        ),
        tool(
            INSPECT_ATTACHMENT_SEGMENTS_TOOL,
            "Inspect attachment segments",
            "Read bounded current PDF pages, image OCR regions, or text line segments using an attachment owner handle returned by Kosh.",
            owner_page_schema(),
        ),
    ]
}

fn tool(name: &str, title: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

fn search_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": { "type": "string", "minLength": 1, "maxLength": 512 },
            "cursor": { "type": "string", "minLength": 1 },
            "limit": { "type": "integer", "minimum": 1 },
        },
        "oneOf": [
            { "required": ["query"] },
            { "required": ["cursor"] },
        ],
    })
}

fn owner_page_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "ownerHandle": { "type": "string", "minLength": 1 },
            "cursor": { "type": "string", "minLength": 1 },
            "limit": { "type": "integer", "minimum": 1 },
        },
        "oneOf": [
            { "required": ["ownerHandle"] },
            { "required": ["cursor"] },
        ],
    })
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InitializeParams {
    protocol_version: String,
    #[allow(dead_code)]
    capabilities: Value,
    #[allow(dead_code)]
    client_info: Value,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListToolsParams {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CallToolParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

struct McpProtocolError {
    code: i32,
    message: &'static str,
    data: Option<Value>,
}

impl McpProtocolError {
    fn new(code: i32, message: &'static str, data: Option<Value>) -> Self {
        Self {
            code,
            message,
            data,
        }
    }
}

fn parse_params<T>(params: Option<Value>) -> Result<T, McpProtocolError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(params.unwrap_or_else(|| json!({}))).map_err(|error| {
        McpProtocolError::new(
            -32602,
            "invalid MCP request parameters",
            Some(json!({ "detail": error.to_string() })),
        )
    })
}

fn json_rpc_error(id: Value, code: i32, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    })
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            capabilities: json!({}),
            client_info: json!({}),
        }
    }
}
