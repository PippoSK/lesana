use super::error_presenter::ApiError;
use axum::{
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
pub async fn same_origin(request: axum::extract::Request, next: Next) -> Response {
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        let headers = request.headers();
        let cross_site = headers
            .get("sec-fetch-site")
            .is_some_and(|h| h == "cross-site");
        let origin_mismatch = headers.get(header::ORIGIN).is_some_and(|origin| {
            let host = headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            let origin = origin.to_str().unwrap_or("");
            origin != format!("http://{host}") && origin != format!("https://{host}")
        });
        if cross_site || origin_mismatch {
            return ApiError(StatusCode::FORBIDDEN, "Neplatný pôvod požiadavky.").into_response();
        }
    }
    next.run(request).await
}
