use anyhow::Result;
use gridops_core::{Config, GitHubClient, Vault};
use reqwest::Url;
use secrecy::ExposeSecret;
use sqlx::SqlitePool;

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
}
