//! Rig implementation of the application language-model port.
use super::{
    http_connection::{ProviderKind, provider_failure, provider_with_route},
    http_mappers::messages,
};
use crate::application::{
    contracts::{CompletionKind, EvidenceReviewConfig, ModelMessage},
    repositories::{LanguageModel, PortFuture, ResearchOutcome, ResearchTools},
    utils::errors::ModelFailure,
};
use anyhow::{Result, ensure};
use rig::{
    completion::{CompletionRequest, FinishReason},
    providers::openai::{OpenAI, Route},
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Model {
    pub name: String,
    pub provider: OpenAI,
    kind: ProviderKind,
}
impl Model {
    pub fn from_env() -> Result<Option<Self>> {
        let base_url = std::env::var("LLM_BASE_URL").unwrap_or_default();
        let name = std::env::var("LLM_MODEL").unwrap_or_default();
        if base_url.is_empty() && name.is_empty() {
            return Ok(None);
        }
        ensure!(
            !base_url.is_empty() && !name.is_empty(),
            "Set both LLM_BASE_URL and LLM_MODEL"
        );
        let kind = ProviderKind::from_config(
            &std::env::var("LLM_PROVIDER").unwrap_or_default(),
            &base_url,
        )?;
        let compatible_key = std::env::var("LLM_API_KEY").unwrap_or_default();
        let openrouter_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        Ok(Some(Self {
            provider: provider_with_route(
                &base_url,
                kind.api_key(&compatible_key, &openrouter_key)?,
                Duration::from_secs(completion_timeout_seconds_for(&name, kind) + 5),
                research_route(&name),
                kind,
            )?,
            name,
            kind,
        }))
    }

    pub fn uses_responses_for_research(&self) -> bool {
        matches!(research_route(&self.name), Route::Responses)
    }

    pub fn is_qwen_38(&self) -> bool {
        self.name.eq_ignore_ascii_case("qwen/qwen3.8-27b")
            || self.name.eq_ignore_ascii_case("qwen3.8-27b")
    }

    pub fn is_deepseek_v41(&self) -> bool {
        is_deepseek_v41_name(&self.name)
    }

    pub fn uses_max_reasoning_profile(&self) -> bool {
        uses_max_reasoning_profile(&self.name, self.kind)
    }

    fn uses_responses_for_completion(&self) -> bool {
        is_native_sol(&self.name, self.kind)
    }

    fn reasoning_effort(&self) -> Option<&'static str> {
        if self.is_qwen_38() {
            Some("xhigh")
        } else if self.uses_max_reasoning_profile() {
            Some("max")
        } else if self.uses_responses_for_research() {
            Some("low")
        } else {
            None
        }
    }

    pub fn research_max_tokens(&self, attempt: u8) -> Option<u64> {
        if self.uses_max_reasoning_profile() {
            Some(if attempt == 2 { 64_000 } else { 32_000 })
        } else if self.uses_responses_for_research() {
            Some(12_000)
        } else if self.is_qwen_38() {
            Some(16_384)
        } else {
            None
        }
    }

    pub fn research_parameters(&self) -> Option<serde_json::Value> {
        if self.uses_responses_for_research() {
            Some(json!({"reasoning": {"effort": self.reasoning_effort()}, "store": false}))
        } else if self.is_qwen_38() || self.is_deepseek_v41() {
            Some(self.chat_parameters())
        } else {
            None
        }
    }

    pub fn uses_strict_schema(&self) -> bool {
        self.uses_responses_for_completion()
            || ((self.is_qwen_38() || self.is_deepseek_v41())
                && self.kind == ProviderKind::OpenRouter)
    }

    pub fn chat_parameters(&self) -> serde_json::Value {
        let mut params = json!({});
        if let Some(effort) = self.reasoning_effort() {
            params["reasoning_effort"] = json!(effort);
        }
        if self.is_qwen_38() && self.kind == ProviderKind::OpenRouter {
            // Do not route to providers that silently ignore reasoning or JSON controls.
            params["provider"] = json!({"require_parameters": true, "order": ["cerebras", "modelrun"], "sort": "throughput", "allow_fallbacks": true});
        }
        if self.is_deepseek_v41() && self.kind == ProviderKind::OpenRouter {
            // Keep max reasoning on the providers verified with tools and strict JSON.
            params["provider"] = json!({"require_parameters": true, "order": ["coreweave/fp8", "together"], "only": ["together", "coreweave/fp8"], "allow_fallbacks": true});
        }
        params
    }

    fn completion_request(
        &self,
        values: &[ModelMessage],
        kind: CompletionKind,
        max_tokens: u32,
    ) -> CompletionRequest {
        let mut request = CompletionRequest::new("");
        request.chat_history = messages(values);
        if self.uses_responses_for_completion() {
            // Native Sol accepts max reasoning through Responses. Chat only
            // supports lower efforts, so use the native route in every phase.
            request.max_tokens = Some(u64::from(max_tokens));
            // Rig's Responses wire supports structured JSON schemas, but not
            // json_object. Reuse the ordinary answer/review contract here.
            let mut format = super::response_schema::response_format(kind)["json_schema"].clone();
            format["type"] = json!("json_schema");
            request.additional_params = Some(json!({
                "reasoning": {"effort": self.reasoning_effort()},
                "store": false,
                "text": {"format": format}
            }));
            return request;
        }
        if self.is_deepseek_v41() && kind == CompletionKind::Answer {
            request.chat_history.push(rig::message::Message::system(
                "Výslednú odpoveď sústreď najviac do 6 vecných tvrdení. Vyber rozhodujúce závery a ich podmienky; neopakuj rovnaký záver ani položky evidence. Každý text má najviac 1 800 znakov a najviac 6 dôkazov. Toto obmedzuje iba výsledný JSON, nie dôkladnosť posúdenia podkladov.",
            ));
        }
        let mut params = self.chat_parameters();
        params["max_completion_tokens"] = json!(max_tokens);
        params["response_format"] = if self.uses_strict_schema() && self.is_deepseek_v41() {
            super::response_schema::deepseek_response_format(kind)
        } else if self.uses_strict_schema() {
            super::response_schema::response_format(kind)
        } else {
            json!({"type": "json_object"})
        };
        if self.is_deepseek_v41() && self.uses_strict_schema() {
            // Supply the same schema in the prompt and on the wire so the
            // model sees the exact bounds during both answering and review.
            request.chat_history.push(rig::message::Message::system(format!(
                "Vráť výhradne JSON podľa tejto schémy. Nevypisuj text mimo JSON ani zbytočné prázdne znaky. Schéma výsledného JSON: {}",
                params["response_format"]["json_schema"]["schema"]
            )));
        }
        request.additional_params = Some(params);
        request
    }

    pub async fn complete_with_limit(
        &self,
        values: &[ModelMessage],
        kind: CompletionKind,
        max_tokens: u32,
    ) -> Result<ModelMessage> {
        let request = self.completion_request(values, kind, max_tokens);
        let started = Instant::now();
        let response = tokio::time::timeout(
            Duration::from_secs(self.completion_timeout_seconds()),
            async {
                if self.uses_responses_for_completion() {
                    self.provider.responses(&self.name).call(request).await
                } else {
                    self.provider.chat(&self.name).call(request).await
                }
            },
        )
        .await
        .map_err(|_| ModelFailure::Timeout)?
        .map_err(provider_failure)?;
        let finish = response.finish_reason();
        let content = response.text();
        let json_parse = match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(_) => "valid",
            Err(error) => match error.classify() {
                serde_json::error::Category::Io => "io",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Data => "data",
                serde_json::error::Category::Eof => "eof",
            },
        };
        let whitespace_bytes = content.bytes().filter(u8::is_ascii_whitespace).count();
        let leading_think = content.trim_start().starts_with("<think>");
        // Log limits and outcomes, never prompt, response or reasoning content.
        tracing::info!(model = %self.name, phase = ?kind, max_tokens, finish_reason = ?finish, elapsed_ms = started.elapsed().as_millis() as u64, output_bytes = content.len(), whitespace_bytes, json_parse, leading_think, output_tokens = ?response.usage.output_tokens, reasoning_tokens = ?response.usage.reasoning_tokens, provider = response.raw.get("provider").and_then(|value| value.as_str()).unwrap_or("unknown"), "Rig completion finished");
        if finish == Some(FinishReason::Length) {
            return Err(ModelFailure::OutputLimit.into());
        }
        if !matches!(finish, Some(FinishReason::Stop)) || response.tool_calls().next().is_some() {
            return Err(ModelFailure::InvalidResponse.into());
        }
        Ok(ModelMessage::assistant(content))
    }
}

impl LanguageModel for Model {
    fn turn_timeout_seconds(&self) -> u64 {
        if self.uses_max_reasoning_profile() {
            1800
        } else {
            300
        }
    }

    fn completion_timeout_seconds(&self) -> u64 {
        completion_timeout_seconds_for(&self.name, self.kind)
    }

    fn evidence_review_config(&self) -> EvidenceReviewConfig {
        if self.uses_max_reasoning_profile() {
            EvidenceReviewConfig {
                claims_per_call: if self.is_deepseek_v41() { 1 } else { 3 },
                initial_tokens: 24_000,
                retry_tokens: 48_000,
            }
        } else {
            EvidenceReviewConfig::default()
        }
    }

    fn complete<'a>(
        &'a self,
        messages: &'a [ModelMessage],
        kind: CompletionKind,
    ) -> PortFuture<'a, ModelMessage> {
        // Max reasoning needs headroom before the final evidence-backed JSON.
        let max_tokens =
            if self.uses_max_reasoning_profile() && matches!(kind, CompletionKind::Answer) {
                32_000
            } else {
                12_000
            };
        Box::pin(async move {
            let result = Model::complete_with_limit(self, messages, kind, max_tokens).await;
            if self.uses_max_reasoning_profile()
                && kind == CompletionKind::Answer
                && result.as_ref().is_err_and(|error| {
                    matches!(
                        error.downcast_ref::<ModelFailure>(),
                        Some(ModelFailure::OutputLimit)
                    )
                })
            {
                tracing::warn!(model = %self.name, phase = ?kind, max_tokens = 64_000, "Retrying truncated answer with a larger budget");
                // Retry from the complete original inputs; never consume truncated JSON.
                Model::complete_with_limit(self, messages, kind, 64_000).await
            } else {
                result
            }
        })
    }

    fn complete_with_limit<'a>(
        &'a self,
        values: &'a [ModelMessage],
        kind: CompletionKind,
        max_tokens: u32,
    ) -> PortFuture<'a, ModelMessage> {
        Box::pin(Model::complete_with_limit(self, values, kind, max_tokens))
    }
    fn research<'a>(
        &'a self,
        tools: Arc<dyn ResearchTools>,
        system: &'a str,
        history: &'a [ModelMessage],
        question: &'a str,
        max_turns: usize,
    ) -> PortFuture<'a, ResearchOutcome> {
        Box::pin(super::research_tools::run(
            self, tools, system, history, question, max_turns,
        ))
    }
}

fn is_deepseek_v41_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("deepseek/deepseek-v4.1-flash")
        || name.eq_ignore_ascii_case("deepseek-v4.1-flash")
}

// Keep the direct Sol migration separate from the existing OpenRouter Sol
// profile. Both max profiles need time and output space for hidden reasoning.
fn uses_max_reasoning_profile(name: &str, kind: ProviderKind) -> bool {
    is_deepseek_v41_name(name) || is_native_sol(name, kind)
}

fn is_native_sol(name: &str, kind: ProviderKind) -> bool {
    kind == ProviderKind::OpenAi && matches!(research_route(name), Route::Responses)
}

fn completion_timeout_seconds_for(name: &str, kind: ProviderKind) -> u64 {
    if uses_max_reasoning_profile(name, kind) {
        360
    } else {
        120
    }
}

// Both native and OpenRouter Sol use Responses for tool research. Native Sol
// also uses Responses for synthesis/review so every phase can retain max effort.
fn research_route(name: &str) -> Route {
    // Preserve the provider prefix on the wire, but recognize Sol's API needs.
    let name = name.strip_prefix("openai/").unwrap_or(name);
    if name == "gpt-6.1-sol" || name.starts_with("gpt-6.1-sol-") {
        Route::Responses
    } else {
        Route::Chat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_completion_budget_includes_native_sol_but_not_openrouter_sol() {
        for name in [
            "deepseek/deepseek-v4.1-flash",
            "deepseek-v4.1-flash",
            "gpt-6.1-sol",
            "gpt-6.1-sol-2026-09-29",
        ] {
            assert_eq!(
                completion_timeout_seconds_for(name, ProviderKind::OpenAi),
                360
            );
        }
        for name in [
            "gpt-6.1-sol",
            "openai/gpt-6.1-sol",
            "openai/gpt-6.1-sol-2026-09-29",
        ] {
            assert_eq!(
                completion_timeout_seconds_for(name, ProviderKind::OpenRouter),
                120
            );
        }
        for kind in [ProviderKind::OpenAi, ProviderKind::OpenRouter] {
            for name in [
                "qwen/qwen3.8-27b",
                "other/model",
                "other/gpt-6.1-sol",
                "gpt-5.4",
            ] {
                assert_eq!(completion_timeout_seconds_for(name, kind), 120);
            }
        }
    }

    fn model(name: &str, kind: ProviderKind) -> Model {
        Model {
            name: name.into(),
            provider: provider_with_route(
                "http://localhost:9000/v1",
                "test-only-key",
                Duration::from_secs(completion_timeout_seconds_for(name, kind) + 5),
                research_route(name),
                kind,
            )
            .unwrap(),
            kind,
        }
    }

    #[test]
    fn native_sol_keeps_max_reasoning_and_headroom_across_phases() {
        let model = model("gpt-6.1-sol", ProviderKind::OpenAi);
        assert!(model.uses_responses_for_completion());
        for (kind, max_tokens) in [
            (CompletionKind::Answer, 32_000),
            (CompletionKind::Answer, 64_000),
            (CompletionKind::EvidenceReview, 24_000),
            (CompletionKind::EvidenceReview, 48_000),
        ] {
            let request =
                model.completion_request(&[ModelMessage::user("Return JSON")], kind, max_tokens);
            assert_eq!(request.max_tokens, Some(u64::from(max_tokens)));
            // Native Responses must never inherit Chat's unsupported max
            // effort field, response_format or max_completion_tokens.
            let params = request.additional_params.unwrap();
            assert_eq!(params.as_object().unwrap().len(), 3);
            assert_eq!(params["reasoning"], json!({"effort": "max"}));
            assert_eq!(params["store"], false);
            let format = &params["text"]["format"];
            assert_eq!(format["type"], "json_schema");
            assert_eq!(format["strict"], true);
            assert_eq!(
                format["name"],
                match kind {
                    CompletionKind::Answer => "lesana_answer",
                    CompletionKind::EvidenceReview => "lesana_evidence_review",
                }
            );
            assert_eq!(format["schema"]["additionalProperties"], false);
            assert!(
                format["schema"]["properties"]["claims"]
                    .get("maxItems")
                    .is_none()
            );
            // Exercise the actual pre-wire parser: json_object is valid at
            // OpenAI but unsupported by Rig 0.43's Responses TextFormat enum.
            let parsed: rig::providers::openai::responses_api::AdditionalParameters =
                serde_json::from_value(params.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), params);
        }
        assert_eq!(
            model.research_parameters(),
            Some(json!({"reasoning": {"effort": "max"}, "store": false}))
        );
        assert_eq!(model.research_max_tokens(1), Some(32_000));
        assert_eq!(model.research_max_tokens(2), Some(64_000));
        assert_eq!(model.turn_timeout_seconds(), 1800);
        assert_eq!(model.completion_timeout_seconds(), 360);
        let review = model.evidence_review_config();
        assert_eq!(review.claims_per_call, 3);
        assert_eq!(review.initial_tokens, 24_000);
        assert_eq!(review.retry_tokens, 48_000);
        assert!(model.uses_strict_schema());
    }

    #[test]
    fn openrouter_sol_retains_its_existing_profile() {
        let model = model("openai/gpt-6.1-sol", ProviderKind::OpenRouter);
        assert!(!model.uses_max_reasoning_profile());
        assert!(!model.uses_responses_for_completion());
        let request = model.completion_request(
            &[ModelMessage::user("Return JSON")],
            CompletionKind::Answer,
            12_000,
        );
        assert_eq!(request.max_tokens, None);
        assert_eq!(
            request.additional_params,
            Some(json!({
                "reasoning_effort": "low",
                "max_completion_tokens": 12_000,
                "response_format": {"type": "json_object"}
            }))
        );
        assert_eq!(model.chat_parameters(), json!({"reasoning_effort": "low"}));
        assert_eq!(
            model.research_parameters(),
            Some(json!({"reasoning": {"effort": "low"}, "store": false}))
        );
        assert_eq!(model.research_max_tokens(1), Some(12_000));
        assert_eq!(model.research_max_tokens(2), Some(12_000));
        assert_eq!(model.turn_timeout_seconds(), 300);
        assert_eq!(model.completion_timeout_seconds(), 120);
        let review = model.evidence_review_config();
        assert_eq!(review.claims_per_call, 3);
        assert_eq!(review.initial_tokens, 6_000);
        assert_eq!(review.retry_tokens, 12_000);
        assert!(!model.uses_strict_schema());
    }

    #[test]
    fn sol_research_uses_responses_with_or_without_openrouter_prefix() {
        for name in [
            "gpt-6.1-sol",
            "openai/gpt-6.1-sol",
            "openai/gpt-6.1-sol-2026-09-29",
        ] {
            assert!(matches!(research_route(name), Route::Responses), "{name}");
        }
        for name in [
            "qwen/qwen3.8-27b",
            "qwen3:8b",
            "other/gpt-6.1-sol",
            "gpt-5.4",
        ] {
            assert!(matches!(research_route(name), Route::Chat), "{name}");
        }
    }
}
