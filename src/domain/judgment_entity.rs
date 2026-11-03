use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Judgment {
    pub court: String,
    pub ecli: String,
    pub celex: String,
    pub language: String,
    pub decided_on: NaiveDate,
    pub legal_context: String,
    /// Foreign case law offered only as persuasive support for Slovak analysis.
    #[serde(default)]
    pub supporting_only: bool,
    pub aliases: Vec<String>,
    pub expected_points: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_pages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<JudgmentCollection>,
}

/// Provenance of a published collection entry; it may be an edited excerpt.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct JudgmentCollection {
    pub issue: String,
    pub year: i32,
    pub number: u32,
    pub first_page: usize,
    pub last_page: usize,
    pub ocr: bool,
}
