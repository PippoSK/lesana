use crate::application::contracts::{MessageRole, ModelMessage};
use rig::message::Message;
/// Only server-created roles enter Rig. User content never becomes a preamble.
pub fn messages(values: &[ModelMessage]) -> Vec<Message> {
    values
        .iter()
        .map(|value| match value.role {
            MessageRole::System => Message::system(&value.content),
            MessageRole::User => Message::user(&value.content),
            MessageRole::Assistant => Message::assistant(&value.content),
        })
        .collect()
}
