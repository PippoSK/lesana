//! A complete conversation turn, independent of persistence and model providers.
use crate::application::{
    repositories::*,
    usecases::agent::{self, Request, RunAnswer},
    usecases::retrieval::{Architecture, Config},
    utils::errors::ApplicationError,
    utils::events::Events,
};
use anyhow::Result;
use chrono::NaiveDate;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

pub struct ChatService {
    pub(super) legal: Arc<dyn LegalRepository>,
    pub(super) model: Option<Arc<dyn LanguageModel>>,
    pub(super) turns: Arc<dyn TurnRepository>,
    pub(super) config: Config,
}
impl ChatService {
    pub fn new(
        legal: Arc<dyn LegalRepository>,
        model: Option<Arc<dyn LanguageModel>>,
        turns: Arc<dyn TurnRepository>,
        config: Config,
    ) -> Self {
        Self {
            legal,
            model,
            turns,
            config,
        }
    }
    pub fn validate_question(&self, question: &str) -> Result<()> {
        self.legal.validate_query(question).map_err(|_| {
            ApplicationError::InvalidInput(
                "Uveďte každý paragraf a odsek samostatne so skratkou zákona, najviac 20 odkazov.",
            )
        })?;
        if question.trim().is_empty() || question.chars().count() > 6000 {
            return Err(
                ApplicationError::InvalidInput("Otázka musí mať 1 až 6 000 znakov.").into(),
            );
        }
        Ok(())
    }
    pub fn turn_timeout_seconds(&self) -> u64 {
        self.model
            .as_deref()
            .map_or(300, LanguageModel::turn_timeout_seconds)
    }
    pub async fn process(
        &self,
        owner: Uuid,
        conversation: Uuid,
        question: &str,
        date: NaiveDate,
        architecture: Architecture,
        events: &Events,
    ) -> Result<RunAnswer> {
        ChatUseCase::new(
            ChatRequest {
                owner,
                conversation,
                question,
                date,
                architecture,
                events,
                deadline: Instant::now() + Duration::from_secs(self.turn_timeout_seconds()),
            },
            self,
        )
        .execute()
        .await
    }
}
#[derive(Clone, Copy)]
pub struct ChatRequest<'a> {
    pub owner: Uuid,
    pub conversation: Uuid,
    pub question: &'a str,
    pub date: NaiveDate,
    pub architecture: Architecture,
    pub events: &'a Events,
    pub deadline: Instant,
}
pub struct ChatUseCase<'a> {
    request: ChatRequest<'a>,
    service: &'a ChatService,
}
impl<'a> ChatUseCase<'a> {
    pub fn new(request: ChatRequest<'a>, service: &'a ChatService) -> Self {
        Self { request, service }
    }
}
use super::interfaces::AbstractUseCase;
impl AbstractUseCase for ChatUseCase<'_> {
    type Output = RunAnswer;
    async fn execute(&self) -> Result<RunAnswer> {
        let ChatRequest {
            owner,
            conversation,
            question,
            date,
            architecture,
            events,
            deadline,
        } = self.request;
        let question = question.trim();
        self.service.validate_question(question)?;
        let mut turn = self.service.turns.begin_turn(owner, conversation).await?;
        let history = turn.history().await?;
        let attachments = turn.attachments().await?;
        let answer = agent::answer(
            self.service.legal.clone(),
            self.service.model.as_deref(),
            Request {
                question,
                history,
                attachments,
                date,
                architecture,
                deadline,
            },
            &self.service.config,
            events,
        )
        .await?;
        turn.commit(question, &answer).await?;
        Ok(answer)
    }
}
