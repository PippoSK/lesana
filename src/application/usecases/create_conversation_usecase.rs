use super::interfaces::AbstractUseCase;
use super::require_session_usecase::RequireSessionUseCase;
use crate::application::contracts::CreatedConversation;
use crate::application::repositories::ConversationRepository;
use anyhow::Result;
use uuid::Uuid;

pub struct CreateConversationUseCase<'a> {
    owner: Uuid,
    repository: &'a dyn ConversationRepository,
}
impl<'a> CreateConversationUseCase<'a> {
    pub fn new(owner: Uuid, repository: &'a dyn ConversationRepository) -> Self {
        Self { owner, repository }
    }
}
impl AbstractUseCase for CreateConversationUseCase<'_> {
    type Output = CreatedConversation;
    async fn execute(&self) -> Result<Self::Output> {
        RequireSessionUseCase::new(self.owner, self.repository)
            .execute()
            .await?;
        let id = Uuid::new_v4();
        self.repository.create(self.owner, id).await?;
        Ok(CreatedConversation {
            id,
            title: "Nový rozhovor".into(),
        })
    }
}
