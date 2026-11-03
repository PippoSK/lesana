use super::ensure_owned_usecase::EnsureOwnedUseCase;
use super::interfaces::AbstractUseCase;
use crate::application::contracts::StoredMessage;
use crate::application::repositories::ConversationRepository;
use anyhow::Result;
use uuid::Uuid;

pub struct GetConversationUseCase<'a> {
    owner: Uuid,
    conversation: Uuid,
    repository: &'a dyn ConversationRepository,
}
impl<'a> GetConversationUseCase<'a> {
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
impl AbstractUseCase for GetConversationUseCase<'_> {
    type Output = Vec<StoredMessage>;
    async fn execute(&self) -> Result<Self::Output> {
        EnsureOwnedUseCase::new(self.owner, self.conversation, self.repository)
            .execute()
            .await?;
        self.repository
            .messages(self.owner, self.conversation)
            .await
    }
}
