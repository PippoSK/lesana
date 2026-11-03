use super::PortFuture;
use crate::application::contracts::{AttachmentDownload, AttachmentMetadata, PreparedAttachment};
use uuid::Uuid;

pub trait AttachmentRepository: Send + Sync {
    fn list(&self, owner: Uuid, conversation: Uuid) -> PortFuture<'_, Vec<AttachmentMetadata>>;
    /// Check limits and insert atomically while holding the conversation turn lock.
    fn insert(
        &self,
        owner: Uuid,
        conversation: Uuid,
        file: PreparedAttachment,
    ) -> PortFuture<'_, AttachmentMetadata>;
    fn download(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> PortFuture<'_, Option<AttachmentDownload>>;
    fn delete(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> PortFuture<'_, bool>;
}

pub trait AttachmentExtractor: Send + Sync {
    fn extract<'a>(
        &'a self,
        content_type: &'a str,
        bytes: &'a [u8],
    ) -> PortFuture<'a, String>;
}
