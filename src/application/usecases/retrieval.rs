use crate::application::{
    contracts::{ModelMessage, ResearchRequest, ResearchResult},
    repositories::*,
    usecases::context::EvidenceContext,
    utils::errors::ModelFailure,
    utils::events::{Events, ProgressEvent, status},
};
use crate::domain::{Source, SourceKind};
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    Pipeline,
    #[default]
    Agentic,
}
impl Architecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
            Self::Agentic => "agentic",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub default: Architecture,
    pub max_turns: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<NaiveDate>,
    pub source_count: usize,
    pub outcome: String,
}
#[derive(Default)]
struct Research {
    context: EvidenceContext,
    calls: Vec<ToolCall>,
}
#[derive(Clone)]
pub struct Session {
    repository: Arc<dyn LegalRepository>,
    date: NaiveDate,
    events: Events,
    research: Arc<Mutex<Research>>,
}
impl Session {
    pub fn new(repository: Arc<dyn LegalRepository>, date: NaiveDate, events: &Events) -> Self {
        Self {
            repository,
            date,
            events: events.clone(),
            research: Arc::new(Mutex::new(Research::default())),
        }
    }
    pub async fn calls(&self) -> Vec<ToolCall> {
        self.research.lock().await.calls.clone()
    }
    pub async fn take_context(&self) -> EvidenceContext {
        std::mem::take(&mut self.research.lock().await.context)
    }

    pub async fn execute(&self, request: ResearchRequest) -> Result<ResearchResult, ToolFailure> {
        let name = request.name();
        let date = request.as_of().unwrap_or(self.date);
        // Rig executes sequentially. Hold this lock also as a hard guard if a
        // future caller enables concurrent tools. The budget includes invalid calls.
        let mut research = self.research.lock().await;
        if research.calls.len() >= 24 {
            return Err(ToolFailure);
        }
        let index = research.calls.len();
        research.calls.push(ToolCall {
            name: name.into(),
            as_of: Some(date),
            source_count: 0,
            outcome: "invalid_arguments".into(),
        });
        status(
            &self.events,
            &format!("Agent volá {name} · stav k {date} · krok {}…", index + 1),
        )
        .await;
        let found: Result<Vec<Source>> = async {
            ensure!(date <= self.date, "Research date exceeds the selected date");
            match &request {
                ResearchRequest::SearchNorms { query, .. }
                | ResearchRequest::SearchJudgments { query, .. } => {
                    let query = query.trim();
                    ensure!(
                        !query.is_empty() && query.chars().count() <= 1000,
                        "Invalid query"
                    );
                    self.repository.validate_query(query)?;
                    let kind = if name == "search_norms" {
                        SourceKind::Legislation
                    } else {
                        SourceKind::Judgments
                    };
                    self.repository.search(query, date, 6, kind).await
                }
                ResearchRequest::GetSection { section, act, .. } => {
                    ensure!(
                        regex::Regex::new(r"^\d{1,4}[a-z]{0,2}$")
                            .unwrap()
                            .is_match(section),
                        "Invalid section"
                    );
                    ensure!(self.repository.valid_act(act), "Specify one imported act");
                    self.repository.get_section(section, date, Some(act)).await
                }
                ResearchRequest::GetJudgment { query, .. } => {
                    let query = query.trim();
                    ensure!(
                        !query.is_empty() && query.chars().count() <= 250,
                        "Invalid judgment identifier"
                    );
                    self.repository.get_judgment(query, date).await
                }
            }
        }
        .await;
        let found = match found {
            Ok(found) => found,
            Err(error) if error.downcast_ref::<StorageFailure>().is_some() => {
                research.calls[index].outcome = "failed".into();
                return Err(ToolFailure);
            }
            Err(_) => {
                return Ok(ResearchResult::InvalidArguments);
            }
        };
        let added = research
            .context
            .add(self.repository.as_ref(), found, date)
            .await
            .map_err(|_| ToolFailure)?;
        research.calls[index].source_count = added.len();
        research.calls[index].outcome = "ok".into();
        self.events
            .emit(ProgressEvent::Tool(research.calls[index].clone()))
            .await;
        Ok(ResearchResult::Sources {
            as_of: date,
            sources: added,
            context_issues: research.context.issues.clone(),
        })
    }

    pub async fn agentic(
        &self,
        model: &dyn LanguageModel,
        system: &str,
        history: &[ModelMessage],
        question: &str,
        max_turns: usize,
    ) -> Result<()> {
        status(&self.events, "Agent vyberá nástroje na právnu rešerš…").await;
        let mut outcome = model
            .research(Arc::new(self.clone()), system, history, question, max_turns)
            .await?;
        // A text-only completion has consumed one model turn but has not searched.
        // Retry once with the same conversation, within the original turn budget.
        if self.calls().await.is_empty()
            && matches!(outcome, ResearchOutcome::Complete)
            && max_turns > 1
        {
            status(
                &self.events,
                "Dopĺňam vyhľadávanie podkladov k pokračovaniu rozhovoru…",
            )
            .await;
            let retry_system = format!(
                "{system}\nPredchádzajúci pokus nevykonal žiadne vyhľadávanie. História vysvetľuje aktuálne doplnenie používateľa, ale nenahrádza právne podklady. Pre túto správu musíš použiť aspoň jeden vyhľadávací nástroj a preveriť relevantné zdroje k zvolenému dátumu. Samotná textová odpoveď rešerš neukončí."
            );
            outcome = model
                .research(
                    Arc::new(self.clone()),
                    &retry_system,
                    history,
                    question,
                    max_turns - 1,
                )
                .await?;
        }
        // Only a valid lookup (including an empty result) can establish no sources.
        // Skipped research and invalid arguments are technical failures; the turn
        // must roll back instead of storing a misleading answer in the history.
        if !self.calls().await.iter().any(|call| call.outcome == "ok") {
            return Err(ModelFailure::InvalidResponse.into());
        }
        if matches!(outcome, ResearchOutcome::TurnLimit) {
            status(
                &self.events,
                "Dosiahnutý limit rešerše; vyhodnocujem získané podklady…",
            )
            .await;
            self.research
                .lock()
                .await
                .context
                .issues
                .push(super::context::ContextIssue {
                    as_of: self.date,
                    source_id: 0,
                    target: String::new(),
                    reason: "agent_turn_limit",
                });
        }
        Ok(())
    }
}
impl ResearchTools for Session {
    fn execute<'a>(
        &'a self,
        request: ResearchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ResearchResult, ToolFailure>> + Send + 'a>,
    > {
        Box::pin(Session::execute(self, request))
    }
}
#[derive(Debug)]
pub struct ToolFailure;
impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Legal retrieval failed or exceeded its tool budget")
    }
}
impl std::error::Error for ToolFailure {}
