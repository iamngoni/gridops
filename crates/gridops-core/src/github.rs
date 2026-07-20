use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Method, StatusCode};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::{Config, now_millis};

const API_VERSION: &str = "2026-03-10";

#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    config: Config,
    installation_tokens: Arc<RwLock<HashMap<i64, CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: i64,
}

#[derive(Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

impl GitHubClient {
    pub fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("GridOps/0.1")
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            config,
            installation_tokens: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let mut request = self
            .http
            .request(method, format!("https://api.github.com{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let details = response.text().await.unwrap_or_default();
            bail!(
                "GitHub API request failed ({status}): {}",
                details.chars().take(500).collect::<String>()
            );
        }
        if response.status() == StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .context("expected an empty GitHub response");
        }
        response.json().await.context("invalid GitHub response")
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str, token: &str) -> Result<T> {
        self.request(Method::GET, path, token, None).await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        body: Value,
    ) -> Result<T> {
        self.request(Method::POST, path, token, Some(body)).await
    }

    pub async fn delete(&self, path: &str, token: &str) -> Result<()> {
        let response = self
            .http
            .delete(format!("https://api.github.com{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .send()
            .await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let status = response.status();
        let details = response.text().await.unwrap_or_default();
        bail!(
            "GitHub API delete failed ({status}): {}",
            details.chars().take(500).collect::<String>()
        );
    }

    pub async fn installation_token(&self, installation_id: i64) -> Result<Option<String>> {
        let (Some(app_id), Some(private_key)) = (
            self.config.github_app_id(),
            self.config.github_app_private_key(),
        ) else {
            return Ok(None);
        };
        if let Some(cached) = self.installation_tokens.read().await.get(&installation_id)
            && cached.expires_at > now_millis() + 5 * 60_000
        {
            return Ok(Some(cached.token.clone()));
        }
        let now = chrono::Utc::now().timestamp();
        let claims = AppClaims {
            iat: now - 60,
            exp: now + 9 * 60,
            iss: app_id.to_owned(),
        };
        let key = EncodingKey::from_rsa_pem(private_key.expose_secret().as_bytes())?;
        let jwt = encode(&Header::new(Algorithm::RS256), &claims, &key)?;
        let response: InstallationToken = self
            .post(
                &format!("/app/installations/{installation_id}/access_tokens"),
                &jwt,
                json!({}),
            )
            .await?;
        let expires_at =
            chrono::DateTime::parse_from_rfc3339(&response.expires_at)?.timestamp_millis();
        self.installation_tokens.write().await.insert(
            installation_id,
            CachedToken {
                token: response.token.clone(),
                expires_at,
            },
        );
        Ok(Some(response.token))
    }

    pub async fn generate_jit_config(
        &self,
        target: RunnerTarget<'_>,
        token: &str,
        request: &JitRequest,
    ) -> Result<JitResponse> {
        let path = match target {
            RunnerTarget::Repository { owner, repository } => {
                format!("/repos/{owner}/{repository}/actions/runners/generate-jitconfig")
            }
            RunnerTarget::Organization { organization } => {
                format!("/orgs/{organization}/actions/runners/generate-jitconfig")
            }
        };
        self.post(&path, token, serde_json::to_value(request)?)
            .await
    }
}

#[derive(Deserialize)]
struct InstallationToken {
    token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
pub struct InstallationPage {
    pub installations: Vec<GitHubInstallation>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubInstallation {
    pub id: i64,
    pub account: Option<GitHubAccount>,
    pub target_type: String,
    pub repository_selection: String,
    #[serde(default)]
    pub permissions: Value,
    #[serde(default)]
    pub events: Vec<String>,
    pub suspended_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAccount {
    pub id: i64,
    pub login: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
pub struct RepositoryPage {
    pub repositories: Vec<GitHubRepository>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubRepository {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub archived: bool,
    pub default_branch: String,
    pub html_url: String,
    pub updated_at: Option<String>,
    pub owner: GitHubOwner,
    pub permissions: Option<HashMap<String, bool>>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubOwner {
    pub login: String,
}

pub enum RunnerTarget<'a> {
    Repository { owner: &'a str, repository: &'a str },
    Organization { organization: &'a str },
}

#[derive(Serialize)]
pub struct JitRequest {
    pub name: String,
    pub runner_group_id: i64,
    pub labels: Vec<String>,
    pub work_folder: String,
}

#[derive(Deserialize)]
pub struct JitResponse {
    pub runner: JitRunner,
    pub encoded_jit_config: String,
}

#[derive(Deserialize)]
pub struct JitRunner {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub busy: bool,
}
