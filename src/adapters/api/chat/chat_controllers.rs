use super::chat_payloads::ChatRequest;
use crate::{
    adapters::api::shared::{
        app_state::AppState,
        chat_runs::{ChatRunStatus, StartRunError},
        current_date::today,
        error_presenter::{ApiError, ApiResult, public_error},
        session::session,
        sse::{self, SseProgress},
    },
    application::utils::events::Events,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use uuid::Uuid;
pub async fn chat(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ChatRequest>,
) -> ApiResult<Response> {
    let owner = session(&app, &headers).await?;
    let question = request.message.trim().to_owned();
    app.services.validate_chat(owner, id, &question).await?;
    let request_id = request.request_id.unwrap_or_else(Uuid::new_v4);
    if app.chat_runs.get(id, request_id).is_some() {
        return Err(duplicate_request());
    }
    let permit = app.chat_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Agent je zaneprázdnený. Skúste to o chvíľu.",
        )
    })?;
    app.chat_runs
        .start(id, request_id)
        .map_err(|error| match error {
            StartRunError::Duplicate => duplicate_request(),
            StartRunError::Full => ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Agent je zaneprázdnený. Skúste to o chvíľu.",
            ),
        })?;
    let date = today();
    let architecture = request
        .architecture
        .unwrap_or(app.services.rag_config().default);
    let (tx, rx) = mpsc::channel::<(&'static str, Value)>(32);
    let events: Events = Arc::new(SseProgress {
        sender: tx.clone(),
        runs: app.chat_runs.clone(),
        conversation: id,
        request: request_id,
    });
    let timeout_seconds = app.services.turn_timeout_seconds();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
        let result = tokio::time::timeout_at(
            deadline,
            app.services
                .process_chat(owner, id, &question, date, architecture, &events, deadline),
        )
        .await;
        // Delivery may wait for a suspended client; processing capacity is free.
        drop(permit);
        match result {
            Ok(Ok(answer)) => {
                // process_chat has committed both messages before returning.
                app.chat_runs.complete(id, request_id);
                let _ = tx
                    .send_timeout(("answer", json!(answer)), Duration::from_secs(5))
                    .await;
            }
            result => {
                // Never log a provider body, user question, API key or legal document content.
                let (reason, code, message) = match result {
                    Ok(Err(error)) => {
                        let (code, message) = public_error(&error);
                        (error.to_string(), code, message)
                    }
                    _ => (
                        "Agent request timed out".into(),
                        "timeout",
                        "Rešerš sa nestihla dokončiť v časovom limite. Skúste otázku zúžiť alebo zopakovať.",
                    ),
                };
                tracing::warn!(%reason, code, "Chat failed");
                app.chat_runs.fail(id, request_id, message, code);
                let _ = tx
                    .send_timeout(
                        ("error", json!({"error":message,"code":code})),
                        Duration::from_secs(5),
                    )
                    .await;
            }
        }
    });
    Ok(sse::response(rx))
}

fn duplicate_request() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Táto otázka už bola prijatá. Obnovujem jej stav.",
    )
}

pub async fn chat_status(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ChatRunStatus>> {
    let owner = session(&app, &headers).await?;
    app.services.ensure_owned(owner, id).await?;
    app.chat_runs.get(id, request).map(Json).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Stav otázky už nie je dostupný. Skontrolujte históriu rozhovoru.",
    ))
}
