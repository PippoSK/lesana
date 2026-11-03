use super::http_language_model::Model;
use crate::application::{
    contracts::{ModelMessage, ResearchRequest, ResearchResult},
    repositories::{ResearchOutcome, ResearchTools},
    usecases::retrieval::ToolFailure,
    utils::errors::ModelFailure,
};
use anyhow::Result;
use chrono::NaiveDate;
use rig::{
    AgentBuilder,
    agent::{AgentHook, HookContext, ModelTurnAction, ModelTurnFinished},
    completion::{FinishReason, PromptError},
    tool::{Tool, ToolContext},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub async fn run(
    model: &Model,
    tools: Arc<dyn ResearchTools>,
    system: &str,
    history: &[ModelMessage],
    question: &str,
    max_turns: usize,
) -> Result<ResearchOutcome> {
    let started = Instant::now();
    let deadline_seconds = if model.uses_max_reasoning_profile() {
        600
    } else {
        180
    };
    let result = tokio::time::timeout(Duration::from_secs(deadline_seconds), async {
        let result = run_attempt(
            model, tools.clone(), system, history, question, max_turns, 1,
        )
        .await;
        if model.uses_max_reasoning_profile()
            && result.as_ref().is_err_and(|error| {
                matches!(error.downcast_ref::<ModelFailure>(), Some(ModelFailure::OutputLimit))
            })
        {
            tracing::warn!(model = %model.name, attempt = 2, max_tokens = 64_000, "Retrying truncated research from original inputs with a larger budget");
            // Retain shared retrieval budgets, but discard every message from
            // the failed run, including its truncated tool calls and reasoning.
            run_attempt(model, tools, system, history, question, max_turns, 2).await
        } else {
            result
        }
    })
    .await;
    tracing::info!(model = %model.name, elapsed_ms = started.elapsed().as_millis() as u64, deadline_seconds, timed_out = result.is_err(), "Rig research finished");
    result.map_err(|_| ModelFailure::Timeout)?
}

async fn run_attempt(
    model: &Model,
    tools: Arc<dyn ResearchTools>,
    system: &str,
    history: &[ModelMessage],
    question: &str,
    max_turns: usize,
    attempt: u8,
) -> Result<ResearchOutcome> {
    let max_tokens = model.research_max_tokens(attempt);
    let mut builder = AgentBuilder::new(model.provider.completion(&model.name))
            .preamble(format!("{system}\nTeraz vykonaj iba rešerš pomocou nástrojov. V každom ťahu začínaš bez vopred vyhľadaných zdrojov, aj keď história obsahuje odpoveď alebo rovnakú otázku. Predchádzajúce citácie nie sú aktuálnym katalógom. Pre aktuálne doplnenie vždy zavolaj aspoň jeden nástroj; podľa histórie rozpoznaj prípad a nežiadaj opäť už uvedené okolnosti. Vyber predpisy alebo judikatúru podľa otázky a predchádzajúceho rozhovoru. Pred skončením vyžiadaj rozhodujúce podklady. Keď sú podklady pripravené, odpovedz stručne 'Rešerš dokončená'; právnu odpoveď následne vytvorí samostatný krok."))
            .tool(SearchNorms(tools.clone()))
            .tool(SearchJudgments(tools.clone()))
            .tool(GetSection(tools.clone()))
            .tool(GetJudgment(tools.clone()))
            .add_hook(ResearchHook { started: Instant::now(), attempt, max_tokens });
    if let Some(parameters) = model.research_parameters() {
        builder = builder.additional_params(parameters);
    }
    if let Some(max_tokens) = max_tokens {
        builder = builder.max_tokens(max_tokens);
    }
    let agent = builder.build();
    let result = agent
        .prompt(question)
        .history(super::http_mappers::messages(history))
        .max_turns(max_turns)
        .tool_concurrency(1)
        .run()
        .await;
    match result {
        Ok(_) => Ok(ResearchOutcome::Complete),
        Err(PromptError::PromptCancelled { reason, .. })
            if reason == "lesana_research_output_limit" =>
        {
            Err(ModelFailure::OutputLimit.into())
        }
        Err(PromptError::MaxTurnsError { .. }) => Ok(ResearchOutcome::TurnLimit),
        Err(error) => Err(super::http_connection::prompt_failure(error).into()),
    }
}
// A truncated model turn can contain valid-looking text or tool calls.
// Stop before Rig executes those calls or considers the research complete.
struct ResearchHook {
    started: Instant,
    attempt: u8,
    max_tokens: Option<u64>,
}
impl AgentHook for ResearchHook {
    async fn on_model_turn_finished(
        &self,
        _: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> ModelTurnAction {
        tracing::info!(attempt = self.attempt, max_tokens = ?self.max_tokens, turn = event.turn, elapsed_ms = self.started.elapsed().as_millis() as u64, finish_reason = ?event.finish_reason, output_tokens = ?event.raw.pointer("/usage/completion_tokens").or_else(|| event.raw.pointer("/usage/output_tokens")).and_then(|value| value.as_u64()), reasoning_tokens = ?event.raw.pointer("/usage/completion_tokens_details/reasoning_tokens").or_else(|| event.raw.pointer("/usage/output_tokens_details/reasoning_tokens")).and_then(|value| value.as_u64()), provider = event.raw.get("provider").and_then(|value| value.as_str()).unwrap_or("unknown"), "Rig research turn finished");
        if event.finish_reason == Some(&FinishReason::Length) {
            ModelTurnAction::stop("lesana_research_output_limit")
        } else {
            ModelTurnAction::Continue
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    query: String,
    as_of: Option<NaiveDate>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionArgs {
    section: String,
    act: String,
    as_of: Option<NaiveDate>,
}

fn date_parameter() -> Value {
    json!({"type":["string","null"],"format":"date","description":"Optional research date in YYYY-MM-DD, no later than the current answer date. Use the relevant event date from the question/history for historical legislation. Omit or use null for the current answer date. Each call filters sources and linked context to this date; it does not change the current answer date."})
}

macro_rules! query_tool {
    ($tool:ident, $name:literal, $description:literal) => {
        pub struct $tool(Arc<dyn ResearchTools>);
        impl Tool for $tool {
            const NAME: &'static str = $name;
            type Args = QueryArgs;
            type Output = Value;
            type Error = ToolFailure;
            fn description(&self) -> String { $description.into() }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{"query":{"type":"string"},"as_of":date_parameter()},"required":["query"],"additionalProperties":false})
            }
            async fn call(&self, _: &mut ToolContext, args: QueryArgs) -> Result<Value, ToolFailure> {
                self.0.execute(ResearchRequest::$tool { query: args.query, as_of: args.as_of }).await.map(tool_output)
            }
        }
    };
}
query_tool!(
    SearchNorms,
    "search_norms",
    "Search only imported legislation by Slovak legal concepts or exact references, e.g. § 181 SSP. Use for statutory definitions, rules and exceptions. Set as_of to the relevant historical date from the question/history when checking an earlier event; otherwise the current answer date is used. Linked context uses the same research date. An empty result only establishes a gap for that query and date."
);
query_tool!(
    SearchJudgments,
    "search_judgments",
    "Search only imported case law by legal concepts, party name, case number, ECLI, collection citation or specific points/pages, e.g. C-464/01 body 39–41, R 20/2002 právna veta or R 20/2002 strana 9. Collection pages use original PDF page numbers. Does not search statutes or the public web. Defaults to the current answer date; set as_of only to limit case law to an earlier research date."
);
query_tool!(
    GetJudgment,
    "get_judgment",
    "Read the imported text of one judgment or collection item, identified by case number, ECLI, CELEX, party name or collection citation, e.g. C-464/01, 4Ndc/31/2024 or R 20/2002. A collection item contains the published text, possibly an excerpt or stanovisko; do not assume a complete decision or operative part. Slovak PDF citations use original pages. Use after searching for case law. Defaults to the current answer date; as_of optionally limits availability to an earlier date."
);

pub struct GetSection(Arc<dyn ResearchTools>);
impl Tool for GetSection {
    const NAME: &'static str = "get_section";
    type Args = SectionArgs;
    type Output = Value;
    type Error = ToolFailure;
    fn description(&self) -> String {
        "Read a whole section including paragraphs, conditions and exceptions. Both section (e.g. 181 or 493c) and an unambiguous act (e.g. SSP, CSP, OZ, 162/2015) are required. Set as_of to the relevant historical event date from the conversation when checking older legislation. Without as_of the current answer date is used. For articles use search_norms.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"section":{"type":"string"},"act":{"type":"string"},"as_of":date_parameter()},"required":["section","act"],"additionalProperties":false})
    }
    async fn call(&self, _: &mut ToolContext, args: SectionArgs) -> Result<Value, ToolFailure> {
        self.0
            .execute(ResearchRequest::GetSection {
                section: args.section,
                act: args.act,
                as_of: args.as_of,
            })
            .await
            .map(tool_output)
    }
}

// Provider-facing serialization stays in this adapter. Retrieval metadata must
// never influence the model's assessment of the legal text.
fn tool_output(result: ResearchResult) -> Value {
    match result {
        ResearchResult::InvalidArguments => {
            json!({"error":"Invalid arguments. Use an unambiguous act or judgment identifier and the documented schema. as_of must be a valid YYYY-MM-DD date no later than the current answer date."})
        }
        ResearchResult::Sources {
            as_of,
            sources,
            context_issues,
        } => {
            let sources: Vec<_> = sources
                .iter()
                .map(|source| {
                    let mut value = json!(source);
                    value
                        .as_object_mut()
                        .expect("source object")
                        .remove("retrieval_methods");
                    value
                })
                .collect();
            json!({"as_of":as_of,"sources":sources,"context_issues":context_issues})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_dates_are_optional_validated_and_reported_for_empty_results() {
        let old: SectionArgs =
            serde_json::from_value(json!({"section":"30","act":"566/2001"})).unwrap();
        assert!(old.as_of.is_none());
        let historical: SectionArgs =
            serde_json::from_value(json!({"section":"30","act":"566/2001","as_of":"2023-09-29"}))
                .unwrap();
        let date = historical.as_of.unwrap();
        assert_eq!(date.to_string(), "2023-09-29");
        assert!(
            serde_json::from_value::<QueryArgs>(
                json!({"query":"§ 30 566/2001","as_of":"2023-02-30"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<QueryArgs>(
                json!({"query":"§ 30 566/2001","date":"2023-09-29"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<QueryArgs>(json!({"query":"§ 30 566/2001","as_of":null}))
                .unwrap()
                .as_of
                .is_none()
        );
        let output = tool_output(ResearchResult::Sources {
            as_of: date,
            sources: vec![],
            context_issues: vec![],
        });
        assert_eq!(output["as_of"], "2023-09-29");
        assert_eq!(output["sources"], json!([]));
    }
}
