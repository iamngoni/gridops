use std::{env, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use secrecy::SecretString;
use url::Url;

#[derive(Clone)]
pub struct Config(Arc<Inner>);

struct Inner {
    base_url: Url,
    database_path: PathBuf,
    log_directory: PathBuf,
    github_client_id: Option<String>,
    github_client_secret: Option<SecretString>,
    github_app_id: Option<String>,
    github_app_private_key: Option<SecretString>,
    github_app_slug: String,
    github_webhook_secret: Option<SecretString>,
    github_webhook_active: Option<bool>,
    session_secret: Option<SecretString>,
    encryption_key: Option<SecretString>,
    manager_url: Url,
    manager_token: Option<SecretString>,
    runner_network: String,
    runner_image: String,
    api_bind: String,
    manager_bind: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base_url =
            env::var("GRIDOPS_BASE_URL").unwrap_or_else(|_| "http://localhost:3000".into());
        let manager_url =
            env::var("GRIDOPS_MANAGER_URL").unwrap_or_else(|_| "http://localhost:8788".into());
        Ok(Self(Arc::new(Inner {
            base_url: Url::parse(&base_url).context("GRIDOPS_BASE_URL must be a URL")?,
            database_path: env::var("GRIDOPS_DATABASE_PATH")
                .unwrap_or_else(|_| "./data/gridops.sqlite".into())
                .into(),
            log_directory: env::var("GRIDOPS_LOG_DIRECTORY")
                .unwrap_or_else(|_| "./data/logs".into())
                .into(),
            github_client_id: optional("GITHUB_CLIENT_ID"),
            github_client_secret: secret("GITHUB_CLIENT_SECRET"),
            github_app_id: optional("GITHUB_APP_ID"),
            github_app_private_key: secret("GITHUB_APP_PRIVATE_KEY")
                .map(|value| SecretString::from(value.expose_secret().replace("\\n", "\n"))),
            github_app_slug: env::var("GITHUB_APP_SLUG").unwrap_or_else(|_| "gridops".into()),
            github_webhook_secret: secret("GITHUB_WEBHOOK_SECRET"),
            github_webhook_active: optional_bool("GRIDOPS_GITHUB_WEBHOOK_ACTIVE")?,
            session_secret: secret("GRIDOPS_SESSION_SECRET"),
            encryption_key: secret("GRIDOPS_ENCRYPTION_KEY"),
            manager_url: Url::parse(&manager_url).context("GRIDOPS_MANAGER_URL must be a URL")?,
            manager_token: secret("GRIDOPS_MANAGER_TOKEN"),
            runner_network: env::var("GRIDOPS_RUNNER_NETWORK")
                .unwrap_or_else(|_| "gridops-runners".into()),
            runner_image: env::var("GRIDOPS_RUNNER_IMAGE")
                .unwrap_or_else(|_| "ghcr.io/actions/actions-runner:latest".into()),
            api_bind: env::var("GRIDOPS_API_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            manager_bind: env::var("GRIDOPS_MANAGER_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8788".into()),
        })))
    }

    pub fn validate_api(&self) -> Result<()> {
        if self.session_secret().is_none() || self.encryption_key().is_none() {
            bail!("GRIDOPS_SESSION_SECRET and GRIDOPS_ENCRYPTION_KEY are required");
        }
        Ok(())
    }

    pub fn base_url(&self) -> &Url {
        &self.0.base_url
    }
    pub fn database_path(&self) -> &PathBuf {
        &self.0.database_path
    }
    pub fn log_directory(&self) -> &PathBuf {
        &self.0.log_directory
    }
    pub fn github_client_id(&self) -> Option<&str> {
        self.0.github_client_id.as_deref()
    }
    pub fn github_client_secret(&self) -> Option<&SecretString> {
        self.0.github_client_secret.as_ref()
    }
    pub fn github_app_id(&self) -> Option<&str> {
        self.0.github_app_id.as_deref()
    }
    pub fn github_app_private_key(&self) -> Option<&SecretString> {
        self.0.github_app_private_key.as_ref()
    }
    pub fn github_app_slug(&self) -> &str {
        &self.0.github_app_slug
    }
    pub fn github_webhook_secret(&self) -> Option<&SecretString> {
        self.0.github_webhook_secret.as_ref()
    }
    pub fn github_webhook_active(&self) -> Option<bool> {
        self.0.github_webhook_active
    }
    pub fn session_secret(&self) -> Option<&SecretString> {
        self.0.session_secret.as_ref()
    }
    pub fn encryption_key(&self) -> Option<&SecretString> {
        self.0.encryption_key.as_ref()
    }
    pub fn manager_url(&self) -> &Url {
        &self.0.manager_url
    }
    pub fn manager_token(&self) -> Option<&SecretString> {
        self.0.manager_token.as_ref()
    }
    pub fn runner_network(&self) -> &str {
        &self.0.runner_network
    }
    pub fn runner_image(&self) -> &str {
        &self.0.runner_image
    }
    pub fn api_bind(&self) -> &str {
        &self.0.api_bind
    }
    pub fn manager_bind(&self) -> &str {
        &self.0.manager_bind
    }
}

use secrecy::ExposeSecret;

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn secret(name: &str) -> Option<SecretString> {
    optional(name).map(SecretString::from)
}

fn optional_bool(name: &str) -> Result<Option<bool>> {
    optional(name)
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => bail!("{name} must be true or false"),
        })
        .transpose()
}
