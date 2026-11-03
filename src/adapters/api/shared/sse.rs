use super::chat_runs::ChatRunRegistry;
use crate::application::{repositories::ProgressSink, utils::events::ProgressEvent};
use axum::response::{
    IntoResponse, Response, Sse,
    sse::{Event, KeepAlive},
};
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use tokio::sync::mpsc;
use uuid::Uuid;
pub type SseMessage = (&'static str, Value);
pub struct SseProgress {
    pub sender: mpsc::Sender<SseMessage>,
    pub runs: ChatRunRegistry,
    pub conversation: Uuid,
    pub request: Uuid,
}
impl ProgressSink for SseProgress {
    fn emit(
        &self,
        event: ProgressEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let (event, text) = match event {
                ProgressEvent::Status(text) => (("status", json!({"text":text})), text),
                ProgressEvent::Tool(call) => {
                    let label = match call.name.as_str() {
                        "search_norms" => "Hľadanie predpisov",
                        "search_judgments" => "Hľadanie judikatúry",
                        "get_section" => "Čítanie paragrafu",
                        "get_judgment" => "Čítanie judikatúry",
                        _ => "Právna rešerš",
                    };
                    let text = format!("{label} · {} podkladov", call.source_count);
                    (("tool", json!(call)), text)
                }
            };
            self.runs.progress(self.conversation, self.request, text);
            // Progress is advisory; a suspended browser must not block research.
            let _ = self.sender.try_send(event);
        })
    }
}

pub fn response(mut receiver: mpsc::Receiver<SseMessage>) -> Response {
    let stream = async_stream::stream! {
        while let Some((name, data)) = receiver.recv().await {
            yield Ok::<Event, Infallible>(Event::default().event(name).data(data.to_string()));
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::api::shared::chat_runs::ChatRunStatus;

    #[tokio::test]
    async fn full_or_disconnected_stream_does_not_block_progress() {
        let (sender, receiver) = mpsc::channel(1);
        let runs = ChatRunRegistry::default();
        let conversation = Uuid::new_v4();
        let request = Uuid::new_v4();
        runs.start(conversation, request).unwrap();
        let progress = SseProgress {
            sender,
            runs: runs.clone(),
            conversation,
            request,
        };
        progress
            .sender
            .try_send(("status", json!({"text":"Prvá udalosť"})))
            .unwrap();
        tokio::time::timeout(
            Duration::from_millis(100),
            progress.emit(ProgressEvent::Status("Plný kanál".into())),
        )
        .await
        .expect("a full progress channel must never hold up the answer");
        assert_eq!(
            runs.get(conversation, request),
            Some(ChatRunStatus::Running {
                text: "Plný kanál".into(),
            })
        );
        drop(receiver);
        tokio::time::timeout(
            Duration::from_millis(100),
            progress.emit(ProgressEvent::Status("Odpojený prehliadač".into())),
        )
        .await
        .expect("a closed progress channel must never hold up the answer");
        assert_eq!(
            runs.get(conversation, request),
            Some(ChatRunStatus::Running {
                text: "Odpojený prehliadač".into(),
            })
        );
    }
}
