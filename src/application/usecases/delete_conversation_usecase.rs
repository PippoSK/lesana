use super::interfaces::AbstractUseCase;
use super::require_session_usecase::RequireSessionUseCase;
use crate::application::repositories::ConversationRepository;
use crate::application::utils::errors::ApplicationError;
use anyhow::Result;
use uuid::Uuid;

pub struct DeleteConversationUseCase<'a> {
    owner: Uuid,
    conversation: Uuid,
    repository: &'a dyn ConversationRepository,
}
impl<'a> DeleteConversationUseCase<'a> {
    pub fn new(
        owner: Uuid,
        conversation: Uuid,
        repository: &'a dyn ConversationRepository,
    ) -> Self {
        Self {
            owner,
            conversation,
            repository,
        }
    }
}
impl AbstractUseCase for DeleteConversationUseCase<'_> {
    type Output = ();
    async fn execute(&self) -> Result<Self::Output> {
        RequireSessionUseCase::new(self.owner, self.repository)
            .execute()
            .await?;
        if !self
            .repository
            .delete(self.owner, self.conversation)
            .await?
        {
            return Err(ApplicationError::ConversationNotFound.into());
        }
        Ok(())
    }
}
