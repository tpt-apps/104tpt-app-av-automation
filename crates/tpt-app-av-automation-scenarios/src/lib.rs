//! Helpers for the integration and chaos suites (spec §18.4, §19).
//!
//! The suites themselves live in the spec-mandated top-level directories:
//!
//! * `tests/integration/` — whole-lifecycle scenarios against the shipped `tpt-av-automation`
//!   binary: validate, simulate, drive it over real sockets, read the API;
//! * `tests/chaos/` — the engine under hostile input and forced restarts.
//!
//! They are wired in as `[[test]]` targets in `Cargo.toml` rather than kept in this crate's own
//! `tests/` directory, so `cargo test --workspace` runs them while the layout still matches
//! spec.txt §4.
//!
//! These suites drive the shipped binary as a separate process, which is the only way to test what
//! an operator actually gets: real exit codes, real sockets, a real SQLite state directory and a
//! real forced restart. Everything here is process and filesystem plumbing; the assertions live
//! with each scenario.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Exit code for a successful run (spec §13).
pub const EXIT_OK: i32 = 0;

/// Exit code for a configuration error (spec §13).
pub const EXIT_CONFIG: i32 = 4;

/// Exit code for a skipped report: nothing happened, which is not a failure (spec §13).
pub const EXIT_SKIPPED: i32 = 3;

/// A token the API requires: at least 16 characters, per the config rules.
pub const TOKEN: &str = "scenario-token-0123456789abcdef";

/// How long a scenario waits for the engine to become observable before failing.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// Path to the CLI binary, building it first if it is not there yet.
///
/// These suites run the binary as a separate process, so `CARGO_BIN_EXE_*` (which only resolves for
/// a binary declared by *this* package) is unavailable. The CLI package cannot be a normal
/// dependency either, because it exposes only a `[[bin]]` target. So the binary is located in the
/// conventional target directory and built on demand if missing.
pub fn cli_bin() -> PathBuf {
    let exe = if cfg!(windows) {
        "tpt-av-automation.exe"
    } else {
        "tpt-av-automation"
    };
    // CARGO_MANIFEST_DIR is <workspace>/crates/tpt-app-av-automation-scenarios.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let debug = workspace.join("target/debug").join(exe);
    if debug.exists() {
        return debug;
    }
    let release = workspace.join("target/release").join(exe);
    if release.exists() {
        return release;
    }

    // Not built yet: build the CLI package. `cargo test --workspace` normally does this first, so
    // this path only runs when a single scenario target is invoked on its own.
    let status = Command::new("cargo")
        .current_dir(workspace)
        .args(["build", "--package", "tpt-app-av-automation-cli"])
        .status()
        .expect("cargo is required to build the CLI under test");
    assert!(
        status.success(),
        "building the CLI under test failed; run `cargo build --workspace` first"
    );
    assert!(
        debug.exists(),
        "the CLI was built but {} is missing",
        debug.display()
    );
    debug
}

/// A temporary directory removed when it goes out of scope.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates a fresh directory named after the scenario.
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tpt-av-scenario-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scenario directory");
        Self(dir)
    }

    /// Writes `content` to `name` and returns the full path.
    pub fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("write scenario file");
        path
    }

    /// Reads a file written by the process under test, retrying until it has content.
    pub fn read_when_present(&self, name: &str, timeout: Duration) -> String {
        let path = self.0.join(name);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.trim().is_empty() {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("`{}` never appeared in {}", name, self.0.display());
    }

    /// The directory itself.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the CLI to completion and returns its output.
pub fn cli(args: &[&str]) -> Output {
    Command::new(cli_bin())
        .args(args)
        .output()
        .expect("failed to run the CLI")
}

/// Exit code of a completed run.
pub fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the CLI was killed by a signal rather than exiting")
}

/// Standard output as text.
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Standard error as text.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A service process under test, killed on drop so a failing scenario cannot leak it.
pub struct Engine {
    child: Option<Child>,
}

impl Engine {
    /// Starts `tpt-av-automation` with the given arguments.
    pub fn start(args: &[&str]) -> Self {
        let child = Command::new(cli_bin())
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start the engine");
        Self { child: Some(child) }
    }

    /// The child process id, for killing it.
    pub fn id(&self) -> u32 {
        self.child.as_ref().expect("already stopped").id()
    }

    /// Kills the process immediately, as a power cut would: no graceful shutdown, no final flush.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Whether the process is still running, reaping it if it has exited.
    ///
    /// This needs `&mut self` because checking reaps the child, and reaping is the only way to
    /// observe an exit without waiting for it.
    pub fn is_running(&mut self) -> bool {
        self.exit_code_if_exited().is_none()
    }

    /// The exit code if the process has already exited, else `None`.
    pub fn exit_code_if_exited(&mut self) -> Option<i32> {
        let status = self.child.as_mut()?.try_wait().ok().flatten()?;
        Some(status.code().unwrap_or(-1))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Reserves a loopback UDP port and releases it, so a config can name a port that is very likely
/// free. There is an inherent race here; the scenarios retry rather than assume.
///
/// The port is also *bound* for the lifetime of the returned guard, so two scenarios running
/// concurrently can never be handed the same number.
pub fn reserve_udp_port() -> UdpPortGuard {
    UdpPortGuard {
        socket: UdpSocket::bind("127.0.0.1:0").expect("bind an ephemeral port"),
    }
}

/// Holds a UDP port number so nothing else in the process claims it while a config names it.
pub struct UdpPortGuard {
    socket: UdpSocket,
}

impl UdpPortGuard {
    /// The reserved port number.
    pub fn port(&self) -> u16 {
        self.socket
            .local_addr()
            .expect("read the ephemeral port")
            .port()
    }
}

/// Reserves a loopback UDP port and releases it immediately.
///
/// Prefer [`reserve_udp_port`], which keeps the port out of circulation while it is in use.
pub fn free_udp_port() -> u16 {
    reserve_udp_port().port()
}

/// A minimal HTTP request against the local API, returning the status code and body.
///
/// A connection failure yields status `0` rather than panicking, so this can be used to poll for
/// readiness while the engine is still starting up.
pub fn http_request(addr: &str, method: &str, path: &str, token: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let Ok(mut stream) = std::net::TcpStream::connect(addr) else {
        return (0, String::new());
    };
    if stream.set_read_timeout(Some(Duration::from_secs(5))).is_err() {
        return (0, String::new());
    }
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return (0, String::new());
    }
    let mut raw = Vec::new();
    if stream.read_to_end(&mut raw).is_err() {
        return (0, String::new());
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// A `GET` against the local API.
pub fn http_get(addr: &str, path: &str, token: &str) -> (u16, String) {
    http_request(addr, "GET", path, token)
}

/// A `POST` against the local API, which is what the mutating endpoints require.
pub fn http_post(addr: &str, path: &str, token: &str) -> (u16, String) {
    http_request(addr, "POST", path, token)
}

/// Polls `condition` until it holds, panicking with `what` if it never does.
pub fn wait_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out after {timeout:?} waiting for {what}");
}
