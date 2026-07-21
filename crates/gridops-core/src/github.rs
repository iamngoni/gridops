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
    app_id: String,
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
        let response = self.send(method, path, token, body).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .context("expected an empty GitHub response");
        }
        response.json().await.context("invalid GitHub response")
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
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
        Ok(response)
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

    pub async fn post_empty(&self, path: &str, token: &str, body: Value) -> Result<()> {
        self.send(Method::POST, path, token, Some(body)).await?;
        Ok(())
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
        self.installation_token_with_credentials(
            installation_id,
            app_id,
            private_key.expose_secret(),
        )
        .await
        .map(Some)
    }

    pub async fn installation_token_with_credentials(
        &self,
        installation_id: i64,
        app_id: &str,
        private_key: &str,
    ) -> Result<String> {
        if let Some(cached) = self.installation_tokens.read().await.get(&installation_id)
            && cached.app_id == app_id
            && cached.expires_at > now_millis() + 5 * 60_000
        {
            return Ok(cached.token.clone());
        }
        let now = chrono::Utc::now().timestamp();
        let claims = AppClaims {
            iat: now - 60,
            exp: now + 9 * 60,
            iss: app_id.to_owned(),
        };
        let key = EncodingKey::from_rsa_pem(private_key.as_bytes())?;
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
                app_id: app_id.to_owned(),
                token: response.token.clone(),
                expires_at,
            },
        );
        Ok(response.token)
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

    pub async fn generate_registration_token(
        &self,
        target: RunnerTarget<'_>,
        token: &str,
    ) -> Result<RegistrationToken> {
        let path = match target {
            RunnerTarget::Repository { owner, repository } => {
                format!("/repos/{owner}/{repository}/actions/runners/registration-token")
            }
            RunnerTarget::Organization { organization } => {
                format!("/orgs/{organization}/actions/runners/registration-token")
            }
        };
        self.post(&path, token, json!({})).await
    }

    pub async fn runner_by_name(
        &self,
        target: RunnerTarget<'_>,
        token: &str,
        name: &str,
    ) -> Result<Option<JitRunner>> {
        for page in 1..=100 {
            let path = match target {
                RunnerTarget::Repository { owner, repository } => {
                    format!("/repos/{owner}/{repository}/actions/runners?per_page=100&page={page}")
                }
                RunnerTarget::Organization { organization } => {
                    format!("/orgs/{organization}/actions/runners?per_page=100&page={page}")
                }
            };
            let response: RunnerPage = self.get(&path, token).await?;
            let final_page = response.runners.len() < 100;
            if let Some(runner) = response
                .runners
                .into_iter()
                .find(|runner| runner.name == name)
            {
                return Ok(Some(runner));
            }
            if final_page {
                break;
            }
        }
        Ok(None)
    }

    pub async fn runner_group_name(
        &self,
        organization: &str,
        runner_group_id: i64,
        token: &str,
    ) -> Result<String> {
        let group: RunnerGroup = self
            .get(
                &format!("/orgs/{organization}/actions/runner-groups/{runner_group_id}"),
                token,
            )
            .await?;
        Ok(group.name)
    }

    pub async fn runner_groups(&self, organization: &str, token: &str) -> Result<Vec<RunnerGroup>> {
        let mut groups = Vec::new();
        for page in 1..=100 {
            let response: RunnerGroupPage = self
                .get(
                    &format!("/orgs/{organization}/actions/runner-groups?per_page=100&page={page}"),
                    token,
                )
                .await?;
            let final_page = response.runner_groups.len() < 100;
            groups.extend(response.runner_groups);
            if final_page {
                break;
            }
        }
        Ok(groups)
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
pub struct GitHubOrganizationMembership {
    pub state: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct RepositoryPage {
    pub total_count: i64,
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
pub struct WorkflowJobPage {
    pub jobs: Vec<GitHubWorkflowJob>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubWorkflowJob {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub runner_id: Option<i64>,
    pub runner_name: Option<String>,
    pub runner_group_id: Option<i64>,
    pub runner_group_name: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub html_url: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRunPage {
    pub workflow_runs: Vec<GitHubWorkflowRun>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubWorkflowRun {
    pub id: i64,
    pub workflow_id: Option<i64>,
    pub name: Option<String>,
    pub display_title: Option<String>,
    pub run_number: i64,
    pub run_attempt: i64,
    pub event: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: String,
    pub actor: Option<WorkflowActor>,
    pub html_url: String,
    pub run_started_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowActor {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct GitHubOwner {
    pub login: String,
}

#[derive(Clone, Copy)]
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

#[derive(Debug, Deserialize)]
pub struct JitRunner {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Deserialize)]
pub struct RegistrationToken {
    pub token: String,
    pub expires_at: String,
}

#[derive(Deserialize)]
struct RunnerPage {
    runners: Vec<JitRunner>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunnerGroup {
    pub id: i64,
    pub name: String,
    pub visibility: String,
    #[serde(rename = "default")]
    pub is_default: bool,
}

#[derive(Deserialize)]
struct RunnerGroupPage {
    runner_groups: Vec<RunnerGroup>,
}
