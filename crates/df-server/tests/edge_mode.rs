//! `df-server edge` smoke test: the mode starts, binds an ephemeral
//! port, and answers `/health` with the store id.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn spawn_edge(port: u16) -> Child {
    let exe = env!("CARGO_BIN_EXE_df-server");
    Command::new(exe)
        .args([
            "edge",
            "--db",
            "sqlite::memory:",
            "--store-id",
            "00000000-0000-7000-8000-000000001000",
            "--listen",
            &format!("127.0.0.1:{port}"),
        ])
        .spawn()
        .expect("spawn df-server edge")
}

fn get_health(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

#[test]
fn edge_mode_starts_and_answers_health() {
    let port = free_port();
    let mut child = spawn_edge(port);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut body = None;
    while Instant::now() < deadline {
        if let Some(response) = get_health(port)
            && response.contains("200")
        {
            body = Some(response);
            break;
        }
        // If the child died, fail fast with its exit status.
        if let Ok(Some(status)) = child.try_wait() {
            panic!("df-server edge exited early with {status}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let response = body.expect("df-server edge did not answer /health in time");
    assert!(response.contains("\"status\":\"ok\""), "{response}");
    assert!(
        response.contains("00000000-0000-7000-8000-000000001000"),
        "{response}"
    );
    child.kill().expect("kill df-server edge");
    let _ = child.wait();
}

#[test]
fn edge_help_works() {
    let exe = env!("CARGO_BIN_EXE_df-server");
    let output = Command::new(exe)
        .args(["edge", "--help"])
        .output()
        .expect("run df-server edge --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--db"));
    assert!(stdout.contains("--store-id"));
    assert!(stdout.contains("--listen"));
}

#[test]
fn other_modes_still_exit_2() {
    let exe = env!("CARGO_BIN_EXE_df-server");
    for mode in ["standalone", "hq", "worker", "migrate"] {
        let output = Command::new(exe)
            .arg(mode)
            .output()
            .expect("run df-server mode");
        assert_eq!(output.status.code(), Some(2), "mode {mode}");
    }
}
