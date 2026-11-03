use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Treaty {
    pub celex: String,
    pub language: String,
    pub signed_on: NaiveDate,
    pub published_on: NaiveDate,
    pub legal_context: String,
    pub download_url: String,
    pub expected_convention_articles: usize,
    pub expected_protocol_articles: usize,
    pub expected_interpretation_articles: usize,
}
