//! MCP Streamable HTTP: session management, SSE and JSON responses, stateless
//! mode, a configurable bind address, authentication, and size/rate limits.
//!
//! P1.10 of `docs/capability-coverage.md`. The stdio transport in
//! [`crate::mcp`] is unchanged and remains the default; this is the shared
//! variant, where more than one client can reach one indexed repository.
//!
//! Two rules the implementation will not bend, because a graph server is a
//! read-anything-in-the-repository server:
//!
//! - Binding anywhere but loopback requires an API key. A shared transport with
//!   no authentication is not a deployment option, it is an accident.
//! - Every request is bounded: a body size ceiling, a request-rate ceiling per
//!   client, and a session count ceiling. A server that can be made to allocate
//!   without limit by a client is not shareable either.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::error::{Error, Result};

/// How the HTTP transport should behave.
#[derive(Debug, Clone)]
pub struct Options {
    /// Address to bind. Anything other than loopback requires `api_key`.
    pub bind: String,
    /// Port to bind; 0 asks the operating system for a free one.
    pub port: u16,
    /// Bearer token every request must present, when set.
    pub api_key: Option<String>,
    /// Skip session tracking entirely: every request stands alone. This is the
    /// mode to run behind a load balancer that will not pin a client to one
    /// process.
    pub stateless: bool,
    /// Largest request body accepted, in bytes.
    pub max_body: usize,
    /// Requests one client may make per minute.
    pub rate_per_minute: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 0,
            api_key: None,
            stateless: false,
            max_body: 1_048_576,
            rate_per_minute: 600,
        }
    }
}

/// Sessions expire after this much silence, so a client that vanishes does not
/// hold a slot forever.
const SESSION_IDLE: Duration = Duration::from_mins(30);

/// Sessions one server will track at once.
const MAX_SESSIONS: usize = 256;

/// Keepalive comments an idle SSE stream emits before closing — an hour of
/// them, after which a forgotten client is asked to reconnect.
const SSE_KEEPALIVES: usize = 720;

/// Gap between keepalive comments.
const SSE_INTERVAL: Duration = Duration::from_secs(5);

/// How often a stream checks whether the index moved. Short enough that a
/// reindex is announced promptly, long enough to be an atomic load per tick.
const SSE_POLL: Duration = Duration::from_millis(50);

/// One client's state.
#[derive(Debug)]
struct Session {
    /// When it was last seen, for idle expiry.
    seen: Instant,
    /// Start of the current rate-limit window.
    window: Instant,
    /// Requests made inside that window.
    requests: u32,
}

/// Everything shared between the request loop and the SSE threads.
#[derive(Debug, Default)]
struct State {
    sessions: HashMap<String, Session>,
}

impl State {
    /// Drops sessions that have gone quiet, so the map stays bounded without a
    /// background task.
    fn expire(&mut self) {
        self.sessions
            .retain(|_, session| session.seen.elapsed() < SESSION_IDLE);
    }
}

/// A monotonic counter so two sessions minted in the same nanosecond differ.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// A session identifier: process, time, and counter. Unguessable is not the
/// claim — the API key is what authenticates; this only has to be unique.
fn mint_session() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{nanos:x}-{counter:x}", std::process::id())
}

/// Runs the Streamable HTTP transport until the process is stopped.
///
/// # Errors
/// Returns [`Error::Protocol`] when the bind address is unusable, or when a
/// non-loopback bind is requested without an API key.
pub fn serve(root: &Path, options: &Options) -> Result<()> {
    let loopback = is_loopback(&options.bind);
    if !loopback && options.api_key.is_none() {
        return Err(Error::Protocol {
            context: "MCP HTTP refused to start",
            detail: format!(
                "binding {} exposes this repository beyond this machine; pass --api-key (or set \
                 AAG_MCP_API_KEY) to require a bearer token",
                options.bind
            ),
        });
    }
    let root = root.to_path_buf();
    if let Err(error) = crate::watch::reconcile(&root) {
        tracing::warn!(%error, "startup reconciliation failed");
    }
    crate::watch::spawn(root.clone());

    let address = (options.bind.as_str(), options.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .ok_or_else(|| Error::Protocol {
            context: "MCP HTTP bind failed",
            detail: format!("`{}` is not an address", options.bind),
        })?;
    let listener = std::net::TcpListener::bind(address).map_err(|error| Error::Protocol {
        context: "MCP HTTP bind failed",
        detail: error.to_string(),
    })?;
    serve_on(listener, &root, options)
}

/// Serves on a listener that is already bound. Splitting this out is what lets
/// a caller — a test, or a supervisor handing over a socket — own the bind, so
/// there is no window between learning a port and listening on it.
fn serve_on(listener: std::net::TcpListener, root: &Path, options: &Options) -> Result<()> {
    let server = Server::from_listener(listener, None).map_err(|error| Error::Protocol {
        context: "MCP HTTP bind failed",
        detail: error.to_string(),
    })?;
    eprintln!(
        "aag MCP Streamable HTTP on http://{}/mcp ({}, {})",
        server.server_addr(),
        if options.stateless {
            "stateless"
        } else {
            "sessions"
        },
        if options.api_key.is_some() {
            "bearer auth"
        } else {
            "no auth, loopback only"
        }
    );

    let state = Arc::new(Mutex::new(State::default()));
    for request in server.incoming_requests() {
        dispatch(root, options, &state, request);
    }
    Ok(())
}

/// Whether an address stays on this machine.
fn is_loopback(bind: &str) -> bool {
    matches!(bind, "127.0.0.1" | "::1" | "localhost")
}

/// Routes one request, answering it directly or handing it to an SSE thread.
fn dispatch(root: &Path, options: &Options, state: &Arc<Mutex<State>>, mut request: Request) {
    if request.url() != "/mcp" {
        respond_empty(request, 404);
        return;
    }
    if !origin_allowed(&request) {
        respond_empty(request, 403);
        return;
    }
    if !authorized(&request, options.api_key.as_deref()) {
        respond_empty(request, 401);
        return;
    }
    let session = header(&request, "Mcp-Session-Id");
    match *request.method() {
        // Ending a session is part of the transport: a client that says it is
        // done should not have to wait out the idle timeout.
        Method::Delete => {
            if let Some(session) = session.as_deref() {
                let _ = state.lock().map(|mut state| state.sessions.remove(session));
            }
            respond_empty(request, 204);
        }
        Method::Get => {
            if accepts_events(&request) {
                let stream = std::thread::Builder::new()
                    .name("aag-mcp-sse".into())
                    .spawn(move || stream_events(request));
                if let Err(error) = stream {
                    tracing::warn!(%error, "could not start SSE stream");
                }
            } else {
                respond_empty(request, 405);
            }
        }
        Method::Post => {
            let ticket = match admit(options, state, session.as_deref()) {
                Admission::Ok(ticket) => ticket,
                Admission::NoSession => {
                    respond_json(
                        request,
                        None,
                        400,
                        &error_body("session not found; start one with `initialize`"),
                        false,
                    );
                    return;
                }
                Admission::RateLimited => {
                    respond_json(
                        request,
                        None,
                        429,
                        &error_body("too many requests; slow down or raise --rate-limit"),
                        false,
                    );
                    return;
                }
                Admission::TooManySessions => {
                    respond_json(
                        request,
                        None,
                        503,
                        &error_body("too many concurrent sessions"),
                        false,
                    );
                    return;
                }
            };
            let events = accepts_events(&request);
            let Some(body) = read_body(&mut request, options.max_body) else {
                respond_json(
                    request,
                    ticket,
                    413,
                    &error_body("request body larger than --max-body"),
                    events,
                );
                return;
            };
            let Ok(message) = serde_json::from_str::<Value>(&body) else {
                respond_json(
                    request,
                    ticket,
                    400,
                    &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}),
                    events,
                );
                return;
            };
            match crate::mcp::handle_message(root, &message) {
                Some(response) => respond_json(request, ticket, 200, &response, events),
                // A notification has no reply, which is 202 and an empty body
                // rather than an empty JSON-RPC envelope.
                None => respond_empty(request, 202),
            }
        }
        _ => respond_empty(request, 405),
    }
}

/// Whether a request may proceed, and the session id to report back.
enum Admission {
    /// Admitted; the payload is the session id to echo when one was minted or
    /// refreshed.
    Ok(Option<String>),
    /// A session was required and the one presented is unknown or expired.
    NoSession,
    RateLimited,
    TooManySessions,
}

/// Applies session and rate rules to one request.
///
/// A first request with no session id mints one — that is what `initialize`
/// does, and treating any first request the same way keeps the rule short. In
/// stateless mode none of this runs, which is the point of stateless mode.
fn admit(options: &Options, state: &Arc<Mutex<State>>, session: Option<&str>) -> Admission {
    if options.stateless {
        return Admission::Ok(None);
    }
    let Ok(mut state) = state.lock() else {
        return Admission::Ok(None);
    };
    state.expire();
    let Some(id) = session.map(str::to_string) else {
        if state.sessions.len() >= MAX_SESSIONS {
            return Admission::TooManySessions;
        }
        let id = mint_session();
        state.sessions.insert(
            id.clone(),
            Session {
                seen: Instant::now(),
                window: Instant::now(),
                requests: 1,
            },
        );
        return Admission::Ok(Some(id));
    };
    let Some(existing) = state.sessions.get_mut(&id) else {
        return Admission::NoSession;
    };
    existing.seen = Instant::now();
    if existing.window.elapsed() >= Duration::from_mins(1) {
        existing.window = Instant::now();
        existing.requests = 0;
    }
    existing.requests += 1;
    if existing.requests > options.rate_per_minute {
        return Admission::RateLimited;
    }
    Admission::Ok(Some(id))
}

/// Reads a body, refusing one over the ceiling instead of allocating it.
/// How much of an oversized body is read and thrown away before answering.
/// Closing a socket with unread bytes still in it is a TCP reset, and a reset
/// destroys the response already written — so the refusal has to be drained
/// for the client to ever see it. Past this the client is the one misbehaving.
const DRAIN_CEILING: u64 = 1 << 20;

fn read_body(request: &mut Request, max_body: usize) -> Option<String> {
    if request
        .body_length()
        .is_some_and(|length| length > max_body)
    {
        let _ = std::io::copy(
            &mut request.as_reader().take(DRAIN_CEILING),
            &mut std::io::sink(),
        );
        return None;
    }
    let mut body = String::new();
    // Read one byte past the ceiling so a chunked body that lied about its
    // length is refused too.
    request
        .as_reader()
        .take((max_body + 1) as u64)
        .read_to_string(&mut body)
        .ok()?;
    (body.len() <= max_body).then_some(body)
}

fn error_body(message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": message}})
}

/// Answers with a JSON-RPC payload, framed as one SSE event when the client
/// asked for a stream — which is what Streamable HTTP allows a POST to return.
fn respond_json(
    request: Request,
    session: Option<String>,
    status: u16,
    body: &Value,
    events: bool,
) {
    let (content_type, payload) = if events {
        (
            "text/event-stream",
            format!("event: message\ndata: {body}\n\n"),
        )
    } else {
        ("application/json", body.to_string())
    };
    let mut response = Response::from_string(payload).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        response = response.with_header(header);
    }
    if let Some(session) = session
        && let Ok(header) = Header::from_bytes("Mcp-Session-Id", session)
    {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
}

fn respond_empty(request: Request, status: u16) {
    let _ = request.respond(Response::empty(StatusCode(status)));
}

/// Holds an SSE stream open, carrying server-initiated notifications.
///
/// The graph changes under the client — that is the whole point of the watcher
/// — so the stream tells it: every time the index is rewritten, one
/// `notifications/resources/updated` for `aag://graph` goes out, carrying the
/// revision the client can read back. Between changes the stream sends
/// keepalive comments. It is bounded: it closes after [`SSE_KEEPALIVES`]
/// intervals so a forgotten client cannot hold a thread forever.
fn stream_events(request: Request) {
    // The response head is written by hand because an SSE body has no length:
    // `Response` wants one, and the writer is what stays open.
    let mut writer = request.into_writer();
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-store\r\n\
                Connection: close\r\n\r\n";
    if writer.write_all(head.as_bytes()).is_err() {
        return;
    }
    let ready =
        "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/ready\"}\n\n";
    let _ = writer.write_all(ready.as_bytes());
    let _ = writer.flush();
    let mut seen = crate::watch::revision();
    for _ in 0..SSE_KEEPALIVES {
        // Polled in small steps so a change is announced promptly while a quiet
        // stream still only writes one keepalive per interval.
        let mut waited = Duration::ZERO;
        while waited < SSE_INTERVAL {
            std::thread::sleep(SSE_POLL);
            waited += SSE_POLL;
            let current = crate::watch::revision();
            if current != seen {
                seen = current;
                let notification = format!(
                    "event: message\ndata: {}\n\n",
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/resources/updated",
                        "params": {"uri": crate::mcp::GRAPH_RESOURCE_URI, "revision": current},
                    })
                );
                if writer.write_all(notification.as_bytes()).is_err() || writer.flush().is_err() {
                    return;
                }
            }
        }
        if writer.write_all(b": keepalive\n\n").is_err() || writer.flush().is_err() {
            return;
        }
    }
}

fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str().to_string())
}

fn accepts_events(request: &Request) -> bool {
    header(request, "Accept").is_some_and(|accept| accept.contains("text/event-stream"))
}

/// A browser page must not be able to drive this server from another origin.
fn origin_allowed(request: &Request) -> bool {
    header(request, "Origin").is_none_or(|origin| {
        origin.starts_with("http://127.0.0.1") || origin.starts_with("http://localhost")
    })
}

fn authorized(request: &Request, api_key: Option<&str>) -> bool {
    let Some(api_key) = api_key else { return true };
    header(request, "Authorization").is_some_and(|value| value == format!("Bearer {api_key}"))
}

/// The repository a served request reads. Kept as a type alias so the signature
/// reads the same as the stdio transport's.
pub type Root = PathBuf;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpStream;

    /// An indexed scratch repository plus a server on a real port.
    ///
    /// The server runs in a thread and the tests speak HTTP to it, because the
    /// behaviour under test *is* the HTTP behaviour — sessions, status codes,
    /// headers, and framing. Nothing here touches the machine's home.
    fn served(options: Options) -> (u16, PathBuf) {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-transport-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("lib.rs"),
            "fn helper() {}\nfn caller() { helper(); }\n",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..crate::bigbang::Options::default()
            },
        )
        .unwrap();

        // Bind here and hand the live listener to the server. Learning a port
        // and then rebinding it leaves a window where a second test can take
        // the same number, and one of the two servers then fails to start —
        // which shows up as a connection reset in whichever test is unlucky.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let options = Options { port, ..options };
        let served_root = root.clone();
        std::thread::spawn(move || {
            let _ = serve_on(listener, &served_root, &options);
        });
        (port, root)
    }

    /// Sends a raw request and returns `(status line, headers, body)`.
    fn send(port: u16, request: &str) -> (String, Vec<String>, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.trim().is_empty() {
                break;
            }
            headers.push(line.trim().to_string());
        }
        let length = headers
            .iter()
            .find_map(|header| {
                let (name, value) = header.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        if length > 0 {
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
        }
        (
            status.trim().to_string(),
            headers,
            String::from_utf8_lossy(&body).to_string(),
        )
    }

    fn post(port: u16, body: &str, extra: &str) -> (String, Vec<String>, String) {
        send(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n{extra}\r\n{body}",
                body.len()
            ),
        )
    }

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

    fn session_of(headers: &[String]) -> Option<String> {
        headers.iter().find_map(|header| {
            let (name, value) = header.split_once(':')?;
            name.eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_string())
        })
    }

    #[test]
    fn a_first_request_mints_a_session_that_later_requests_must_present() {
        let (port, _root) = served(Options::default());

        let (status, headers, body) = post(port, INITIALIZE, "");
        assert!(status.contains("200"), "{status}");
        assert!(
            body.contains("serverInfo") || body.contains("result"),
            "{body}"
        );
        let session = session_of(&headers).expect("a session id header");

        // The same session is accepted and echoed back.
        let (status, headers, _) =
            post(port, INITIALIZE, &format!("Mcp-Session-Id: {session}\r\n"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(session_of(&headers).as_deref(), Some(session.as_str()));

        // An unknown one is refused rather than silently starting a new session.
        let (status, _, body) = post(port, INITIALIZE, "Mcp-Session-Id: nonsense\r\n");
        assert!(status.contains("400"), "{status}");
        assert!(body.contains("session not found"), "{body}");
    }

    #[test]
    fn deleting_a_session_ends_it() {
        let (port, _root) = served(Options::default());
        let (_, headers, _) = post(port, INITIALIZE, "");
        let session = session_of(&headers).unwrap();

        let (status, _, _) = send(
            port,
            &format!(
                "DELETE /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nMcp-Session-Id: {session}\r\n\
                 Content-Length: 0\r\n\r\n"
            ),
        );
        assert!(status.contains("204"), "{status}");

        let (status, _, _) = post(port, INITIALIZE, &format!("Mcp-Session-Id: {session}\r\n"));
        assert!(
            status.contains("400"),
            "a deleted session is gone: {status}"
        );
    }

    #[test]
    fn stateless_mode_needs_no_session_at_all() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (status, headers, _) = post(port, INITIALIZE, "");

        assert!(status.contains("200"), "{status}");
        assert_eq!(
            session_of(&headers),
            None,
            "stateless mode tracks nothing, so it hands out nothing"
        );
        let (status, _, _) = post(port, INITIALIZE, "Mcp-Session-Id: nonsense\r\n");
        assert!(
            status.contains("200"),
            "and it does not care what a client claims: {status}"
        );
    }

    #[test]
    fn a_client_that_asks_for_events_gets_its_answer_framed_as_one() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (status, headers, body) = post(port, INITIALIZE, "Accept: text/event-stream\r\n");

        assert!(status.contains("200"), "{status}");
        assert!(
            headers
                .iter()
                .any(|header| header.to_ascii_lowercase().contains("text/event-stream")),
            "{headers:?}"
        );
        assert!(body.starts_with("event: message\ndata: {"), "{body}");
    }

    #[test]
    fn a_get_opens_a_stream_and_a_plain_get_does_not() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n")
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        assert!(status.contains("200"), "{status}");
        let mut saw_event = false;
        for _ in 0..10 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.starts_with("event: message") {
                saw_event = true;
                break;
            }
        }
        assert!(saw_event, "the stream opens with a message event");

        let (status, _, _) = send(
            port,
            "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json\r\n\r\n",
        );
        assert!(
            status.contains("405"),
            "a GET that does not want a stream has nothing to receive: {status}"
        );
    }

    #[test]
    fn a_body_over_the_ceiling_is_refused_rather_than_allocated() {
        let (port, _root) = served(Options {
            stateless: true,
            max_body: 64,
            ..Options::default()
        });

        let padded = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"pad":"{}"}}}}"#,
            "x".repeat(200)
        );
        let (status, _, body) = post(port, &padded, "");

        assert!(status.contains("413"), "{status}");
        assert!(body.contains("--max-body"), "{body}");
    }

    #[test]
    fn the_rate_limit_answers_429_instead_of_working_harder() {
        let (port, _root) = served(Options {
            rate_per_minute: 3,
            ..Options::default()
        });
        let (_, headers, _) = post(port, INITIALIZE, "");
        let session = session_of(&headers).unwrap();
        let with_session = format!("Mcp-Session-Id: {session}\r\n");

        let mut limited = false;
        for _ in 0..8 {
            let (status, _, body) = post(port, INITIALIZE, &with_session);
            if status.contains("429") {
                assert!(body.contains("too many requests"), "{body}");
                limited = true;
                break;
            }
        }
        assert!(limited, "the ceiling has to actually stop something");
    }

    #[test]
    fn a_cross_origin_request_is_refused() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (status, _, _) = post(port, INITIALIZE, "Origin: https://evil.example\r\n");

        assert!(status.contains("403"), "{status}");
    }

    #[test]
    fn an_unknown_path_is_not_the_mcp_endpoint() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (status, _, _) = send(
            port,
            "POST /admin HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
        );

        assert!(status.contains("404"), "{status}");
    }

    #[test]
    fn a_bearer_token_is_required_when_one_is_configured() {
        let (port, _root) = served(Options {
            stateless: true,
            api_key: Some("secret".to_string()),
            ..Options::default()
        });

        let (status, _, _) = post(port, INITIALIZE, "");
        assert!(status.contains("401"), "no token: {status}");

        let (status, _, _) = post(port, INITIALIZE, "Authorization: Bearer wrong\r\n");
        assert!(status.contains("401"), "wrong token: {status}");

        let (status, _, _) = post(port, INITIALIZE, "Authorization: Bearer secret\r\n");
        assert!(status.contains("200"), "right token: {status}");
    }

    #[test]
    fn binding_beyond_loopback_without_a_key_refuses_to_start() {
        let root = std::env::temp_dir().join("aag-transport-refusal");
        let error = serve(
            &root,
            &Options {
                bind: "0.0.0.0".to_string(),
                ..Options::default()
            },
        )
        .expect_err("an unauthenticated shared server must not start");

        let message = error.to_string();
        assert!(message.contains("--api-key"), "{message}");
        assert!(message.contains("beyond this machine"), "{message}");
    }

    #[test]
    fn a_reindex_reaches_an_open_stream() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n")
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("200"), "{line}");

        // The stream is open; the index moves under it.
        std::thread::sleep(Duration::from_millis(100));
        crate::watch::mark_indexed();

        let mut update = None;
        for _ in 0..200 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.contains("notifications/resources/updated") {
                update = Some(line);
                break;
            }
        }
        let update = update.expect("the stream carries the change the client did not ask for");
        assert!(update.contains("aag://graph"), "{update}");
        assert!(
            update.contains("\"revision\""),
            "the client is told which revision to read back: {update}"
        );
    }

    #[test]
    fn the_graph_is_a_readable_resource() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (_, _, listed) = post(
            port,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
            "",
        );
        assert!(listed.contains("aag://graph"), "{listed}");

        let (_, _, read) = post(
            port,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"aag://graph"}}"#,
            "",
        );
        assert!(read.contains("\\\"files\\\""), "{read}");
        assert!(read.contains("\\\"revision\\\""), "{read}");

        let (_, _, unknown) = post(
            port,
            r#"{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"aag://nope"}}"#,
            "",
        );
        assert!(
            unknown.contains("unknown resource"),
            "an unknown uri says so rather than answering with the graph: {unknown}"
        );
    }

    #[test]
    fn a_notification_gets_no_body() {
        let (port, _root) = served(Options {
            stateless: true,
            ..Options::default()
        });

        let (status, _, body) = post(
            port,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
        );

        assert!(status.contains("202"), "{status}");
        assert!(body.is_empty(), "{body}");
    }
}
