use super::PortFuture;
use crate::application::contracts::{ConversationSummary, StoredMessage};
use uuid::Uuid;

pub trait ConversationRepository: Send + Sync {
    fn session_exists(&self, id: Uuid) -> PortFuture<'_, bool>;
    fn create_session(&self, id: Uuid) -> PortFuture<'_, ()>;
    fn list(&self, owner: Uuid) -> PortFuture<'_, Vec<ConversationSummary>>;
    fn create(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, ()>;
    fn owns(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, bool>;
    fn messages(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, Vec<StoredMessage>>;
    fn delete(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, bool>;
}
