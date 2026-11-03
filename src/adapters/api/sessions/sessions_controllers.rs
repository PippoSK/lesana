use super::sessions_payloads::Login;
use crate::adapters::api::shared::{
    app_state::AppState,
    error_presenter::{ApiError, ApiResult},
    session::session,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
pub async fn login(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(login): Json<Login>,
) -> ApiResult<Response> {
    if session(&app, &headers).await.is_ok() {
        return Ok(Json(json!({"ok":true})).into_response());
    }
    if !app.password.is_empty() {
        let mut attempts = app.login_attempts.lock().await;
        attempts.retain(|t| t.elapsed() < Duration::from_secs(60));
        if attempts.len() >= 20 {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Príliš veľa pokusov. Skúste to o minútu.",
            ));
        }
        attempts.push(Instant::now());
        let provided = Sha256::digest(login.password.as_bytes());
        let expected = Sha256::digest(app.password.as_bytes());
        if !bool::from(provided.ct_eq(&expected)) {
            return Err(ApiError(
                StatusCode::UNAUTHORIZED,
                "Zadajte prístupové heslo.",
            ));
        }
    }
    let id = app.services.start_session().await?;
    Ok(super::sessions_presenters::logged_in(id, app.secure_cookie))
}
