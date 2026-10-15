//! Opt-in outbound HTTP notifications (spec §8.3).
//!
//! The sandbox rules mirror [`crate::exec`]:
//!
//! * disabled unless the operator enables it, and then only for an explicit allow-list of URLs;
//! * plain `http://` only — TLS needs a dependency this crate does not carry, so an `https://`
//!   target is refused with a reason rather than sent in the clear or silently dropped;
//! * one request per action, with a wall-clock bound; redirects are not followed;
//! * the response body is read only as far as the status line.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::Value;
use tpt_app_av_automation_model::ActionOutcome;

/// Which URLs an operator has permitted.
#[derive(Debug, Clone, Default)]
pub struct WebhookPolicy {
    /// Master switch. Off by default.
    pub enabled: bool,
    /// URL prefixes that may be called. A prefix matches the exact URL, or a URL that continues it
    /// at a path, query or fragment boundary, so `http://host:80` does not admit `http://host:8080`.
    pub allow: Vec<String>,
}

impl WebhookPolicy {
    /// A policy that permits exactly the URLs under the given prefixes.
    pub fn allowing<I, S>(prefixes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            enabled: true,
            allow: prefixes.into_iter().map(Into::into).collect(),
        }
    }

    fn permits(&self, url: &str) -> bool {
        self.allow.iter().any(|prefix| {
            let prefix = prefix.as_str();
            if url == prefix {
                return true;
            }
            match url.strip_prefix(prefix) {
                Some(rest) => prefix.ends_with('/') || rest.starts_with(['/', '?', '#']),
                None => false,
            }
        })
    }
}

/// Splits an `http://` URL into `(authority, path-and-query)`.
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("http://")?;
    if rest.is_empty() {
        return None;
    }
    Some(match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    })
}

/// POSTs `body` to `url` under `policy`, bounded by `timeout_ms`.
pub(crate) fn post(
    policy: &WebhookPolicy,
    url: &str,
    body: &Value,
    timeout_ms: u64,
) -> ActionOutcome {
    if !policy.enabled {
        return ActionOutcome::Failed {
            reason: "outbound webhooks are disabled".into(),
        };
    }
    if !policy.permits(url) {
        return ActionOutcome::Failed {
            reason: format!("webhook `{url}` is not on the allow-list"),
        };
    }
    let Some((authority, path)) = split_url(url) else {
        return ActionOutcome::Failed {
            reason: format!("webhook `{url}` must be an http:// URL (https is not supported)"),
        };
    };

    let timeout = Duration::from_millis(timeout_ms.max(1));
    let started = Instant::now();
    let timed_out = || ActionOutcome::TimedOut {
        after_ms: timeout_ms,
    };

    let addrs = match authority.to_socket_addrs() {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(e) => {
            return ActionOutcome::Failed {
                reason: format!("could not resolve `{authority}`: {e}"),
            }
        }
    };
    let Some(addr) = addrs.first() else {
        return ActionOutcome::Failed {
            reason: format!("`{authority}` resolved to no addresses"),
        };
    };
    let mut stream = match TcpStream::connect_timeout(addr, timeout) {
        Ok(stream) => stream,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return timed_out(),
        Err(e) => {
            return ActionOutcome::Failed {
                reason: format!("could not connect to `{authority}`: {e}"),
            }
        }
    };
    let payload = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let remaining = timeout
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(1));
    if stream.set_write_timeout(Some(remaining)).is_err()
        || stream.set_read_timeout(Some(remaining)).is_err()
    {
        return ActionOutcome::Failed {
            reason: "could not bound the webhook socket".into(),
        };
    }
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return io_outcome(e, authority, timeout_ms);
    }

    let mut reader = BufReader::new(stream.take(64 * 1024));
    let mut status_line = String::new();
    if let Err(e) = reader.read_line(&mut status_line) {
        return io_outcome(e, authority, timeout_ms);
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok());
    match status {
        Some(code) if (200..300).contains(&code) => ActionOutcome::Success,
        Some(code) => ActionOutcome::Failed {
            reason: format!("webhook `{url}` answered with status {code}"),
        },
        None => ActionOutcome::Failed {
            reason: format!("webhook `{url}` sent an unreadable response"),
        },
    }
}

fn io_outcome(e: std::io::Error, authority: &str, timeout_ms: u64) -> ActionOutcome {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ActionOutcome::TimedOut {
            after_ms: timeout_ms,
        },
        _ => ActionOutcome::Failed {
            reason: format!("webhook to `{authority}` failed: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;

    /// Serves one request on a loopback port, answering with `status_line`.
    fn one_shot_server(status_line: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the headers, then as much body as they declare.
            let header_end = loop {
                if let Some(i) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
                let n = conn.read(&mut buf).unwrap();
                if n == 0 {
                    break received.len();
                }
                received.extend_from_slice(&buf[..n]);
            };
            let headers = String::from_utf8_lossy(&received[..header_end]).into_owned();
            let declared: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            while received.len() < header_end + declared {
                let n = conn.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            let response =
                format!("{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            conn.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&received).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn disabled_by_default() {
        let out = post(
            &WebhookPolicy::default(),
            "http://127.0.0.1:1/",
            &json!({}),
            500,
        );
        assert!(matches!(out, ActionOutcome::Failed { reason } if reason.contains("disabled")));
    }

    #[test]
    fn urls_outside_the_allow_list_are_refused() {
        let policy = WebhookPolicy::allowing(["http://10.0.0.5:8080/"]);
        let out = post(&policy, "http://10.0.0.6:8080/x", &json!({}), 500);
        assert!(matches!(out, ActionOutcome::Failed { reason } if reason.contains("allow-list")));
    }

    #[test]
    fn a_prefix_admits_only_its_own_boundary() {
        let policy = WebhookPolicy::allowing(["http://host:80"]);
        assert!(policy.permits("http://host:80"));
        assert!(policy.permits("http://host:80/a"));
        assert!(policy.permits("http://host:80?q=1"));
        assert!(!policy.permits("http://host:8080/a"));
        let slashed = WebhookPolicy::allowing(["http://host:80/"]);
        assert!(slashed.permits("http://host:80/a"));
        assert!(!slashed.permits("http://host:8080/a"));
    }

    #[test]
    fn https_is_refused_with_a_reason() {
        let policy = WebhookPolicy::allowing(["https://example.com/"]);
        let out = post(&policy, "https://example.com/", &json!({}), 500);
        assert!(matches!(out, ActionOutcome::Failed { reason } if reason.contains("https")));
    }

    #[test]
    fn a_2xx_response_is_success_and_the_body_is_sent() {
        let (url, server) = one_shot_server("HTTP/1.1 204 No Content");
        let policy = WebhookPolicy::allowing([url.clone()]);
        let out = post(&policy, &url, &json!({"message": "hi"}), 5000);
        assert_eq!(out, ActionOutcome::Success);
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /hook HTTP/1.1"));
        assert!(received.contains("Content-Type: application/json"));
        assert!(received.ends_with(r#"{"message":"hi"}"#));
    }

    #[test]
    fn a_non_2xx_response_is_a_failure() {
        let (url, server) = one_shot_server("HTTP/1.1 503 Service Unavailable");
        let policy = WebhookPolicy::allowing([url.clone()]);
        let out = post(&policy, &url, &json!({}), 5000);
        assert!(matches!(out, ActionOutcome::Failed { reason } if reason.contains("503")));
        server.join().unwrap();
    }

    /// Nothing listens on the target, so the delivery must end without success. Whether the OS
    /// reports the refusal at once or the bound expires first is platform-specific, so either
    /// outcome is accepted.
    #[test]
    fn an_unreachable_target_is_never_a_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/");
        let policy = WebhookPolicy::allowing([url.clone()]);
        assert!(matches!(
            post(&policy, &url, &json!({}), 2000),
            ActionOutcome::Failed { .. } | ActionOutcome::TimedOut { .. }
        ));
    }
}
