use super::PortFuture;
use crate::application::{
    contracts::{AttachmentContext, ModelMessage},
    usecases::agent::RunAnswer,
};
use uuid::Uuid;

pub trait ConversationTurn: Send {
    fn history(&mut self) -> PortFuture<'_, Vec<ModelMessage>>;
    /// Read the current conversation's documents under the same lock as its history.
    fn attachments(&mut self) -> PortFuture<'_, Vec<AttachmentContext>> {
        Box::pin(async { Ok(vec![]) })
    }
    /// Commit both messages together. Dropping an unfinished turn rolls it back.
    fn commit<'a>(self: Box<Self>, question: &'a str, answer: &'a RunAnswer) -> PortFuture<'a, ()>;
}
pub trait TurnRepository: Send + Sync {
    fn begin_turn(
        &self,
        owner: Uuid,
        conversation: Uuid,
    ) -> PortFuture<'_, Box<dyn ConversationTurn>>;
}
