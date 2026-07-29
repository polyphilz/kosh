use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::database::CitationResolution;

use super::{
    ClaudeMcpBridge, ResearchError, ResearchErrorCode, ResearchMcpReply, ResearchMcpSession,
    ResearchRun, MCP_PROTOCOL_VERSION,
};

const MCP_PATH: &str = "/mcp";
const RECEIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub struct EphemeralResearchMcpServer {
    endpoint: String,
    session: Arc<Mutex<ResearchMcpSession>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for EphemeralResearchMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EphemeralResearchMcpServer")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl EphemeralResearchMcpServer {
    pub fn start(run: ResearchRun) -> Result<Self, ResearchError> {
        let server = Server::http("127.0.0.1:0").map_err(|_| {
            ResearchError::new(
                ResearchErrorCode::ContentUnavailable,
                "Kosh could not bind the ephemeral research MCP server",
            )
        })?;
        let address = server.server_addr().to_ip().ok_or_else(|| {
            ResearchError::new(
                ResearchErrorCode::Unauthorized,
                "the research MCP server did not bind a loopback TCP address",
            )
        })?;
        if !address.ip().is_loopback() {
            return Err(ResearchError::new(
                ResearchErrorCode::Unauthorized,
                "the research MCP server must bind only to loopback",
            ));
        }
        let endpoint = format!("http://{address}{MCP_PATH}");
        let expected_origin = format!("http://{address}");
        let session = Arc::new(Mutex::new(ResearchMcpSession::new(run)));
        let stop = Arc::new(AtomicBool::new(false));
        let server_session = Arc::clone(&session);
        let server_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("kosh-research-mcp".into())
            .spawn(move || {
                while !server_stop.load(Ordering::Acquire) {
                    match server.recv_timeout(RECEIVE_POLL_INTERVAL) {
                        Ok(Some(request)) => {
                            handle_request(request, &server_session, &expected_origin);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            log::warn!("ephemeral research MCP receive failed: {error}");
                            break;
                        }
                    }
                }
            })
            .map_err(|_| {
                ResearchError::new(
                    ResearchErrorCode::ContentUnavailable,
                    "Kosh could not start the ephemeral research MCP server",
                )
            })?;

        Ok(Self {
            endpoint,
            session,
            stop,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn bridge(&self) -> Result<ClaudeMcpBridge, ResearchError> {
        self.session
            .lock()
            .map_err(|_| poisoned_session())?
            .bridge(&self.endpoint)
    }

    pub fn resolve_citation_handle(
        &self,
        handle: &str,
    ) -> Result<CitationResolution, ResearchError> {
        self.session
            .lock()
            .map_err(|_| poisoned_session())?
            .run()
            .resolve_citation_handle(handle)
            .cloned()
    }
}

impl Drop for EphemeralResearchMcpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("ephemeral research MCP server did not stop cleanly");
            }
        }
    }
}

fn handle_request(
    mut request: Request,
    session: &Mutex<ResearchMcpSession>,
    expected_origin: &str,
) {
    if request.url() != MCP_PATH {
        respond_empty(request, 404);
        return;
    }
    if request.method() != &Method::Post {
        respond_empty(request, 405);
        return;
    }
    let origin = header_value(&request, "Origin");
    if origin
        .as_deref()
        .is_some_and(|origin| origin != expected_origin)
    {
        respond_empty(request, 403);
        return;
    }
    let content_type = header_value(&request, "Content-Type").unwrap_or_default();
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        respond_empty(request, 415);
        return;
    }
    let accept = header_value(&request, "Accept").unwrap_or_default();
    if !(accept.contains("application/json") && accept.contains("text/event-stream")) {
        respond_empty(request, 406);
        return;
    }
    let authorization = header_value(&request, "Authorization");
    let (max_request_bytes, negotiated_protocol) = match session.lock() {
        Ok(session) if session.is_authorized(authorization.as_deref()) => (
            session.run().limits().max_request_bytes,
            session.protocol_version(),
        ),
        Ok(_) => {
            respond_empty(request, 401);
            return;
        }
        Err(_) => {
            respond_empty(request, 503);
            return;
        }
    };
    if let Some(protocol_version) = negotiated_protocol {
        if header_value(&request, "MCP-Protocol-Version").as_deref() != Some(protocol_version) {
            respond_empty(request, 400);
            return;
        }
    }
    let mut body = Vec::new();
    let read_result = request
        .as_reader()
        .take(max_request_bytes.saturating_add(1) as u64)
        .read_to_end(&mut body);
    if read_result.is_err() {
        respond_empty(request, 400);
        return;
    }
    if body.len() > max_request_bytes {
        respond_empty(request, 413);
        return;
    }

    let (reply, protocol_version) = match session.lock() {
        Ok(mut session) => {
            let reply = session.handle_json(authorization.as_deref(), &body);
            (
                reply,
                session.protocol_version().unwrap_or(MCP_PROTOCOL_VERSION),
            )
        }
        Err(_) => {
            respond_empty(request, 503);
            return;
        }
    };
    match reply {
        ResearchMcpReply::Response(value) => {
            let response_body = match serde_json::to_vec(&value) {
                Ok(response) => response,
                Err(_) => {
                    respond_empty(request, 500);
                    return;
                }
            };
            let response = Response::from_data(response_body)
                .with_status_code(StatusCode(200))
                .with_header(header("Content-Type", "application/json"))
                .with_header(header("Cache-Control", "no-store"))
                .with_header(header("MCP-Protocol-Version", protocol_version));
            if let Err(error) = request.respond(response) {
                log::warn!("ephemeral research MCP response failed: {error}");
            }
        }
        ResearchMcpReply::AcceptedNotification => respond_empty(request, 202),
    }
}

fn header_value(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().to_owned())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static HTTP header names and values are valid")
}

fn respond_empty(request: Request, status: u16) {
    let response = Response::empty(StatusCode(status))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION));
    if let Err(error) = request.respond(response) {
        log::warn!("ephemeral research MCP error response failed: {error}");
    }
}

fn poisoned_session() -> ResearchError {
    ResearchError::new(
        ResearchErrorCode::ContentUnavailable,
        "the ephemeral research MCP session is unavailable",
    )
}
