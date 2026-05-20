use std::io::Write;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use codex_app_server::AppServerRuntimeOptions;
use codex_app_server::AppServerTransport;
use codex_app_server::AppServerWebsocketAuthSettings;
use codex_app_server_daemon::LifecycleCommand as AppServerLifecycleCommand;
use codex_app_server_daemon::LifecycleOutput as AppServerLifecycleOutput;
use codex_app_server_daemon::LifecycleStatus as AppServerLifecycleStatus;
use codex_app_server_daemon::RemoteControlReadyOutput as AppServerRemoteControlReadyOutput;
use codex_app_server_daemon::RemoteControlReadyStatus as AppServerRemoteControlReadyStatus;
use codex_app_server_daemon::RemoteControlStartOutput as AppServerRemoteControlStartOutput;
use codex_app_server_protocol::RemoteControlConnectionStatus;
use codex_arg0::Arg0DispatchPaths;
use codex_core::config::Config;
use codex_config::LoaderOverrides;
use codex_login::AuthDotJson;
use codex_login::load_auth_dot_json;
use codex_protocol::protocol::SessionSource;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::CliConfigOverrides;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::header::AUTHORIZATION;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde::Serialize;
use std::io::IsTerminal;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const FOREGROUND_SOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FOREGROUND_SOCKET_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const FOREGROUND_APP_SERVER_ABORT_TIMEOUT: Duration = Duration::from_secs(1);
const REMOTE_CONTROL_HOSTS_LIMIT: usize = 100;
const CHATGPT_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const ORIGINATOR_HEADER: &str = "originator";

#[derive(Debug, Args)]
pub(crate) struct RemoteControlCommand {
    /// Emit machine-readable JSON.
    #[arg(long = "json", global = true)]
    json: bool,

    #[command(subcommand)]
    subcommand: Option<RemoteControlSubcommand>,
}

impl RemoteControlCommand {
    pub(crate) fn subcommand_name(&self) -> &'static str {
        match &self.subcommand {
            None => "remote-control",
            Some(RemoteControlSubcommand::Start) => "remote-control start",
            Some(RemoteControlSubcommand::Stop) => "remote-control stop",
            Some(RemoteControlSubcommand::Hosts(_)) => "remote-control hosts",
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum RemoteControlSubcommand {
    /// Start the app-server daemon with remote control enabled.
    Start,

    /// Stop the app-server daemon.
    Stop,

    /// List and remove remote-control hosts.
    Hosts(RemoteControlHostsCommand),
}

#[derive(Debug, Args)]
struct RemoteControlHostsCommand {
    /// Only show hosts that are not currently online.
    #[arg(long)]
    stale: bool,

    /// Allow deletion of hosts that are currently online.
    #[arg(long = "include-online")]
    include_online: bool,

    #[command(subcommand)]
    subcommand: Option<RemoteControlHostsSubcommand>,
}

#[derive(Debug, clap::Subcommand)]
enum RemoteControlHostsSubcommand {
    /// Print registered remote-control hosts.
    List,

    /// Delete a registered remote-control host by environment id.
    Delete {
        /// Environment id to delete.
        environment_id: String,

        /// Allow deletion if the host is currently online.
        #[arg(long = "include-online")]
        include_online: bool,

        /// Confirm deletion without an interactive prompt.
        #[arg(long)]
        yes: bool,
    },
}

pub(crate) async fn run(
    command: RemoteControlCommand,
    arg0_paths: Arg0DispatchPaths,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    match command.subcommand {
        None => {
            print_remote_control_progress(
                command.json,
                "Starting app-server with remote control enabled...",
            )?;
            run_foreground_remote_control(command.json, arg0_paths, root_config_overrides).await?;
        }
        Some(RemoteControlSubcommand::Start) => {
            print_remote_control_progress(
                command.json,
                "Starting app-server daemon with remote control enabled...",
            )?;
            let output = codex_app_server_daemon::ensure_remote_control_ready().await?;
            print_remote_control_start_output(&output, command.json)?;
        }
        Some(RemoteControlSubcommand::Stop) => {
            print_remote_control_progress(command.json, "Stopping remote control...")?;
            let output = codex_app_server_daemon::run(AppServerLifecycleCommand::Stop).await?;
            print_remote_control_stop_output(&output, command.json)?;
        }
        Some(RemoteControlSubcommand::Hosts(hosts_command)) => {
            run_remote_control_hosts(command.json, hosts_command, root_config_overrides).await?;
        }
    }
    Ok(())
}

fn print_remote_control_progress(json: bool, message: &str) -> anyhow::Result<()> {
    if json {
        return Ok(());
    }

    println!("{message}");
    std::io::stdout()
        .flush()
        .context("failed to flush remote-control progress message")?;
    Ok(())
}

async fn run_foreground_remote_control(
    json: bool,
    arg0_paths: Arg0DispatchPaths,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    let socket_dir = tempfile::Builder::new()
        .prefix("codex-rc-")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::tempdir())
        .context("failed to create private app-server socket directory")?;
    let socket_path = socket_dir.path().join("rc.sock");
    let socket_path = AbsolutePathBuf::from_absolute_path(&socket_path)
        .context("private app-server socket path was not absolute")?;
    let transport = AppServerTransport::UnixSocket {
        socket_path: socket_path.clone(),
    };
    let runtime_options = AppServerRuntimeOptions {
        remote_control_enabled: true,
        install_shutdown_signal_handler: false,
        ..Default::default()
    };
    let (stop_rx, stop_signal_task) = foreground_stop_signal();
    let mut app_server_task = tokio::spawn(codex_app_server::run_main_with_transport_options(
        arg0_paths,
        root_config_overrides,
        LoaderOverrides::default(),
        /*strict_config*/ false,
        /*default_analytics_enabled*/ false,
        transport,
        SessionSource::VSCode,
        AppServerWebsocketAuthSettings::default(),
        runtime_options,
    ));

    let summary = match wait_for_foreground_remote_control_start(
        &mut app_server_task,
        wait_for_foreground_remote_control_ready(socket_path),
        stop_rx.clone(),
    )
    .await
    {
        ForegroundStartupResult::Ready(summary) => summary,
        ForegroundStartupResult::Stopped => {
            abort_foreground_app_server(app_server_task).await;
            stop_signal_task.abort();
            return Ok(());
        }
        ForegroundStartupResult::ReadyFailed(error) => {
            abort_foreground_app_server(app_server_task).await;
            stop_signal_task.abort();
            return Err(error);
        }
        ForegroundStartupResult::AppServerExited(error) => {
            stop_signal_task.abort();
            return Err(error);
        }
    };

    if *stop_rx.borrow() {
        abort_foreground_app_server(app_server_task).await;
        stop_signal_task.abort();
        return Ok(());
    }

    if let Err(error) = print_foreground_ready_output(&summary, json) {
        abort_foreground_app_server(app_server_task).await;
        stop_signal_task.abort();
        return Err(error);
    }

    let result = wait_for_foreground_app_server(app_server_task, stop_rx).await;
    stop_signal_task.abort();
    result
}

fn foreground_stop_signal() -> (watch::Receiver<bool>, JoinHandle<()>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        if let Err(err) = tokio::signal::ctrl_c().await {
            eprintln!("failed to listen for Ctrl-C: {err}");
        }
        let _ = stop_tx.send(true);
    });
    (stop_rx, task)
}

enum ForegroundStartupResult {
    Ready(AppServerRemoteControlReadyStatus),
    Stopped,
    ReadyFailed(anyhow::Error),
    AppServerExited(anyhow::Error),
}

async fn wait_for_foreground_remote_control_start(
    app_server_task: &mut JoinHandle<std::io::Result<()>>,
    ready: impl std::future::Future<Output = anyhow::Result<AppServerRemoteControlReadyStatus>>,
    mut stop_rx: watch::Receiver<bool>,
) -> ForegroundStartupResult {
    tokio::pin!(ready);

    tokio::select! {
        ready_result = &mut ready => match ready_result {
            Ok(summary) => ForegroundStartupResult::Ready(summary),
            Err(error) => ForegroundStartupResult::ReadyFailed(error),
        },
        app_server_result = app_server_task => {
            ForegroundStartupResult::AppServerExited(
                foreground_app_server_exited_before_ready(app_server_result)
            )
        }
        _ = wait_for_stop_signal(&mut stop_rx) => ForegroundStartupResult::Stopped,
    }
}

async fn wait_for_foreground_app_server(
    mut app_server_task: JoinHandle<std::io::Result<()>>,
    mut stop_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    tokio::select! {
        app_server_result = &mut app_server_task => {
            app_server_result
                .context("foreground app-server task failed to join")?
                .context("foreground app-server exited with an error")?;
        }
        _ = wait_for_stop_signal(&mut stop_rx) => {
            abort_foreground_app_server(app_server_task).await;
        }
    }

    Ok(())
}

async fn wait_for_stop_signal(stop_rx: &mut watch::Receiver<bool>) {
    if *stop_rx.borrow() {
        return;
    }
    let _ = stop_rx.wait_for(|stopped| *stopped).await;
}

fn foreground_app_server_exited_before_ready(
    result: Result<std::io::Result<()>, tokio::task::JoinError>,
) -> anyhow::Error {
    match result {
        Ok(Ok(())) => {
            anyhow::anyhow!("foreground app-server exited before remote control became ready")
        }
        Ok(Err(error)) => anyhow::Error::new(error)
            .context("foreground app-server exited before remote control became ready"),
        Err(error) => anyhow::Error::new(error)
            .context("foreground app-server task failed before remote control became ready"),
    }
}

async fn abort_foreground_app_server(app_server_task: JoinHandle<std::io::Result<()>>) {
    app_server_task.abort();
    let _ = timeout(FOREGROUND_APP_SERVER_ABORT_TIMEOUT, app_server_task).await;
}

async fn wait_for_foreground_remote_control_ready(
    socket_path: AbsolutePathBuf,
) -> anyhow::Result<AppServerRemoteControlReadyStatus> {
    codex_app_server_daemon::enable_remote_control_on_socket(
        socket_path.as_path(),
        FOREGROUND_SOCKET_CONNECT_TIMEOUT,
        FOREGROUND_SOCKET_CONNECT_RETRY_DELAY,
    )
    .await
}

fn print_remote_control_start_output(
    output: &AppServerRemoteControlReadyOutput,
    json: bool,
) -> anyhow::Result<()> {
    ensure_remote_control_startable(&output.remote_control)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&RemoteControlStartJsonOutput::daemon(output))?
        );
        return Ok(());
    }

    for line in remote_control_start_human_lines(
        &output.remote_control,
        RemoteControlHumanOutputMode::Daemon,
    )? {
        println!("{line}");
    }
    for line in daemon_app_server_human_lines(&output.daemon) {
        println!("{line}");
    }
    Ok(())
}

fn print_foreground_ready_output(
    summary: &AppServerRemoteControlReadyStatus,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        ensure_remote_control_startable(summary)?;
        println!(
            "{}",
            serde_json::to_string(&RemoteControlStartJsonOutput::foreground(summary))?
        );
        return Ok(());
    }

    for line in remote_control_start_human_lines(summary, RemoteControlHumanOutputMode::Foreground)?
    {
        println!("{line}");
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlStartJsonOutput<'a> {
    mode: RemoteControlModeJson,
    status: RemoteControlConnectionStatus,
    server_name: &'a str,
    environment_id: Option<&'a str>,
    timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    daemon: Option<&'a AppServerRemoteControlStartOutput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RemoteControlModeJson {
    Foreground,
    Daemon,
}

impl<'a> RemoteControlStartJsonOutput<'a> {
    fn foreground(summary: &'a AppServerRemoteControlReadyStatus) -> Self {
        Self {
            mode: RemoteControlModeJson::Foreground,
            status: summary.status,
            server_name: &summary.server_name,
            environment_id: summary.environment_id.as_deref(),
            timed_out: summary.timed_out,
            daemon: None,
        }
    }

    fn daemon(output: &'a AppServerRemoteControlReadyOutput) -> Self {
        let remote_control = &output.remote_control;
        Self {
            mode: RemoteControlModeJson::Daemon,
            status: remote_control.status,
            server_name: &remote_control.server_name,
            environment_id: remote_control.environment_id.as_deref(),
            timed_out: remote_control.timed_out,
            daemon: Some(&output.daemon),
        }
    }
}

fn remote_control_start_human_message(
    output: &AppServerRemoteControlReadyStatus,
) -> anyhow::Result<String> {
    ensure_remote_control_startable(output)?;
    match output.status {
        RemoteControlConnectionStatus::Connected => Ok(format!(
            "This machine is available for remote control as {}.",
            output.server_name
        )),
        RemoteControlConnectionStatus::Connecting => Ok(format!(
            "Remote control is enabled on {} and still connecting.",
            output.server_name
        )),
        RemoteControlConnectionStatus::Errored | RemoteControlConnectionStatus::Disabled => {
            unreachable!("errored and disabled statuses are rejected before formatting")
        }
    }
}

fn ensure_remote_control_startable(
    output: &AppServerRemoteControlReadyStatus,
) -> anyhow::Result<()> {
    match output.status {
        RemoteControlConnectionStatus::Connected | RemoteControlConnectionStatus::Connecting => {
            Ok(())
        }
        RemoteControlConnectionStatus::Errored => {
            anyhow::bail!(
                "Remote control is enabled on {} but the connection is errored.",
                output.server_name
            );
        }
        RemoteControlConnectionStatus::Disabled => {
            anyhow::bail!("Remote control is disabled on {}.", output.server_name);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlHumanOutputMode {
    Foreground,
    Daemon,
}

fn remote_control_start_human_lines(
    summary: &AppServerRemoteControlReadyStatus,
    mode: RemoteControlHumanOutputMode,
) -> anyhow::Result<Vec<String>> {
    let mut lines = vec![remote_control_start_human_message(summary)?];
    match mode {
        RemoteControlHumanOutputMode::Foreground => {
            lines.push("Press Ctrl-C to stop.".to_string());
        }
        RemoteControlHumanOutputMode::Daemon => {}
    }
    Ok(lines)
}

fn daemon_app_server_human_lines(output: &AppServerRemoteControlStartOutput) -> Vec<String> {
    let (managed_codex_path, managed_codex_version) = daemon_app_server_identity(output);
    vec![
        "Daemon used app-server:".to_string(),
        format!("  path: {}", managed_codex_path.display()),
        format!("  version: {}", managed_codex_version.unwrap_or("unknown")),
    ]
}

fn daemon_app_server_identity(
    output: &AppServerRemoteControlStartOutput,
) -> (&std::path::Path, Option<&str>) {
    match output {
        AppServerRemoteControlStartOutput::Bootstrap(output) => (
            &output.managed_codex_path,
            output.managed_codex_version.as_deref(),
        ),
        AppServerRemoteControlStartOutput::Start(output) => (
            &output.managed_codex_path,
            output.managed_codex_version.as_deref(),
        ),
    }
}

fn print_remote_control_stop_output(
    output: &AppServerLifecycleOutput,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(output)?);
        return Ok(());
    }

    println!("{}", remote_control_stop_human_message(output));
    Ok(())
}

fn remote_control_stop_human_message(output: &AppServerLifecycleOutput) -> String {
    match output.status {
        AppServerLifecycleStatus::Stopped => "Remote control stopped.".to_string(),
        AppServerLifecycleStatus::NotRunning => "Remote control is not running.".to_string(),
        AppServerLifecycleStatus::Started
        | AppServerLifecycleStatus::Restarted
        | AppServerLifecycleStatus::AlreadyRunning
        | AppServerLifecycleStatus::Running => {
            format!(
                "Remote control stop completed with status {:?}.",
                output.status
            )
        }
    }
}

async fn run_remote_control_hosts(
    json: bool,
    command: RemoteControlHostsCommand,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    let client = RemoteControlHostsClient::new(root_config_overrides).await?;
    match command.subcommand {
        Some(RemoteControlHostsSubcommand::List) => {
            let hosts = client.list_hosts().await?;
            print_remote_control_hosts(&hosts, command.stale, json)?;
        }
        Some(RemoteControlHostsSubcommand::Delete {
            environment_id,
            include_online,
            yes,
        }) => {
            delete_remote_control_host(
                &client,
                &environment_id,
                command.include_online || include_online,
                yes,
                json,
            )
            .await?;
        }
        None => {
            let hosts = client.list_hosts().await?;
            if json {
                print_remote_control_hosts(&hosts, command.stale, true)?;
            } else {
                interactive_remote_control_hosts(
                    &client,
                    hosts,
                    command.stale,
                    command.include_online,
                )
                .await?;
            }
        }
    }
    Ok(())
}

struct RemoteControlHostsClient {
    http: reqwest::Client,
    base_url: String,
}

impl RemoteControlHostsClient {
    async fn new(root_config_overrides: CliConfigOverrides) -> anyhow::Result<Self> {
        let config = Config::load_with_cli_overrides(root_config_overrides.parse_overrides()?)
            .await
            .context("failed to load Codex config")?;
        let auth = load_auth_dot_json(&config.codex_home, config.cli_auth_credentials_store_mode)
            .context("failed to load Codex auth credentials")?
            .context("codex remote-control hosts requires ChatGPT login")?;
        let headers = remote_control_hosts_headers(&auth)?;
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build remote-control hosts HTTP client")?;
        Ok(Self {
            http,
            base_url: config.chatgpt_base_url.trim_end_matches('/').to_string(),
        })
    }

    async fn list_hosts(&self) -> anyhow::Result<Vec<RemoteControlHost>> {
        let url = format!(
            "{}/codex/remote/control/environments?limit={}",
            self.base_url, REMOTE_CONTROL_HOSTS_LIMIT
        );
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("failed to request remote-control hosts")?
            .error_for_status()
            .context("remote-control hosts request failed")?;
        let body: RemoteControlHostsResponse = response
            .json()
            .await
            .context("failed to decode remote-control hosts response")?;
        Ok(body.items)
    }

    async fn delete_host(&self, environment_id: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/codex/remote/control/environments/{}",
            self.base_url,
            percent_encode_path_segment(environment_id)
        );
        self.http
            .delete(url)
            .send()
            .await
            .context("failed to delete remote-control host")?
            .error_for_status()
            .context("remote-control host delete failed")?;
        Ok(())
    }
}

fn remote_control_hosts_headers(auth: &AuthDotJson) -> anyhow::Result<HeaderMap> {
    let tokens = auth
        .tokens
        .as_ref()
        .context("codex remote-control hosts requires ChatGPT login")?;
    let mut headers = HeaderMap::new();
    let auth_header = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token))
        .context("failed to build authorization header")?;
    headers.insert(AUTHORIZATION, auth_header);
    headers.insert(USER_AGENT, HeaderValue::from_static("Codex CLI"));
    headers.insert(
        ORIGINATOR_HEADER,
        HeaderValue::from_static("Codex CLI remote-control hosts"),
    );
    if let Some(account_id) = tokens
        .account_id
        .as_deref()
        .or(tokens.id_token.chatgpt_account_id.as_deref())
    {
        headers.insert(
            CHATGPT_ACCOUNT_ID_HEADER,
            HeaderValue::from_str(account_id).context("failed to build account header")?,
        );
    }
    Ok(headers)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlHost {
    #[serde(alias = "env_id")]
    env_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, alias = "display_name")]
    display_name: Option<String>,
    #[serde(default, alias = "host_name")]
    host_name: Option<String>,
    #[serde(default, alias = "client_type")]
    client_type: Option<String>,
    #[serde(default)]
    online: Option<bool>,
    #[serde(default)]
    busy: Option<bool>,
    #[serde(default, alias = "last_seen_at")]
    last_seen_at: Option<String>,
    #[serde(default, alias = "installation_id")]
    installation_id: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default, alias = "app_server_version")]
    app_server_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoteControlHostsResponse {
    #[serde(default, alias = "data")]
    items: Vec<RemoteControlHost>,
}

impl RemoteControlHost {
    fn display_name(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.name.as_deref())
            .or(self.host_name.as_deref())
            .unwrap_or("(unnamed)")
    }

    fn is_online(&self) -> bool {
        self.online.unwrap_or(false)
    }

    fn status_label(&self) -> &'static str {
        match (self.is_online(), self.busy.unwrap_or(false)) {
            (true, true) => "online/busy",
            (true, false) => "online",
            (false, _) => "offline",
        }
    }

    fn client_label(&self) -> &str {
        self.client_type.as_deref().unwrap_or("unknown")
    }

    fn last_seen_label(&self) -> &str {
        self.last_seen_at.as_deref().unwrap_or("never")
    }
}

fn filtered_remote_control_hosts(
    hosts: &[RemoteControlHost],
    stale_only: bool,
) -> Vec<&RemoteControlHost> {
    hosts
        .iter()
        .filter(|host| !stale_only || !host.is_online())
        .collect()
}

fn print_remote_control_hosts(
    hosts: &[RemoteControlHost],
    stale_only: bool,
    json: bool,
) -> anyhow::Result<()> {
    let filtered = filtered_remote_control_hosts(hosts, stale_only);
    if json {
        println!("{}", serde_json::to_string(&filtered)?);
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No remote-control hosts found.");
        return Ok(());
    }

    for line in remote_control_hosts_table_lines(&filtered) {
        println!("{line}");
    }
    Ok(())
}

fn remote_control_hosts_table_lines(hosts: &[&RemoteControlHost]) -> Vec<String> {
    let mut lines = Vec::with_capacity(hosts.len() + 1);
    lines.push(format!(
        "{:<3} {:<28} {:<12} {:<14} {:<24} {}",
        "#", "Name", "Status", "Client", "Last seen", "Environment"
    ));
    for (index, host) in hosts.iter().enumerate() {
        lines.push(format!(
            "{:<3} {:<28} {:<12} {:<14} {:<24} {}",
            index + 1,
            truncate_for_table(host.display_name(), 28),
            host.status_label(),
            truncate_for_table(host.client_label(), 14),
            truncate_for_table(host.last_seen_label(), 24),
            host.env_id
        ));
    }
    lines
}

fn truncate_for_table(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

async fn interactive_remote_control_hosts(
    client: &RemoteControlHostsClient,
    mut hosts: Vec<RemoteControlHost>,
    mut stale_only: bool,
    mut include_online: bool,
) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        print_remote_control_hosts(&hosts, stale_only, false)?;
        return Ok(());
    }

    loop {
        println!();
        println!("Remote-control hosts");
        println!(
            "  filter: {}   online delete: {}",
            if stale_only { "stale only" } else { "all" },
            if include_online { "unlocked" } else { "locked" }
        );
        let filtered = filtered_remote_control_hosts(&hosts, stale_only);
        if filtered.is_empty() {
            println!("No remote-control hosts found.");
        } else {
            for line in remote_control_hosts_table_lines(&filtered) {
                println!("{line}");
            }
        }
        println!();
        println!("Enter host number to delete, f filter, o online lock, r refresh, q quit:");
        print!("> ");
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();
        match input {
            "" | "q" | "quit" => return Ok(()),
            "f" => {
                stale_only = !stale_only;
            }
            "o" => {
                include_online = !include_online;
            }
            "r" => {
                hosts = client.list_hosts().await?;
            }
            selection => {
                let index = match selection.parse::<usize>() {
                    Ok(index) => index,
                    Err(_) => {
                        println!("Unrecognized selection: {selection}");
                        continue;
                    }
                };
                if index == 0 || index > filtered.len() {
                    println!("Host selection out of range.");
                    continue;
                }
                let host = (*filtered[index - 1]).clone();
                confirm_and_delete_remote_control_host(client, &host, include_online).await?;
                hosts = client.list_hosts().await?;
            }
        }
    }
}

async fn delete_remote_control_host(
    client: &RemoteControlHostsClient,
    environment_id: &str,
    include_online: bool,
    yes: bool,
    json: bool,
) -> anyhow::Result<()> {
    let hosts = client.list_hosts().await?;
    let host = hosts
        .iter()
        .find(|host| host.env_id == environment_id)
        .with_context(|| format!("Remote-control host {environment_id} was not found."))?;
    ensure_remote_control_host_deletable(host, include_online)?;
    if !yes {
        anyhow::bail!("Refusing to delete without --yes.");
    }
    client.delete_host(environment_id).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "deleted": true,
                "environmentId": environment_id,
            }))?
        );
    } else {
        println!("Deleted remote-control host {environment_id}.");
    }
    Ok(())
}

async fn confirm_and_delete_remote_control_host(
    client: &RemoteControlHostsClient,
    host: &RemoteControlHost,
    include_online: bool,
) -> anyhow::Result<()> {
    ensure_remote_control_host_deletable(host, include_online)?;
    println!();
    println!("Delete {}?", host.display_name());
    println!("  status: {}", host.status_label());
    println!("  last seen: {}", host.last_seen_label());
    println!("  environment: {}", host.env_id);
    println!("Type the environment id to confirm:");
    print!("> ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim() != host.env_id {
        println!("Delete cancelled.");
        return Ok(());
    }
    client.delete_host(&host.env_id).await?;
    println!("Deleted remote-control host {}.", host.env_id);
    Ok(())
}

fn ensure_remote_control_host_deletable(
    host: &RemoteControlHost,
    include_online: bool,
) -> anyhow::Result<()> {
    if host.is_online() && !include_online {
        anyhow::bail!(
            "Refusing to delete online host {}. Re-run with --include-online to unlock online deletion.",
            host.env_id
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::path::PathBuf;

    use super::*;

    fn remote_control_status(
        status: RemoteControlConnectionStatus,
    ) -> AppServerRemoteControlReadyStatus {
        AppServerRemoteControlReadyStatus {
            status,
            server_name: "owen-mbp".to_string(),
            environment_id: Some("env_test".to_string()),
            timed_out: status == RemoteControlConnectionStatus::Connecting,
        }
    }

    fn daemon_ready_output(
        status: RemoteControlConnectionStatus,
    ) -> AppServerRemoteControlReadyOutput {
        AppServerRemoteControlReadyOutput {
            daemon: AppServerRemoteControlStartOutput::Start(AppServerLifecycleOutput {
                status: AppServerLifecycleStatus::Started,
                backend: None,
                pid: Some(42),
                managed_codex_path: PathBuf::from("/opt/codex/bin/codex"),
                managed_codex_version: Some("1.0.0".to_string()),
                socket_path: PathBuf::from("/tmp/app-server-control.sock"),
                cli_version: Some("1.0.0".to_string()),
                app_server_version: Some("2.0.0".to_string()),
            }),
            remote_control: AppServerRemoteControlReadyStatus {
                status,
                server_name: "owen-mbp".to_string(),
                environment_id: Some("env_test".to_string()),
                timed_out: status == RemoteControlConnectionStatus::Connecting,
            },
        }
    }

    #[test]
    fn remote_control_human_start_messages_use_server_name() {
        assert_eq!(
            remote_control_start_human_message(&remote_control_status(
                RemoteControlConnectionStatus::Connected
            ))
            .expect("connected message"),
            "This machine is available for remote control as owen-mbp."
        );
        assert_eq!(
            remote_control_start_human_message(&remote_control_status(
                RemoteControlConnectionStatus::Connecting
            ))
            .expect("connecting message"),
            "Remote control is enabled on owen-mbp and still connecting."
        );
        assert_eq!(
            remote_control_start_human_message(&remote_control_status(
                RemoteControlConnectionStatus::Errored
            ))
            .expect_err("errored status should fail")
            .to_string(),
            "Remote control is enabled on owen-mbp but the connection is errored."
        );
        assert_eq!(
            remote_control_start_human_message(&remote_control_status(
                RemoteControlConnectionStatus::Disabled
            ))
            .expect_err("disabled status should fail")
            .to_string(),
            "Remote control is disabled on owen-mbp."
        );
    }

    #[test]
    fn remote_control_human_lines_include_foreground_stop_hint_only() {
        let summary = remote_control_status(RemoteControlConnectionStatus::Connected);

        assert_eq!(
            remote_control_start_human_lines(&summary, RemoteControlHumanOutputMode::Foreground)
                .expect("foreground lines"),
            vec![
                "This machine is available for remote control as owen-mbp.".to_string(),
                "Press Ctrl-C to stop.".to_string(),
            ]
        );
        assert_eq!(
            remote_control_start_human_lines(&summary, RemoteControlHumanOutputMode::Daemon)
                .expect("daemon lines"),
            vec!["This machine is available for remote control as owen-mbp.".to_string()]
        );
    }

    #[test]
    fn daemon_app_server_human_lines_include_path_and_version() {
        assert_eq!(
            daemon_app_server_human_lines(
                &daemon_ready_output(RemoteControlConnectionStatus::Connected).daemon
            ),
            vec![
                "Daemon used app-server:".to_string(),
                "  path: /opt/codex/bin/codex".to_string(),
                "  version: 1.0.0".to_string(),
            ]
        );
    }

    #[test]
    fn remote_control_json_output_marks_foreground_or_daemon() {
        let foreground_summary = remote_control_status(RemoteControlConnectionStatus::Connected);
        assert_eq!(
            serde_json::to_value(RemoteControlStartJsonOutput::foreground(
                &foreground_summary
            ))
            .expect("foreground JSON"),
            json!({
                "mode": "foreground",
                "status": "connected",
                "serverName": "owen-mbp",
                "environmentId": "env_test",
                "timedOut": false,
            })
        );

        let daemon_output = daemon_ready_output(RemoteControlConnectionStatus::Connected);
        assert_eq!(
            serde_json::to_value(RemoteControlStartJsonOutput::daemon(&daemon_output))
                .expect("daemon JSON"),
            json!({
                "mode": "daemon",
                "status": "connected",
                "serverName": "owen-mbp",
                "environmentId": "env_test",
                "timedOut": false,
                "daemon": {
                    "status": "started",
                    "pid": 42,
                    "managedCodexPath": "/opt/codex/bin/codex",
                    "managedCodexVersion": "1.0.0",
                    "socketPath": "/tmp/app-server-control.sock",
                    "cliVersion": "1.0.0",
                    "appServerVersion": "2.0.0",
                },
            })
        );
    }

    #[test]
    fn remote_control_daemon_json_rejects_unstartable_status() {
        assert_eq!(
            print_remote_control_start_output(
                &daemon_ready_output(RemoteControlConnectionStatus::Errored),
                /*json*/ true
            )
            .expect_err("errored daemon status should fail")
            .to_string(),
            "Remote control is enabled on owen-mbp but the connection is errored."
        );
    }

    fn test_host(env_id: &str, name: &str, online: bool) -> RemoteControlHost {
        RemoteControlHost {
            env_id: env_id.to_string(),
            name: Some(name.to_string()),
            display_name: None,
            host_name: None,
            client_type: Some("codex_cli".to_string()),
            online: Some(online),
            busy: Some(false),
            last_seen_at: Some("2026-05-20T18:00:00Z".to_string()),
            installation_id: None,
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            app_server_version: Some("1.0.0".to_string()),
        }
    }

    #[test]
    fn remote_control_hosts_filter_stale_hosts() {
        let hosts = vec![
            test_host("env_online", "bluefin", /*online*/ true),
            test_host("env_offline", "devcontainer", /*online*/ false),
        ];

        let filtered = filtered_remote_control_hosts(&hosts, /*stale_only*/ true);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].env_id, "env_offline");
    }

    #[test]
    fn remote_control_hosts_table_includes_status_and_environment_id() {
        let hosts = vec![test_host("env_test", "bluefin", /*online*/ true)];
        let filtered = filtered_remote_control_hosts(&hosts, /*stale_only*/ false);

        assert_eq!(
            remote_control_hosts_table_lines(&filtered),
            vec![
                "#   Name                         Status       Client         Last seen                Environment".to_string(),
                "1   bluefin                      online       codex_cli      2026-05-20T18:00:00Z     env_test".to_string(),
            ]
        );
    }

    #[test]
    fn remote_control_hosts_json_uses_camel_case_fields() {
        let hosts = vec![test_host("env_test", "bluefin", /*online*/ false)];
        let filtered = filtered_remote_control_hosts(&hosts, /*stale_only*/ true);

        assert_eq!(
            serde_json::to_value(&filtered).expect("serialize hosts"),
            json!([
                {
                    "envId": "env_test",
                    "name": "bluefin",
                    "displayName": null,
                    "hostName": null,
                    "clientType": "codex_cli",
                    "online": false,
                    "busy": false,
                    "lastSeenAt": "2026-05-20T18:00:00Z",
                    "installationId": null,
                    "os": "linux",
                    "arch": "x86_64",
                    "appServerVersion": "1.0.0",
                }
            ])
        );
    }

    #[test]
    fn remote_control_hosts_response_accepts_data_or_items() {
        let data_response: RemoteControlHostsResponse = serde_json::from_value(json!({
            "data": [
                {
                    "envId": "env_data",
                    "name": "bluefin",
                    "online": false
                }
            ]
        }))
        .expect("decode data response");
        let items_response: RemoteControlHostsResponse = serde_json::from_value(json!({
            "items": [
                {
                    "env_id": "env_items",
                    "display_name": "devcontainer",
                    "online": true
                }
            ]
        }))
        .expect("decode items response");

        assert_eq!(
            (data_response.items, items_response.items),
            (
                vec![RemoteControlHost {
                    env_id: "env_data".to_string(),
                    name: Some("bluefin".to_string()),
                    display_name: None,
                    host_name: None,
                    client_type: None,
                    online: Some(false),
                    busy: None,
                    last_seen_at: None,
                    installation_id: None,
                    os: None,
                    arch: None,
                    app_server_version: None,
                }],
                vec![RemoteControlHost {
                    env_id: "env_items".to_string(),
                    name: None,
                    display_name: Some("devcontainer".to_string()),
                    host_name: None,
                    client_type: None,
                    online: Some(true),
                    busy: None,
                    last_seen_at: None,
                    installation_id: None,
                    os: None,
                    arch: None,
                    app_server_version: None,
                }],
            )
        );
    }

    #[test]
    fn remote_control_hosts_delete_url_escapes_environment_id() {
        assert_eq!(
            percent_encode_path_segment("env/with space"),
            "env%2Fwith%20space"
        );
    }

    #[test]
    fn remote_control_hosts_protect_online_hosts_by_default() {
        let host = test_host("env_online", "bluefin", /*online*/ true);

        assert_eq!(
            ensure_remote_control_host_deletable(&host, /*include_online*/ false)
                .expect_err("online host should be protected")
                .to_string(),
            "Refusing to delete online host env_online. Re-run with --include-online to unlock online deletion."
        );
        ensure_remote_control_host_deletable(&host, /*include_online*/ true)
            .expect("include-online unlocks delete");
    }

    #[tokio::test]
    async fn foreground_wait_aborts_app_server_on_stop_signal() {
        let app_server_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        stop_tx.send(true).expect("send stop signal");

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_foreground_app_server(app_server_task, stop_rx),
        )
        .await
        .expect("foreground wait should return after stop signal")
        .expect("stop signal should shut down cleanly");
    }

    #[tokio::test]
    async fn foreground_start_wait_stops_before_ready() {
        let mut app_server_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        stop_tx.send(true).expect("send stop signal");

        let startup = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_foreground_remote_control_start(
                &mut app_server_task,
                std::future::pending::<anyhow::Result<AppServerRemoteControlReadyStatus>>(),
                stop_rx,
            ),
        )
        .await
        .expect("foreground startup wait should return after stop signal");

        assert!(matches!(startup, ForegroundStartupResult::Stopped));
        app_server_task.abort();
        let _ = app_server_task.await;
    }

    #[tokio::test]
    async fn foreground_start_wait_reports_app_server_exit_before_ready() {
        let mut app_server_task =
            tokio::spawn(async { Err(std::io::Error::other("startup failed before socket bind")) });
        let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);

        let startup = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_foreground_remote_control_start(
                &mut app_server_task,
                std::future::pending::<anyhow::Result<AppServerRemoteControlReadyStatus>>(),
                stop_rx,
            ),
        )
        .await
        .expect("foreground startup wait should return after app-server exits");

        let ForegroundStartupResult::AppServerExited(error) = startup else {
            panic!("expected app-server exit before ready");
        };

        assert_eq!(
            error.to_string(),
            "foreground app-server exited before remote control became ready"
        );
    }
}
