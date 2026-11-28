//! Integration tests for the plugin host: spawn/health/call/stop
//! against a real fake HTTP plugin child process, and `NotSupported`
//! for gRPC/WASM.
//!
//! The fake plugin is spawned with `cargo run --example
//! fake_http_plugin` (offline), so this test takes a few seconds on
//! a cold cache while the example builds.

use df_plugin_sdk::{PluginError, PluginHost, PluginManifest};
use std::time::Duration;

/// A free loopback port, allocated by binding `:0` and releasing it.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn http_plugin_manifest(port: u16) -> PluginManifest {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    PluginManifest {
        name: "echo-test-plugin".to_string(),
        version: "0.1.0".to_string(),
        protocol_version: "1".to_string(),
        transport: df_plugin_sdk::Transport::Http,
        endpoint: format!("http://127.0.0.1:{port}"),
        capabilities: vec!["edi.send_po".to_string()],
        command: Some(cargo),
        args: vec![
            "run".to_string(),
            "--quiet".to_string(),
            "--offline".to_string(),
            "--example".to_string(),
            "fake_http_plugin".to_string(),
            "--".to_string(),
            "--port".to_string(),
            port.to_string(),
        ],
        working_dir: Some(env!("CARGO_MANIFEST_DIR").to_string()),
    }
}

#[tokio::test]
async fn host_spawns_fake_plugin_and_calls_it_then_stops() {
    let port = free_port();
    let manifest = http_plugin_manifest(port);
    let host = PluginHost::spawn(manifest, Duration::from_secs(120))
        .await
        .expect("spawn fake plugin");
    assert!(host.is_running());
    host.health_check().await.expect("health check");
    let echoed = host
        .call("/echo", serde_json::json!({ "ping": "pong" }))
        .await
        .expect("echo call");
    assert_eq!(echoed["ping"], "pong");
    host.stop().await.expect("stop");
}

#[tokio::test]
async fn grpc_and_wasm_spawn_return_not_supported() {
    for transport in ["grpc", "wasm"] {
        let manifest = PluginManifest::parse(&format!(
            "name = \"p\"\nversion = \"0.1.0\"\nprotocol_version = \"1\"\ntransport = \"{transport}\"\nendpoint = \"http://127.0.0.1:1\"\n"
        ))
        .unwrap();
        let err = PluginHost::spawn(manifest, Duration::from_secs(1))
            .await
            .unwrap_err();
        match err {
            PluginError::NotSupported { transport: got, .. } => assert_eq!(got, transport),
            other => panic!("expected NotSupported for {transport}, got {other}"),
        }
    }
}

#[tokio::test]
async fn spawn_without_command_is_a_manifest_error() {
    let manifest = PluginManifest::parse(
        "name = \"p\"\nversion = \"0.1.0\"\nprotocol_version = \"1\"\ntransport = \"http\"\nendpoint = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();
    let err = PluginHost::spawn(manifest, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(err, PluginError::Manifest(_)));
}
