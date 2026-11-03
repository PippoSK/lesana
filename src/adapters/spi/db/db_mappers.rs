use super::models::SourceRow;
use crate::application::repositories::StorageFailure;
use crate::domain::Source;
use sqlx::{FromRow, Row};
pub(super) fn storage_error(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<sqlx::Error>().is_some() {
        error.context(StorageFailure)
    } else {
        error
    }
}

// Decode database fields into domain entities at the adapter boundary.

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SourceRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self(Source {
            id: row.try_get("id")?,
            anchor: row.try_get("anchor")?,
            kind: row.try_get("kind")?,
            citation: row.try_get("citation")?,
            heading: row.try_get("heading")?,
            text: row.try_get("text")?,
            context: row.try_get("context")?,
            source_url: row.try_get("source_url")?,
            pdf_url: row.try_get("pdf_url")?,
            act_title: row.try_get("act_title")?,
            judgment: row
                .try_get::<Option<sqlx::types::Json<crate::domain::Judgment>>, _>("judgment")?
                .map(|v| v.0),
            treaty: row
                .try_get::<Option<sqlx::types::Json<crate::domain::Treaty>>, _>("treaty")?
                .map(|v| v.0),
            regulation: row
                .try_get::<Option<sqlx::types::Json<crate::domain::Regulation>>, _>("regulation")?
                .map(|v| v.0),
            valid_from: row.try_get("valid_from")?,
            valid_to: row.try_get("valid_to")?,
            verified_on: row.try_get("verified_on")?,
            retrieval_methods: Vec::new(),
        }))
    }
}
