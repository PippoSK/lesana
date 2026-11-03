//! Called after the entry adapter authenticates login credentials.
use super::interfaces::AbstractUseCase;
use crate::application::repositories::ConversationRepository;
use anyhow::Result;
use uuid::Uuid;

pub struct StartSessionUseCase<'a> {
    repository: &'a dyn ConversationRepository,
}
impl<'a> StartSessionUseCase<'a> {
    pub fn new(repository: &'a dyn ConversationRepository) -> Self {
        Self { repository }
    }
}
impl AbstractUseCase for StartSessionUseCase<'_> {
    type Output = Uuid;
    async fn execute(&self) -> Result<Self::Output> {
        let id = Uuid::new_v4();
        self.repository.create_session(id).await?;
        Ok(id)
    }
}
