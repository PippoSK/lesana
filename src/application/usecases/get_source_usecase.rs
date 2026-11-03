use super::interfaces::AbstractUseCase;
use crate::application::utils::errors::ApplicationError;
use crate::{application::repositories::LegalRepository, domain::Source};
use anyhow::Result;

pub struct GetSourceUseCase<'a> {
    id: i64,
    repository: &'a dyn LegalRepository,
}
impl<'a> GetSourceUseCase<'a> {
    pub fn new(id: i64, repository: &'a dyn LegalRepository) -> Self {
        Self { id, repository }
    }
}
impl AbstractUseCase for GetSourceUseCase<'_> {
    type Output = Source;
    async fn execute(&self) -> Result<Self::Output> {
        self.repository
            .get_node(self.id)
            .await?
            .ok_or_else(|| ApplicationError::SourceNotFound.into())
    }
}
