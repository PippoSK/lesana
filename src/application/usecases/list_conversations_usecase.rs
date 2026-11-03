use super::interfaces::AbstractUseCase;
use super::require_session_usecase::RequireSessionUseCase;
use crate::application::contracts::ConversationSummary;
use crate::application::repositories::ConversationRepository;
use anyhow::Result;
use uuid::Uuid;

pub struct ListConversationsUseCase<'a> {
    owner: Uuid,
    repository: &'a dyn ConversationRepository,
}
impl<'a> ListConversationsUseCase<'a> {
    pub fn new(owner: Uuid, repository: &'a dyn ConversationRepository) -> Self {
        Self { owner, repository }
    }
}
impl AbstractUseCase for ListConversationsUseCase<'_> {
    type Output = Vec<ConversationSummary>;
    async fn execute(&self) -> Result<Self::Output> {
        RequireSessionUseCase::new(self.owner, self.repository)
            .execute()
            .await?;
        self.repository.list(self.owner).await
    }
}
