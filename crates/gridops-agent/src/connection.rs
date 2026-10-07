//! Turns a stored provider connection into a model client, and checks API
//! keys against their provider before `GridOps` stores them.
//!
//! Secrets arrive here already decrypted and leave only inside the request to
//! the provider. Error text from providers is bounded and never echoes the key.

use std::time::Duration;

use agent_runtime::{
    AgentProviderKind, AnthropicClient, AnthropicClientConfig, AssistantTurn, ChatMessage,
    OpenAiClient, OpenAiClientConfig, ReqwestHttpClient, RetryPolicy, TextProvider, ToolDefinition,
};
use anyhow::{Context as _, Result, bail};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    PROVIDER_HTTP_TIMEOUT_SECONDS,
    catalog::{ModelOption, ProviderId, default_models, openai_reasoning_efforts},
    subscriptions::{
        ClaudeCodeSubscriptionProvider, CodexSubscriptionProvider, SubscriptionOptions,
    },
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const OPENROUTER_REFERER: &str = "https://github.com/iamngoni/gridops";

/// What `GridOps` asks of the model on every turn.
#[derive(Debug, Clone)]
pub struct ModelSettings {
    pub model: String,
    pub reasoning_effort: Option<String>,
    /// Output budget per turn, reasoning included where the provider counts it.
    pub max_output_tokens: u32,
}

#[derive(Serialize, Deserialize)]
struct ApiKeySecret {
    api_key: String,
}

/// The JSON `GridOps` stores for an API-key connection.
pub fn api_key_secret(api_key: &str) -> Result<String> {
    serde_json::to_string(&ApiKeySecret {
        api_key: api_key.to_owned(),
    })
    .context("serialize API key")
}

fn api_key_from_secret(secret: &str) -> Result<String> {
    serde_json::from_str::<ApiKeySecret>(secret)
        .map(|secret| secret.api_key)
        .context("stored API key is invalid")
}

/// The last four characters of a key, for recognising it later.
pub fn fingerprint(api_key: &str) -> String {
    let tail = api_key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("••••{tail}")
}

/// Rejects keys that cannot belong to the provider before any request.
pub fn validate_api_key_shape(provider: ProviderId, api_key: &str) -> Result<()> {
    let Some(prefix) = provider.key_prefix() else {
        bail!("{} connects by signing in, not with a key", provider.name());
    };
    let trimmed = api_key.trim();
    if trimmed.len() < 20 || trimmed.len() > 512 || trimmed.chars().any(char::is_whitespace) {
        bail!("That doesn't look like a complete {} key.", provider.name());
    }
    // OpenAI keys also start with `sk-`, so an Anthropic or OpenRouter key
    // pasted as OpenAI is caught by checking the more specific prefixes first.
    if provider == ProviderId::OpenAi
        && (trimmed.starts_with("sk-ant-") || trimmed.starts_with("sk-or-"))
    {
        bail!("That key belongs to a different provider.");
    }
    if !trimmed.starts_with(prefix) {
        bail!("{} keys start with {prefix}.", provider.name());
    }
    Ok(())
}

enum Client {
    Codex(CodexSubscriptionProvider),
    Claude(ClaudeCodeSubscriptionProvider),
    Api(Box<dyn TextProvider>),
}

/// A connected model, ready to take turns.
pub struct AgentModel {
    client: Client,
    settings: ModelSettings,
}

impl AgentModel {
    pub fn connect(provider: ProviderId, secret: &str, settings: ModelSettings) -> Result<Self> {
        let client = match provider {
            ProviderId::Codex => Client::Codex(CodexSubscriptionProvider::from_secret(secret)?),
            ProviderId::ClaudeCode => Client::Claude(ClaudeCodeSubscriptionProvider::from_secret(
                secret,
                SubscriptionOptions {
                    max_tokens: settings.max_output_tokens,
                    effort: settings.reasoning_effort.clone(),
                },
            )?),
            ProviderId::Anthropic => {
                let config = AnthropicClientConfig {
                    max_tokens: settings.max_output_tokens,
                    retry: RetryPolicy::with_retries(2),
                    ..AnthropicClientConfig::default()
                };
                Client::Api(Box::new(AnthropicClient::with_config(
                    runtime_http()?,
                    api_key_from_secret(secret)?,
                    config,
                )))
            }
            ProviderId::OpenAi | ProviderId::OpenRouter => {
                let kind = if provider == ProviderId::OpenAi {
                    AgentProviderKind::OpenAi
                } else {
                    AgentProviderKind::OpenRouter
                };
                let mut config = OpenAiClientConfig::for_kind(kind);
                config.retry = RetryPolicy::with_retries(2);
                config.max_completion_tokens = Some(u64::from(settings.max_output_tokens));
                config
                    .reasoning_effort
                    .clone_from(&settings.reasoning_effort);
                if provider == ProviderId::OpenRouter {
                    config.extra_headers = vec![
                        ("HTTP-Referer".to_owned(), OPENROUTER_REFERER.to_owned()),
                        ("X-Title".to_owned(), "GridOps".to_owned()),
                    ];
                }
                Client::Api(Box::new(OpenAiClient::with_config(
                    runtime_http()?,
                    api_key_from_secret(secret)?,
                    config,
                )))
            }
        };
        Ok(Self { client, settings })
    }

    pub fn model(&self) -> &str {
        &self.settings.model
    }

    pub async fn turn(
        &self,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        let model = self.settings.model.as_str();
        match &self.client {
            Client::Codex(provider) => {
                provider
                    .request_assistant_turn(model, system_prompt, history, tools)
                    .await
            }
            Client::Claude(provider) => {
                provider
                    .request_assistant_turn(model, system_prompt, history, tools)
                    .await
            }
            Client::Api(provider) => {
                provider
                    .request_assistant_turn(model, system_prompt, history, tools)
                    .await
            }
        }
    }

    /// The subscription's rotated tokens, when a refresh happened since the
    /// last call. Callers must store them or the next run signs in with a
    /// spent refresh token.
    pub async fn refreshed_credential(&self) -> Result<Option<SecretString>> {
        match &self.client {
            Client::Codex(provider) => provider.refreshed_credential().await,
            Client::Claude(provider) => provider.refreshed_credential().await,
            Client::Api(_) => Ok(None),
        }
    }
}

fn runtime_http() -> Result<agent_runtime::SharedHttpClient> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(PROVIDER_HTTP_TIMEOUT_SECONDS))
        .build()
        .context("build provider HTTP client")?;
    Ok(ReqwestHttpClient::new(client).into_shared())
}

fn listing_http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent("GridOps/0.1")
        .build()
        .context("build provider HTTP client")
}

/// Lists the models a connection can use. Subscriptions use the built-in list;
/// API-key providers are asked, which also proves the key works.
pub async fn list_models(provider: ProviderId, secret: &str) -> Result<Vec<ModelOption>> {
    match provider {
        ProviderId::Codex | ProviderId::ClaudeCode => Ok(default_models(provider)),
        ProviderId::Anthropic => anthropic_models(&api_key_from_secret(secret)?).await,
        ProviderId::OpenAi => openai_models(&api_key_from_secret(secret)?).await,
        ProviderId::OpenRouter => openrouter_models(&api_key_from_secret(secret)?).await,
    }
}

/// Checks a new key with the provider. Returns its models so the settings
/// page can offer them without a second round trip.
pub async fn verify_api_key(provider: ProviderId, api_key: &str) -> Result<Vec<ModelOption>> {
    validate_api_key_shape(provider, api_key)?;
    list_models(provider, &api_key_secret(api_key.trim())?).await
}

async fn get_json(request: reqwest::RequestBuilder, provider: ProviderId) -> Result<Value> {
    let response = request
        .send()
        .await
        .with_context(|| format!("{} could not be reached", provider.name()))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!("{} rejected the key ({status}).", provider.name());
    }
    if !status.is_success() {
        bail!(
            "{} returned {status} while listing models.",
            provider.name()
        );
    }
    response
        .json::<Value>()
        .await
        .with_context(|| format!("{} returned an unreadable model list", provider.name()))
}

async fn anthropic_models(api_key: &str) -> Result<Vec<ModelOption>> {
    let body = get_json(
        listing_http()?
            .get("https://api.anthropic.com/v1/models?limit=100")
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        ProviderId::Anthropic,
    )
    .await?;
    Ok(parse_model_list(&body, |item| {
        let id = item.get("id")?.as_str()?;
        let name = item.get("display_name").and_then(Value::as_str);
        Some(ModelOption::listed(id, name))
    }))
}

async fn openai_models(api_key: &str) -> Result<Vec<ModelOption>> {
    let body = get_json(
        listing_http()?
            .get("https://api.openai.com/v1/models")
            .bearer_auth(api_key),
        ProviderId::OpenAi,
    )
    .await?;
    let mut models = parse_model_list(&body, |item| {
        let id = item.get("id")?.as_str()?;
        if !openai_chat_model(id) {
            return None;
        }
        let efforts = openai_reasoning_efforts(id);
        Some(ModelOption {
            id: id.to_owned(),
            name: id.to_owned(),
            reasoning_efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            default_effort: (!efforts.is_empty()).then(|| "medium".to_owned()),
        })
    });
    models.sort_by(|left, right| right.id.cmp(&left.id));
    Ok(models)
}

/// `OpenAI` lists every model it serves; only chat models can drive the agent.
fn openai_chat_model(id: &str) -> bool {
    let family = id.starts_with("gpt-")
        || id.starts_with("chatgpt-")
        || (id.starts_with('o') && id.chars().nth(1).is_some_and(|c| c.is_ascii_digit()));
    let excluded = [
        "audio",
        "realtime",
        "tts",
        "transcribe",
        "image",
        "search",
        "embedding",
        "instruct",
    ];
    family && !excluded.iter().any(|word| id.contains(word))
}

async fn openrouter_models(api_key: &str) -> Result<Vec<ModelOption>> {
    // The model list is public, so the key is checked on its own endpoint.
    get_json(
        listing_http()?
            .get("https://openrouter.ai/api/v1/key")
            .bearer_auth(api_key),
        ProviderId::OpenRouter,
    )
    .await?;
    let body = get_json(
        listing_http()?
            .get("https://openrouter.ai/api/v1/models")
            .bearer_auth(api_key),
        ProviderId::OpenRouter,
    )
    .await?;
    let mut models = parse_model_list(&body, |item| {
        let id = item.get("id")?.as_str()?;
        let parameters = item
            .get("supported_parameters")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        // The agent works only through tools.
        if !parameters.contains(&"tools") {
            return None;
        }
        let reasoning = parameters.contains(&"reasoning");
        Some(ModelOption {
            id: id.to_owned(),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned(),
            reasoning_efforts: if reasoning {
                vec!["low".into(), "medium".into(), "high".into()]
            } else {
                Vec::new()
            },
            default_effort: reasoning.then(|| "medium".to_owned()),
        })
    });
    models.insert(0, default_models(ProviderId::OpenRouter).remove(0));
    Ok(models)
}

fn parse_model_list(
    body: &Value,
    parse: impl Fn(&Value) -> Option<ModelOption>,
) -> Vec<ModelOption> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn key_shapes_are_checked_per_provider() {
        let anthropic = format!("sk-ant-{}", "a".repeat(40));
        let openai = format!("sk-proj-{}", "b".repeat(40));
        assert!(validate_api_key_shape(ProviderId::Anthropic, &anthropic).is_ok());
        assert!(validate_api_key_shape(ProviderId::OpenAi, &openai).is_ok());
        assert!(validate_api_key_shape(ProviderId::OpenAi, &anthropic).is_err());
        assert!(validate_api_key_shape(ProviderId::Anthropic, &openai).is_err());
        assert!(validate_api_key_shape(ProviderId::OpenRouter, "sk-or-short").is_err());
        assert!(validate_api_key_shape(ProviderId::ClaudeCode, &anthropic).is_err());
    }

    #[test]
    fn api_key_secrets_round_trip_and_fingerprint() -> anyhow::Result<()> {
        let secret = api_key_secret("sk-ant-example-1234")?;
        assert_eq!(api_key_from_secret(&secret)?, "sk-ant-example-1234");
        assert_eq!(fingerprint("sk-ant-example-1234"), "••••1234");
        Ok(())
    }

    #[test]
    fn openai_listing_keeps_chat_models_only() {
        assert!(openai_chat_model("gpt-5.6-sol"));
        assert!(openai_chat_model("o4-mini"));
        assert!(!openai_chat_model("gpt-4o-realtime-preview"));
        assert!(!openai_chat_model("text-embedding-3-large"));
        assert!(!openai_chat_model("omni-moderation-latest"));
    }

    #[test]
    fn model_lists_parse_provider_payloads() {
        let body = json!({ "data": [
            { "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5" },
            { "nope": true },
        ]});
        let models = parse_model_list(&body, |item| {
            let id = item.get("id")?.as_str()?;
            Some(ModelOption::listed(
                id,
                item.get("display_name").and_then(Value::as_str),
            ))
        });
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "Claude Opus 5.5");
    }

    #[test]
    fn subscription_and_api_connections_build_clients() -> anyhow::Result<()> {
        let settings = ModelSettings {
            model: "claude-opus-5-5".into(),
            reasoning_effort: Some("high".into()),
            max_output_tokens: 32_000,
        };
        let secret = api_key_secret(&format!("sk-ant-{}", "x".repeat(40)))?;
        let model = AgentModel::connect(ProviderId::Anthropic, &secret, settings.clone())?;
        assert_eq!(model.model(), "claude-opus-5-5");
        let claude = json!({ "access_token": "a", "refresh_token": "r" }).to_string();
        assert!(AgentModel::connect(ProviderId::ClaudeCode, &claude, settings.clone()).is_ok());
        assert!(AgentModel::connect(ProviderId::OpenAi, "not json", settings).is_err());
        Ok(())
    }
}
