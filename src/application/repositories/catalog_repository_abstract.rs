use super::PortFuture;
use crate::application::contracts::CorpusSnapshot;

pub trait CatalogRepository: Send + Sync {
    fn health(&self) -> PortFuture<'_, ()>;
    fn corpus(&self) -> PortFuture<'_, CorpusSnapshot>;
}
