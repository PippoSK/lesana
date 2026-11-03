use super::interfaces::AbstractUseCase;
use crate::application::repositories::CatalogRepository;
use anyhow::Result;

pub struct CheckHealthUseCase<'a> {
    repository: &'a dyn CatalogRepository,
}
impl<'a> CheckHealthUseCase<'a> {
    pub fn new(repository: &'a dyn CatalogRepository) -> Self {
        Self { repository }
    }
}
impl AbstractUseCase for CheckHealthUseCase<'_> {
    type Output = ();
    async fn execute(&self) -> Result<Self::Output> {
        self.repository.health().await
    }
}
