use crate::application::utils::errors::ModelFailure;
use anyhow::{Result, ensure};
use rig::{
    error::ProviderError,
    providers::openai::{
        OpenAI, OpenAIConfig, Route,
        wire::{Auth, OPENROUTER},
    },
};
use std::time::Duration;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    OpenRouter,
}
impl ProviderKind {
    pub fn from_config(setting: &str, base_url: &str) -> Result<Self> {
        let url = reqwest::Url::parse(base_url)?;
        let is_openrouter = url
            .host_str()
            .is_some_and(|host| host.trim_end_matches('.') == "openrouter.ai");
        ensure!(
            !(is_openrouter && setting.trim() == "openai"),
            "Use LLM_PROVIDER=openrouter or auto with the OpenRouter endpoint"
        );
        match setting.trim() {
            "" | "auto" => Ok(if is_openrouter {
                Self::OpenRouter
            } else {
                Self::OpenAi
            }),
            "openrouter" => Ok(Self::OpenRouter),
            "openai" => Ok(Self::OpenAi),
            _ => anyhow::bail!("LLM_PROVIDER must be auto, openai or openrouter"),
        }
    }

    // Never forward a credential for direct OpenAI access to OpenRouter.
    pub fn api_key<'a>(self, compatible_key: &'a str, openrouter_key: &'a str) -> Result<&'a str> {
        match self {
            Self::OpenAi => Ok(compatible_key),
            Self::OpenRouter => {
                ensure!(
                    !openrouter_key.trim().is_empty(),
                    "Set OPENROUTER_API_KEY for OpenRouter"
                );
                Ok(openrouter_key.trim())
            }
        }
    }
}

/// Keep endpoints, credentials and provider error bodies out of application logs.
pub fn provider(base_url: &str, api_key: &str, timeout: Duration) -> Result<OpenAI> {
    provider_with_route(
        base_url,
        api_key,
        timeout,
        Route::Chat,
        ProviderKind::OpenAi,
    )
}

pub fn provider_with_route(
    base_url: &str,
    api_key: &str,
    timeout: Duration,
    route: Route,
    kind: ProviderKind,
) -> Result<OpenAI> {
    let url = reqwest::Url::parse(base_url)?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Model endpoint must be HTTP(S)"
    );
    let http = rig::rig_reqwest::reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .redirect(rig::rig_reqwest::reqwest::redirect::Policy::none())
        .build()?;
    let config = match kind {
        ProviderKind::OpenAi => OpenAIConfig::new(api_key.to_owned()),
        ProviderKind::OpenRouter => OpenAIConfig::with_key(&OPENROUTER, api_key.to_owned()),
    };
    let transport =
        rig::http_client::DynHttpClient::new(rig::rig_reqwest::ReqwestClient::from(http));
    let transport = if kind == ProviderKind::OpenAi && matches!(route, Route::Responses) {
        rig::http_client::DynHttpClient::new(super::native_rate_limit::NativeRateLimitClient::new(
            transport,
        ))
    } else {
        transport
    };
    Ok(config
        .with_auth(Auth::OptionalBearer)
        .with_route(route)
        .with_base_url(base_url.trim_end_matches('/'))
        .connect(transport))
}

pub fn provider_failure(error: ProviderError) -> ModelFailure {
    if error
        .provider_response_status()
        .is_some_and(|status| status.as_u16() == 429)
        && super::native_rate_limit::is_quota_failure(error.provider_response_body())
    {
        return ModelFailure::Quota;
    }
    if let Some(status) = error
        .provider_response_status()
        .filter(|status| !status.is_success())
    {
        return ModelFailure::Http(status.as_u16());
    }
    match error {
        ProviderError::Http(_) => ModelFailure::Connection,
        ProviderError::Relayed(report) => report_failure(report.kind),
        _ => ModelFailure::InvalidResponse,
    }
}

pub fn prompt_failure(error: rig::completion::PromptError) -> ModelFailure {
    let quota = match &error {
        rig::completion::PromptError::CompletionError(error) => {
            super::native_rate_limit::is_quota_failure(error.provider_response_body())
        }
        rig::completion::PromptError::Report(report) => {
            super::native_rate_limit::is_quota_failure(report.provider_response_body())
        }
        _ => false,
    };
    if quota
        && error
            .provider_response_status()
            .is_some_and(|status| status.as_u16() == 429)
    {
        return ModelFailure::Quota;
    }
    if let Some(status) = error
        .provider_response_status()
        .filter(|status| !status.is_success())
    {
        return ModelFailure::Http(status.as_u16());
    }
    match error {
        rig::completion::PromptError::CompletionError(error) => provider_failure(error),
        rig::completion::PromptError::Report(report) => report_failure(report.kind),
        _ => ModelFailure::InvalidResponse,
    }
}

fn report_failure(kind: rig::error::ErrorKind) -> ModelFailure {
    match kind {
        rig::error::ErrorKind::Http => ModelFailure::Connection,
        rig::error::ErrorKind::Timeout => ModelFailure::Timeout,
        _ => ModelFailure::InvalidResponse,
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn detects_openrouter_by_exact_host_or_explicit_provider() {
        assert_eq!(
            ProviderKind::from_config("", "https://openrouter.ai/api/v1").unwrap(),
            ProviderKind::OpenRouter
        );
        assert_eq!(
            ProviderKind::from_config("auto", "https://openrouter.ai:443/api/v1/").unwrap(),
            ProviderKind::OpenRouter
        );
        assert_eq!(
            ProviderKind::from_config("", "https://openrouter.ai.example/v1").unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderKind::from_config("", "http://localhost:11434/v1").unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderKind::from_config("openrouter", "http://mock-model:9000/api/v1").unwrap(),
            ProviderKind::OpenRouter
        );
        assert_eq!(
            ProviderKind::from_config("auto", "https://openrouter.ai./api/v1").unwrap(),
            ProviderKind::OpenRouter
        );
        assert!(ProviderKind::from_config("openai", "https://openrouter.ai/api/v1").is_err());
        assert!(ProviderKind::from_config("typo", "http://localhost:11434/v1").is_err());
    }

    #[test]
    fn keeps_provider_credentials_separate() {
        assert!(
            ProviderKind::OpenRouter
                .api_key("direct-openai-secret", "")
                .is_err()
        );
        assert!(
            ProviderKind::OpenRouter
                .api_key("direct-openai-secret", "  ")
                .is_err()
        );
        assert_eq!(
            ProviderKind::OpenRouter
                .api_key("direct-openai-secret", "router-secret")
                .unwrap(),
            "router-secret"
        );
        assert_eq!(
            ProviderKind::OpenAi
                .api_key("direct-openai-secret", "router-secret")
                .unwrap(),
            "direct-openai-secret"
        );
        assert_eq!(
            ProviderKind::OpenAi.api_key("", "router-secret").unwrap(),
            ""
        );
    }
}
