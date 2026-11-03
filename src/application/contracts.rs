//! Typed data crossing application boundaries; no provider or persistence types.
use crate::application::{
    usecases::context::ContextIssue,
    usecases::retrieval::{Architecture, ToolCall},
};
use crate::domain::{Judgment, Regulation, Source, Treaty, evidence::Claim};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

/// The application declares the expected response contract independently of provider syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Answer,
    EvidenceReview,
}

/// Bounded review work; the adapter accounts for its model's reasoning needs.
#[derive(Clone, Copy, Debug)]
pub struct EvidenceReviewConfig {
    pub claims_per_call: usize,
    pub initial_tokens: u32,
    pub retry_tokens: u32,
}

impl Default for EvidenceReviewConfig {
    fn default() -> Self {
        Self {
            claims_per_call: 3,
            initial_tokens: 6000,
            retry_tokens: 12000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: MessageRole,
    pub content: String,
}
impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Serialize)]
pub struct CreatedConversation {
    pub id: Uuid,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub text_chars: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentContext {
    pub id: Uuid,
    pub filename: String,
    pub text: String,
}

pub struct AttachmentDownload {
    pub metadata: AttachmentMetadata,
    pub bytes: Vec<u8>,
}

pub struct PreparedAttachment {
    pub metadata: AttachmentMetadata,
    pub bytes: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub role: MessageRole,
    pub content: String,
    pub sources: Vec<Source>,
    pub claims: Vec<Claim>,
    pub as_of: NaiveDate,
    pub mode: String,
    // Historical replies predate the Rig architecture selection.
    pub architecture: Option<Architecture>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CorpusVersion {
    pub title: String,
    pub number: String,
    pub judgment: Option<Judgment>,
    pub treaty: Option<Treaty>,
    #[serde(default)]
    pub regulation: Option<Regulation>,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
    pub verified_on: NaiveDate,
    pub source_url: String,
    pub pdf_url: String,
    pub sha256: String,
    pub nodes: i64,
    pub sections: i64,
    pub paragraphs: i64,
    pub letters: i64,
    pub points: i64,
    pub articles: i64,
}

#[derive(Debug)]
pub enum EmbeddingStatus {
    Disabled,
    Enabled {
        model: String,
        indexed_nodes: i64,
        total_nodes: i64,
        ready: bool,
    },
}
impl Serialize for EmbeddingStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            Self::Disabled => {
                let mut state = serializer.serialize_struct("EmbeddingStatus", 1)?;
                state.serialize_field("enabled", &false)?;
                state.end()
            }
            Self::Enabled {
                model,
                indexed_nodes,
                total_nodes,
                ready,
            } => {
                let mut state = serializer.serialize_struct("EmbeddingStatus", 5)?;
                state.serialize_field("enabled", &true)?;
                state.serialize_field("model", model)?;
                state.serialize_field("indexed_nodes", indexed_nodes)?;
                state.serialize_field("total_nodes", total_nodes)?;
                state.serialize_field("ready", ready)?;
                state.end()
            }
        }
    }
}
pub struct CorpusSnapshot {
    pub versions: Vec<CorpusVersion>,
    pub embeddings: EmbeddingStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceRequest {
    pub id: uuid::Uuid,
    pub kind: String,
    pub reference: String,
    pub status: String,
    pub message: Option<String>,
    pub act_id: Option<String>,
    pub title: Option<String>,
    pub node_count: Option<i64>,
    pub imported_count: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub enum ResearchRequest {
    SearchNorms {
        query: String,
        as_of: Option<NaiveDate>,
    },
    SearchJudgments {
        query: String,
        as_of: Option<NaiveDate>,
    },
    GetSection {
        section: String,
        act: String,
        as_of: Option<NaiveDate>,
    },
    GetJudgment {
        query: String,
        as_of: Option<NaiveDate>,
    },
}
impl ResearchRequest {
    pub fn as_of(&self) -> Option<NaiveDate> {
        match self {
            Self::SearchNorms { as_of, .. }
            | Self::SearchJudgments { as_of, .. }
            | Self::GetSection { as_of, .. }
            | Self::GetJudgment { as_of, .. } => *as_of,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::SearchNorms { .. } => "search_norms",
            Self::SearchJudgments { .. } => "search_judgments",
            Self::GetSection { .. } => "get_section",
            Self::GetJudgment { .. } => "get_judgment",
        }
    }
}
pub enum ResearchResult {
    Sources {
        as_of: NaiveDate,
        sources: Vec<Source>,
        context_issues: Vec<ContextIssue>,
    },
    InvalidArguments,
}
