use super::interfaces::AbstractUseCase;
use crate::application::utils::errors::ApplicationError;
use crate::{
    application::repositories::LegalRepository,
    domain::{Source, SourceKind},
};
use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

pub struct SearchUseCase<'a> {
    query: &'a str,
    date: NaiveDate,
    repository: &'a dyn LegalRepository,
}
impl<'a> SearchUseCase<'a> {
    pub fn new(query: &'a str, date: NaiveDate, repository: &'a dyn LegalRepository) -> Self {
        Self {
            query,
            date,
            repository,
        }
    }
}
impl AbstractUseCase for SearchUseCase<'_> {
    type Output = SearchResult;
    async fn execute(&self) -> Result<Self::Output> {
        self.repository.validate_query(self.query).map_err(|_| {
            ApplicationError::InvalidInput(
                "Uveďte každý paragraf a odsek samostatne so skratkou zákona, najviac 20 odkazov.",
            )
        })?;
        if self.query.trim().is_empty() || self.query.len() > 1000 {
            return Err(ApplicationError::InvalidInput("Zadajte text do 1 000 bajtov.").into());
        }
        Ok(SearchResult {
            sources: self
                .repository
                .search(self.query, self.date, 12, SourceKind::All)
                .await?,
            as_of: self.date,
            covered: self.repository.available(self.date).await?,
        })
    }
}

#[derive(Serialize)]
pub struct SearchResult {
    pub sources: Vec<Source>,
    pub as_of: NaiveDate,
    pub covered: bool,
}
