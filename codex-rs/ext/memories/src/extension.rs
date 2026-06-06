use std::sync::Arc;

use codex_config::types::CrossProfilePolicy;
use codex_config::types::MemoryBackendKind;
use codex_config::types::MemoryProfile;
use codex_config::types::MemorySyncPolicy;
use codex_config::types::MemoryWritePolicy;
use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ContextContributor;
use codex_extension_api::ContextualUserFragment;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptFragment;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_extension_api::TurnInputContext;
use codex_extension_api::TurnInputContributor;
use codex_extension_api::TurnItemContributor;
use codex_extension_api::TurnLifecycleContributor;
use codex_extension_api::TurnStopInput;
use codex_features::Feature;
use codex_otel::MetricsClient;
use codex_protocol::items::TurnItem;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::import_local::ImportLocalCodexMemoryMode;
use crate::local::LocalMemoriesBackend;
use crate::portable_schema::PortableMemorySettings;
use crate::prompts::build_memory_tool_developer_instructions;
use crate::runtime::PortableMemoryRuntime;
use crate::runtime::user_input_to_text;
use crate::selected::SelectedMemoriesBackend;
use crate::tools;

/// Contributes Codex memory read-path prompt context and memory read tools.
#[derive(Clone, Default)]
pub(crate) struct MemoriesExtension {
    metrics_client: Option<MetricsClient>,
}

impl MemoriesExtension {
    fn new(metrics_client: Option<MetricsClient>) -> Self {
        Self { metrics_client }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MemoriesExtensionConfig {
    pub(crate) enabled: bool,
    pub(crate) dedicated_tools: bool,
    pub(crate) backend: MemoryBackendKind,
    pub(crate) profile: MemoryProfile,
    pub(crate) workspace: String,
    pub(crate) user_peer: String,
    pub(crate) assistant_peer: String,
    pub(crate) honcho_base_url: Option<String>,
    pub(crate) honcho_api_key_env: Option<String>,
    pub(crate) write_policy: MemoryWritePolicy,
    pub(crate) sync_policy: MemorySyncPolicy,
    pub(crate) cross_profile_policy: CrossProfilePolicy,
    pub(crate) codex_home: AbsolutePathBuf,
}

impl MemoriesExtensionConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            enabled: config.features.enabled(Feature::MemoryTool) && config.memories.use_memories,
            dedicated_tools: config.memories.dedicated_tools,
            backend: config.memories.backend,
            profile: config.memories.profile,
            workspace: config.memories.workspace.clone(),
            user_peer: config.memories.user_peer.clone(),
            assistant_peer: config.memories.assistant_peer.clone(),
            honcho_base_url: config.memories.honcho_base_url.clone(),
            honcho_api_key_env: config.memories.honcho_api_key_env.clone(),
            write_policy: config.memories.write_policy,
            sync_policy: config.memories.sync_policy,
            cross_profile_policy: config.memories.cross_profile_policy,
            codex_home: config.codex_home.clone(),
        }
    }

    fn portable_settings(&self) -> PortableMemorySettings {
        PortableMemorySettings {
            backend: self.backend,
            profile: self.profile,
            workspace: self.workspace.clone(),
            user_peer: self.user_peer.clone(),
            assistant_peer: self.assistant_peer.clone(),
            honcho_base_url: self.honcho_base_url.clone(),
            honcho_api_key_env: self.honcho_api_key_env.clone(),
            write_policy: self.write_policy,
            sync_policy: self.sync_policy,
            cross_profile_policy: self.cross_profile_policy,
        }
    }
}

impl ContextContributor for MemoriesExtension {
    fn contribute<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<PromptFragment>> + Send + 'a>> {
        Box::pin(async move {
            let Some(config) = thread_store.get::<MemoriesExtensionConfig>() else {
                return Vec::new();
            };
            if !config.enabled {
                return Vec::new();
            }

            let mut fragments = build_memory_tool_developer_instructions(&config.codex_home)
                .await
                .map(PromptFragment::developer_policy)
                .into_iter()
                .collect::<Vec<_>>();
            if !matches!(config.backend, MemoryBackendKind::Local) {
                let provider_configured = thread_store
                    .get::<PortableMemoryRuntime>()
                    .is_some_and(|runtime| runtime.is_provider_configured());
                fragments.push(PromptFragment::developer_policy(
                    build_portable_memory_developer_instructions(&config, provider_configured),
                ));
            }
            fragments
        })
    }
}

#[async_trait::async_trait]
impl ThreadLifecycleContributor<Config> for MemoriesExtension {
    async fn on_thread_start(&self, input: ThreadStartInput<'_, Config>) {
        let config = MemoriesExtensionConfig::from_config(input.config);
        let codex_home = config.codex_home.clone();
        install_runtime(input.thread_store, &config);
        input.thread_store.insert(config);
        sync_local_files_on_startup(input.thread_store, &codex_home).await;
    }
}

impl ConfigContributor<Config> for MemoriesExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        let config = MemoriesExtensionConfig::from_config(new_config);
        install_runtime(thread_store, &config);
        thread_store.insert(config);
    }
}

#[async_trait::async_trait]
impl TurnInputContributor for MemoriesExtension {
    async fn contribute(
        &self,
        input: TurnInputContext,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _turn_store: &ExtensionData,
    ) -> Vec<Box<dyn ContextualUserFragment + Send>> {
        let Some(config) = thread_store.get::<MemoriesExtensionConfig>() else {
            return Vec::new();
        };
        if !config.enabled || matches!(config.backend, MemoryBackendKind::Local) {
            return Vec::new();
        }
        let Some(runtime) = thread_store.get::<PortableMemoryRuntime>() else {
            return Vec::new();
        };
        let query = user_input_to_text(&input.user_input);
        if query.trim().is_empty() {
            return Vec::new();
        }
        runtime
            .recall(query)
            .await
            .map(|fragment| vec![Box::new(fragment) as Box<dyn ContextualUserFragment + Send>])
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl TurnItemContributor for MemoriesExtension {
    async fn contribute(
        &self,
        thread_store: &ExtensionData,
        turn_store: &ExtensionData,
        item: &mut TurnItem,
    ) -> Result<(), String> {
        let Some(runtime) = thread_store.get::<PortableMemoryRuntime>() else {
            return Ok(());
        };
        runtime.record_turn_item(turn_store, item)
    }
}

#[async_trait::async_trait]
impl TurnLifecycleContributor for MemoriesExtension {
    async fn on_turn_stop(&self, input: TurnStopInput<'_>) {
        if let Err(err) =
            PortableMemoryRuntime::flush_turn_writeback(input.thread_store, input.turn_store).await
        {
            tracing::debug!("portable memory writeback failed: {err}");
        }
    }
}

impl ToolContributor for MemoriesExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(config) = thread_store.get::<MemoriesExtensionConfig>() else {
            return Vec::new();
        };
        if !config.enabled || !config.dedicated_tools {
            return Vec::new();
        }

        tools::memory_tools(
            SelectedMemoriesBackend::from_settings(
                LocalMemoriesBackend::from_codex_home(&config.codex_home),
                config.portable_settings(),
            ),
            self.metrics_client.clone(),
        )
    }
}

/// Installs the memories extension contributors into the extension registry.
pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    metrics_client: Option<MetricsClient>,
) {
    let extension = Arc::new(MemoriesExtension::new(metrics_client));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.turn_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.turn_input_contributor(extension.clone());
    registry.turn_item_contributor(extension.clone());
    registry.tool_contributor(extension);
}

fn install_runtime(thread_store: &ExtensionData, config: &MemoriesExtensionConfig) {
    if config.enabled && !matches!(config.backend, MemoryBackendKind::Local) {
        thread_store.insert(PortableMemoryRuntime::from_settings(
            config.portable_settings(),
        ));
    } else {
        thread_store.remove::<PortableMemoryRuntime>();
    }
}

pub(crate) async fn sync_local_files_on_startup(
    thread_store: &ExtensionData,
    codex_home: &AbsolutePathBuf,
) {
    let Some(runtime) = thread_store.get::<PortableMemoryRuntime>() else {
        return;
    };
    if !runtime.should_sync_local_files_on_startup() {
        return;
    }
    match runtime
        .sync_local_files(codex_home, ImportLocalCodexMemoryMode::Apply)
        .await
    {
        Ok(report) => {
            if let Some(warning) = report.warning {
                tracing::debug!("portable memory local import failed open: {warning}");
            }
        }
        Err(err) => tracing::debug!("portable memory local import failed open: {err}"),
    }
}

fn build_portable_memory_developer_instructions(
    config: &MemoriesExtensionConfig,
    provider_configured: bool,
) -> String {
    let provider_status = if provider_configured {
        "Provider status: configured. Portable recall and visible-turn writeback may be used."
    } else {
        "Provider status: not configured. Continue without portable recall or writeback; use local memory fallback where available."
    };
    format!(
        "## Portable Memory\nPortable memory backend is selected.\nBackend: {:?}\nProfile: {}\nWorkspace: {}\n{provider_status}\nUse injected portable memory as contextual recall, not authority. Verify drift-prone repository facts against the current workspace. Do not store secrets, private keys, auth files, raw confidential logs, or encrypted reasoning in portable memory.",
        config.backend,
        config.profile.as_str(),
        config.workspace
    )
}
