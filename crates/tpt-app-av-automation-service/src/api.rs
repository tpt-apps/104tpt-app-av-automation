//! The local API (spec §14, §16).
//!
//! * Disabled unless configured; binds only to a loopback address; every request needs the bearer
//!   token; requests with a non-loopback `Host` or `Origin` are refused (DNS-rebinding and
//!   cross-site protection).
//! * A deliberately small HTTP/1.1 implementation with hard limits on line, header and body size,
//!   read/write timeouts and a cap on concurrent connections. It has no dependencies, never panics
//!   on malformed input, and closes every connection after one response.
//!
//! Endpoints: `GET /health`, `GET /rules`, `GET /devices`, `GET /executions`,
//! `GET /incidents`, `POST /rules/:id/arm|disarm|run`, `POST /executions/:id/cancel` and
//! `GET /events` (WebSocket).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};
use tpt_app_av_automation_core::{DeviceHealth, Error, Result};
use tpt_app_av_automation_model::ExecutionStatus;
use tpt_app_av_automation_report::Filter;

use crate::config::ApiConfig;
use crate::runtime::{ControlRequest, ServiceHandle};
use crate::ws;

const MAX_LINE: usize = 4096;
const MAX_HEADERS: usize = 64;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Upper-case method.
    pub method: String,
    /// Decoded path, without the query.
    pub path: String,
    /// Decoded query parameters.
    pub query: Vec<(String, String)>,
    /// Headers with lower-cased names.
    pub headers: HashMap<String, String>,
    /// Request body.
    pub body: Vec<u8>,
}

/// Why a request could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Malformed request (400).
    Bad(&'static str),
    /// A line, the headers or the body exceeded a limit (431/413).
    TooLarge(&'static str),
    /// A feature this server deliberately does not implement (501).
    Unsupported(&'static str),
    /// The connection failed or timed out.
    Io,
}

impl HttpError {
    fn status(&self) -> u16 {
        match self {
            HttpError::Bad(_) => 400,
            HttpError::TooLarge(_) => 413,
            HttpError::Unsupported(_) => 501,
            HttpError::Io => 408,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            HttpError::Bad(m) | HttpError::TooLarge(m) | HttpError::Unsupported(m) => m,
            HttpError::Io => "connection error",
        }
    }
}

fn read_limited_line<R: BufRead>(r: &mut R, max: usize) -> std::result::Result<String, HttpError> {
    let mut buf = Vec::new();
    let n = (&mut *r)
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(|_| HttpError::Io)?;
    if n == 0 {
        return Err(HttpError::Io);
    }
    if !buf.ends_with(b"\n") {
        return Err(if buf.len() > max {
            HttpError::TooLarge("line too long")
        } else {
            HttpError::Bad("truncated request")
        });
    }
    while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        buf.pop();
    }
    if buf.iter().any(|b| *b == 0 || (*b < 0x20 && *b != b'\t')) {
        return Err(HttpError::Bad("control character in request"));
    }
    String::from_utf8(buf).map_err(|_| HttpError::Bad("request is not valid UTF-8"))
}

fn percent_decode(text: &str) -> std::result::Result<String, HttpError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or(HttpError::Bad("bad percent escape"))?;
                out.push(hex);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| HttpError::Bad("bad percent escape"))
}

/// Parses one HTTP/1.x request from a reader, enforcing every limit.
pub fn parse_request<R: BufRead>(r: &mut R) -> std::result::Result<Request, HttpError> {
    let request_line = read_limited_line(r, MAX_LINE)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Bad("malformed request line"));
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(HttpError::Bad("unsupported HTTP version"));
    }
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(HttpError::Bad("malformed method"));
    }
    if !target.starts_with('/') {
        return Err(HttpError::Bad("target must be an absolute path"));
    }
    let (raw_path, raw_query) = target.split_once('?').unwrap_or((target, ""));
    let path = percent_decode(raw_path)?;
    let mut query = Vec::new();
    for pair in raw_query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.push((percent_decode(k)?, percent_decode(v)?));
    }

    let mut headers: HashMap<String, String> = HashMap::new();
    let mut header_bytes = 0usize;
    loop {
        let line = read_limited_line(r, MAX_LINE)?;
        if line.is_empty() {
            break;
        }
        header_bytes += line.len();
        if headers.len() >= MAX_HEADERS || header_bytes > MAX_HEADER_BYTES {
            return Err(HttpError::TooLarge("too many headers"));
        }
        let (name, value) = line.split_once(':').ok_or(HttpError::Bad("malformed header"))?;
        if name.is_empty() || name.contains(' ') {
            return Err(HttpError::Bad("malformed header name"));
        }
        let name = name.to_ascii_lowercase();
        if name == "content-length" && headers.contains_key(&name) {
            return Err(HttpError::Bad("duplicate content-length"));
        }
        headers.insert(name, value.trim().to_owned());
    }

    if headers.contains_key("transfer-encoding") {
        return Err(HttpError::Unsupported("transfer-encoding is not supported"));
    }
    let length = match headers.get("content-length") {
        None => 0,
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| HttpError::Bad("bad content-length"))?,
    };
    if length > MAX_BODY {
        return Err(HttpError::TooLarge("body too large"));
    }
    let mut body = vec![0u8; length];
    r.read_exact(&mut body).map_err(|_| HttpError::Io)?;

    Ok(Request {
        method: method.to_owned(),
        path,
        query,
        headers,
        body,
    })
}

/// A response to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// JSON body.
    pub body: Value,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
}

impl Response {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body,
            headers: Vec::new(),
        }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "error": message.into() }))
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            413 => "Payload Too Large",
            501 => "Not Implemented",
            503 => "Service Unavailable",
            _ => "Error",
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let body = serde_json::to_string(&self.body).unwrap_or_else(|_| "{}".to_string());
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
            self.status,
            self.reason(),
            body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    }
}

/// What the router wants done with a request.
pub enum Outcome {
    /// Send this response and close.
    Respond(Response),
    /// Upgrade to the event stream using this `Sec-WebSocket-Accept` value.
    Upgrade(String),
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn origin_ok(origin: &str) -> bool {
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or("");
    is_loopback_authority(authority.split('/').next().unwrap_or(""))
}

fn query_value<'a>(req: &'a Request, key: &str) -> Option<&'a str> {
    req.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn filter_from(req: &Request) -> std::result::Result<(Filter, usize), String> {
    let mut filter = Filter {
        rule: query_value(req, "rule").map(str::to_owned),
        device: query_value(req, "device").map(str::to_owned),
        ..Filter::default()
    };
    if let Some(status) = query_value(req, "status") {
        filter.status = Some(
            serde_json::from_value::<ExecutionStatus>(Value::String(status.to_owned()))
                .map_err(|_| format!("unknown status `{status}`"))?,
        );
    }
    let number = |key: &str| -> std::result::Result<Option<u64>, String> {
        query_value(req, key)
            .map(|v| v.parse::<u64>().map_err(|_| format!("`{key}` must be a number")))
            .transpose()
    };
    filter.since_ms = number("since")?;
    filter.until_ms = number("until")?;
    if let Some(v) = query_value(req, "simulated") {
        filter.simulated = Some(match v {
            "true" => true,
            "false" => false,
            _ => return Err("`simulated` must be true or false".into()),
        });
    }
    let limit = number("limit")?.unwrap_or(100).clamp(1, 1000) as usize;
    Ok((filter, limit))
}

/// Routes one request. Pure apart from the calls into the service handle.
pub fn route(req: &Request, handle: &ServiceHandle, token: &str) -> Outcome {
    if let Some(host) = req.headers.get("host") {
        if !is_loopback_authority(host) {
            return Outcome::Respond(Response::error(403, "unexpected Host header"));
        }
    }
    if let Some(origin) = req.headers.get("origin") {
        if !origin_ok(origin) {
            return Outcome::Respond(Response::error(403, "cross-origin requests are not allowed"));
        }
    }
    let presented = req
        .headers
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        let mut response = Response::error(401, "a valid bearer token is required");
        response.headers.push(("WWW-Authenticate", "Bearer".to_string()));
        return Outcome::Respond(response);
    }

    let segments: Vec<&str> = req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let method = req.method.as_str();
    let not_allowed = || Outcome::Respond(Response::error(405, "method not allowed"));
    let respond = |r: Response| Outcome::Respond(r);

    match (segments.as_slice(), method) {
        (["health"], "GET") => respond(Response::json(200, health(handle))),
        (["health"], _) => not_allowed(),
        (["rules"], "GET") => {
            let snapshot = handle.snapshot();
            respond(Response::json(200, json!({ "rules": snapshot.rules })))
        }
        (["rules"], _) => not_allowed(),
        (["devices"], "GET") => {
            let snapshot = handle.snapshot();
            respond(Response::json(200, json!({ "devices": snapshot.devices })))
        }
        (["devices"], _) => not_allowed(),
        (["executions"], "GET") => match filter_from(req) {
            Err(message) => respond(Response::error(400, message)),
            Ok((filter, limit)) => respond(executions(handle, &filter, limit)),
        },
        (["executions"], _) => not_allowed(),
        (["incidents"], "GET") => {
            let limit = query_value(req, "limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(100)
                .clamp(1, 1000);
            respond(match &handle.store {
                Some(store) => match store.lock().unwrap_or_else(|e| e.into_inner()).incidents(limit) {
                    Ok(incidents) => Response::json(200, json!({ "incidents": incidents })),
                    Err(e) => Response::error(500, e.to_string()),
                },
                None => Response::json(200, json!({ "incidents": [] })),
            })
        }
        (["incidents"], _) => not_allowed(),
        (["rules", id, action @ ("arm" | "disarm" | "run")], "POST") => {
            let request = match *action {
                "arm" => ControlRequest::Arm((*id).to_owned()),
                "disarm" => ControlRequest::Disarm((*id).to_owned()),
                _ => ControlRequest::Run((*id).to_owned()),
            };
            respond(match handle.control(request) {
                Ok(value) => Response::json(200, value),
                Err(message) if message.contains("unknown rule") => Response::error(404, message),
                Err(message) => Response::error(500, message),
            })
        }
        (["rules", _, "arm" | "disarm" | "run"], _) => not_allowed(),
        (["executions", id, "cancel"], "POST") => respond(if handle.cancel_handle().cancel(id) {
            Response::json(200, json!({ "execution": id, "cancelled": true }))
        } else {
            Response::error(404, format!("no running execution `{id}`"))
        }),
        (["executions", _, "cancel"], _) => not_allowed(),
        (["events"], "GET") => {
            let wants_upgrade = req
                .headers
                .get("upgrade")
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
                && req
                    .headers
                    .get("connection")
                    .is_some_and(|v| v.to_ascii_lowercase().contains("upgrade"))
                && req.headers.get("sec-websocket-version").is_some_and(|v| v == "13");
            match req.headers.get("sec-websocket-key") {
                Some(key) if wants_upgrade && ws::valid_client_key(key) => {
                    Outcome::Upgrade(ws::accept_key(key))
                }
                _ => respond(Response::error(400, "this endpoint requires a WebSocket upgrade")),
            }
        }
        (["events"], _) => not_allowed(),
        _ => respond(Response::error(404, "not found")),
    }
}

fn health(handle: &ServiceHandle) -> Value {
    let snapshot = handle.snapshot();
    let count = |h: DeviceHealth| snapshot.devices.iter().filter(|d| d.health == h.to_string()).count();
    let status = if count(DeviceHealth::Offline) > 0 {
        "offline"
    } else if count(DeviceHealth::Degraded) > 0 {
        "degraded"
    } else {
        "nominal"
    };
    let (malformed, rate_limited, queue_dropped) = handle.rejected_counts();
    json!({
        "status": status,
        "pack": snapshot.pack_name,
        "mode": if snapshot.simulation { "simulation" } else { "live" },
        "rules": snapshot.rules.len(),
        "armed_rules": snapshot.rules.iter().filter(|r| r.armed).count(),
        "devices": {
            "total": snapshot.devices.len(),
            "online": count(DeviceHealth::Online),
            "degraded": count(DeviceHealth::Degraded),
            "offline": count(DeviceHealth::Offline),
            "unknown": count(DeviceHealth::Unknown),
        },
        "active_chains": snapshot.active_chains.len(),
        "rejected_inbound": {
            "malformed": malformed,
            "rate_limited": rate_limited,
            "backed_off": handle.backed_off(),
            "backoff_episodes": handle.backoff_episodes(),
            "queue_dropped": queue_dropped,
        },
    })
}

fn executions(handle: &ServiceHandle, filter: &Filter, limit: usize) -> Response {
    if let Some(store) = &handle.store {
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        return match store.executions(filter, limit) {
            Ok(records) => Response::json(200, json!({ "executions": records })),
            Err(e) => Response::error(500, e.to_string()),
        };
    }
    let snapshot = handle.snapshot();
    let records: Vec<_> = snapshot
        .recent
        .iter()
        .rev()
        .filter(|r| filter.matches(r))
        .take(limit)
        .collect();
    Response::json(200, json!({ "executions": records }))
}

/// Starts the API listener. The bind address was validated to be loopback by the service config.
pub(crate) fn spawn(handle: ServiceHandle, config: &ApiConfig) -> Result<(SocketAddr, JoinHandle<()>)> {
    let token = config
        .token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Error::InvalidOperation("the API cannot start without a token".into()))?;
    let bind: SocketAddr = config
        .bind
        .parse()
        .map_err(|_| Error::InvalidOperation(format!("invalid API bind address `{}`", config.bind)))?;
    if !bind.ip().is_loopback() {
        return Err(Error::InvalidOperation("the API only binds to loopback addresses".into()));
    }
    let listener = TcpListener::bind(bind).map_err(|e| Error::Control(format!("cannot bind API on {bind}: {e}")))?;
    listener.set_nonblocking(true).map_err(|e| Error::Control(e.to_string()))?;
    let addr = listener.local_addr().map_err(|e| Error::Control(e.to_string()))?;
    let token = Arc::new(token);
    let active = Arc::new(AtomicUsize::new(0));

    let worker = std::thread::spawn(move || {
        while !handle.is_shutdown() {
            match listener.accept() {
                Ok((stream, _)) => {
                    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        active.fetch_sub(1, Ordering::SeqCst);
                        let mut stream = stream;
                        let _ = stream.write_all(&Response::error(503, "too many connections").to_bytes());
                        continue;
                    }
                    let (handle, token, active) = (handle.clone(), token.clone(), active.clone());
                    std::thread::spawn(move || {
                        serve(stream, &handle, &token);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    tracing::warn!("API accept error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    });
    Ok((addr, worker))
}

fn serve(stream: TcpStream, handle: &ServiceHandle, token: &str) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);
    let mut stream = stream;

    let request = match parse_request(&mut reader) {
        Ok(request) => request,
        Err(e) => {
            let _ = stream.write_all(&Response::error(e.status(), e.message()).to_bytes());
            return;
        }
    };
    match route(&request, handle, token) {
        Outcome::Respond(response) => {
            let _ = stream.write_all(&response.to_bytes());
        }
        Outcome::Upgrade(accept) => stream_events(stream, handle, &accept),
    }
}

fn stream_events(mut stream: TcpStream, handle: &ServiceHandle, accept: &str) {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let rx = handle.subscribe();
    let mut idle = 0u32;
    loop {
        if handle.is_shutdown() {
            let _ = stream.write_all(&ws::frame(0x8, &[]));
            return;
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => {
                idle = 0;
                if stream.write_all(&ws::frame(0x1, line.as_bytes())).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                idle += 1;
                // A periodic ping lets us notice clients that vanished without closing.
                if idle >= 15 {
                    idle = 0;
                    if stream.write_all(&ws::frame(0x9, &[])).is_err() {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(text: &str) -> std::result::Result<Request, HttpError> {
        parse_request(&mut Cursor::new(text.as_bytes().to_vec()))
    }

    #[test]
    fn parses_a_simple_request() {
        let r = parse("GET /executions?rule=main%20hall&limit=5 HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer abc\r\n\r\n").unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/executions");
        assert_eq!(r.query, [("rule".into(), "main hall".into()), ("limit".into(), "5".into())]);
        assert_eq!(r.headers["authorization"], "Bearer abc");
        assert!(r.body.is_empty());
    }

    #[test]
    fn reads_a_body_of_the_declared_length() {
        let r = parse("POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(r.body, b"hello");
        assert_eq!(parse("POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhe"), Err(HttpError::Io));
    }

    #[test]
    fn rejects_malformed_requests() {
        for bad in [
            "",
            "GET\r\n\r\n",
            "GET / HTTP/1.1 extra\r\n\r\n",
            "get / HTTP/1.1\r\n\r\n",
            "GET http://evil/ HTTP/1.1\r\n\r\n",
            "GET / HTTP/2.0\r\n\r\n",
            "GET / HTTP/1.1\r\nno-colon\r\n\r\n",
            "GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
            "GET /%zz HTTP/1.1\r\n\r\n",
            "GET /%ff HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\nContent-Length: abc\r\n\r\n",
            "GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nxx",
            "GET / HTTP/1.1\r\nX: \u{0}\r\n\r\n",
        ] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn enforces_size_limits() {
        let long_target = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(MAX_LINE + 10));
        assert_eq!(parse(&long_target), Err(HttpError::TooLarge("line too long")));
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X-H: 1\r\n".repeat(200));
        // Repeated names collapse, so use distinct ones to exceed the header cap.
        let distinct: String = (0..100).map(|i| format!("X-{i}: 1\r\n")).collect();
        assert!(matches!(
            parse(&format!("GET / HTTP/1.1\r\n{distinct}\r\n")),
            Err(HttpError::TooLarge(_))
        ));
        let _ = many;
        let body = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(parse(&body), Err(HttpError::TooLarge("body too large")));
    }

    #[test]
    fn chunked_bodies_are_refused() {
        assert!(matches!(
            parse("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
            Err(HttpError::Unsupported(_))
        ));
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut x: u32 = 0xDEAD_BEEF;
        for len in 0..300usize {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = parse_request(&mut Cursor::new(data));
        }
        // Truncations of a valid request at every byte.
        let valid = b"POST /rules/a/run?x=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\nabc";
        for cut in 0..valid.len() {
            let _ = parse_request(&mut Cursor::new(valid[..cut].to_vec()));
        }
    }

    #[test]
    fn loopback_authorities_and_origins() {
        assert!(is_loopback_authority("127.0.0.1:8787"));
        assert!(is_loopback_authority("localhost"));
        assert!(is_loopback_authority("[::1]:80"));
        assert!(!is_loopback_authority("evil.example:80"));
        assert!(!is_loopback_authority("127.0.0.1.evil.example"));
        assert!(origin_ok("http://localhost:3000"));
        assert!(!origin_ok("https://evil.example"));
        assert!(!origin_ok("null"));
    }

    #[test]
    fn token_comparison_is_length_and_content_sensitive() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-tokeN"));
        assert!(!constant_time_eq(b"secret", b"secret-token"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn query_filters_are_validated() {
        let req = |q: &str| parse(&format!("GET /executions?{q} HTTP/1.1\r\n\r\n")).unwrap();
        let (filter, limit) = filter_from(&req("status=failed&rule=a&limit=5000&simulated=true")).unwrap();
        assert_eq!(filter.status, Some(ExecutionStatus::Failed));
        assert_eq!(filter.rule.as_deref(), Some("a"));
        assert_eq!(filter.simulated, Some(true));
        assert_eq!(limit, 1000, "limit is capped");
        assert!(filter_from(&req("status=bogus")).is_err());
        assert!(filter_from(&req("since=abc")).is_err());
        assert!(filter_from(&req("simulated=maybe")).is_err());
    }
}
