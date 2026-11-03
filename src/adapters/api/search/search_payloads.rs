use chrono::NaiveDate;
use serde::Deserialize;
#[derive(Deserialize)]
pub struct SearchRequest {
    pub q: String,
    pub as_of: Option<NaiveDate>,
}
