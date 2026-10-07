//! Fix-agent configuration shared by the API, which edits it, and the
//! reconciler, which runs agents with it.
//!
//! Settings live in the `settings` table as JSON values; connection
//! credentials live sealed in `runtime_secrets`.

use anyhow::Result;
use serde_json::Value;
use sqlx::{Row as _, SqlitePool};

pub const CONNECTION_SETTING: &str = "fixAgentConnectionId";
pub const MODEL_SETTING: &str = "fixAgentModel";
pub const REASONING_EFFORT_SETTING: &str = "fixAgentReasoningEffort";
pub const TRIGGER_MODE_SETTING: &str = "fixAgentTriggerMode";
pub const DAILY_LIMIT_SETTING: &str = "fixAgentDailyLimit";
/// When automatic mode was last switched on. Failures from before then are
/// never picked up, so enabling it does not replay old failures.
pub const AUTOMATIC_SINCE_SETTING: &str = "fixAgentAutomaticSince";

pub const DEFAULT_DAILY_LIMIT: i64 = 10;
/// Branches the agent opens pull requests from. Failures on them never start
/// another automatic run, so a bad fix cannot loop.
pub const BRANCH_PREFIX: &str = "gridops/fix-";
/// GitHub App permissions the agent needs on an installation.
pub const REQUIRED_PERMISSIONS: [&str; 2] = ["contents", "pull_requests"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMode {
    Manual,
    Automatic,
}

impl TriggerMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Automatic => "automatic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub connection_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub trigger_mode: TriggerMode,
    pub daily_limit: i64,
    pub automatic_since: Option<i64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            connection_id: None,
            model: None,
            reasoning_effort: None,
            trigger_mode: TriggerMode::Manual,
            daily_limit: DEFAULT_DAILY_LIMIT,
            automatic_since: None,
        }
    }
}

/// A ready connection plus the model to use with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConnection {
    pub id: String,
    pub provider: String,
    pub credential_key: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

pub async fn load_config(database: &SqlitePool) -> Result<AgentConfig> {
    let rows = sqlx::query("SELECT key,value FROM settings WHERE key IN (?,?,?,?,?,?)")
        .bind(CONNECTION_SETTING)
        .bind(MODEL_SETTING)
        .bind(REASONING_EFFORT_SETTING)
        .bind(TRIGGER_MODE_SETTING)
        .bind(DAILY_LIMIT_SETTING)
        .bind(AUTOMATIC_SINCE_SETTING)
        .fetch_all(database)
        .await?;
    let mut config = AgentConfig::default();
    for row in rows {
        let value = serde_json::from_str::<Value>(row.get::<&str, _>("value")).unwrap_or_default();
        let text = value
            .as_str()
            .map(ToOwned::to_owned)
            .filter(|value| !value.is_empty());
        match row.get::<&str, _>("key") {
            CONNECTION_SETTING => config.connection_id = text,
            MODEL_SETTING => config.model = text,
            REASONING_EFFORT_SETTING => config.reasoning_effort = text,
            TRIGGER_MODE_SETTING => {
                config.trigger_mode = text
                    .as_deref()
                    .and_then(TriggerMode::parse)
                    .unwrap_or(TriggerMode::Manual);
            }
            DAILY_LIMIT_SETTING => {
                config.daily_limit = value.as_i64().unwrap_or(DEFAULT_DAILY_LIMIT).clamp(1, 100);
            }
            AUTOMATIC_SINCE_SETTING => config.automatic_since = value.as_i64(),
            _ => {}
        }
    }
    Ok(config)
}

/// The connection and model runs would use now, if the agent is usable:
/// a model is chosen and its connection exists and is ready.
pub async fn active_connection(database: &SqlitePool) -> Result<Option<ActiveConnection>> {
    let config = load_config(database).await?;
    let (Some(connection_id), Some(model)) = (config.connection_id, config.model) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT id,provider,credential_key FROM ai_connections WHERE id=? AND status='ready'",
    )
    .bind(&connection_id)
    .fetch_optional(database)
    .await?;
    Ok(row.map(|row| ActiveConnection {
        id: row.get("id"),
        provider: row.get("provider"),
        credential_key: row.get("credential_key"),
        model,
        reasoning_effort: config.reasoning_effort,
    }))
}

pub fn credential_key(connection_id: &str) -> String {
    format!("ai.connection.{connection_id}.credential")
}

/// Which of the agent's required App permissions an installation lacks.
pub fn missing_permissions(permissions: &Value) -> Vec<&'static str> {
    REQUIRED_PERMISSIONS
        .into_iter()
        .filter(|permission| permissions.get(*permission).and_then(Value::as_str) != Some("write"))
        .collect()
}

/// A branch name for a fix, unique per agent run.
pub fn fix_branch(job_name: &str, run_id: &str) -> String {
    let slug = job_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = slug.chars().take(40).collect::<String>();
    let short = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>();
    if slug.is_empty() {
        format!("{BRANCH_PREFIX}{short}")
    } else {
        format!("{BRANCH_PREFIX}{}-{short}", slug.trim_end_matches('-'))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::connect_database_path;

    #[test]
    fn missing_permissions_require_write_access() {
        assert_eq!(
            missing_permissions(&json!({ "contents": "write", "pull_requests": "write" })),
            Vec::<&str>::new()
        );
        assert_eq!(
            missing_permissions(&json!({ "contents": "read", "actions": "write" })),
            vec!["contents", "pull_requests"]
        );
    }

    #[test]
    fn fix_branches_are_readable_and_unique() {
        assert_eq!(
            fix_branch("Build (ubuntu, node 22)", "4f9c2b1e-aaaa"),
            "gridops/fix-build-ubuntu-node-22-4f9c2b1e"
        );
        assert_eq!(fix_branch("🚀", "abcdef123"), "gridops/fix-abcdef12");
    }

    #[tokio::test]
    async fn configuration_reads_defaults_and_requires_a_ready_connection() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let database = connect_database_path(&directory.join("test.sqlite")).await?;
        assert_eq!(load_config(&database).await?, AgentConfig::default());
        assert_eq!(active_connection(&database).await?, None);

        for (key, value) in [
            (CONNECTION_SETTING, json!("conn-1")),
            (MODEL_SETTING, json!("claude-opus-5-5")),
            (TRIGGER_MODE_SETTING, json!("automatic")),
            (DAILY_LIMIT_SETTING, json!(500)),
        ] {
            sqlx::query("INSERT INTO settings (key,value,updated_at) VALUES (?,?,0)")
                .bind(key)
                .bind(value.to_string())
                .execute(&database)
                .await?;
        }
        let config = load_config(&database).await?;
        assert_eq!(config.trigger_mode, TriggerMode::Automatic);
        assert_eq!(config.daily_limit, 100);
        assert_eq!(active_connection(&database).await?, None);

        sqlx::query(
            "INSERT INTO ai_connections (id,provider,auth_method,credential_key,status,created_at,updated_at) VALUES ('conn-1','anthropic','api_key','k','ready',0,0)",
        )
        .execute(&database)
        .await?;
        let active = active_connection(&database).await?;
        assert_eq!(
            active.map(|active| active.model).as_deref(),
            Some("claude-opus-5-5")
        );

        sqlx::query("UPDATE ai_connections SET status='needs_reconnect'")
            .execute(&database)
            .await?;
        assert_eq!(active_connection(&database).await?, None);
        database.close().await;
        let _ = std::fs::remove_dir_all(directory);
        Ok(())
    }
}
