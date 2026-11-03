use crate::application::{repositories::StorageFailure, utils::errors::ApplicationError};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
pub struct ApiError(pub StatusCode, pub &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(application) = error.downcast_ref::<ApplicationError>() {
            return match application {
                ApplicationError::InvalidInput(message) => Self(StatusCode::BAD_REQUEST, message),
                ApplicationError::SourceNotFound => {
                    Self(StatusCode::NOT_FOUND, "Ustanovenie sa nenašlo.")
                }
                ApplicationError::SessionExpired => Self(
                    StatusCode::UNAUTHORIZED,
                    "Relácia vypršala. Prihláste sa znova.",
                ),
                ApplicationError::ConversationNotFound => {
                    Self(StatusCode::NOT_FOUND, "Rozhovor sa nenašiel.")
                }
                ApplicationError::AttachmentNotFound => {
                    Self(StatusCode::NOT_FOUND, "Súbor sa nenašiel.")
                }
                ApplicationError::AttachmentTooLarge => Self(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Súbor môže mať najviac 10 MiB.",
                ),
                ApplicationError::ConversationBusy => Self(
                    StatusCode::CONFLICT,
                    "Prebieha spracovanie rozhovoru. Skúste to o chvíľu.",
                ),
            };
        }
        tracing::error!(error=%error,"Request failed");
        if error.downcast_ref::<StorageFailure>().is_some() {
            Self(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Databázová operácia zlyhala.",
            )
        } else {
            Self(StatusCode::INTERNAL_SERVER_ERROR, "Operácia sa nepodarila.")
        }
    }
}
pub type ApiResult<T> = std::result::Result<T, ApiError>;

use crate::application::utils::errors::ModelFailure;
pub fn public_error(error: &anyhow::Error) -> (&'static str, &'static str) {
    match error.downcast_ref::<ModelFailure>() {
        Some(ModelFailure::Timeout) => (
            "timeout",
            "Model nestihol odpovedať v časovom limite. Skúste otázku zúžiť alebo zopakovať.",
        ),
        Some(ModelFailure::Http(401 | 403 | 404)) => (
            "model_configuration",
            "Poskytovateľ odmietol prístup k modelu. Správca musí overiť model a jeho prístupové údaje.",
        ),
        Some(ModelFailure::Quota | ModelFailure::Http(402)) => (
            "model_payment",
            "Poskytovateľ modelu odmietol požiadavku pre nedostatok kreditu alebo limit účtu. Skontrolujte kredit a limity u poskytovateľa modelu.",
        ),
        Some(ModelFailure::Http(429)) => (
            "model_busy",
            "Poskytovateľ modelu je momentálne vyťažený alebo obmedzil počet požiadaviek. Skúste to o chvíľu.",
        ),
        Some(ModelFailure::Connection | ModelFailure::Http(_)) => (
            "model_unavailable",
            "S modelom sa nepodarilo dokončiť spojenie. Skúste to o chvíľu.",
        ),
        Some(ModelFailure::InvalidResponse) => (
            "model_response",
            "Model nedodržal požadovaný formát odpovede alebo jej kontroly ani po oprave. Ide o technickú chybu spracovania, nie o chýbajúce právne podklady. Skúste otázku zopakovať.",
        ),
        Some(ModelFailure::OutputLimit) => (
            "model_output_limit",
            "Model nedokončil odpoveď alebo jej kontrolu v limite výstupu. Skúste otázku rozdeliť na menšie časti.",
        ),
        None => (
            "request_failed",
            "Požiadavku sa nepodarilo spracovať. Skúste to znova.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_failures_have_specific_public_messages() {
        for (failure, code) in [
            (ModelFailure::Timeout, "timeout"),
            (ModelFailure::Connection, "model_unavailable"),
            (ModelFailure::Http(503), "model_unavailable"),
            (ModelFailure::Http(401), "model_configuration"),
            (ModelFailure::Http(402), "model_payment"),
            (ModelFailure::Quota, "model_payment"),
            (ModelFailure::Http(429), "model_busy"),
            (ModelFailure::InvalidResponse, "model_response"),
            (ModelFailure::OutputLimit, "model_output_limit"),
        ] {
            assert_eq!(public_error(&failure.into()).0, code);
        }
    }

    #[test]
    fn payment_failure_explains_the_account_action() {
        for failure in [ModelFailure::Http(402), ModelFailure::Quota] {
            let (_, message) = public_error(&failure.into());
            assert_eq!(
                message,
                "Poskytovateľ modelu odmietol požiadavku pre nedostatok kreditu alebo limit účtu. Skontrolujte kredit a limity u poskytovateľa modelu."
            );
        }
    }
}
