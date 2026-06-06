use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;

use codex_config::types::MemorySyncPolicy;
use codex_config::types::MemoryWritePolicy;
use codex_extension_api::ExtensionData;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::import_local::ImportLocalCodexMemoryError;
use crate::import_local::ImportLocalCodexMemoryMode;
use crate::import_local::ImportLocalCodexMemoryReport;
use crate::import_local::sync_local_codex_memory_with_provider;
use crate::policy::portable_metadata;
use crate::policy::sanitize_visible_memory_content;
use crate::portable_schema::PortableMemoryActor;
use crate::portable_schema::PortableMemoryContext;
use crate::portable_schema::PortableMemoryFragment;
use crate::portable_schema::PortableMemorySettings;
use crate::portable_schema::VisibleMemoryMessage;
use crate::portable_schema::render_context_fragment;
use crate::provider::MemoryProvider;
use crate::selected::portable_provider_for_settings;

const TURN_RECALL_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) struct PortableMemoryRuntime {
    settings: PortableMemorySettings,
    provider: Option<Arc<dyn MemoryProvider>>,
    cached_context: Mutex<Option<PortableMemoryContext>>,
}

#[derive(Default)]
struct TurnWritebackBuffer {
    messages: Mutex<Vec<VisibleMemoryMessage>>,
}

impl PortableMemoryRuntime {
    pub(crate) fn from_settings(settings: PortableMemorySettings) -> Self {
        let provider = portable_provider_for_settings(&settings);
        Self {
            settings,
            provider,
            cached_context: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_provider_tests(
        settings: PortableMemorySettings,
        provider: Arc<dyn MemoryProvider>,
    ) -> Self {
        Self {
            settings,
            provider: Some(provider),
            cached_context: Mutex::new(None),
        }
    }

    pub(crate) async fn recall(&self, query: String) -> Option<PortableMemoryFragment> {
        let provider = self.provider.as_ref()?;
        let result = tokio::time::timeout(TURN_RECALL_TIMEOUT, provider.recall(query)).await;
        match result {
            Ok(Ok(context)) if !context.is_empty() => {
                *self.cached_context() = Some(context.clone());
                Some(render_context_fragment(&self.settings, &context))
            }
            _ => self
                .cached_context()
                .clone()
                .filter(|context| !context.is_empty())
                .map(|context| render_context_fragment(&self.settings, &context)),
        }
    }

    pub(crate) async fn sync_local_files(
        &self,
        codex_home: &AbsolutePathBuf,
        mode: ImportLocalCodexMemoryMode,
    ) -> Result<ImportLocalCodexMemoryReport, ImportLocalCodexMemoryError> {
        sync_local_codex_memory_with_provider(
            codex_home,
            &self.settings,
            mode,
            self.provider.clone(),
        )
        .await
    }

    pub(crate) fn is_provider_configured(&self) -> bool {
        self.provider.is_some()
    }

    pub(crate) fn should_sync_local_files_on_startup(&self) -> bool {
        matches!(self.settings.sync_policy, MemorySyncPolicy::Startup)
            && self.is_provider_configured()
    }

    pub(crate) fn record_turn_item(
        &self,
        turn_store: &ExtensionData,
        item: &TurnItem,
    ) -> Result<(), String> {
        if !matches!(self.settings.write_policy, MemoryWritePolicy::VisibleTurns) {
            return Ok(());
        }
        let Some(message) = self.message_from_turn_item(item) else {
            return Ok(());
        };
        turn_store
            .get_or_init(TurnWritebackBuffer::default)
            .messages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(message);
        Ok(())
    }

    pub(crate) async fn flush_turn_writeback(
        thread_store: &ExtensionData,
        turn_store: &ExtensionData,
    ) -> Result<(), String> {
        let Some(runtime) = thread_store.get::<PortableMemoryRuntime>() else {
            return Ok(());
        };
        let Some(provider) = runtime.provider.as_ref() else {
            return Ok(());
        };
        let Some(buffer) = turn_store.get::<TurnWritebackBuffer>() else {
            return Ok(());
        };
        let messages = std::mem::take(
            &mut *buffer
                .messages
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        provider
            .write_visible_turn(messages)
            .await
            .map_err(|err| err.to_string())?;
        provider.flush().await.map_err(|err| err.to_string())
    }

    fn message_from_turn_item(&self, item: &TurnItem) -> Option<VisibleMemoryMessage> {
        match item {
            TurnItem::UserMessage(message) => {
                let content = user_input_to_text(&message.content);
                sanitize_visible_memory_content(&content).map(|content| VisibleMemoryMessage {
                    actor: PortableMemoryActor::User,
                    content,
                    metadata: portable_metadata(
                        &self.settings,
                        "codex-turn-item",
                        "visible-user-message",
                        "public",
                    ),
                })
            }
            TurnItem::AgentMessage(message) => {
                let content = message
                    .content
                    .iter()
                    .map(|content| match content {
                        AgentMessageContent::Text { text } => text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                sanitize_visible_memory_content(&content).map(|content| VisibleMemoryMessage {
                    actor: PortableMemoryActor::Assistant,
                    content,
                    metadata: portable_metadata(
                        &self.settings,
                        "codex-turn-item",
                        "visible-assistant-message",
                        "public",
                    ),
                })
            }
            _ => None,
        }
    }

    fn cached_context(&self) -> std::sync::MutexGuard<'_, Option<PortableMemoryContext>> {
        self.cached_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) fn user_input_to_text(input: &[UserInput]) -> String {
    input
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.as_str()),
            UserInput::Skill { name, .. } | UserInput::Mention { name, .. } => Some(name.as_str()),
            UserInput::Image { .. } | UserInput::LocalImage { .. } => None,
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
