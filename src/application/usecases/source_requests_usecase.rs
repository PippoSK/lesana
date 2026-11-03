use super::{interfaces::AbstractUseCase, require_session_usecase::RequireSessionUseCase};
use crate::application::{
    contracts::SourceRequest,
    repositories::{ConversationRepository, SourceRequestRepository},
    utils::errors::ApplicationError,
};
use anyhow::Result;
use uuid::Uuid;

pub struct SourceRequestsUseCase<'a> {
    pub owner: Uuid,
    pub sessions: &'a dyn ConversationRepository,
    pub requests: &'a dyn SourceRequestRepository,
}

impl SourceRequestsUseCase<'_> {
    pub async fn list(&self) -> Result<Vec<SourceRequest>> {
        RequireSessionUseCase::new(self.owner, self.sessions)
            .execute()
            .await?;
        self.requests.list(self.owner).await
    }

    pub async fn enqueue(&self, kind: &str, reference: &str) -> Result<SourceRequest> {
        RequireSessionUseCase::new(self.owner, self.sessions)
            .execute()
            .await?;
        let reference = reference.trim();
        if !matches!(
            kind,
            "legislation"
                | "judgment"
                | "eu_regulation"
                | "supreme_court_judgment"
                | "supreme_court_collection"
                | "czech_supreme_court_judgment"
        ) {
            return Err(ApplicationError::InvalidInput(
                "Vyberte slovenský predpis, rozsudok súdov EÚ, rozhodnutie alebo zbierku Najvyššieho súdu SR, rozhodnutie Najvyššieho súdu ČR alebo nariadenie EÚ.",
            )
            .into());
        }
        if reference.is_empty() || reference.len() > 1000 || reference.chars().any(char::is_control)
        {
            return Err(ApplicationError::InvalidInput(
                "Zadajte platné označenie alebo odkaz na zdroj (najviac 1 000 bajtov).",
            )
            .into());
        }
        self.requests.enqueue(self.owner, kind, reference).await
    }
}
