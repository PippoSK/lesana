use super::{
    app_state::AppState,
    error_presenter::{ApiError, ApiResult},
};
use axum::http::{HeaderMap, StatusCode, header};
use uuid::Uuid;
pub async fn session(app: &AppState, headers: &HeaderMap) -> ApiResult<Uuid> {
    let id = headers
        .get(header::COOKIE)
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
        .split(';')
        .find_map(|part| part.trim().strip_prefix("lexa_session="))
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "Prihláste sa do aplikácie.",
        ))?;
    app.services.require_session(id).await?;
    Ok(id)
}
