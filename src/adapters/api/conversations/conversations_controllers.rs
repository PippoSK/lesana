use crate::{
    adapters::api::shared::{app_state::AppState, error_presenter::ApiResult, session::session},
    application::contracts::{ConversationSummary, CreatedConversation, StoredMessage},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use uuid::Uuid;
pub async fn conversations(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<ConversationSummary>>> {
    let owner = session(&app, &headers).await?;
    let rows = app.services.list_conversations(owner).await?;
    Ok(Json(rows))
}
pub async fn create_conversation(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<CreatedConversation>> {
    let owner = session(&app, &headers).await?;
    Ok(Json(app.services.create_conversation(owner).await?))
}
pub async fn messages(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<StoredMessage>>> {
    let owner = session(&app, &headers).await?;
    let rows = app.services.get_conversation(owner, id).await?;
    Ok(Json(rows))
}
pub async fn delete_conversation(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let owner = session(&app, &headers).await?;
    app.services.delete_conversation(owner, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
