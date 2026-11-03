use crate::adapters::api::shared::{app_state::AppState, error_presenter::ApiResult};
use axum::{Json, extract::State};
use serde_json::{Value, json};
pub async fn health(State(app): State<AppState>) -> ApiResult<Json<Value>> {
    app.services.health().await?;
    Ok(Json(json!({"status":"ok"})))
}

pub async fn corpus(State(app): State<AppState>) -> ApiResult<Json<Value>> {
    let snapshot = app.services.corpus().await?;
    Ok(Json(super::catalog_presenters::corpus(&app, snapshot)))
}
