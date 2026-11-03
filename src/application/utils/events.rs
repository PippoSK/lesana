use crate::application::{repositories::ProgressSink, usecases::retrieval::ToolCall};
use std::sync::Arc;

pub type Events = Arc<dyn ProgressSink>;
pub enum ProgressEvent {
    Status(String),
    Tool(ToolCall),
}
pub async fn status(events: &Events, text: &str) {
    events.emit(ProgressEvent::Status(text.to_owned())).await;
}
