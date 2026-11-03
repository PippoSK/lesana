use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
/// Original Official Journal text; publication does not establish applicability.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Regulation {
    pub celex: String,
    pub language: String,
    pub published_on: NaiveDate,
    pub legal_context: String,
}
