use crate::application::usecases::retrieval;
use serde::Deserialize;
#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub architecture: Option<retrieval::Architecture>,
    pub request_id: Option<uuid::Uuid>,
}
