use std::sync::Arc;

use codex_config::types::MemoryBackendKind;

use crate::backend::AddAdHocMemoryNoteRequest;
use crate::backend::AddAdHocMemoryNoteResponse;
use crate::backend::ListMemoriesRequest;
use crate::backend::ListMemoriesResponse;
use crate::backend::MemoriesBackend;
use crate::backend::MemoriesBackendError;
use crate::backend::ReadMemoryRequest;
use crate::backend::ReadMemoryResponse;
use crate::backend::SearchMemoriesRequest;
use crate::backend::SearchMemoriesResponse;
use crate::honcho::provider_from_settings as honcho_provider_from_settings;
use crate::local::LocalMemoriesBackend;
use crate::portable_schema::PortableMemorySettings;
use crate::provider::MemoryProvider;
use crate::provider::PortableMemoryError;

#[derive(Clone)]
pub(crate) enum SelectedMemoriesBackend {
    Local(LocalMemoriesBackend),
    Provider {
        local: LocalMemoriesBackend,
        provider: Arc<dyn MemoryProvider>,
    },
    Hybrid {
        local: LocalMemoriesBackend,
        provider: Option<Arc<dyn MemoryProvider>>,
    },
}

impl SelectedMemoriesBackend {
    pub(crate) fn from_settings(
        local: LocalMemoriesBackend,
        settings: PortableMemorySettings,
    ) -> Self {
        match settings.backend {
            MemoryBackendKind::Local => Self::Local(local),
            MemoryBackendKind::Honcho => portable_provider_for_settings(&settings)
                .map(|provider| Self::Provider {
                    local: local.clone(),
                    provider,
                })
                .unwrap_or_else(|| Self::Local(local)),
            MemoryBackendKind::Hybrid => Self::Hybrid {
                local,
                provider: portable_provider_for_settings(&settings),
            },
        }
    }
}

impl MemoriesBackend for SelectedMemoriesBackend {
    async fn add_ad_hoc_note(
        &self,
        request: AddAdHocMemoryNoteRequest,
    ) -> Result<AddAdHocMemoryNoteResponse, MemoriesBackendError> {
        match self {
            Self::Local(local) => local.add_ad_hoc_note(request).await,
            Self::Provider { local, provider } => match provider.add_note(request.clone()).await {
                Ok(response) => Ok(response),
                Err(err) if provider_error_should_fallback(&err) => {
                    local.add_ad_hoc_note(request).await
                }
                Err(err) => Err(provider_error_to_backend_error(err)),
            },
            Self::Hybrid { local, provider } => {
                let response = local.add_ad_hoc_note(request.clone()).await?;
                if let Some(provider) = provider {
                    let _ = provider.add_note(request).await;
                }
                Ok(response)
            }
        }
    }

    async fn list(
        &self,
        request: ListMemoriesRequest,
    ) -> Result<ListMemoriesResponse, MemoriesBackendError> {
        match self {
            Self::Local(local) => local.list(request).await,
            Self::Provider { local, provider } => match provider.list(request.clone()).await {
                Ok(response) => Ok(response),
                Err(err) if provider_error_should_fallback(&err) => local.list(request).await,
                Err(err) => Err(provider_error_to_backend_error(err)),
            },
            Self::Hybrid { local, provider } => match provider {
                Some(provider) => match provider.list(request.clone()).await {
                    Ok(response) => Ok(response),
                    Err(_) => local.list(request).await,
                },
                None => local.list(request).await,
            },
        }
    }

    async fn read(
        &self,
        request: ReadMemoryRequest,
    ) -> Result<ReadMemoryResponse, MemoriesBackendError> {
        match self {
            Self::Local(local) => local.read(request).await,
            Self::Provider { local, provider } => match provider.read(request.clone()).await {
                Ok(response) => Ok(response),
                Err(err) if provider_error_should_fallback(&err) => local.read(request).await,
                Err(err) => Err(provider_error_to_backend_error(err)),
            },
            Self::Hybrid { local, provider } => match provider {
                Some(provider) => match provider.read(request.clone()).await {
                    Ok(response) => Ok(response),
                    Err(_) => local.read(request).await,
                },
                None => local.read(request).await,
            },
        }
    }

    async fn search(
        &self,
        request: SearchMemoriesRequest,
    ) -> Result<SearchMemoriesResponse, MemoriesBackendError> {
        match self {
            Self::Local(local) => local.search(request).await,
            Self::Provider { local, provider } => match provider.search(request.clone()).await {
                Ok(response) => Ok(response),
                Err(err) if provider_error_should_fallback(&err) => local.search(request).await,
                Err(err) => Err(provider_error_to_backend_error(err)),
            },
            Self::Hybrid { local, provider } => match provider {
                Some(provider) => match provider.search(request.clone()).await {
                    Ok(response) => Ok(response),
                    Err(_) => local.search(request).await,
                },
                None => local.search(request).await,
            },
        }
    }
}

pub(crate) fn portable_provider_for_settings(
    settings: &PortableMemorySettings,
) -> Option<Arc<dyn MemoryProvider>> {
    match settings.backend {
        MemoryBackendKind::Local => None,
        MemoryBackendKind::Honcho | MemoryBackendKind::Hybrid => {
            honcho_provider_from_settings(settings)
        }
    }
}

fn provider_error_to_backend_error(err: PortableMemoryError) -> MemoriesBackendError {
    match err {
        PortableMemoryError::Backend(err) => err,
        PortableMemoryError::NotConfigured => {
            MemoriesBackendError::Io(std::io::Error::other(err.to_string()))
        }
        PortableMemoryError::Request(message) => {
            MemoriesBackendError::Io(std::io::Error::other(message))
        }
        PortableMemoryError::RejectedContent(message) => {
            MemoriesBackendError::Io(std::io::Error::other(message))
        }
    }
}

fn provider_error_should_fallback(err: &PortableMemoryError) -> bool {
    matches!(
        err,
        PortableMemoryError::NotConfigured | PortableMemoryError::Request(_)
    )
}
