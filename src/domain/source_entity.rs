use super::{Judgment, Regulation, Treaty};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMethod {
    Fulltext,
    Embedding,
    Exact,
    Related,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: i64,
    pub anchor: String,
    pub kind: String,
    pub citation: String,
    pub heading: String,
    pub text: String,
    pub context: String,
    pub source_url: String,
    pub pdf_url: String,
    pub act_title: String,
    #[serde(default)]
    pub judgment: Option<Judgment>,
    #[serde(default)]
    pub treaty: Option<Treaty>,
    #[serde(default)]
    pub regulation: Option<Regulation>,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
    pub verified_on: NaiveDate,
    // Query provenance belongs to the answer snapshot, not to the legal node.
    // Old snapshots have no provenance; never infer it from today's settings.
    #[serde(default)]
    pub retrieval_methods: Vec<RetrievalMethod>,
}

impl Source {
    pub fn extend_retrieval_methods(&mut self, methods: &[RetrievalMethod]) {
        self.retrieval_methods.extend_from_slice(methods);
        self.retrieval_methods.sort_unstable();
        self.retrieval_methods.dedup();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReferenceLink {
    pub href: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    All,
    Legislation,
    Judgments,
}
