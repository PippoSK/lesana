use super::search_payloads::SearchRequest;
use crate::{
    adapters::api::shared::{
        app_state::AppState, current_date::today, error_presenter::ApiResult, session::session,
    },
    domain::Source,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde_json::{Value, json};
pub async fn search_api(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchRequest>,
) -> ApiResult<Json<Value>> {
    session(&app, &headers).await?;
    let date = query.as_of.unwrap_or_else(today);
    Ok(Json(json!(app.services.search(&query.q, date).await?)))
}

pub async fn node_api(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Source>> {
    session(&app, &headers).await?;
    Ok(Json(app.services.get_source(id).await?))
}
