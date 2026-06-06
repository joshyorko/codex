use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use codex_login::default_client::build_reqwest_client;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::backend::AddAdHocMemoryNoteRequest;
use crate::backend::AddAdHocMemoryNoteResponse;
use crate::backend::ListMemoriesRequest;
use crate::backend::ListMemoriesResponse;
use crate::backend::MemoriesBackendError;
use crate::backend::MemoryEntry;
use crate::backend::MemoryEntryType;
use crate::backend::MemorySearchMatch;
use crate::backend::ReadMemoryRequest;
use crate::backend::ReadMemoryResponse;
use crate::backend::SearchMemoriesRequest;
use crate::backend::SearchMemoriesResponse;
use crate::policy::portable_metadata;
use crate::policy::sanitize_visible_memory_content;
use crate::portable_schema::LocalCodexMemorySyncMode;
use crate::portable_schema::LocalCodexMemorySyncRequest;
use crate::portable_schema::LocalCodexMemorySyncResponse;
use crate::portable_schema::PortableMemoryActor;
use crate::portable_schema::PortableMemoryConclusion;
use crate::portable_schema::PortableMemoryContext;
use crate::portable_schema::PortableMemorySettings;
use crate::portable_schema::VisibleMemoryMessage;
use crate::provider::MemoryProvider;
use crate::provider::PortableMemoryError;
use crate::provider::ProviderFuture;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HonchoContextRequest {
    pub(crate) query: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HonchoMemoryContext {
    pub(crate) representation: Option<String>,
    pub(crate) peer_card: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HonchoMemoryMessage {
    pub(crate) peer_id: String,
    pub(crate) content: String,
    pub(crate) metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HonchoSearchMatch {
    pub(crate) content: String,
    pub(crate) peer_id: String,
    pub(crate) session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HonchoSearchRequest {
    pub(crate) query: String,
    pub(crate) limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HonchoConclusionRequest {
    pub(crate) content: String,
    pub(crate) metadata: Value,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum HonchoMemoryError {
    #[error("Honcho is not configured")]
    NotConfigured,
    #[error("Honcho request failed: {0}")]
    Request(String),
}

/// Narrow transport boundary for Honcho memory calls.
///
/// Implementations must keep requests bounded and return errors instead of
/// panicking so Codex can fail open when portable memory is unavailable.
pub(crate) trait HonchoMemoryClient: Send + Sync {
    fn context(
        &self,
        request: HonchoContextRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HonchoMemoryContext, HonchoMemoryError>> + Send + '_>>;

    fn search(
        &self,
        request: HonchoSearchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HonchoSearchMatch>, HonchoMemoryError>> + Send + '_>>;

    fn write_messages(
        &self,
        messages: Vec<HonchoMemoryMessage>,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>>;

    fn create_conclusion(
        &self,
        request: HonchoConclusionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>>;
}

#[derive(Clone)]
pub(crate) struct HonchoClientConfig {
    pub(crate) workspace: String,
    pub(crate) session_id: String,
    pub(crate) user_peer: String,
    pub(crate) assistant_peer: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) timeout: Duration,
}

impl HonchoClientConfig {
    pub(crate) fn new(
        workspace: impl Into<String>,
        session_id: impl Into<String>,
        user_peer: impl Into<String>,
        assistant_peer: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, HonchoMemoryError> {
        let workspace = sanitize_honcho_id(&workspace.into());
        let session_id = sanitize_honcho_id(&session_id.into());
        let user_peer = sanitize_honcho_id(&user_peer.into());
        let assistant_peer = sanitize_honcho_id(&assistant_peer.into());
        let base_url = normalize_base_url(&base_url.into());
        let api_key = api_key.into();
        if workspace.is_empty()
            || session_id.is_empty()
            || user_peer.is_empty()
            || assistant_peer.is_empty()
            || api_key.is_empty()
        {
            return Err(HonchoMemoryError::NotConfigured);
        }

        Ok(Self {
            workspace,
            session_id,
            user_peer,
            assistant_peer,
            base_url,
            api_key,
            timeout: Duration::from_secs(2),
        })
    }
}

pub(crate) struct DirectHonchoMemoryClient {
    config: HonchoClientConfig,
    http: reqwest::Client,
}

impl DirectHonchoMemoryClient {
    pub(crate) fn new(config: HonchoClientConfig) -> Self {
        Self {
            config,
            http: build_reqwest_client(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    async fn ensure_session(&self) -> Result<(), HonchoMemoryError> {
        let metadata = json!({
            "source": "codex-native-memory",
            "session_id": self.config.session_id,
        });
        self.post_json(
            "/workspaces",
            json!({
                "id": self.config.workspace,
                "metadata": metadata,
                "configuration": {}
            }),
        )
        .await?;

        for peer_id in [&self.config.user_peer, &self.config.assistant_peer] {
            self.post_json(
                &format!("/workspaces/{}/peers", self.config.workspace),
                json!({
                    "id": peer_id,
                    "metadata": metadata,
                    "configuration": {}
                }),
            )
            .await?;
        }

        self.post_json(
            &format!("/workspaces/{}/sessions", self.config.workspace),
            json!({
                "id": self.config.session_id,
                "metadata": metadata,
                "configuration": {}
            }),
        )
        .await?;
        self.post_json(
            &format!(
                "/workspaces/{}/sessions/{}/peers",
                self.config.workspace, self.config.session_id
            ),
            json!([self.config.user_peer, self.config.assistant_peer]),
        )
        .await?;

        Ok(())
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value, HonchoMemoryError> {
        let response = self
            .http
            .post(self.endpoint(path))
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .timeout(self.config.timeout)
            .send()
            .await
            .map_err(|err| HonchoMemoryError::Request(err.to_string()))?;
        parse_response(response).await
    }
}

impl HonchoMemoryClient for DirectHonchoMemoryClient {
    fn context(
        &self,
        request: HonchoContextRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HonchoMemoryContext, HonchoMemoryError>> + Send + '_>>
    {
        Box::pin(async move {
            let response = self
                .http
                .get(self.endpoint(&format!(
                    "/workspaces/{}/peers/{}/context",
                    self.config.workspace, self.config.assistant_peer
                )))
                .bearer_auth(&self.config.api_key)
                .query(&[
                    ("target", self.config.user_peer.as_str()),
                    ("search_query", request.query.as_str()),
                ])
                .timeout(self.config.timeout)
                .send()
                .await
                .map_err(|err| HonchoMemoryError::Request(err.to_string()))?;
            let body = parse_response(response).await?;
            let decoded: PeerContextResponse = serde_json::from_value(body)
                .map_err(|err| HonchoMemoryError::Request(err.to_string()))?;
            Ok(HonchoMemoryContext {
                representation: decoded.representation,
                peer_card: decoded.peer_card.unwrap_or_default(),
            })
        })
    }

    fn search(
        &self,
        request: HonchoSearchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HonchoSearchMatch>, HonchoMemoryError>> + Send + '_>>
    {
        Box::pin(async move {
            let body = self
                .post_json(
                    &format!(
                        "/workspaces/{}/peers/{}/search",
                        self.config.workspace, self.config.user_peer
                    ),
                    json!({
                        "query": request.query,
                        "filters": {},
                        "limit": request.limit.clamp(1, 100),
                    }),
                )
                .await?;
            let decoded: Vec<SearchResponseItem> = serde_json::from_value(body)
                .map_err(|err| HonchoMemoryError::Request(err.to_string()))?;
            Ok(decoded
                .into_iter()
                .map(|item| HonchoSearchMatch {
                    content: item.content,
                    peer_id: item.peer_id,
                    session_id: item.session_id,
                })
                .collect())
        })
    }

    fn write_messages(
        &self,
        messages: Vec<HonchoMemoryMessage>,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>> {
        Box::pin(async move {
            if messages.is_empty() {
                return Ok(());
            }
            self.ensure_session().await?;
            let messages = messages
                .into_iter()
                .map(|message| {
                    json!({
                        "content": message.content,
                        "peer_id": message.peer_id,
                        "metadata": message.metadata,
                    })
                })
                .collect::<Vec<_>>();
            self.post_json(
                &format!(
                    "/workspaces/{}/sessions/{}/messages",
                    self.config.workspace, self.config.session_id
                ),
                json!({ "messages": messages }),
            )
            .await?;
            Ok(())
        })
    }

    fn create_conclusion(
        &self,
        request: HonchoConclusionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>> {
        Box::pin(async move {
            self.ensure_session().await?;
            self.post_json(
                &format!("/workspaces/{}/conclusions", self.config.workspace),
                json!({
                    "conclusions": [{
                        "content": request.content,
                        "observer_id": self.config.assistant_peer,
                        "observed_id": self.config.user_peer,
                        "session_id": self.config.session_id,
                        "metadata": request.metadata,
                    }]
                }),
            )
            .await?;
            Ok(())
        })
    }
}

#[derive(Deserialize)]
struct PeerContextResponse {
    representation: Option<String>,
    peer_card: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SearchResponseItem {
    content: String,
    peer_id: String,
    session_id: String,
}

async fn parse_response(response: reqwest::Response) -> Result<Value, HonchoMemoryError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| HonchoMemoryError::Request(err.to_string()))?;
    if !status.is_success() {
        return Err(HonchoMemoryError::Request(format!(
            "status {status}: {body}"
        )));
    }
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|err| HonchoMemoryError::Request(err.to_string()))
}

pub(crate) fn sanitize_honcho_id(raw: &str) -> String {
    let mut sanitized = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            sanitized.push(ch);
        } else if !sanitized.ends_with('-') {
            sanitized.push('-');
        }
    }
    sanitized.trim_matches('-').to_string()
}

pub(crate) fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "https://api.honcho.dev/v3".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn default_session_id(assistant_peer: &str, workspace: &str) -> String {
    sanitize_honcho_id(&format!("{assistant_peer}-{workspace}"))
}

pub(crate) fn shared_client(
    client: impl HonchoMemoryClient + 'static,
) -> Arc<dyn HonchoMemoryClient> {
    Arc::new(client)
}

#[derive(Clone)]
pub(crate) struct HonchoMemoryProvider {
    settings: PortableMemorySettings,
    client: Arc<dyn HonchoMemoryClient>,
}

impl HonchoMemoryProvider {
    fn new(settings: PortableMemorySettings, client: Arc<dyn HonchoMemoryClient>) -> Self {
        Self { settings, client }
    }
}

impl MemoryProvider for HonchoMemoryProvider {
    fn recall(&self, query: String) -> ProviderFuture<'_, PortableMemoryContext> {
        Box::pin(async move {
            let context = self
                .client
                .context(HonchoContextRequest { query })
                .await
                .map_err(honcho_error_to_provider_error)?;
            Ok(PortableMemoryContext {
                summary: context.representation,
                facts: context.peer_card,
            })
        })
    }

    fn search(&self, request: SearchMemoriesRequest) -> ProviderFuture<'_, SearchMemoriesResponse> {
        Box::pin(async move {
            if request.queries.is_empty()
                || request.queries.iter().any(|query| query.trim().is_empty())
            {
                return Err(PortableMemoryError::Backend(
                    MemoriesBackendError::EmptyQuery,
                ));
            }
            let queries = request.queries.clone();
            let match_mode = request.match_mode.clone();
            let path = request.path.clone();
            let mut matches = Vec::new();
            for query in &request.queries {
                for (idx, item) in self
                    .client
                    .search(HonchoSearchRequest {
                        query: query.clone(),
                        limit: request.max_results,
                    })
                    .await
                    .map_err(honcho_error_to_provider_error)?
                    .into_iter()
                    .enumerate()
                {
                    matches.push(MemorySearchMatch {
                        path: format!("portable/{}.md", item.session_id),
                        match_line_number: idx + 1,
                        content_start_line_number: idx + 1,
                        content: item.content,
                        matched_queries: vec![query.clone()],
                    });
                    if matches.len() >= request.max_results {
                        break;
                    }
                }
            }
            Ok(SearchMemoriesResponse {
                queries,
                match_mode,
                path,
                matches,
                next_cursor: None,
                truncated: false,
            })
        })
    }

    fn list(&self, request: ListMemoriesRequest) -> ProviderFuture<'_, ListMemoriesResponse> {
        Box::pin(async move {
            if request
                .path
                .as_deref()
                .is_some_and(|path| path != "portable")
            {
                return Err(PortableMemoryError::Backend(
                    MemoriesBackendError::NotFound {
                        path: request.path.unwrap_or_default(),
                    },
                ));
            }
            Ok(ListMemoriesResponse {
                path: request.path,
                entries: vec![MemoryEntry {
                    path: "portable/context.md".to_string(),
                    entry_type: MemoryEntryType::File,
                }],
                next_cursor: None,
                truncated: false,
            })
        })
    }

    fn read(&self, request: ReadMemoryRequest) -> ProviderFuture<'_, ReadMemoryResponse> {
        Box::pin(async move {
            if request.path != "portable/context.md" {
                return Err(PortableMemoryError::Backend(
                    MemoriesBackendError::NotFound { path: request.path },
                ));
            }
            let context = self
                .client
                .context(HonchoContextRequest {
                    query: String::new(),
                })
                .await
                .map_err(honcho_error_to_provider_error)?;
            Ok(ReadMemoryResponse {
                path: "portable/context.md".to_string(),
                start_line_number: 1,
                content: render_context_markdown(&PortableMemoryContext {
                    summary: context.representation,
                    facts: context.peer_card,
                }),
                truncated: false,
            })
        })
    }

    fn add_note(
        &self,
        request: AddAdHocMemoryNoteRequest,
    ) -> ProviderFuture<'_, AddAdHocMemoryNoteResponse> {
        Box::pin(async move {
            let Some(content) = sanitize_visible_memory_content(&request.note) else {
                return Err(PortableMemoryError::RejectedContent(
                    "content rejected by portable memory safety policy".to_string(),
                ));
            };
            self.conclude(PortableMemoryConclusion {
                content,
                metadata: portable_metadata(
                    &self.settings,
                    "codex-memory-tool",
                    "ad-hoc-note",
                    "public",
                ),
            })
            .await?;
            Ok(AddAdHocMemoryNoteResponse {})
        })
    }

    fn write_visible_turn(&self, messages: Vec<VisibleMemoryMessage>) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            let messages = messages
                .into_iter()
                .map(|message| HonchoMemoryMessage {
                    peer_id: match message.actor {
                        PortableMemoryActor::User => self.settings.user_peer.clone(),
                        PortableMemoryActor::Assistant => self.settings.assistant_peer.clone(),
                    },
                    content: message.content,
                    metadata: message.metadata,
                })
                .collect::<Vec<_>>();
            self.client
                .write_messages(messages)
                .await
                .map_err(honcho_error_to_provider_error)
        })
    }

    fn conclude(&self, conclusion: PortableMemoryConclusion) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            self.client
                .create_conclusion(HonchoConclusionRequest {
                    content: conclusion.content,
                    metadata: conclusion.metadata,
                })
                .await
                .map_err(honcho_error_to_provider_error)
        })
    }

    fn sync_local_files(
        &self,
        request: LocalCodexMemorySyncRequest,
    ) -> ProviderFuture<'_, LocalCodexMemorySyncResponse> {
        Box::pin(async move {
            if matches!(request.mode, LocalCodexMemorySyncMode::Preview) {
                return Ok(LocalCodexMemorySyncResponse { synced_files: 0 });
            }
            let file_count = request.files.len();
            for file in request.files {
                let mut metadata = file.metadata;
                if let Value::Object(map) = &mut metadata {
                    map.insert("sync_endpoint".to_string(), json!(request.endpoint));
                    map.insert("sync_profile".to_string(), json!(request.profile.clone()));
                    map.insert(
                        "sync_workspace".to_string(),
                        json!(request.workspace.clone()),
                    );
                    map.insert("local_path".to_string(), json!(file.path));
                    map.insert("idempotency_key".to_string(), json!(file.idempotency_key));
                }
                self.conclude(PortableMemoryConclusion {
                    content: file.content,
                    metadata,
                })
                .await?;
            }
            Ok(LocalCodexMemorySyncResponse {
                synced_files: file_count,
            })
        })
    }
}

pub(crate) fn provider_from_settings(
    settings: &PortableMemorySettings,
) -> Option<Arc<dyn MemoryProvider>> {
    let base_url = normalize_base_url(settings.honcho_base_url.as_deref().unwrap_or_default());
    let api_key = settings
        .honcho_api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| is_loopback_url(&base_url).then(|| "local".to_string()))?;
    let config = HonchoClientConfig::new(
        &settings.workspace,
        default_session_id(&settings.assistant_peer, &settings.workspace),
        &settings.user_peer,
        &settings.assistant_peer,
        base_url,
        api_key,
    )
    .ok()?;
    Some(Arc::new(HonchoMemoryProvider::new(
        settings.clone(),
        shared_client(DirectHonchoMemoryClient::new(config)),
    )))
}

#[cfg(test)]
pub(crate) fn provider_for_tests(
    settings: PortableMemorySettings,
    client: impl HonchoMemoryClient + 'static,
) -> Arc<dyn MemoryProvider> {
    Arc::new(HonchoMemoryProvider::new(settings, shared_client(client)))
}

fn honcho_error_to_provider_error(err: HonchoMemoryError) -> PortableMemoryError {
    match err {
        HonchoMemoryError::NotConfigured => PortableMemoryError::NotConfigured,
        HonchoMemoryError::Request(message) => PortableMemoryError::Request(message),
    }
}

fn render_context_markdown(context: &PortableMemoryContext) -> String {
    let mut rendered = String::new();
    if let Some(summary) = context.summary.as_deref() {
        rendered.push_str("# Summary\n\n");
        rendered.push_str(summary.trim());
        rendered.push_str("\n\n");
    }
    if !context.facts.is_empty() {
        rendered.push_str("# Facts\n\n");
        for fact in &context.facts {
            rendered.push_str("- ");
            rendered.push_str(fact.trim());
            rendered.push('\n');
        }
    }
    if rendered.is_empty() {
        rendered.push_str("No portable provider context available.\n");
    }
    rendered
}

pub(crate) fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct InMemoryHonchoMemoryClient {
    inner: Arc<std::sync::Mutex<InMemoryHonchoMemoryState>>,
}

#[cfg(test)]
#[derive(Default)]
struct InMemoryHonchoMemoryState {
    context: HonchoMemoryContext,
    context_queries: Vec<String>,
    messages: Vec<HonchoMemoryMessage>,
    conclusions: Vec<HonchoConclusionRequest>,
}

#[cfg(test)]
impl InMemoryHonchoMemoryClient {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_context(&self, context: HonchoMemoryContext) {
        self.inner.lock().expect("in-memory client lock").context = context;
    }

    pub(crate) fn context_queries(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("in-memory client lock")
            .context_queries
            .clone()
    }

    pub(crate) fn messages(&self) -> Vec<HonchoMemoryMessage> {
        self.inner
            .lock()
            .expect("in-memory client lock")
            .messages
            .clone()
    }

    pub(crate) fn conclusions(&self) -> Vec<HonchoConclusionRequest> {
        self.inner
            .lock()
            .expect("in-memory client lock")
            .conclusions
            .clone()
    }
}

#[cfg(test)]
impl HonchoMemoryClient for InMemoryHonchoMemoryClient {
    fn context(
        &self,
        request: HonchoContextRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HonchoMemoryContext, HonchoMemoryError>> + Send + '_>>
    {
        Box::pin(async move {
            let mut state = self.inner.lock().expect("in-memory client lock");
            state.context_queries.push(request.query);
            Ok(state.context.clone())
        })
    }

    fn search(
        &self,
        _request: HonchoSearchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HonchoSearchMatch>, HonchoMemoryError>> + Send + '_>>
    {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn write_messages(
        &self,
        messages: Vec<HonchoMemoryMessage>,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>> {
        Box::pin(async move {
            self.inner
                .lock()
                .expect("in-memory client lock")
                .messages
                .extend(messages);
            Ok(())
        })
    }

    fn create_conclusion(
        &self,
        request: HonchoConclusionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), HonchoMemoryError>> + Send + '_>> {
        Box::pin(async move {
            self.inner
                .lock()
                .expect("in-memory client lock")
                .conclusions
                .push(request);
            Ok(())
        })
    }
}
