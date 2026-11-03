use super::{app_state::AppState, middleware::same_origin};
use crate::adapters::api::{
    catalog::catalog_controllers::{corpus, health},
    chat::chat_controllers::{chat, chat_status},
    conversations::conversations_controllers::{
        conversations, create_conversation, delete_conversation, messages,
    },
    search::search_controllers::{node_api, search_api},
    sessions::sessions_controllers::login,
};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, header},
    middleware,
    response::Html,
    routing::{get, post},
};
use tower_http::{set_header::SetResponseHeaderLayer, trace::TraceLayer};
pub fn router(app: AppState) -> Router {
    Router::new()
        .route("/",get(||async { Html(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/web/index.html"))) }))
        .route("/app.js",get(||async { ([(header::CONTENT_TYPE,"application/javascript; charset=utf-8")],include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/web/app.js"))) }))
        .route("/style.css",get(||async { ([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/web/style.css"))) }))
        .route("/health",get(health))
        .route("/api/session",post(login))
        .route("/api/corpus",get(corpus))
        .route("/api/source-requests",get(crate::adapters::api::source_requests::list).post(crate::adapters::api::source_requests::create))
        .route("/api/conversations",get(conversations).post(create_conversation))
        .route("/api/conversations/{id}",get(messages).delete(delete_conversation))
        .route("/api/conversations/{id}/attachments",get(crate::adapters::api::attachments::list).post(crate::adapters::api::attachments::upload))
        .route("/api/conversations/{id}/attachments/{attachment_id}",get(crate::adapters::api::attachments::download).delete(crate::adapters::api::attachments::delete))
        .route("/api/conversations/{id}/chat",post(chat))
        .route("/api/conversations/{id}/chat/{request_id}",get(chat_status))
        .route("/api/search",get(search_api))
        .route("/api/nodes/{id}",get(node_api))
        .layer(DefaultBodyLimit::max(64*1024))
        .layer(middleware::from_fn(same_origin))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL,HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")))
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}
