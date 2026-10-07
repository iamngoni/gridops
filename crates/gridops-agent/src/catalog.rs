//! The AI providers `GridOps` can connect, and the models offered for each.
//!
//! Subscription providers have no model-listing endpoint, so their models are
//! listed here. API-key providers are listed live from the provider when a
//! connection is made, with these entries as the fallback.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Codex,
    ClaudeCode,
    OpenAi,
    Anthropic,
    OpenRouter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    SubscriptionOAuth,
    ApiKey,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionOAuth => "subscription_oauth",
            Self::ApiKey => "api_key",
        }
    }
}

pub const ALL_PROVIDERS: [ProviderId; 5] = [
    ProviderId::ClaudeCode,
    ProviderId::Codex,
    ProviderId::Anthropic,
    ProviderId::OpenAi,
    ProviderId::OpenRouter,
];

impl ProviderId {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "claude_code" => Some(Self::ClaudeCode),
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            "openrouter" => Some(Self::OpenRouter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "ChatGPT",
            Self::ClaudeCode => "Claude",
            Self::OpenAi => "OpenAI API",
            Self::Anthropic => "Anthropic API",
            Self::OpenRouter => "OpenRouter",
        }
    }

    pub fn auth_method(self) -> AuthMethod {
        match self {
            Self::Codex | Self::ClaudeCode => AuthMethod::SubscriptionOAuth,
            Self::OpenAi | Self::Anthropic | Self::OpenRouter => AuthMethod::ApiKey,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Codex => {
                "Sign in with a ChatGPT plan. Runs count against that plan's Codex limits."
            }
            Self::ClaudeCode => {
                "Sign in with a Claude plan. Runs count against that plan's Claude Code limits."
            }
            Self::OpenAi => "Pay per token with an OpenAI API key.",
            Self::Anthropic => "Pay per token with an Anthropic API key.",
            Self::OpenRouter => "Use any model OpenRouter routes to, with an OpenRouter key.",
        }
    }

    /// The prefix a pasted key starts with, used for a placeholder and a
    /// cheap sanity check before the key is tried against the provider.
    pub fn key_prefix(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => Some("sk-"),
            Self::Anthropic => Some("sk-ant-"),
            Self::OpenRouter => Some("sk-or-"),
            Self::Codex | Self::ClaudeCode => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub reasoning_efforts: Vec<String>,
    pub default_effort: Option<String>,
}

impl ModelOption {
    fn fixed(id: &str, name: &str, efforts: &[&str], default_effort: Option<&str>) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            reasoning_efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            default_effort: default_effort.map(ToOwned::to_owned),
        }
    }

    /// A model from a provider's live listing. `GridOps` does not know which of
    /// those accept a reasoning effort, so none is offered.
    pub fn listed(id: &str, name: Option<&str>) -> Self {
        Self::fixed(id, name.unwrap_or(id), &[], None)
    }
}

const CLAUDE_EFFORTS: [&str; 3] = ["low", "medium", "high"];
const OPENAI_EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// The models offered without asking the provider: the full list for
/// subscriptions, and the fallback when an API listing fails.
pub fn default_models(provider: ProviderId) -> Vec<ModelOption> {
    match provider {
        ProviderId::ClaudeCode => vec![
            ModelOption::fixed(
                "claude-opus-5-5",
                "Claude Opus 5.5",
                &CLAUDE_EFFORTS,
                Some("high"),
            ),
            ModelOption::fixed(
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                &CLAUDE_EFFORTS,
                Some("high"),
            ),
            ModelOption::fixed(
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                &CLAUDE_EFFORTS,
                Some("medium"),
            ),
        ],
        ProviderId::Codex => vec![
            ModelOption::fixed("gpt-5.6-sol", "GPT-5.6 Sol", &[], None),
            ModelOption::fixed("gpt-5.6-terra", "GPT-5.6 Terra", &[], None),
        ],
        ProviderId::Anthropic => vec![
            ModelOption::fixed("claude-opus-5-5", "Claude Opus 5.5", &[], None),
            ModelOption::fixed("claude-sonnet-5-5", "Claude Sonnet 5.5", &[], None),
        ],
        ProviderId::OpenAi => vec![ModelOption::fixed(
            "gpt-5.6-sol",
            "GPT-5.6 Sol",
            &OPENAI_EFFORTS,
            Some("medium"),
        )],
        ProviderId::OpenRouter => vec![ModelOption::fixed(
            "openrouter/auto",
            "OpenRouter Auto",
            &[],
            None,
        )],
    }
}

/// Reasoning efforts a listed `OpenAI` model accepts. `OpenAI`'s listing does not
/// say, so reasoning families are recognised by name.
pub fn openai_reasoning_efforts(model: &str) -> &'static [&'static str] {
    let reasoning = model.starts_with("gpt-5") || model.starts_with('o');
    if reasoning { &OPENAI_EFFORTS } else { &[] }
}

/// Checks a requested effort against the efforts a model offers. An effort is
/// dropped rather than rejected when the model offers none, so a model switch
/// never strands a stale effort.
pub fn effective_effort(model: &ModelOption, requested: Option<&str>) -> Option<String> {
    let requested = requested?;
    model
        .reasoning_efforts
        .iter()
        .find(|effort| effort.as_str() == requested)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_round_trip() {
        for provider in ALL_PROVIDERS {
            assert_eq!(ProviderId::parse(provider.as_str()), Some(provider));
            assert!(!default_models(provider).is_empty());
        }
        assert_eq!(ProviderId::parse("kimi"), None);
    }

    #[test]
    fn subscriptions_use_oauth_and_keys_use_prefixes() {
        assert_eq!(
            ProviderId::ClaudeCode.auth_method(),
            AuthMethod::SubscriptionOAuth
        );
        assert_eq!(ProviderId::Anthropic.key_prefix(), Some("sk-ant-"));
        assert_eq!(ProviderId::Codex.key_prefix(), None);
    }

    #[test]
    fn efforts_are_kept_only_when_the_model_offers_them() {
        let opus = &default_models(ProviderId::ClaudeCode)[0];
        assert_eq!(
            effective_effort(opus, Some("high")).as_deref(),
            Some("high")
        );
        assert_eq!(effective_effort(opus, Some("max")), None);
        let listed = ModelOption::listed("some/model", None);
        assert_eq!(effective_effort(&listed, Some("high")), None);
        assert_eq!(openai_reasoning_efforts("gpt-4o-mini"), &[] as &[&str]);
        assert_eq!(openai_reasoning_efforts("o4-mini").len(), 3);
    }
}
