use super::PortFuture;
use crate::domain::{ReferenceLink, Source, SourceKind};
use anyhow::Result;
use chrono::NaiveDate;

pub trait LegalRepository: Send + Sync {
    fn validate_query(&self, query: &str) -> Result<()>;
    fn valid_act(&self, name: &str) -> bool;
    fn available(&self, date: NaiveDate) -> PortFuture<'_, bool>;
    fn search<'a>(
        &'a self,
        query: &'a str,
        date: NaiveDate,
        limit: i64,
        kind: SourceKind,
    ) -> PortFuture<'a, Vec<Source>>;
    fn search_concepts<'a>(
        &'a self,
        query: &'a str,
        date: NaiveDate,
        limit: i64,
    ) -> PortFuture<'a, Vec<Source>>;
    fn get_node(&self, id: i64) -> PortFuture<'_, Option<Source>>;
    fn get_section<'a>(
        &'a self,
        section: &'a str,
        date: NaiveDate,
        act: Option<&'a str>,
    ) -> PortFuture<'a, Vec<Source>>;
    fn get_judgment<'a>(&'a self, query: &'a str, date: NaiveDate) -> PortFuture<'a, Vec<Source>>;
    fn containing_provision(&self, id: i64, date: NaiveDate) -> PortFuture<'_, Option<Source>>;
    fn reference_links(&self, id: i64) -> PortFuture<'_, Vec<ReferenceLink>>;
    fn section_range<'a>(
        &'a self,
        start: &'a str,
        end: &'a str,
        act: &'a str,
        date: NaiveDate,
    ) -> PortFuture<'a, Vec<Source>>;
    fn get_provision<'a>(
        &'a self,
        anchor: &'a str,
        date: NaiveDate,
        act: Option<&'a str>,
    ) -> PortFuture<'a, Vec<Source>>;
}
