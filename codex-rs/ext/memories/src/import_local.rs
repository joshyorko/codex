use std::path::Component;
use std::path::Path;
use std::sync::Arc;

use codex_config::types::LocalImportPolicy;
use codex_config::types::MemoriesConfig;
use codex_config::types::MemoryBackendKind;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;

use crate::policy::portable_metadata;
use crate::policy::sanitize_local_import_memory_content;
use crate::portable_schema::LOCAL_CODEX_MEMORY_SYNC_ENDPOINT;
use crate::portable_schema::LocalCodexMemorySyncMode;
use crate::portable_schema::LocalCodexMemorySyncRequest;
use crate::portable_schema::PortableMemoryFile;
use crate::portable_schema::PortableMemorySettings;
use crate::provider::MemoryProvider;
use crate::selected::portable_provider_for_settings;

const MAX_IMPORT_FILES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportLocalCodexMemoryMode {
    Preview,
    Apply,
}

impl ImportLocalCodexMemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Apply => "apply",
        }
    }
}

impl From<ImportLocalCodexMemoryMode> for LocalCodexMemorySyncMode {
    fn from(mode: ImportLocalCodexMemoryMode) -> Self {
        match mode {
            ImportLocalCodexMemoryMode::Preview => Self::Preview,
            ImportLocalCodexMemoryMode::Apply => Self::Apply,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportLocalCodexMemoryReport {
    pub endpoint: &'static str,
    pub mode: &'static str,
    pub backend: String,
    pub profile: String,
    pub workspace: String,
    pub provider_configured: bool,
    pub accepted_files: usize,
    pub rejected_files: usize,
    pub synced_files: usize,
    pub warning: Option<String>,
    pub files: Vec<ImportLocalCodexMemoryFileReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportLocalCodexMemoryFileReport {
    pub path: String,
    pub status: ImportLocalCodexMemoryFileStatus,
    pub bytes: u64,
    pub reason: Option<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportLocalCodexMemoryFileStatus {
    Accepted,
    Rejected,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportLocalCodexMemoryError {
    #[error("failed to inspect local Codex memory: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn import_local_codex_memory(
    codex_home: &AbsolutePathBuf,
    memories: &MemoriesConfig,
    mode: ImportLocalCodexMemoryMode,
) -> Result<ImportLocalCodexMemoryReport, ImportLocalCodexMemoryError> {
    let settings = settings_from_config(memories);
    let provider = if matches!(settings.backend, MemoryBackendKind::Local) {
        None
    } else {
        portable_provider_for_settings(&settings)
    };
    sync_local_codex_memory_with_provider(codex_home, &settings, mode, provider).await
}

pub(crate) async fn sync_local_codex_memory_with_provider(
    codex_home: &AbsolutePathBuf,
    settings: &PortableMemorySettings,
    mode: ImportLocalCodexMemoryMode,
    provider: Option<Arc<dyn MemoryProvider>>,
) -> Result<ImportLocalCodexMemoryReport, ImportLocalCodexMemoryError> {
    let provider_configured = provider.is_some();
    let collected = collect_local_codex_memory(codex_home, settings).await?;
    let accepted_files = collected.files.len();
    let rejected_files = collected
        .report_files
        .iter()
        .filter(|file| matches!(file.status, ImportLocalCodexMemoryFileStatus::Rejected))
        .count();
    let mut report = ImportLocalCodexMemoryReport {
        endpoint: LOCAL_CODEX_MEMORY_SYNC_ENDPOINT,
        mode: mode.as_str(),
        backend: format!("{:?}", settings.backend),
        profile: settings.profile.as_str().to_string(),
        workspace: settings.workspace.clone(),
        provider_configured,
        accepted_files,
        rejected_files,
        synced_files: 0,
        warning: None,
        files: collected.report_files,
    };

    if matches!(mode, ImportLocalCodexMemoryMode::Preview) && provider.is_none() {
        return Ok(report);
    }

    let Some(provider) = provider else {
        report.warning = Some("portable memory provider is not configured".to_string());
        return Ok(report);
    };

    let response = provider
        .sync_local_files(LocalCodexMemorySyncRequest {
            mode: mode.into(),
            endpoint: LOCAL_CODEX_MEMORY_SYNC_ENDPOINT,
            profile: settings.profile.as_str().to_string(),
            workspace: settings.workspace.clone(),
            source_root: codex_home
                .join("memories")
                .to_path_buf()
                .display()
                .to_string(),
            files: collected.files,
        })
        .await;
    match response {
        Ok(response) => report.synced_files = response.synced_files,
        Err(err) => report.warning = Some(err.to_string()),
    }
    Ok(report)
}

struct CollectedLocalCodexMemory {
    files: Vec<PortableMemoryFile>,
    report_files: Vec<ImportLocalCodexMemoryFileReport>,
}

async fn collect_local_codex_memory(
    codex_home: &AbsolutePathBuf,
    settings: &PortableMemorySettings,
) -> Result<CollectedLocalCodexMemory, ImportLocalCodexMemoryError> {
    let root = codex_home.join("memories").to_path_buf();
    let Some(metadata) = metadata_or_none(&root).await? else {
        return Ok(CollectedLocalCodexMemory {
            files: Vec::new(),
            report_files: Vec::new(),
        });
    };
    if !metadata.is_dir() {
        return Ok(CollectedLocalCodexMemory {
            files: Vec::new(),
            report_files: vec![ImportLocalCodexMemoryFileReport {
                path: "memories".to_string(),
                status: ImportLocalCodexMemoryFileStatus::Rejected,
                bytes: metadata.len(),
                reason: Some("memories root is not a directory".to_string()),
                idempotency_key: None,
            }],
        });
    }

    let mut dirs = vec![root.clone()];
    let mut files = Vec::new();
    let mut report_files = Vec::new();
    while let Some(dir) = dirs.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let relative_path = display_relative_path(&root, &path);
            if has_hidden_component(&path, &root) {
                continue;
            }
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                dirs.push(path);
                continue;
            }
            if !metadata.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            if files.len() >= MAX_IMPORT_FILES {
                report_files.push(rejected_file(
                    relative_path,
                    metadata.len(),
                    "local import file limit reached",
                ));
                continue;
            }
            let raw_content = tokio::fs::read_to_string(&path).await?;
            let Some(content) = sanitize_local_import_memory_content(&raw_content) else {
                report_files.push(rejected_file(
                    relative_path,
                    metadata.len(),
                    "rejected by portable memory safety policy",
                ));
                continue;
            };
            let idempotency_key = import_idempotency_key(settings, &relative_path, &content);
            files.push(PortableMemoryFile {
                path: relative_path.clone(),
                content,
                metadata: local_memory_file_metadata(settings, &relative_path, &idempotency_key),
                idempotency_key: idempotency_key.clone(),
            });
            report_files.push(ImportLocalCodexMemoryFileReport {
                path: relative_path,
                status: ImportLocalCodexMemoryFileStatus::Accepted,
                bytes: metadata.len(),
                reason: None,
                idempotency_key: Some(idempotency_key),
            });
        }
    }

    report_files.sort_by(|left, right| left.path.cmp(&right.path));
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(CollectedLocalCodexMemory {
        files,
        report_files,
    })
}

async fn metadata_or_none(path: &Path) -> Result<Option<std::fs::Metadata>, std::io::Error> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn rejected_file(
    path: String,
    bytes: u64,
    reason: impl Into<String>,
) -> ImportLocalCodexMemoryFileReport {
    ImportLocalCodexMemoryFileReport {
        path,
        status: ImportLocalCodexMemoryFileStatus::Rejected,
        bytes,
        reason: Some(reason.into()),
        idempotency_key: None,
    }
}

fn display_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn has_hidden_component(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| match component {
            Component::Normal(part) => part.to_string_lossy().starts_with('.'),
            _ => false,
        })
}

fn import_idempotency_key(
    settings: &PortableMemorySettings,
    relative_path: &str,
    content: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(settings.profile.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(settings.workspace.as_bytes());
    hasher.update(b"\0");
    hasher.update(relative_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("codex-local-memory:{hex}")
}

fn local_memory_file_metadata(
    settings: &PortableMemorySettings,
    relative_path: &str,
    idempotency_key: &str,
) -> Value {
    let mut metadata = portable_metadata(
        settings,
        "codex-local-memory",
        &format!("local-codex-memory:{relative_path}"),
        "public",
    );
    if let Value::Object(map) = &mut metadata {
        map.insert(
            "sync_endpoint".to_string(),
            json!(LOCAL_CODEX_MEMORY_SYNC_ENDPOINT),
        );
        map.insert("local_path".to_string(), json!(relative_path));
        map.insert("idempotency_key".to_string(), json!(idempotency_key));
        map.insert("content_kind".to_string(), json!("local-codex-memory-file"));
    }
    metadata
}

pub(crate) fn settings_from_config(memories: &MemoriesConfig) -> PortableMemorySettings {
    PortableMemorySettings {
        backend: memories.backend,
        profile: memories.profile,
        workspace: memories.workspace.clone(),
        user_peer: memories.user_peer.clone(),
        assistant_peer: memories.assistant_peer.clone(),
        provider: memories.provider,
        provider_url: memories.provider_url.clone(),
        honcho_base_url: memories.honcho_base_url.clone(),
        honcho_api_key_env: memories.honcho_api_key_env.clone(),
        write_policy: memories.write_policy,
        sync_policy: memories.sync_policy,
        local_import_policy: memories.local_import_policy,
        cross_profile_policy: memories.cross_profile_policy,
    }
}

pub(crate) fn startup_import_mode(policy: LocalImportPolicy) -> Option<ImportLocalCodexMemoryMode> {
    match policy {
        LocalImportPolicy::StartupPreview => Some(ImportLocalCodexMemoryMode::Preview),
        LocalImportPolicy::StartupApply => Some(ImportLocalCodexMemoryMode::Apply),
        LocalImportPolicy::Prompt | LocalImportPolicy::Manual => None,
    }
}
