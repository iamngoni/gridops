use anyhow::{Context as _, Result, bail};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const API_BASE_URL: &str = "https://api.bitbucket.org/2.0/";

/// A Bitbucket Pipelines runner is registered either for one repository or an
/// entire workspace. The latter is the useful default for a `GridOps` pool: it
/// lets one local capacity pool serve several repositories without duplicating
/// runner credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitbucketRunnerTarget {
    Workspace {
        workspace: String,
    },
    Repository {
        workspace: String,
        repository: String,
    },
}

impl BitbucketRunnerTarget {
    pub fn validate(&self) -> Result<()> {
        let valid_segment = |value: &str| {
            !value.is_empty()
                && value.len() <= 255
                && !value.contains(['/', '?', '#', '\\', '\r', '\n'])
        };
        match self {
            Self::Workspace { workspace } => {
                if valid_segment(workspace) {
                    Ok(())
                } else {
                    bail!("Bitbucket workspace is invalid")
                }
            }
            Self::Repository {
                workspace,
                repository,
            } => {
                if valid_segment(workspace) && valid_segment(repository) {
                    Ok(())
                } else {
                    bail!("Bitbucket workspace or repository is invalid")
                }
            }
        }
    }

    fn runner_path(&self) -> String {
        match self {
            Self::Workspace { workspace } => {
                format!("workspaces/{workspace}/pipelines-config/runners")
            }
            Self::Repository {
                workspace,
                repository,
            } => format!("repositories/{workspace}/{repository}/pipelines-config/runners"),
        }
    }
}

#[derive(Clone)]
pub struct BitbucketClient {
    http: reqwest::Client,
    base_url: Url,
}

impl BitbucketClient {
    pub fn new() -> Result<Self> {
        Self::with_base_url(API_BASE_URL)
    }

    pub fn with_base_url(base_url: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("GridOps/0.1")
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base_url: Url::parse(base_url).context("Bitbucket API base URL is invalid")?,
        })
    }

    pub async fn create_runner(
        &self,
        target: &BitbucketRunnerTarget,
        access_token: &str,
        name: &str,
        labels: &[String],
    ) -> Result<BitbucketRunner> {
        target.validate()?;
        if name.is_empty() || name.len() > 255 || name.contains(['\r', '\n']) {
            bail!("Bitbucket runner name is invalid")
        }
        if labels.len() > 10
            || labels.iter().any(|label| {
                label.is_empty()
                    || label.len() > 255
                    || !label.chars().all(|character| {
                        character.is_ascii_lowercase()
                            || character.is_ascii_digit()
                            || character == '.'
                    })
            })
        {
            bail!("Bitbucket runner labels must contain lowercase letters, numbers, or dots")
        }
        self.request(
            Method::POST,
            &target.runner_path(),
            access_token,
            Some(json!({ "name": name, "labels": labels })),
        )
        .await
    }

    pub async fn workspace(
        &self,
        workspace: &str,
        access_token: &str,
    ) -> Result<BitbucketWorkspace> {
        BitbucketRunnerTarget::Workspace {
            workspace: workspace.to_owned(),
        }
        .validate()?;
        self.request(
            Method::GET,
            &format!("workspaces/{workspace}"),
            access_token,
            None,
        )
        .await
    }

    pub async fn delete_runner(
        &self,
        target: &BitbucketRunnerTarget,
        access_token: &str,
        runner_uuid: &str,
    ) -> Result<()> {
        target.validate()?;
        if !valid_uuid(runner_uuid) {
            bail!("Bitbucket runner UUID is invalid")
        }
        let response = self
            .send(
                Method::DELETE,
                &format!("{}/{runner_uuid}", target.runner_path()),
                access_token,
                None,
            )
            .await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let status = response.status();
        let details = response.text().await.unwrap_or_default();
        bail!(
            "Bitbucket API delete failed ({status}): {}",
            details.chars().take(500).collect::<String>()
        )
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        access_token: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let response = self.send(method, path, access_token, body).await?;
        response
            .json()
            .await
            .context("invalid Bitbucket API response")
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        access_token: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        let url = self.base_url.join(path)?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(access_token)
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let details = response.text().await.unwrap_or_default();
        bail!(
            "Bitbucket API request failed ({status}): {}",
            details.chars().take(500).collect::<String>()
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitbucketRunner {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub state: Option<BitbucketRunnerState>,
    pub oauth_client: BitbucketRunnerOAuthClient,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitbucketRunnerState {
    pub status: Option<String>,
    pub cordoned: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitbucketRunnerOAuthClient {
    pub id: String,
    /// Bitbucket returns this exactly once, when the runner is created. It
    /// must be sealed immediately and never surfaced by a `GridOps` response.
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitbucketWorkspace {
    pub uuid: String,
    pub slug: String,
    pub name: Option<String>,
}

fn valid_uuid(value: &str) -> bool {
    let value = value.trim_matches(['{', '}']);
    uuid::Uuid::parse_str(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_targets_build_scoped_api_paths() {
        assert_eq!(
            BitbucketRunnerTarget::Workspace {
                workspace: "acme".into()
            }
            .runner_path(),
            "workspaces/acme/pipelines-config/runners"
        );
        assert_eq!(
            BitbucketRunnerTarget::Repository {
                workspace: "acme".into(),
                repository: "ios-app".into()
            }
            .runner_path(),
            "repositories/acme/ios-app/pipelines-config/runners"
        );
    }

    #[test]
    fn runner_target_rejects_path_injection() {
        assert!(
            BitbucketRunnerTarget::Workspace {
                workspace: "acme/../other".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn bitbucket_runner_secret_is_deserialized_but_not_required_after_creation() -> Result<()> {
        let created: BitbucketRunner = serde_json::from_value(json!({
            "uuid": "{da18d7c2-1d42-4c9a-bac8-4f5227d8ec17}",
            "name": "gridops-macos-01",
            "oauth_client": { "id": "client", "secret": "only-once" }
        }))?;
        assert_eq!(created.oauth_client.secret.as_deref(), Some("only-once"));
        Ok(())
    }
}
