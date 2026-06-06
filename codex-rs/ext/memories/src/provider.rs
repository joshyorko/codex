use crate::backend::AddAdHocMemoryNoteRequest;
use crate::backend::AddAdHocMemoryNoteResponse;
use crate::backend::ListMemoriesRequest;
use crate::backend::ListMemoriesResponse;
use crate::backend::MemoriesBackendError;
use crate::backend::ReadMemoryRequest;
use crate::backend::ReadMemoryResponse;
use crate::backend::SearchMemoriesRequest;
use crate::backend::SearchMemoriesResponse;
use crate::portable_schema::LocalCodexMemorySyncRequest;
use crate::portable_schema::LocalCodexMemorySyncResponse;
use crate::portable_schema::PortableMemoryConclusion;
use crate::portable_schema::PortableMemoryContext;
use crate::portable_schema::VisibleMemoryMessage;
use std::future::Future;
use std::pin::Pin;

pub(crate) type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, PortableMemoryError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum PortableMemoryError {
    #[error("portable memory provider is not configured")]
    NotConfigured,
    #[error("portable memory provider request failed: {0}")]
    Request(String),
    #[error(transparent)]
    Backend(#[from] MemoriesBackendError),
}

pub(crate) trait MemoryProvider: Send + Sync {
    fn recall(&self, query: String) -> ProviderFuture<'_, PortableMemoryContext>;

    fn search(&self, request: SearchMemoriesRequest) -> ProviderFuture<'_, SearchMemoriesResponse>;

    fn list(&self, request: ListMemoriesRequest) -> ProviderFuture<'_, ListMemoriesResponse>;

    fn read(&self, request: ReadMemoryRequest) -> ProviderFuture<'_, ReadMemoryResponse>;

    fn add_note(
        &self,
        request: AddAdHocMemoryNoteRequest,
    ) -> ProviderFuture<'_, AddAdHocMemoryNoteResponse>;

    fn write_visible_turn(&self, messages: Vec<VisibleMemoryMessage>) -> ProviderFuture<'_, ()>;

    fn conclude(&self, conclusion: PortableMemoryConclusion) -> ProviderFuture<'_, ()>;

    fn sync_local_files(
        &self,
        request: LocalCodexMemorySyncRequest,
    ) -> ProviderFuture<'_, LocalCodexMemorySyncResponse>;

    fn flush(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
