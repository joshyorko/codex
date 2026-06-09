use codex_config::types::MemoriesConfig;
use codex_config::types::MemoryBackendKind;
use serde::Serialize;

use crate::DEFAULT_READ_MAX_TOKENS;
use crate::backend::ReadMemoryRequest;
use crate::import_local::settings_from_config;
use crate::selected::portable_provider_for_settings;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MemoryStatusReport {
    pub backend: String,
    pub provider: String,
    pub profile: String,
    pub workspace: String,
    pub provider_url: Option<String>,
    pub honcho_api_key_env: Option<String>,
    pub write_policy: String,
    pub local_import_policy: String,
    pub provider_configured: bool,
    pub health: MemoryHealthReport,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MemoryHealthReport {
    pub status: String,
    pub detail: Option<String>,
}

pub async fn memory_status_report(memories: &MemoriesConfig) -> MemoryStatusReport {
    let settings = settings_from_config(memories);
    let provider = portable_provider_for_settings(&settings);
    let provider_configured = provider.is_some();
    let health = match settings.backend {
        MemoryBackendKind::Local => MemoryHealthReport {
            status: "local".to_string(),
            detail: Some("portable memory provider is disabled".to_string()),
        },
        MemoryBackendKind::Provider | MemoryBackendKind::Hybrid => match provider {
            Some(provider) => match provider
                .read(ReadMemoryRequest {
                    path: provider_status_path(&settings.provider).to_string(),
                    line_offset: 1,
                    max_lines: None,
                    max_tokens: DEFAULT_READ_MAX_TOKENS,
                })
                .await
            {
                Ok(_) => MemoryHealthReport {
                    status: "ok".to_string(),
                    detail: None,
                },
                Err(err) => MemoryHealthReport {
                    status: "unreachable".to_string(),
                    detail: Some(err.to_string()),
                },
            },
            None => MemoryHealthReport {
                status: "unconfigured".to_string(),
                detail: Some("portable memory provider is not configured".to_string()),
            },
        },
    };

    MemoryStatusReport {
        backend: memory_backend_kind_as_str(memories.backend).to_string(),
        provider: memories.provider.as_str().to_string(),
        profile: memories.profile.as_str().to_string(),
        workspace: memories.workspace.clone(),
        provider_url: memories.provider_url.clone(),
        honcho_api_key_env: memories.honcho_api_key_env.clone(),
        write_policy: memory_write_policy_as_str(memories.write_policy).to_string(),
        local_import_policy: local_import_policy_as_str(memories.local_import_policy).to_string(),
        provider_configured,
        health,
    }
}

fn provider_status_path(provider: &codex_config::types::MemoryProviderKind) -> &'static str {
    match provider {
        codex_config::types::MemoryProviderKind::Honcho => "portable/context.md",
        codex_config::types::MemoryProviderKind::CodexMemoryd => "portable/status.md",
    }
}

fn memory_backend_kind_as_str(value: MemoryBackendKind) -> &'static str {
    match value {
        MemoryBackendKind::Local => "local",
        MemoryBackendKind::Provider => "provider",
        MemoryBackendKind::Hybrid => "hybrid",
    }
}

fn memory_write_policy_as_str(value: codex_config::types::MemoryWritePolicy) -> &'static str {
    match value {
        codex_config::types::MemoryWritePolicy::Off => "off",
        codex_config::types::MemoryWritePolicy::VisibleTurns => "visible_turns",
    }
}

fn local_import_policy_as_str(value: codex_config::types::LocalImportPolicy) -> &'static str {
    match value {
        codex_config::types::LocalImportPolicy::Prompt => "prompt",
        codex_config::types::LocalImportPolicy::Manual => "manual",
        codex_config::types::LocalImportPolicy::StartupPreview => "startup_preview",
        codex_config::types::LocalImportPolicy::StartupApply => "startup_apply",
    }
}
