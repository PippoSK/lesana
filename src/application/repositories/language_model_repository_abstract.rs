use super::{PortFuture, ResearchOutcome, ResearchTools};
use crate::application::contracts::{CompletionKind, EvidenceReviewConfig, ModelMessage};
use std::sync::Arc;

pub trait LanguageModel: Send + Sync {
    fn turn_timeout_seconds(&self) -> u64 {
        300
    }

    fn completion_timeout_seconds(&self) -> u64 {
        120
    }

    fn evidence_review_config(&self) -> EvidenceReviewConfig {
        EvidenceReviewConfig::default()
    }

    fn complete<'a>(
        &'a self,
        messages: &'a [ModelMessage],
        kind: CompletionKind,
    ) -> PortFuture<'a, ModelMessage> {
        self.complete_with_limit(messages, kind, 12000)
    }
    fn complete_with_limit<'a>(
        &'a self,
        messages: &'a [ModelMessage],
        kind: CompletionKind,
        max_tokens: u32,
    ) -> PortFuture<'a, ModelMessage>;
    fn research<'a>(
        &'a self,
        tools: Arc<dyn ResearchTools>,
        system: &'a str,
        history: &'a [ModelMessage],
        question: &'a str,
        max_turns: usize,
    ) -> PortFuture<'a, ResearchOutcome>;
}
