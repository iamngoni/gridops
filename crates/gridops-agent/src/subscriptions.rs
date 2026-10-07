// Adapted from Heimdall's subscription-backed Codex and Claude Code providers,
// by way of ccs. GridOps changes: configurable output budget and effort,
// thinking blocks carried across tool turns, and no panicking constructors.
// Copyright (c) 2026 Codecraft Solutions ZA. All rights reserved.
// SPDX-License-Identifier: LicenseRef-Heimdall-FSL

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_runtime::{
    AgentProviderKind, AssistantTurn, ChatMessage, EventSink, MessageRole, ModelTiers,
    ProviderInfo, RuntimeEvent, TextProvider, ToolCall, ToolDefinition,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::StatusCode;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::sync::Mutex;
use uuid::Uuid;

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_ISSUER: &str = "https://auth.openai.com";
const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
const TOKEN_REFRESH_WINDOW_SECONDS: i64 = 5 * 60;

/// A stalled connection would otherwise hang until the caller's own outer
/// deadline fires; this timeout fails it fast instead. See
/// [`crate::PROVIDER_HTTP_TIMEOUT_SECONDS`] for why this specific value.
fn timed_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(crate::PROVIDER_HTTP_TIMEOUT_SECONDS))
        .build()
        .context("build provider HTTP client")
}

/// Per-connection request settings for the subscription transports.
#[derive(Debug, Clone)]
pub struct SubscriptionOptions {
    /// Output budget per turn, thinking included.
    pub max_tokens: u32,
    /// Claude effort (`low`, `medium`, `high`). Ignored by Codex.
    pub effort: Option<String>,
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            effort: None,
        }
    }
}

const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const CLAUDE_SESSION_SCOPE: &str = "user:sessions:claude_code";
const CLAUDE_API_BASE: &str = "https://api.anthropic.com";
const CLAUDE_ANTHROPIC_VERSION: &str = "2023-06-01";
const CLAUDE_BETAS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05,mid-conversation-system-2026-04-07,advisor-tool-2026-03-01,effort-2025-11-24,extended-cache-ttl-2025-04-11";
const CLAUDE_USER_AGENT: &str = "claude-cli/2.1.211 (external, sdk-cli)";
const CLAUDE_BILLING_HEADER: &str =
    "x-anthropic-billing-header: cc_version=2.1.211.cfa; cc_entrypoint=sdk-cli;";
const CLAUDE_SYSTEM_PREFIX: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    is_fedramp_account: bool,
    #[serde(default)]
    access_token_expires_at_unix: Option<i64>,
    #[serde(default)]
    connected_at_unix: i64,
    #[serde(default)]
    last_refresh_unix: Option<i64>,
}

impl CodexTokens {
    fn from_secret(secret: &str) -> Result<Self> {
        let mut tokens: Self =
            serde_json::from_str(secret).context("stored Codex credential is invalid")?;
        tokens.refresh_metadata()?;
        Ok(tokens)
    }

    fn refresh_metadata(&mut self) -> Result<()> {
        let claims = jwt_payload(&self.id_token).context("parse Codex account metadata")?;
        let auth = claims
            .get("https://api.openai.com/auth")
            .and_then(Value::as_object);
        if let Some(auth) = auth {
            self.account_id = auth
                .get("chatgpt_account_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| self.account_id.clone());
            self.plan_type = auth
                .get("chatgpt_plan_type")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| self.plan_type.clone());
            self.is_fedramp_account = auth
                .get("chatgpt_account_is_fedramp")
                .and_then(Value::as_bool)
                .unwrap_or(self.is_fedramp_account);
        }
        self.access_token_expires_at_unix = jwt_payload(&self.access_token)
            .ok()
            .and_then(|payload| payload.get("exp").and_then(Value::as_i64));
        Ok(())
    }

    fn needs_refresh(&self) -> bool {
        self.access_token_expires_at_unix.is_some_and(|expires_at| {
            expires_at <= OffsetDateTime::now_utc().unix_timestamp() + TOKEN_REFRESH_WINDOW_SECONDS
        })
    }

    fn apply_refresh(&mut self, response: CodexRefreshResponse) -> Result<()> {
        if let Some(id_token) = response.id_token {
            self.id_token = id_token;
        }
        if let Some(access_token) = response.access_token {
            self.access_token = access_token;
        }
        if let Some(refresh_token) = response.refresh_token {
            self.refresh_token = refresh_token;
        }
        self.last_refresh_unix = Some(OffsetDateTime::now_utc().unix_timestamp());
        self.refresh_metadata()
    }
}

pub struct CodexSubscriptionProvider {
    client: reqwest::Client,
    tokens: Mutex<CodexTokens>,
    dirty: AtomicBool,
    model_tiers: ModelTiers,
}

impl CodexSubscriptionProvider {
    pub fn from_secret(secret: &str) -> Result<Self> {
        Ok(Self {
            client: timed_http_client()?,
            tokens: Mutex::new(CodexTokens::from_secret(secret)?),
            dirty: AtomicBool::new(false),
            model_tiers: ModelTiers::new("gpt-5.6-terra", "gpt-5.6-terra", "gpt-5.6-sol"),
        })
    }

    pub async fn refreshed_credential(&self) -> Result<Option<SecretString>> {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return Ok(None);
        }
        let secret = serde_json::to_string(&*self.tokens.lock().await)
            .context("serialize refreshed Codex credential")?;
        Ok(Some(SecretString::from(secret)))
    }

    async fn current_tokens(&self, force_refresh: bool) -> Result<CodexTokens> {
        let mut tokens = self.tokens.lock().await;
        if force_refresh || tokens.needs_refresh() {
            self.refresh_tokens(&mut tokens).await?;
        }
        Ok(tokens.clone())
    }

    async fn refresh_tokens(&self, tokens: &mut CodexTokens) -> Result<()> {
        let response = self
            .client
            .post(format!("{CODEX_ISSUER}/oauth/token"))
            .header("Content-Type", "application/json")
            .header("originator", CODEX_ORIGINATOR)
            .json(&json!({
                "client_id": CODEX_CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": tokens.refresh_token,
            }))
            .send()
            .await
            .context("Codex token refresh request failed")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read Codex refresh response")?;
        if !status.is_success() {
            anyhow::bail!(
                "Codex connection needs to be reconnected ({status}): {}",
                safe_provider_error(&body)
            );
        }
        let response =
            serde_json::from_str(&body).context("Codex token refresh returned invalid data")?;
        tokens.apply_refresh(response)?;
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    async fn complete(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        let tokens = self.current_tokens(false).await?;
        match self
            .send_completion(model, system_prompt, history, tools, &tokens)
            .await
        {
            Ok(turn) => Ok(turn),
            Err(SubscriptionRequestError::Unauthorized) => {
                let tokens = self.current_tokens(true).await?;
                self.send_completion(model, system_prompt, history, tools, &tokens)
                    .await
                    .map_err(SubscriptionRequestError::into_anyhow)
            }
            Err(error) => Err(error.into_anyhow()),
        }
    }

    async fn send_completion(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
        tokens: &CodexTokens,
    ) -> std::result::Result<AssistantTurn, SubscriptionRequestError> {
        let body = codex_request_body(model, system_prompt, history, tools);
        let mut request = self
            .client
            .post(format!("{CODEX_BASE_URL}/responses"))
            .bearer_auth(&tokens.access_token)
            .header("Accept", "text/event-stream")
            .header("originator", CODEX_ORIGINATOR)
            .header("User-Agent", "codex_cli_rs/gridops")
            .json(&body);
        if let Some(account_id) = tokens.account_id.as_deref() {
            request = request.header("ChatGPT-Account-ID", account_id);
        }
        if tokens.is_fedramp_account {
            request = request.header("X-OpenAI-Fedramp", "true");
        }
        let response = request
            .send()
            .await
            .map_err(SubscriptionRequestError::from)?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(SubscriptionRequestError::from)?;
        if status == StatusCode::UNAUTHORIZED {
            return Err(SubscriptionRequestError::Unauthorized);
        }
        if !status.is_success() {
            return Err(SubscriptionRequestError::Other(anyhow::anyhow!(
                "Codex request failed ({status}): {}",
                safe_provider_error(&body)
            )));
        }
        parse_codex_sse(&body).map_err(SubscriptionRequestError::Other)
    }
}

impl ProviderInfo for CodexSubscriptionProvider {
    fn kind(&self) -> AgentProviderKind {
        AgentProviderKind::Custom("codex_subscription".to_owned())
    }

    fn verbose(&self) -> bool {
        false
    }

    fn model_tiers(&self) -> &ModelTiers {
        &self.model_tiers
    }
}

#[async_trait]
impl TextProvider for CodexSubscriptionProvider {
    async fn request_assistant_turn(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tool_definitions: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        self.complete(model, system_prompt, history, tool_definitions)
            .await
    }

    async fn stream_message(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        sink: &mut dyn EventSink,
    ) -> Result<String> {
        let turn = self.complete(model, system_prompt, messages, &[]).await?;
        let content = turn.content.unwrap_or_default();
        sink.emit(RuntimeEvent::AssistantDelta {
            delta: content.clone(),
        })
        .await?;
        Ok(content)
    }
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
struct CodexRefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn codex_request_body(
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    tools: &[ToolDefinition],
) -> Value {
    let mut input = Vec::new();
    for message in history {
        match message.role {
            MessageRole::System => {}
            MessageRole::User | MessageRole::Assistant => {
                let role = if message.role == MessageRole::Assistant {
                    "assistant"
                } else {
                    "user"
                };
                if let Some(content) = message.content.as_deref().filter(|text| !text.is_empty()) {
                    input.push(json!({
                        "type": "message",
                        "role": role,
                        "content": [{
                            "type": if role == "assistant" { "output_text" } else { "input_text" },
                            "text": content,
                        }],
                    }));
                }
                for call in &message.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    }));
                }
            }
            MessageRole::Tool => {
                if let Some(call_id) = message.tool_call_id.as_deref() {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": message.content.as_deref().unwrap_or_default(),
                    }));
                }
            }
        }
    }

    let mut body = json!({
        "model": model,
        "instructions": system_prompt,
        "input": input,
        "reasoning": null,
        "store": false,
        "stream": true,
        "include": [],
        "client_metadata": { "application": "gridops" },
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

fn parse_codex_sse(body: &str) -> Result<AssistantTurn> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut data_lines = Vec::new();

    for line in body.lines() {
        if line.trim().is_empty() {
            flush_codex_event(&mut data_lines, &mut content, &mut tool_calls)?;
        } else if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_owned());
        }
    }
    flush_codex_event(&mut data_lines, &mut content, &mut tool_calls)?;

    if content.is_empty() && tool_calls.is_empty() {
        anyhow::bail!("Codex returned no usable assistant content");
    }
    Ok(AssistantTurn {
        content: (!content.is_empty()).then_some(content),
        tool_calls,
    })
}

fn flush_codex_event(
    lines: &mut Vec<String>,
    content: &mut String,
    tool_calls: &mut Vec<ToolCall>,
) -> Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let data = lines.join("\n");
    lines.clear();
    if data == "[DONE]" {
        return Ok(());
    }
    let event: Value = serde_json::from_str(&data)
        .with_context(|| format!("parse Codex response event: {}", safe_provider_error(&data)))?;
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                content.push_str(delta);
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.get("item") {
                parse_codex_output_item(item, content, tool_calls);
            }
        }
        "response.failed" | "error" => {
            let message = event
                .pointer("/response/error/message")
                .or_else(|| event.pointer("/error/message"))
                .or_else(|| event.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Codex request failed");
            anyhow::bail!("{}", safe_provider_error(message));
        }
        _ => {}
    }
    Ok(())
}

fn parse_codex_output_item(item: &Value, content: &mut String, tool_calls: &mut Vec<ToolCall>) {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "function_call" => {
            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_else(|| Value::Object(Map::new()));
            if !id.is_empty() && !name.is_empty() {
                tool_calls.push(ToolCall {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    arguments,
                });
            }
        }
        "message" if content.is_empty() => {
            if let Some(items) = item.get("content").and_then(Value::as_array) {
                for item in items {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeTokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    account_uuid: Option<String>,
    #[serde(default)]
    organization_uuid: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    access_token_expires_at_unix: Option<i64>,
    #[serde(default)]
    connected_at_unix: i64,
    #[serde(default)]
    last_refresh_unix: Option<i64>,
}

impl ClaudeTokens {
    fn needs_refresh(&self) -> bool {
        (!self.refresh_token.is_empty()
            && !self
                .scopes
                .iter()
                .any(|scope| scope == CLAUDE_SESSION_SCOPE))
            || self.access_token_expires_at_unix.is_some_and(|expires_at| {
                expires_at
                    <= OffsetDateTime::now_utc().unix_timestamp() + TOKEN_REFRESH_WINDOW_SECONDS
            })
    }

    fn apply_refresh(&mut self, response: ClaudeRefreshResponse) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        self.access_token = response.access_token;
        if let Some(refresh_token) = response.refresh_token {
            self.refresh_token = refresh_token;
        }
        if let Some(expires_in) = response.expires_in {
            self.access_token_expires_at_unix = Some(now + expires_in.max(0));
        }
        if let Some(scope) = response.scope {
            self.scopes = scope.split_whitespace().map(str::to_owned).collect();
        }
        if let Some(account) = response.account {
            self.email = account.email_address.or_else(|| self.email.clone());
            self.account_uuid = account.uuid.or_else(|| self.account_uuid.clone());
        }
        if let Some(organization) = response.organization {
            self.organization_uuid = organization.uuid.or_else(|| self.organization_uuid.clone());
        }
        self.last_refresh_unix = Some(now);
    }
}

pub struct ClaudeCodeSubscriptionProvider {
    client: reqwest::Client,
    tokens: Mutex<ClaudeTokens>,
    dirty: AtomicBool,
    session_id: Uuid,
    model_tiers: ModelTiers,
    options: SubscriptionOptions,
    /// Thinking blocks returned alongside tool calls, keyed by the turn's
    /// first tool-call id. With thinking enabled, the API requires a
    /// tool-using assistant turn to be replayed with its thinking intact, and
    /// agent-runtime's `AssistantTurn` has nowhere to carry it.
    thinking: Mutex<HashMap<String, Vec<Value>>>,
}

impl ClaudeCodeSubscriptionProvider {
    pub fn from_secret(secret: &str, options: SubscriptionOptions) -> Result<Self> {
        let tokens =
            serde_json::from_str(secret).context("stored Claude Code credential is invalid")?;
        Ok(Self {
            client: timed_http_client()?,
            tokens: Mutex::new(tokens),
            dirty: AtomicBool::new(false),
            session_id: Uuid::new_v4(),
            model_tiers: ModelTiers::new(
                "claude-sonnet-5-5",
                "claude-haiku-4-5-20251001",
                "claude-opus-5-5",
            ),
            options,
            thinking: Mutex::new(HashMap::new()),
        })
    }

    pub async fn refreshed_credential(&self) -> Result<Option<SecretString>> {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return Ok(None);
        }
        let secret = serde_json::to_string(&*self.tokens.lock().await)
            .context("serialize refreshed Claude Code credential")?;
        Ok(Some(SecretString::from(secret)))
    }

    async fn current_tokens(&self, force_refresh: bool) -> Result<ClaudeTokens> {
        let mut tokens = self.tokens.lock().await;
        if force_refresh || tokens.needs_refresh() {
            self.refresh_tokens(&mut tokens).await?;
        }
        Ok(tokens.clone())
    }

    async fn refresh_tokens(&self, tokens: &mut ClaudeTokens) -> Result<()> {
        if tokens.refresh_token.is_empty() {
            anyhow::bail!("Claude.ai connection needs to be reconnected");
        }
        let response = self
            .client
            .post(CLAUDE_TOKEN_URL)
            .json(&json!({
                "grant_type": "refresh_token",
                "client_id": CLAUDE_CLIENT_ID,
                "refresh_token": tokens.refresh_token,
                "scope": CLAUDE_SCOPE,
            }))
            .send()
            .await
            .context("Claude Code token refresh request failed")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read Claude Code refresh response")?;
        if !status.is_success() {
            anyhow::bail!(
                "Claude.ai connection needs to be reconnected ({status}): {}",
                safe_provider_error(&body)
            );
        }
        let response = serde_json::from_str(&body)
            .context("Claude Code token refresh returned invalid data")?;
        tokens.apply_refresh(response);
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    async fn complete(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        let mut tokens = self.current_tokens(false).await?;
        let mut refreshed = false;
        for attempt in 0..3_u32 {
            match self
                .send_completion(model, system_prompt, history, tools, &tokens)
                .await
            {
                Ok(turn) => return Ok(turn),
                Err(SubscriptionRequestError::Unauthorized) if !refreshed => {
                    tokens = self.current_tokens(true).await?;
                    refreshed = true;
                }
                Err(SubscriptionRequestError::Transient(error)) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(500 * 2_u64.pow(attempt))).await;
                    if attempt == 1 {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error.into_anyhow()),
            }
        }
        anyhow::bail!("Claude Code request exhausted its retry budget")
    }

    async fn send_completion(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
        tokens: &ClaudeTokens,
    ) -> std::result::Result<AssistantTurn, SubscriptionRequestError> {
        let mut body = {
            let thinking = self.thinking.lock().await;
            claude_request_body(
                model,
                system_prompt,
                history,
                tools,
                &self.options,
                &thinking,
            )
        };
        attach_claude_metadata(&mut body, tokens, self.session_id);
        let response = self
            .client
            .post(format!("{CLAUDE_API_BASE}/v1/messages?beta=true"))
            .bearer_auth(&tokens.access_token)
            .header("anthropic-version", CLAUDE_ANTHROPIC_VERSION)
            .header("anthropic-beta", CLAUDE_BETAS)
            .header("anthropic-dangerous-direct-browser-access", "true")
            .header("user-agent", CLAUDE_USER_AGENT)
            .header("x-app", "cli")
            .header("x-claude-code-session-id", self.session_id.to_string())
            .json(&body)
            .send()
            .await
            .map_err(SubscriptionRequestError::from)?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(SubscriptionRequestError::from)?;
        if status == StatusCode::UNAUTHORIZED {
            return Err(SubscriptionRequestError::Unauthorized);
        }
        if status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::BAD_GATEWAY
            || status == StatusCode::SERVICE_UNAVAILABLE
            || status.as_u16() == 529
        {
            return Err(SubscriptionRequestError::Transient(anyhow::anyhow!(
                "Claude Code is temporarily unavailable ({status}): {}",
                safe_provider_error(&body)
            )));
        }
        if !status.is_success() {
            return Err(SubscriptionRequestError::Other(anyhow::anyhow!(
                "Claude Code request failed ({status}): {}",
                safe_provider_error(&body)
            )));
        }
        let (turn, thinking) =
            parse_claude_response(&body).map_err(SubscriptionRequestError::Other)?;
        if let Some(first_call) = turn.tool_calls.first()
            && !thinking.is_empty()
        {
            self.thinking
                .lock()
                .await
                .insert(first_call.id.clone(), thinking);
        }
        Ok(turn)
    }
}

impl ProviderInfo for ClaudeCodeSubscriptionProvider {
    fn kind(&self) -> AgentProviderKind {
        AgentProviderKind::Custom("claude_code_subscription".to_owned())
    }

    fn verbose(&self) -> bool {
        false
    }

    fn model_tiers(&self) -> &ModelTiers {
        &self.model_tiers
    }
}

#[async_trait]
impl TextProvider for ClaudeCodeSubscriptionProvider {
    async fn request_assistant_turn(
        &self,
        model: &str,
        system_prompt: &str,
        history: &[ChatMessage],
        tool_definitions: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        self.complete(model, system_prompt, history, tool_definitions)
            .await
    }

    async fn stream_message(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        sink: &mut dyn EventSink,
    ) -> Result<String> {
        let turn = self.complete(model, system_prompt, messages, &[]).await?;
        let content = turn.content.unwrap_or_default();
        sink.emit(RuntimeEvent::AssistantDelta {
            delta: content.clone(),
        })
        .await?;
        Ok(content)
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeRefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    account: Option<ClaudeAccount>,
    #[serde(default)]
    organization: Option<ClaudeOrganization>,
}

#[derive(Debug, Deserialize)]
struct ClaudeAccount {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    email_address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOrganization {
    #[serde(default)]
    uuid: Option<String>,
}

fn claude_request_body(
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    tools: &[ToolDefinition],
    options: &SubscriptionOptions,
    thinking: &HashMap<String, Vec<Value>>,
) -> Value {
    let system = vec![
        json!({ "type": "text", "text": CLAUDE_BILLING_HEADER }),
        json!({
            "type": "text",
            "text": CLAUDE_SYSTEM_PREFIX,
            "cache_control": { "type": "ephemeral", "ttl": "1h" },
        }),
        json!({
            "type": "text",
            "text": system_prompt,
            "cache_control": { "type": "ephemeral", "ttl": "1h" },
        }),
    ];
    let mut messages = Vec::new();
    for message in history {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => push_claude_message(
                &mut messages,
                "user",
                vec![json!({
                    "type": "text",
                    "text": message.content.as_deref().unwrap_or_default(),
                })],
            ),
            MessageRole::Assistant => {
                let mut blocks = message
                    .tool_calls
                    .first()
                    .and_then(|call| thinking.get(&call.id))
                    .cloned()
                    .unwrap_or_default();
                if let Some(content) = message.content.as_deref().filter(|text| !text.is_empty()) {
                    blocks.push(json!({ "type": "text", "text": content }));
                }
                blocks.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    })
                }));
                push_claude_message(&mut messages, "assistant", blocks);
            }
            MessageRole::Tool => push_claude_message(
                &mut messages,
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.as_deref().unwrap_or_default(),
                    "content": message.content.as_deref().unwrap_or_default(),
                })],
            ),
        }
    }
    json!({
        "model": model,
        "max_tokens": options.max_tokens,
        "system": system,
        "messages": messages,
        "tools": tools.iter().map(|tool| json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.input_schema,
        })).collect::<Vec<_>>(),
        "thinking": { "type": "adaptive", "display": "omitted" },
        "context_management": {
            "edits": [{ "type": "clear_thinking_20251015", "keep": "all" }],
        },
        "output_config": { "effort": options.effort.as_deref().unwrap_or("high") },
        "stream": false,
    })
}

fn push_claude_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(content) = last.get_mut("content").and_then(Value::as_array_mut)
    {
        content.extend(blocks);
        return;
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

fn attach_claude_metadata(body: &mut Value, tokens: &ClaudeTokens, session_id: Uuid) {
    let account_uuid = tokens.account_uuid.as_deref().unwrap_or_default();
    let device_id = format!(
        "{:x}",
        Sha256::digest(format!("gridops:{account_uuid}").as_bytes())
    );
    body["metadata"] = json!({
        "user_id": json!({
            "device_id": device_id,
            "account_uuid": account_uuid,
            "session_id": session_id,
        }).to_string(),
    });
}

/// The turn, plus any thinking blocks to replay with it.
fn parse_claude_response(body: &str) -> Result<(AssistantTurn, Vec<Value>)> {
    let response: Value = serde_json::from_str(body)
        .with_context(|| format!("parse Claude Code response: {}", safe_provider_error(body)))?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut thinking = Vec::new();
    if let Some(blocks) = response.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "thinking" | "redacted_thinking" => thinking.push(block.clone()),
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
                "tool_use" => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !id.is_empty() && !name.is_empty() {
                        tool_calls.push(ToolCall {
                            id: id.to_owned(),
                            name: name.to_owned(),
                            arguments: block
                                .get("input")
                                .cloned()
                                .unwrap_or_else(|| Value::Object(Map::new())),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    if content.is_empty() && tool_calls.is_empty() {
        let stop_reason = response
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        anyhow::bail!(
            "Claude Code returned no usable assistant content (stop reason: {stop_reason})"
        );
    }
    Ok((
        AssistantTurn {
            content: (!content.is_empty()).then_some(content),
            tool_calls,
        },
        thinking,
    ))
}

#[derive(Debug)]
enum SubscriptionRequestError {
    Unauthorized,
    Transient(anyhow::Error),
    Other(anyhow::Error),
}

impl SubscriptionRequestError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Unauthorized => anyhow::anyhow!("provider connection needs to be reconnected"),
            Self::Transient(error) | Self::Other(error) => error,
        }
    }
}

impl From<reqwest::Error> for SubscriptionRequestError {
    fn from(error: reqwest::Error) -> Self {
        Self::Other(error.into())
    }
}

fn jwt_payload(token: &str) -> Result<Value> {
    let payload = token.split('.').nth(1).context("token is not a JWT")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decode JWT payload")?;
    serde_json::from_slice(&decoded).context("parse JWT payload")
}

fn safe_provider_error(value: &str) -> String {
    const MAX_CHARS: usize = 400;
    let mut clean = value.replace(['\n', '\r'], " ");
    if clean.chars().count() > MAX_CHARS {
        clean = clean.chars().take(MAX_CHARS).collect::<String>();
        clean.push('…');
    }
    clean
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use agent_runtime::{ChatMessage, ToolCall};
    use serde_json::json;

    use super::{
        SubscriptionOptions, claude_request_body, codex_request_body, parse_claude_response,
        parse_codex_sse,
    };

    #[test]
    fn codex_request_keeps_tool_results_structured() {
        let history = vec![
            ChatMessage::user("What is ready?"),
            ChatMessage::assistant_with_tools(
                None,
                vec![ToolCall {
                    id: "call-1".into(),
                    name: "inventory".into(),
                    arguments: json!({}),
                }],
            ),
            ChatMessage::tool("call-1", r#"{"documents":0}"#),
        ];
        let body = codex_request_body("gpt-test", "system", &history, &[]);
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][2]["type"], "function_call_output");
    }

    #[test]
    fn codex_sse_collects_text() -> anyhow::Result<()> {
        let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Ngoni\"}\n\n";
        let turn = parse_codex_sse(body)?;
        assert_eq!(turn.content.as_deref(), Some("Hello Ngoni"));
        Ok(())
    }

    #[test]
    fn claude_request_keeps_tool_results_structured() {
        let history = vec![
            ChatMessage::assistant_with_tools(
                None,
                vec![ToolCall {
                    id: "call-1".into(),
                    name: "inventory".into(),
                    arguments: json!({}),
                }],
            ),
            ChatMessage::tool("call-1", "done"),
        ];
        let body = claude_request_body(
            "claude-test",
            "system",
            &history,
            &[],
            &SubscriptionOptions::default(),
            &HashMap::new(),
        );
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["output_config"]["effort"], "high");
    }

    #[test]
    fn claude_replays_thinking_before_the_tool_call_it_preceded() -> anyhow::Result<()> {
        let response = json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "tool_use", "id": "call-7", "name": "read_file", "input": { "path": "a" } },
            ],
            "stop_reason": "tool_use",
        });
        let (turn, thinking) = parse_claude_response(&response.to_string())?;
        assert_eq!(thinking.len(), 1);
        let history = vec![
            ChatMessage::user("fix it"),
            ChatMessage::assistant_with_tools(turn.content, turn.tool_calls),
            ChatMessage::tool("call-7", "contents"),
        ];
        let options = SubscriptionOptions {
            max_tokens: 32_000,
            effort: Some("medium".into()),
        };
        let replay = HashMap::from([("call-7".to_owned(), thinking)]);
        let body = claude_request_body("claude-test", "system", &history, &[], &options, &replay);
        let assistant = &body["messages"][1]["content"];
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["signature"], "sig");
        assert_eq!(assistant[1]["type"], "tool_use");
        assert_eq!(body["max_tokens"], 32_000);
        assert_eq!(body["output_config"]["effort"], "medium");
        Ok(())
    }
}
