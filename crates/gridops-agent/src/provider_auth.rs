// Adapted from Heimdall's Codex and Claude Code OAuth implementations, by way
// of ccs. GridOps changes: rand 0.10, timed HTTP client, bounded error text.
// Copyright (c) 2026 Codecraft Solutions ZA. All rights reserved.
// SPDX-License-Identifier: LicenseRef-Heimdall-FSL

use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_ISSUER: &str = "https://auth.openai.com";
const CODEX_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
const CODEX_CALLBACK_PATH: &str = "/auth/callback";

const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const CLAUDE_SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

#[derive(Debug)]
struct PkceCodes {
    verifier: String,
    challenge: String,
}

pub struct PreparedOAuth {
    pub authorize_url: String,
    pub state: String,
    pub code_verifier: String,
    pub callback_port: Option<i32>,
}

pub struct ExchangedCredential {
    pub account_label: String,
    pub secret: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CodexStoredTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_type: Option<String>,
    connected_at_unix: i64,
}

#[derive(Debug, Deserialize)]
struct CodexTokenResponse {
    #[serde(rename = "id_token")]
    id: String,
    #[serde(rename = "access_token")]
    access: String,
    #[serde(rename = "refresh_token")]
    refresh: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeStoredTokens {
    access_token: String,
    refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    organization_uuid: Option<String>,
    scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token_expires_at_unix: Option<i64>,
    connected_at_unix: i64,
}

#[derive(Debug, Deserialize)]
struct ClaudeTokenResponse {
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

pub fn prepare(provider: &str) -> anyhow::Result<PreparedOAuth> {
    let pkce = generate_pkce();
    let state = generate_state();

    match provider {
        "codex" => {
            let callback_port = 1455;
            let redirect_uri = format!("http://localhost:{callback_port}{CODEX_CALLBACK_PATH}");
            let mut url = reqwest::Url::parse(&format!("{CODEX_ISSUER}/oauth/authorize"))
                .context("build Codex authorization URL")?;
            url.query_pairs_mut()
                .append_pair("response_type", "code")
                .append_pair("client_id", CODEX_CLIENT_ID)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", CODEX_SCOPE)
                .append_pair("code_challenge", &pkce.challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("id_token_add_organizations", "true")
                .append_pair("codex_cli_simplified_flow", "true")
                .append_pair("state", &state)
                .append_pair("originator", CODEX_ORIGINATOR);

            Ok(PreparedOAuth {
                authorize_url: url.to_string(),
                state,
                code_verifier: pkce.verifier,
                callback_port: Some(callback_port),
            })
        }
        "claude_code" => {
            let mut url = reqwest::Url::parse(CLAUDE_AUTHORIZE_URL)
                .context("build Claude authorization URL")?;
            url.query_pairs_mut()
                .append_pair("code", "true")
                .append_pair("client_id", CLAUDE_CLIENT_ID)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", CLAUDE_REDIRECT_URI)
                .append_pair("scope", CLAUDE_SCOPE)
                .append_pair("code_challenge", &pkce.challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", &state);

            Ok(PreparedOAuth {
                authorize_url: url.to_string(),
                state,
                code_verifier: pkce.verifier,
                callback_port: None,
            })
        }
        _ => anyhow::bail!("unsupported subscription provider"),
    }
}

pub fn split_pasted_value(provider: &str, pasted: &str) -> (String, Option<String>) {
    match provider {
        "codex" => split_codex_callback(pasted),
        "claude_code" => split_claude_code(pasted),
        _ => (String::new(), None),
    }
}

pub async fn exchange(
    provider: &str,
    code_verifier: &str,
    state: &str,
    callback_port: Option<i32>,
    code: &str,
) -> anyhow::Result<ExchangedCredential> {
    match provider {
        "codex" => exchange_codex(code_verifier, callback_port, code).await,
        "claude_code" => exchange_claude(code_verifier, state, code).await,
        _ => anyhow::bail!("unsupported subscription provider"),
    }
}

async fn exchange_codex(
    code_verifier: &str,
    callback_port: Option<i32>,
    code: &str,
) -> anyhow::Result<ExchangedCredential> {
    let callback_port = callback_port.context("Codex callback port is missing")?;
    let redirect_uri = format!("http://localhost:{callback_port}{CODEX_CALLBACK_PATH}");
    let response = exchange_client()?
        .post(format!("{CODEX_ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("originator", CODEX_ORIGINATOR)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", CODEX_CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("Codex token exchange request failed")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "ChatGPT rejected the sign-in ({status}): {}",
            bounded(&body)
        );
    }

    let response: CodexTokenResponse = response
        .json()
        .await
        .context("Codex token exchange returned invalid data")?;
    let claims = jwt_payload(&response.id).context("parse Codex account details")?;
    let auth_claims = claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    let profile_claims = claims
        .get("https://api.openai.com/profile")
        .and_then(Value::as_object);
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| {
            profile_claims
                .and_then(|profile| profile.get("email"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned);
    let account_id = auth_claims
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let plan_type = auth_claims
        .and_then(|auth| auth.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let label = email
        .as_deref()
        .map_or_else(|| "ChatGPT subscription".to_owned(), ToOwned::to_owned);
    let tokens = CodexStoredTokens {
        id_token: response.id,
        access_token: response.access,
        refresh_token: response.refresh,
        account_id,
        email,
        plan_type,
        connected_at_unix: OffsetDateTime::now_utc().unix_timestamp(),
    };

    Ok(ExchangedCredential {
        account_label: label,
        secret: serde_json::to_string(&tokens).context("serialize Codex credential")?,
    })
}

async fn exchange_claude(
    code_verifier: &str,
    state: &str,
    code: &str,
) -> anyhow::Result<ExchangedCredential> {
    let response = exchange_client()?
        .post(CLAUDE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLAUDE_CLIENT_ID,
            "code": code,
            "redirect_uri": CLAUDE_REDIRECT_URI,
            "code_verifier": code_verifier,
            "state": state,
        }))
        .send()
        .await
        .context("Claude token exchange request failed")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Claude rejected the sign-in ({status}): {}", bounded(&body));
    }

    let response: ClaudeTokenResponse = response
        .json()
        .await
        .context("Claude token exchange returned invalid data")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let email = response
        .account
        .as_ref()
        .and_then(|account| account.email_address.clone());
    let label = email
        .as_deref()
        .map_or_else(|| "Claude.ai subscription".to_owned(), ToOwned::to_owned);
    let tokens = ClaudeStoredTokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token.unwrap_or_default(),
        email,
        account_uuid: response.account.and_then(|account| account.uuid),
        organization_uuid: response
            .organization
            .and_then(|organization| organization.uuid),
        scopes: response
            .scope
            .as_deref()
            .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
        access_token_expires_at_unix: response.expires_in.map(|seconds| now + seconds.max(0)),
        connected_at_unix: now,
    };

    Ok(ExchangedCredential {
        account_label: label,
        secret: serde_json::to_string(&tokens).context("serialize Claude credential")?,
    })
}

fn exchange_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .context("build OAuth HTTP client")
}

fn bounded(body: &str) -> String {
    body.replace(['\n', '\r'], " ").chars().take(300).collect()
}

fn generate_pkce() -> PkceCodes {
    let bytes: [u8; 64] = rand::rng().random();
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    PkceCodes {
        verifier,
        challenge,
    }
}

fn generate_state() -> String {
    URL_SAFE_NO_PAD.encode(rand::rng().random::<[u8; 32]>())
}

fn split_codex_callback(pasted: &str) -> (String, Option<String>) {
    let trimmed = pasted.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }

    if let Ok(url) = reqwest::Url::parse(trimmed) {
        let mut code = None;
        let mut state = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.trim().to_owned()),
                "state" => state = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        if code.is_some() || state.is_some() {
            return (
                code.unwrap_or_default(),
                state.filter(|value| !value.is_empty()),
            );
        }
    }

    let mut parts = trimmed.splitn(2, '#');
    let code = parts.next().unwrap_or_default().trim().to_owned();
    let state = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    (code, state)
}

fn split_claude_code(pasted: &str) -> (String, Option<String>) {
    let trimmed = pasted.trim();
    let after_url = trimmed
        .rsplit("code=")
        .next()
        .map_or(trimmed, |tail| tail.split('&').next().unwrap_or(tail));
    let mut parts = after_url.splitn(2, '#');
    let code = parts.next().unwrap_or_default().trim().to_owned();
    let state = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    (code, state)
}

fn jwt_payload(token: &str) -> anyhow::Result<Value> {
    let payload = token
        .split('.')
        .nth(1)
        .filter(|payload| !payload.is_empty())
        .context("invalid JWT")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decode JWT account details")?;
    serde_json::from_slice(&decoded).context("parse JWT account details")
}

#[cfg(test)]
mod tests {
    use super::{prepare, split_pasted_value};

    #[test]
    fn codex_callback_url_yields_code_and_state() {
        let (code, state) = split_pasted_value(
            "codex",
            "http://localhost:1455/auth/callback?code=abc.123&state=state-456",
        );
        assert_eq!(code, "abc.123");
        assert_eq!(state.as_deref(), Some("state-456"));
    }

    #[test]
    fn claude_blob_yields_code_and_state() {
        let (code, state) = split_pasted_value("claude_code", "code-123#state-456");
        assert_eq!(code, "code-123");
        assert_eq!(state.as_deref(), Some("state-456"));
    }

    #[test]
    fn authorization_requests_use_pkce() -> anyhow::Result<()> {
        let codex = prepare("codex")?;
        assert!(codex.authorize_url.contains("code_challenge="));
        assert_eq!(codex.callback_port, Some(1455));

        let claude = prepare("claude_code")?;
        assert!(claude.authorize_url.contains("code_challenge="));
        assert!(claude.callback_port.is_none());
        assert!(prepare("openai").is_err());
        Ok(())
    }

    #[test]
    fn claude_callback_url_yields_code() {
        let (code, state) = split_pasted_value(
            "claude_code",
            "https://platform.claude.com/oauth/code/callback?code=abc#state-1",
        );
        assert_eq!(code, "abc");
        assert_eq!(state.as_deref(), Some("state-1"));
    }
}
