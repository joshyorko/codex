use std::sync::Arc;
use std::time::Duration;

use codex_login::default_client::build_reqwest_client;
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

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const STATUS_PATH: &str = "portable/status.md";

#[derive(Clone)]
pub(crate) struct CodexMemorydProvider {
    settings: PortableMemorySettings,
    base_url: String,
    http: reqwest::Client,
}

impl CodexMemorydProvider {
    fn new(settings: PortableMemorySettings, base_url: String) -> Self {
        Self {
            settings,
            base_url,
            http: build_reqwest_client(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        if self.base_url.ends_with("/v1") && path.starts_with("/v1/") {
            format!("{}{}", self.base_url, &path[3..])
        } else {
            format!("{}{}", self.base_url, path)
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, PortableMemoryError> {
        let response = self
            .http
            .get(self.endpoint(path))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| PortableMemoryError::Request(err.to_string()))?;
        parse_memoryd_response(response).await
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value, PortableMemoryError> {
        let response = self
            .http
            .post(self.endpoint(path))
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| PortableMemoryError::Request(err.to_string()))?;
        parse_memoryd_response(response).await
    }

    fn base_request(&self) -> Value {
        json!({
            "profile": self.settings.profile.as_str(),
            "workspace": self.settings.workspace.as_str(),
            "repo": Value::Null,
            "source": "codex-native-memory",
        })
    }

    fn session(&self) -> Value {
        json!({
            "id": format!("codex-{}-{}", self.settings.profile.as_str(), self.settings.workspace),
            "profile_id": self.settings.profile.as_str(),
            "workspace_id": self.settings.workspace.as_str(),
            "repo_id": Value::Null,
            "thread_id": Value::Null,
            "source": "codex-native-memory",
            "metadata": {},
        })
    }
}

impl MemoryProvider for CodexMemorydProvider {
    fn recall(&self, query: String) -> ProviderFuture<'_, PortableMemoryContext> {
        Box::pin(async move {
            let mut request = self.base_request();
            insert_object_field(&mut request, "session", Value::Null);
            insert_object_field(&mut request, "query", json!(query));
            insert_object_field(&mut request, "max_tokens", json!(1200));
            insert_object_field(&mut request, "metadata", json!({}));
            let data = self.post_json("/v1/recall", request).await?;
            Ok(PortableMemoryContext {
                summary: data
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                facts: data
                    .get("facts")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(memory_fact_text).collect())
                    .unwrap_or_default(),
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
                let mut body = self.base_request();
                insert_object_field(&mut body, "query", json!(query));
                insert_object_field(&mut body, "limit", json!(request.max_results));
                insert_object_field(&mut body, "include_archived", json!(false));
                let data = self.post_json("/v1/search", body).await?;
                for item in data
                    .get("matches")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(content) = memory_fact_text(item) else {
                        continue;
                    };
                    matches.push(MemorySearchMatch {
                        path: memory_match_path(item),
                        match_line_number: matches.len() + 1,
                        content_start_line_number: matches.len() + 1,
                        content,
                        matched_queries: vec![query.clone()],
                    });
                    if matches.len() >= request.max_results {
                        break;
                    }
                }
                if matches.len() >= request.max_results {
                    break;
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
                    path: STATUS_PATH.to_string(),
                    entry_type: MemoryEntryType::File,
                }],
                next_cursor: None,
                truncated: false,
            })
        })
    }

    fn read(&self, request: ReadMemoryRequest) -> ProviderFuture<'_, ReadMemoryResponse> {
        Box::pin(async move {
            if request.path != STATUS_PATH {
                return Err(PortableMemoryError::Backend(
                    MemoriesBackendError::NotFound { path: request.path },
                ));
            }
            let data = self.get_json("/v1/status").await?;
            let content = serde_json::to_string_pretty(&data)
                .unwrap_or_else(|_| "codex-memoryd status unavailable".to_string());
            Ok(ReadMemoryResponse {
                path: STATUS_PATH.to_string(),
                start_line_number: 1,
                content,
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
            let mut metadata =
                portable_metadata(&self.settings, "codex-memory-tool", "ad-hoc-note", "public");
            insert_object_field(&mut metadata, "filename", json!(request.filename));
            self.conclude(PortableMemoryConclusion { content, metadata })
                .await?;
            Ok(AddAdHocMemoryNoteResponse {})
        })
    }

    fn write_visible_turn(&self, messages: Vec<VisibleMemoryMessage>) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            let messages = messages
                .into_iter()
                .map(|message| {
                    json!({
                        "actor": match message.actor {
                            PortableMemoryActor::User => "user",
                            PortableMemoryActor::Assistant => "assistant",
                        },
                        "content": message.content,
                        "created_at": Value::Null,
                        "metadata": message.metadata,
                    })
                })
                .collect::<Vec<_>>();
            let mut request = self.base_request();
            insert_object_field(&mut request, "session", self.session());
            insert_object_field(&mut request, "messages", json!(messages));
            insert_object_field(&mut request, "write_policy", json!("visible_turns"));
            self.post_json("/v1/turns", request).await?;
            Ok(())
        })
    }

    fn conclude(&self, conclusion: PortableMemoryConclusion) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            let mut request = self.base_request();
            insert_object_field(&mut request, "target", json!("user"));
            insert_object_field(&mut request, "conclusions", json!([conclusion.content]));
            insert_object_field(&mut request, "metadata", conclusion.metadata);
            self.post_json("/v1/conclusions", request).await?;
            Ok(())
        })
    }

    fn sync_local_files(
        &self,
        request: LocalCodexMemorySyncRequest,
    ) -> ProviderFuture<'_, LocalCodexMemorySyncResponse> {
        Box::pin(async move {
            let files = request
                .files
                .into_iter()
                .map(|file| {
                    let kind = local_memory_kind(&file.path);
                    json!({
                        "path": file.path,
                        "kind": kind,
                        "content": file.content,
                        "hash": file.idempotency_key,
                        "modified_at": Value::Null,
                        "metadata": file.metadata,
                    })
                })
                .collect::<Vec<_>>();
            let mut body = self.base_request();
            insert_object_field(&mut body, "source_root", json!(request.source_root));
            insert_object_field(&mut body, "files", json!(files));
            insert_object_field(
                &mut body,
                "mode",
                json!(match request.mode {
                    LocalCodexMemorySyncMode::Preview => "preview",
                    LocalCodexMemorySyncMode::Apply => "apply",
                }),
            );
            insert_object_field(
                &mut body,
                "metadata",
                json!({
                    "endpoint": request.endpoint,
                }),
            );
            let data = self.post_json("/v1/sync/local-codex-memory", body).await?;
            let synced_files = ["created", "updated", "proposed"]
                .iter()
                .filter_map(|field| data.get(*field).and_then(Value::as_u64))
                .map(|value| value as usize)
                .sum();
            Ok(LocalCodexMemorySyncResponse { synced_files })
        })
    }
}

pub(crate) fn provider_from_settings(
    settings: &PortableMemorySettings,
) -> Option<Arc<dyn MemoryProvider>> {
    let base_url = normalize_memoryd_base_url(settings.provider_url.as_deref()?);
    if base_url.is_empty() {
        return None;
    }
    Some(Arc::new(CodexMemorydProvider::new(
        settings.clone(),
        base_url,
    )))
}

async fn parse_memoryd_response(response: reqwest::Response) -> Result<Value, PortableMemoryError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| PortableMemoryError::Request(err.to_string()))?;
    if !status.is_success() {
        return Err(PortableMemoryError::Request(format!(
            "HTTP {status}: {}",
            truncate_response_body(&body)
        )));
    }
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&body)
            .map_err(|err| PortableMemoryError::Request(err.to_string()))?
    };
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(PortableMemoryError::Request(memoryd_error_message(&value)));
    }
    Ok(value.get("data").cloned().unwrap_or(value))
}

fn memoryd_error_message(value: &Value) -> String {
    value
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .or_else(|| error.get("code"))
                .and_then(Value::as_str)
        })
        .unwrap_or("codex-memoryd request failed")
        .to_string()
}

fn normalize_memoryd_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn truncate_response_body(body: &str) -> String {
    const MAX_ERROR_CHARS: usize = 500;
    if body.chars().count() <= MAX_ERROR_CHARS {
        return body.to_string();
    }
    body.chars().take(MAX_ERROR_CHARS).collect()
}

fn insert_object_field(target: &mut Value, field: &str, value: Value) {
    if let Value::Object(map) = target {
        map.insert(field.to_string(), value);
    }
}

fn memory_fact_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return non_empty(text);
    }
    ["content", "snippet", "text", "summary"]
        .iter()
        .find_map(|field| {
            value
                .get(*field)
                .and_then(Value::as_str)
                .and_then(non_empty)
        })
}

fn memory_match_path(value: &Value) -> String {
    value
        .get("path")
        .and_then(Value::as_str)
        .and_then(non_empty)
        .unwrap_or_else(|| {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .and_then(non_empty)
                .unwrap_or_else(|| "match".to_string());
            format!("portable/{id}.md")
        })
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn local_memory_kind(path: &str) -> &'static str {
    match path {
        "memory_summary.md" => "memory_summary",
        "MEMORY.md" => "memory_registry",
        path if path.starts_with("rollout_summaries/") => "rollout_summary",
        path if path.starts_with("extensions/ad_hoc/notes/") => "ad_hoc_note",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use codex_config::types::CrossProfilePolicy;
    use codex_config::types::LocalImportPolicy;
    use codex_config::types::MemoryBackendKind;
    use codex_config::types::MemoryProfile;
    use codex_config::types::MemoryProviderKind;
    use codex_config::types::MemorySyncPolicy;
    use codex_config::types::MemoryWritePolicy;

    use super::*;

    #[tokio::test]
    async fn recall_posts_to_v1_recall_and_parses_context() {
        let response_body = r#"{"ok":true,"data":{"summary":"Use repo-native commands.","facts":["Keep provider boundaries explicit."]}}"#;
        let (base_url, request_rx, server) = serve_once(response_body);
        let settings = memoryd_settings(Some(base_url.clone()));
        let provider = CodexMemorydProvider::new(settings, base_url);

        let context = provider
            .recall("inspect memory provider".to_string())
            .await
            .expect("recall should parse codex-memoryd response");

        let request = request_rx.recv().expect("server should capture request");
        server.join().expect("server thread should finish");
        assert!(request.starts_with("POST /v1/recall "));
        assert!(request.contains(r#""query":"inspect memory provider""#));
        assert_eq!(
            context.summary,
            Some("Use repo-native commands.".to_string())
        );
        assert_eq!(context.facts, vec!["Keep provider boundaries explicit."]);
    }

    #[tokio::test]
    async fn sync_local_preview_posts_to_memoryd_sync_endpoint() {
        let response_body =
            r#"{"ok":true,"data":{"proposed":1,"created":0,"updated":0,"skipped":0,"rejected":0}}"#;
        let (base_url, request_rx, server) = serve_once(response_body);
        let settings = memoryd_settings(Some(base_url.clone()));
        let provider = CodexMemorydProvider::new(settings, base_url);

        let response = provider
            .sync_local_files(LocalCodexMemorySyncRequest {
                mode: LocalCodexMemorySyncMode::Preview,
                endpoint: crate::portable_schema::LOCAL_CODEX_MEMORY_SYNC_ENDPOINT,
                profile: "personal".to_string(),
                workspace: "codex-memory-lab".to_string(),
                source_root: "/tmp/codex-home/memories".to_string(),
                files: vec![crate::portable_schema::PortableMemoryFile {
                    path: "MEMORY.md".to_string(),
                    content: "Use repo-native commands.".to_string(),
                    metadata: json!({}),
                    idempotency_key: "codex-local-memory:abc123".to_string(),
                }],
            })
            .await
            .expect("sync preview should parse codex-memoryd response");

        let request = request_rx.recv().expect("server should capture request");
        server.join().expect("server thread should finish");
        assert!(request.starts_with("POST /v1/sync/local-codex-memory "));
        assert!(request.contains(r#""mode":"preview""#));
        assert_eq!(response.synced_files, 1);
    }

    #[test]
    fn provider_requires_provider_url() {
        assert!(provider_from_settings(&memoryd_settings(None)).is_none());
    }

    fn serve_once(
        response_body: &'static str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 8192];
            let bytes = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            tx.send(request).expect("send captured request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        (format!("http://{addr}"), rx, handle)
    }

    fn memoryd_settings(provider_url: Option<String>) -> PortableMemorySettings {
        PortableMemorySettings {
            backend: MemoryBackendKind::Provider,
            provider: MemoryProviderKind::CodexMemoryd,
            profile: MemoryProfile::Personal,
            workspace: "codex-memory-lab".to_string(),
            user_peer: "user".to_string(),
            assistant_peer: "codex".to_string(),
            provider_url,
            honcho_base_url: None,
            honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
            write_policy: MemoryWritePolicy::VisibleTurns,
            sync_policy: MemorySyncPolicy::Manual,
            local_import_policy: LocalImportPolicy::Manual,
            cross_profile_policy: CrossProfilePolicy::DefaultDeny,
        }
    }
}
