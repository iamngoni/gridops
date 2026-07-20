use anyhow::{Context as _, Result};
use gridops_core::{Config, GitHubClient, Vault};
use reqwest::Url;
use secrecy::ExposeSecret;
use sqlx::SqlitePool;

pub const GITHUB_CLIENT_ID: &str = "github.client_id";
pub const GITHUB_CLIENT_SECRET: &str = "github.client_secret";
pub const GITHUB_APP_ID: &str = "github.app_id";
pub const GITHUB_APP_PRIVATE_KEY: &str = "github.app_private_key";
pub const GITHUB_APP_SLUG: &str = "github.app_slug";
pub const GITHUB_WEBHOOK_SECRET: &str = "github.webhook_secret";

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub database: SqlitePool,
    pub vault: Vault,
    pub github: GitHubClient,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(
        config: Config,
        database: SqlitePool,
        vault: Vault,
        github: GitHubClient,
    ) -> Result<Self> {
        Ok(Self {
            config,
            database,
            vault,
            github,
            http: reqwest::Client::builder()
                .user_agent("GridOps/0.1")
                .build()?,
        })
    }

    pub fn manager_url(&self, path: &str) -> Result<Url> {
        Ok(self
            .config
            .manager_url()
            .join(path.trim_start_matches('/'))?)
    }

    pub fn manager_token(&self) -> Option<&str> {
        self.config.manager_token().map(ExposeSecret::expose_secret)
    }

    pub async fn runtime_secret(&self, key: &str) -> Result<Option<String>> {
        let sealed =
            sqlx::query_scalar::<_, String>("SELECT value FROM runtime_secrets WHERE key=?")
                .bind(key)
                .fetch_optional(&self.database)
                .await?;
        sealed
            .map(|value| {
                self.vault
                    .open(&value)
                    .context("could not decrypt runtime secret")
            })
            .transpose()
    }

    pub async fn github_oauth_credentials(&self) -> Result<Option<(String, String)>> {
        let client_id = match self.runtime_secret(GITHUB_CLIENT_ID).await? {
            Some(value) => Some(value),
            None => self.config.github_client_id().map(ToOwned::to_owned),
        };
        let client_secret = match self.runtime_secret(GITHUB_CLIENT_SECRET).await? {
            Some(value) => Some(value),
            None => self
                .config
                .github_client_secret()
                .map(|value| value.expose_secret().to_owned()),
        };
        Ok(client_id.zip(client_secret))
    }

    pub async fn github_app_credentials(&self) -> Result<Option<(String, String)>> {
        let app_id = match self.runtime_secret(GITHUB_APP_ID).await? {
            Some(value) => Some(value),
            None => self.config.github_app_id().map(ToOwned::to_owned),
        };
        let private_key = match self.runtime_secret(GITHUB_APP_PRIVATE_KEY).await? {
            Some(value) => Some(value),
            None => self
                .config
                .github_app_private_key()
                .map(|value| value.expose_secret().to_owned()),
        };
        Ok(app_id.zip(private_key))
    }

    pub async fn github_app_slug(&self) -> Result<String> {
        Ok(self
            .runtime_secret(GITHUB_APP_SLUG)
            .await?
            .unwrap_or_else(|| self.config.github_app_slug().to_owned()))
    }

    pub async fn github_webhook_secret(&self) -> Result<Option<String>> {
        match self.runtime_secret(GITHUB_WEBHOOK_SECRET).await? {
            Some(value) => Ok(Some(value)),
            None => Ok(self
                .config
                .github_webhook_secret()
                .map(|value| value.expose_secret().to_owned())),
        }
    }

    pub async fn installation_token(&self, installation_id: i64) -> Result<Option<String>> {
        let Some((app_id, private_key)) = self.github_app_credentials().await? else {
            return Ok(None);
        };
        self.github
            .installation_token_with_credentials(installation_id, &app_id, &private_key)
            .await
            .map(Some)
    }

    pub async fn validate_api(&self) -> Result<()> {
        self.github_oauth_credentials()
            .await?
            .context("GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET are required")?;
        Ok(())
    }
}
