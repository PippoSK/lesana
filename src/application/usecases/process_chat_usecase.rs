use super::interfaces::AbstractUseCase;
use super::{
    agent::RunAnswer,
    chat_usecase::{ChatRequest, ChatService, ChatUseCase},
    require_session_usecase::RequireSessionUseCase,
};
use crate::application::repositories::ConversationRepository;
use anyhow::Result;

pub struct ProcessChatUseCase<'a> {
    request: ChatRequest<'a>,
    repository: &'a dyn ConversationRepository,
    chat: &'a ChatService,
}
impl<'a> ProcessChatUseCase<'a> {
    pub fn new(
        request: ChatRequest<'a>,
        repository: &'a dyn ConversationRepository,
        chat: &'a ChatService,
    ) -> Self {
        Self {
            request,
            repository,
            chat,
        }
    }
}
impl AbstractUseCase for ProcessChatUseCase<'_> {
    type Output = RunAnswer;
    async fn execute(&self) -> Result<Self::Output> {
        RequireSessionUseCase::new(self.request.owner, self.repository)
            .execute()
            .await?;
        ChatUseCase::new(self.request, self.chat).execute().await
    }
}
