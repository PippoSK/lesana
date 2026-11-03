use super::interfaces::AbstractUseCase;
use super::{chat_usecase::ChatService, ensure_owned_usecase::EnsureOwnedUseCase};
use crate::application::repositories::ConversationRepository;
use anyhow::Result;
use uuid::Uuid;

pub struct ValidateChatUseCase<'a> {
    owner: Uuid,
    conversation: Uuid,
    question: &'a str,
    repository: &'a dyn ConversationRepository,
    chat: &'a ChatService,
}
impl<'a> ValidateChatUseCase<'a> {
    pub fn new(
        owner: Uuid,
        conversation: Uuid,
        question: &'a str,
        repository: &'a dyn ConversationRepository,
        chat: &'a ChatService,
    ) -> Self {
        Self {
            owner,
            conversation,
            question,
            repository,
            chat,
        }
    }
}
impl AbstractUseCase for ValidateChatUseCase<'_> {
    type Output = ();
    async fn execute(&self) -> Result<Self::Output> {
        EnsureOwnedUseCase::new(self.owner, self.conversation, self.repository)
            .execute()
            .await?;
        self.chat.validate_question(self.question)
    }
}
