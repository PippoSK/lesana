use super::shared::{app_state::AppState, error_presenter::ApiResult, session::session};
use crate::application::contracts::SourceRequest;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestSource {
    kind: String,
    reference: String,
}

pub async fn list(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<SourceRequest>>> {
    let owner = session(&app, &headers).await?;
    Ok(Json(app.services.list_source_requests(owner).await?))
}

pub async fn create(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<RequestSource>,
) -> ApiResult<(StatusCode, Json<SourceRequest>)> {
    let owner = session(&app, &headers).await?;
    let request = app
        .services
        .request_source(owner, &input.kind, &input.reference)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(request)))
}
