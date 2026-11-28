//! `df-server hq` smoke test: the mode starts against an in-memory
//! SQLite database, migrates it, and answers `/health`; an admin
//! route without a token is 401 (the Phase 7 gate applies on HQ
//! exactly as on `df-api`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn spawn_hq(port: u16) -> Child {
    let exe = env!("CARGO_BIN_EXE_df-server");
    Command::new(exe)
        .args([
            "hq",
            "--db",
            "sqlite::memory:",
            "--org-id",
            "00000000-0000-7000-8000-000000002000",
            "--listen",
            &format!("127.0.0.1:{port}"),
        ])
        .spawn()
        .expect("spawn df-server hq")
}

fn get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .ok()?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

#[test]
fn hq_mode_starts_migrates_and_gates_admin_routes() {
    let port = free_port();
    let mut child = spawn_hq(port);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut health = None;
    while Instant::now() < deadline {
        if let Some(response) = get(port, "/health")
            && response.contains("200")
        {
            health = Some(response);
            break;
        }
        // If the child died, fail fast with its exit status.
        if let Ok(Some(status)) = child.try_wait() {
            panic!("df-server hq exited early with {status}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let health = health.expect("hq did not answer /health in 15s");
    assert!(health.contains(r#""version":"1.2.2""#), "{health}");
    // Admin routes are gated: no token is a 401, not a 503 (the
    // pool is configured) and not a 200 (the gate is live).
    let balances = get(port, "/api/v1/inventory/balances").expect("hq answers");
    assert!(balances.contains("401"), "{balances}");
    // The OpenAPI document stays public.
    let openapi = get(port, "/api-docs/openapi.json").expect("hq answers");
    assert!(openapi.contains("200"), "{openapi}");
    child.kill().expect("kill hq");
    let _ = child.wait();
}
