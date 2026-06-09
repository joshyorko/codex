use std::path::Path;

use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

fn serve_memoryd_status_once() -> Result<(String, std::sync::mpsc::Receiver<String>)> {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 4096];
        let bytes_read = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..bytes_read]).to_string();
        let _ = tx.send(request);
        let body = br#"{"ok":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });

    Ok((format!("http://{addr}"), rx))
}

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEX_HOME", codex_home);
    cmd.env(
        "LD_LIBRARY_PATH",
        match std::env::var("LD_LIBRARY_PATH") {
            Ok(existing) if !existing.is_empty() => {
                format!("/home/linuxbrew/.linuxbrew/lib:{existing}")
            }
            _ => "/home/linuxbrew/.linuxbrew/lib".to_string(),
        },
    );
    Ok(cmd)
}

#[tokio::test]
async fn memory_status_reports_local_defaults_without_writing_config() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = codex_command(codex_home.path())?;
    let output = cmd.args(["memory", "status"]).assert().success();
    let status: Value = serde_json::from_slice(&output.get_output().stdout)?;

    assert_eq!(status["backend"], "local");
    assert_eq!(status["provider"], "honcho");
    assert_eq!(status["profile"], "personal");
    assert_eq!(status["workspace"], "default");
    assert_eq!(status["provider_configured"], false);
    assert_eq!(status["health"]["status"], "local");
    assert!(!codex_home.path().join("config.toml").exists());

    Ok(())
}

#[tokio::test]
async fn memory_setup_codex_memoryd_writes_provider_config() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args([
        "memory",
        "setup",
        "--provider",
        "codex-memoryd",
        "--backend",
        "hybrid",
        "--provider-url",
        "http://127.0.0.1:9",
        "--profile",
        "oss",
        "--workspace",
        "codex-memory-lab",
    ])
    .assert()
    .success();

    let config = std::fs::read_to_string(codex_home.path().join("config.toml"))?;
    assert!(config.contains("[memories]"));
    assert!(config.contains("backend = \"hybrid\""));
    assert!(config.contains("provider = \"codex_memoryd\""));
    assert!(config.contains("provider_url = \"http://127.0.0.1:9\""));
    assert!(config.contains("profile = \"oss\""));
    assert!(config.contains("workspace = \"codex-memory-lab\""));
    assert!(config.contains("write_policy = \"visible_turns\""));
    assert!(config.contains("local_import_policy = \"manual\""));

    let mut status_cmd = codex_command(codex_home.path())?;
    let output = status_cmd.args(["memory", "status"]).assert().success();
    let status: Value = serde_json::from_slice(&output.get_output().stdout)?;
    assert_eq!(status["backend"], "hybrid");
    assert_eq!(status["provider"], "codex_memoryd");
    assert_eq!(status["provider_configured"], true);
    assert_eq!(status["health"]["status"], "unreachable");

    Ok(())
}

#[tokio::test]
async fn memory_setup_honcho_writes_env_var_name_not_secret() -> Result<()> {
    let codex_home = TempDir::new()?;

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args([
        "memory",
        "setup",
        "--provider",
        "honcho",
        "--backend",
        "provider",
        "--honcho-api-key-env",
        "HONCHO_TOKEN",
        "--workspace",
        "codex-memory-lab",
    ])
    .assert()
    .success();

    let config = std::fs::read_to_string(codex_home.path().join("config.toml"))?;
    assert!(config.contains("backend = \"provider\""));
    assert!(config.contains("provider = \"honcho\""));
    assert!(config.contains("honcho_api_key_env = \"HONCHO_TOKEN\""));
    assert!(!config.contains("api_key ="));

    let mut disable_cmd = codex_command(codex_home.path())?;
    disable_cmd.args(["memory", "disable"]).assert().success();

    let config = std::fs::read_to_string(codex_home.path().join("config.toml"))?;
    assert!(config.contains("backend = \"local\""));
    assert!(config.contains("honcho_api_key_env = \"HONCHO_TOKEN\""));

    Ok(())
}

#[tokio::test]
async fn memory_status_health_checks_codex_memoryd_status_endpoint() -> Result<()> {
    let codex_home = TempDir::new()?;
    let (provider_url, request_rx) = serve_memoryd_status_once()?;

    let mut setup_cmd = codex_command(codex_home.path())?;
    setup_cmd
        .args([
            "memory",
            "setup",
            "--provider",
            "codex-memoryd",
            "--backend",
            "provider",
            "--provider-url",
            &provider_url,
        ])
        .assert()
        .success();

    let mut status_cmd = codex_command(codex_home.path())?;
    let output = status_cmd.args(["memory", "status"]).assert().success();
    let status: Value = serde_json::from_slice(&output.get_output().stdout)?;

    assert_eq!(status["provider"], "codex_memoryd");
    assert_eq!(status["health"]["status"], "ok");
    let request = request_rx.recv_timeout(std::time::Duration::from_secs(2))?;
    assert!(request.starts_with("GET /v1/status "));

    Ok(())
}
