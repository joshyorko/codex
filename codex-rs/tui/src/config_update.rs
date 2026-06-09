//! App-server-backed config update helpers for the TUI.
//!
//! This module centralizes the small typed update helpers the TUI uses
//! when a config mutation must be owned by the app server rather than written
//! to the local `config.toml` directly.

use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigEdit;
use codex_app_server_protocol::ConfigReadParams;
use codex_app_server_protocol::ConfigReadResponse;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::MergeStrategy;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SkillsConfigWriteParams;
use codex_app_server_protocol::SkillsConfigWriteResponse;
use codex_config::loader::project_trust_key;
use codex_config::types::LocalImportPolicy;
use codex_config::types::MemoryBackendKind;
use codex_config::types::MemoryProfile;
use codex_config::types::MemoryProviderKind;
use codex_config::types::MemoryWritePolicy;
use codex_features::FEATURES;
use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
use codex_protocol::config_types::TrustLevel;
use codex_utils_absolute_path::AbsolutePathBuf;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use serde_json::Value as JsonValue;
use std::fmt::Display;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableMemorySetup {
    pub(crate) backend: MemoryBackendKind,
    pub(crate) provider: MemoryProviderKind,
    pub(crate) profile: MemoryProfile,
    pub(crate) workspace: String,
    pub(crate) user_peer: String,
    pub(crate) assistant_peer: String,
    pub(crate) provider_url: Option<String>,
    pub(crate) honcho_api_key_env: Option<String>,
}

pub(crate) fn replace_config_value(key_path: impl Into<String>, value: JsonValue) -> ConfigEdit {
    ConfigEdit {
        key_path: key_path.into(),
        value,
        merge_strategy: MergeStrategy::Replace,
    }
}

pub(crate) fn clear_config_value(key_path: impl Into<String>) -> ConfigEdit {
    replace_config_value(key_path, JsonValue::Null)
}

pub(crate) fn app_scoped_key_path(app_id: &str, key_path: &str) -> String {
    let app_id = serde_json::Value::String(app_id.to_string()).to_string();
    format!("apps.{app_id}.{key_path}")
}

pub(crate) fn format_config_error(err: &impl Display) -> String {
    format!("{err:#}")
}

fn trusted_project_edit(project_path: &Path) -> ConfigEdit {
    let project_key = project_trust_key(project_path)
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    replace_config_value(
        format!("projects.\"{project_key}\".trust_level"),
        serde_json::json!(TrustLevel::Trusted.to_string()),
    )
}

pub(crate) fn build_model_selection_edits(
    model: &str,
    effort: Option<impl ToString>,
) -> Vec<ConfigEdit> {
    let effort_edit = effort.map_or_else(
        || clear_config_value("model_reasoning_effort"),
        |effort| {
            replace_config_value(
                "model_reasoning_effort",
                serde_json::json!(effort.to_string()),
            )
        },
    );
    vec![
        replace_config_value("model", serde_json::json!(model)),
        effort_edit,
    ]
}

pub(crate) fn build_service_tier_selection_edits(service_tier: Option<&str>) -> Vec<ConfigEdit> {
    let service_tier_edit = service_tier.map_or_else(
        || clear_config_value("service_tier"),
        |service_tier| {
            let config_value = if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE {
                SERVICE_TIER_DEFAULT_REQUEST_VALUE
            } else {
                match codex_protocol::config_types::ServiceTier::from_request_value(service_tier) {
                    Some(codex_protocol::config_types::ServiceTier::Fast) => "fast",
                    Some(codex_protocol::config_types::ServiceTier::Flex) => "flex",
                    None => service_tier,
                }
            };
            replace_config_value("service_tier", serde_json::json!(config_value))
        },
    );
    vec![service_tier_edit]
}

#[cfg(target_os = "windows")]
pub(crate) fn build_windows_sandbox_mode_edits(elevated_enabled: bool) -> Vec<ConfigEdit> {
    let feature_key_path = |feature: &str| format!("features.{feature}");
    vec![
        replace_config_value(
            "windows.sandbox",
            serde_json::json!(if elevated_enabled {
                "elevated"
            } else {
                "unelevated"
            }),
        ),
        clear_config_value(feature_key_path("experimental_windows_sandbox")),
        clear_config_value(feature_key_path("elevated_windows_sandbox")),
        clear_config_value(feature_key_path("enable_experimental_windows_sandbox")),
    ]
}

pub(crate) fn build_feature_enabled_edit(feature_key: &str, enabled: bool) -> ConfigEdit {
    let key_path = format!("features.{feature_key}");
    let is_default_false_feature = FEATURES
        .iter()
        .find(|spec| spec.key == feature_key)
        .is_some_and(|spec| !spec.default_enabled);
    if enabled || !is_default_false_feature {
        replace_config_value(key_path, serde_json::json!(enabled))
    } else {
        clear_config_value(key_path)
    }
}

pub(crate) fn build_memory_settings_edits(
    use_memories: bool,
    generate_memories: bool,
) -> Vec<ConfigEdit> {
    vec![
        replace_config_value("memories.use_memories", serde_json::json!(use_memories)),
        replace_config_value(
            "memories.generate_memories",
            serde_json::json!(generate_memories),
        ),
    ]
}

pub(crate) fn build_portable_memory_setup_edits(setup: &PortableMemorySetup) -> Vec<ConfigEdit> {
    let mut edits = vec![
        replace_config_value(
            "memories.backend",
            serde_json::json!(setup.backend.as_str()),
        ),
        replace_config_value(
            "memories.provider",
            serde_json::json!(setup.provider.as_str()),
        ),
        replace_config_value(
            "memories.profile",
            serde_json::json!(setup.profile.as_str()),
        ),
        replace_config_value("memories.workspace", serde_json::json!(setup.workspace)),
        replace_config_value("memories.user_peer", serde_json::json!(setup.user_peer)),
        replace_config_value(
            "memories.assistant_peer",
            serde_json::json!(setup.assistant_peer),
        ),
        replace_config_value(
            "memories.write_policy",
            serde_json::json!(MemoryWritePolicy::VisibleTurns.as_str()),
        ),
        replace_config_value(
            "memories.local_import_policy",
            serde_json::json!(LocalImportPolicy::Manual.as_str()),
        ),
    ];

    match setup
        .provider_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(provider_url) => edits.push(replace_config_value(
            "memories.provider_url",
            serde_json::json!(provider_url.trim()),
        )),
        None => edits.push(clear_config_value("memories.provider_url")),
    }

    match setup.provider {
        MemoryProviderKind::Honcho => {
            edits.push(clear_config_value("memories.honcho_base_url"));
            edits.push(replace_config_value(
                "memories.honcho_api_key_env",
                serde_json::json!(
                    setup
                        .honcho_api_key_env
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("HONCHO_API_KEY")
                ),
            ));
        }
        MemoryProviderKind::CodexMemoryd => {
            edits.push(clear_config_value("memories.honcho_base_url"));
            edits.push(clear_config_value("memories.honcho_api_key_env"));
        }
    }

    edits
}

pub(crate) fn build_portable_memory_disable_edits() -> Vec<ConfigEdit> {
    vec![replace_config_value(
        "memories.backend",
        serde_json::json!(MemoryBackendKind::Local.as_str()),
    )]
}

pub(crate) fn build_oss_provider_edit(provider: &str) -> ConfigEdit {
    replace_config_value("oss_provider", serde_json::json!(provider))
}

pub(crate) async fn write_config_batch(
    request_handle: AppServerRequestHandle,
    edits: Vec<ConfigEdit>,
) -> Result<ConfigWriteResponse> {
    let request_id = RequestId::String(format!("tui-config-write-{}", Uuid::new_v4()));
    request_handle
        .request_typed(ClientRequest::ConfigBatchWrite {
            request_id,
            params: ConfigBatchWriteParams {
                edits,
                file_path: None,
                expected_version: None,
                reload_user_config: true,
            },
        })
        .await
        .wrap_err("config/batchWrite failed in TUI")
}

pub(crate) async fn write_trusted_project(
    request_handle: AppServerRequestHandle,
    project_path: &Path,
) -> Result<ConfigWriteResponse> {
    write_config_batch(request_handle, vec![trusted_project_edit(project_path)]).await
}

pub(crate) async fn read_effective_config(
    request_handle: AppServerRequestHandle,
    cwd: String,
) -> Result<ConfigReadResponse> {
    let request_id = RequestId::String(format!("tui-config-read-{}", Uuid::new_v4()));
    request_handle
        .request_typed(ClientRequest::ConfigRead {
            request_id,
            params: ConfigReadParams {
                include_layers: false,
                cwd: Some(cwd),
            },
        })
        .await
        .wrap_err("config/read failed in TUI")
}

pub(crate) async fn write_skill_enabled(
    request_handle: AppServerRequestHandle,
    path: AbsolutePathBuf,
    enabled: bool,
) -> Result<()> {
    let request_id = RequestId::String(format!("tui-skill-config-write-{}", Uuid::new_v4()));
    let _: SkillsConfigWriteResponse = request_handle
        .request_typed(ClientRequest::SkillsConfigWrite {
            request_id,
            params: SkillsConfigWriteParams {
                path: Some(path),
                name: None,
                enabled,
            },
        })
        .await
        .wrap_err("skills/config/write failed in TUI")?;
    Ok(())
}

#[cfg(test)]
#[path = "config_update_tests.rs"]
mod tests;
