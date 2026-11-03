use super::{db_mappers::storage_error, embedding_index::Embeddings, search};
use crate::application::repositories::*;
use crate::domain::{ReferenceLink, Source, SourceKind};
use anyhow::Result;
use chrono::NaiveDate;
use sqlx::PgPool;
#[derive(Clone)]
pub struct PgLegalRepository {
    pool: PgPool,
    embeddings: Option<Embeddings>,
}
impl PgLegalRepository {
    pub fn new(pool: PgPool, embeddings: Option<Embeddings>) -> Self {
        Self { pool, embeddings }
    }
}
impl LegalRepository for PgLegalRepository {
    fn validate_query(&self, query: &str) -> Result<()> {
        search::validate_query(query)
    }
    fn valid_act(&self, name: &str) -> bool {
        search::valid_act(name)
    }
    fn available(&self, date: NaiveDate) -> PortFuture<'_, bool> {
        Box::pin(async move {
            search::available(&self.pool, date)
                .await
                .map_err(storage_error)
        })
    }
    fn search<'a>(
        &'a self,
        query: &'a str,
        date: NaiveDate,
        limit: i64,
        kind: SourceKind,
    ) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::search_kind(
                &self.pool,
                self.embeddings.as_ref(),
                query,
                date,
                limit,
                kind,
            )
            .await
            .map_err(storage_error)
        })
    }
    fn search_concepts<'a>(
        &'a self,
        query: &'a str,
        date: NaiveDate,
        limit: i64,
    ) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::search_concepts(&self.pool, self.embeddings.as_ref(), query, date, limit)
                .await
                .map_err(storage_error)
        })
    }
    fn get_node(&self, id: i64) -> PortFuture<'_, Option<Source>> {
        Box::pin(async move {
            search::get_node(&self.pool, id)
                .await
                .map_err(storage_error)
        })
    }
    fn get_section<'a>(
        &'a self,
        section: &'a str,
        date: NaiveDate,
        act: Option<&'a str>,
    ) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::get_section(&self.pool, section, date, act)
                .await
                .map_err(storage_error)
        })
    }
    fn get_judgment<'a>(&'a self, query: &'a str, date: NaiveDate) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::get_judgment(&self.pool, query, date)
                .await
                .map_err(storage_error)
        })
    }
    fn containing_provision(&self, id: i64, date: NaiveDate) -> PortFuture<'_, Option<Source>> {
        Box::pin(async move {
            search::containing_provision(&self.pool, id, date)
                .await
                .map_err(storage_error)
        })
    }
    fn reference_links(&self, id: i64) -> PortFuture<'_, Vec<ReferenceLink>> {
        Box::pin(async move {
            search::reference_links(&self.pool, id)
                .await
                .map_err(storage_error)
        })
    }
    fn section_range<'a>(
        &'a self,
        start: &'a str,
        end: &'a str,
        act: &'a str,
        date: NaiveDate,
    ) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::section_range(&self.pool, start, end, act, date)
                .await
                .map_err(storage_error)
        })
    }
    fn get_provision<'a>(
        &'a self,
        anchor: &'a str,
        date: NaiveDate,
        act: Option<&'a str>,
    ) -> PortFuture<'a, Vec<Source>> {
        Box::pin(async move {
            search::get_provision(&self.pool, anchor, date, act)
                .await
                .map_err(storage_error)
        })
    }
}
