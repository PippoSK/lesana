use axum::{
    Json,
    http::header,
    response::{IntoResponse, Response},
};
use serde_json::json;
use uuid::Uuid;
pub fn logged_in(id: Uuid, secure_cookie: bool) -> Response {
    let secure = if secure_cookie { "; Secure" } else { "" };
    let cookie =
        format!("lexa_session={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000{secure}");
    ([(header::SET_COOKIE, cookie)], Json(json!({"ok":true}))).into_response()
}
