use super::interfaces::AbstractUseCase;
use crate::application::{contracts::CorpusSnapshot, repositories::CatalogRepository};
use anyhow::Result;

pub struct GetCorpusUseCase<'a> {
    repository: &'a dyn CatalogRepository,
}
impl<'a> GetCorpusUseCase<'a> {
    pub fn new(repository: &'a dyn CatalogRepository) -> Self {
        Self { repository }
    }
}
impl AbstractUseCase for GetCorpusUseCase<'_> {
    type Output = CorpusSnapshot;
    async fn execute(&self) -> Result<Self::Output> {
        self.repository.corpus().await
    }
}
