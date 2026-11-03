use super::interfaces::AbstractUseCase;
use crate::application::repositories::ConversationRepository;
use crate::application::utils::errors::ApplicationError;
use anyhow::Result;
use uuid::Uuid;

pub struct RequireSessionUseCase<'a> {
    owner: Uuid,
    repository: &'a dyn ConversationRepository,
}
impl<'a> RequireSessionUseCase<'a> {
    pub fn new(owner: Uuid, repository: &'a dyn ConversationRepository) -> Self {
        Self { owner, repository }
    }
}
impl AbstractUseCase for RequireSessionUseCase<'_> {
    type Output = ();
    async fn execute(&self) -> Result<Self::Output> {
        if !self.repository.session_exists(self.owner).await? {
            return Err(ApplicationError::SessionExpired.into());
        }
        Ok(())
    }
}
