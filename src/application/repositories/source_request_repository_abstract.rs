use super::PortFuture;
use crate::application::contracts::SourceRequest;
use uuid::Uuid;

pub trait SourceRequestRepository: Send + Sync {
    fn enqueue<'a>(
        &'a self,
        owner: Uuid,
        kind: &'a str,
        reference: &'a str,
    ) -> PortFuture<'a, SourceRequest>;
    fn list(&self, owner: Uuid) -> PortFuture<'_, Vec<SourceRequest>>;
}
