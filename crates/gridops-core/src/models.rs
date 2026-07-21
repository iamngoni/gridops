use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewer {
    pub id: String,
    pub github_id: i64,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub alerts: Alerts,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alerts {
    pub failed_runners: i64,
    pub failed_webhooks: i64,
    pub queued_jobs: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationState {
    #[serde(rename = "githubOAuth")]
    pub github_oauth: bool,
    pub github_app_control: bool,
    pub webhook_verification: bool,
    pub secure_storage: bool,
    pub runner_manager: bool,
    pub installation_tokens: bool,
    pub callback_url: String,
    pub webhook_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunnerPool {
    pub installation_id: i64,
    pub repository_id: Option<i64>,
    pub name: String,
    pub scope: String,
    pub mode: String,
    pub labels: Vec<String>,
    pub image: String,
    pub desired_count: i64,
    pub min_count: i64,
    pub max_count: i64,
    pub autoscaling_enabled: bool,
    pub queue_scale_factor: i64,
    pub idle_timeout_minutes: i64,
    pub cpu_limit: f64,
    pub memory_limit_mb: i64,
    pub runner_group_id: i64,
}

impl CreateRunnerPool {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.len() < 2 || self.name.len() > 48 || !valid_pool_name(&self.name) {
            return Err(
                "Use 2-48 lowercase letters, numbers, and hyphens for the pool name.".into(),
            );
        }
        if self.scope == "repository" && self.repository_id.is_none() {
            return Err("Repository scope requires a repository.".into());
        }
        if self.scope == "organization" && self.repository_id.is_some() {
            return Err("Organization pools cannot target one repository.".into());
        }
        if !matches!(self.scope.as_str(), "repository" | "organization") {
            return Err("Runner pool scope is invalid.".into());
        }
        if !matches!(self.mode.as_str(), "ephemeral" | "persistent") {
            return Err("Runner pool mode is invalid.".into());
        }
        if self.min_count < 0
            || self.desired_count < self.min_count
            || self.desired_count > self.max_count
            || self.max_count > 100
        {
            return Err("Capacity must satisfy 0 <= minimum <= desired <= maximum <= 100.".into());
        }
        if !(0.25..=64.0).contains(&self.cpu_limit)
            || !(256..=262_144).contains(&self.memory_limit_mb)
        {
            return Err("Runner resource limits are outside the supported range.".into());
        }
        if !(1..=20).contains(&self.queue_scale_factor)
            || !(1..=1_440).contains(&self.idle_timeout_minutes)
        {
            return Err("Autoscaling settings are outside the supported range.".into());
        }
        if self.labels.len() > 20
            || self.labels.iter().any(|label| {
                label.is_empty()
                    || label.len() > 64
                    || label.contains([',', '\n', '\r'])
                    || label.trim() != label
            })
        {
            return Err("Use at most 20 runner labels of 1-64 characters each.".into());
        }
        if self.image.trim() != self.image || self.image.is_empty() || self.image.len() > 300 {
            return Err("Runner image must contain 1-300 non-padding characters.".into());
        }
        if self.runner_group_id <= 0 {
            return Err("Runner group ID must be positive.".into());
        }
        Ok(())
    }
}

fn valid_pool_name(value: &str) -> bool {
    !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> CreateRunnerPool {
        CreateRunnerPool {
            installation_id: 1,
            repository_id: Some(2),
            name: "linux-general".into(),
            scope: "repository".into(),
            mode: "ephemeral".into(),
            labels: vec!["docker".into()],
            image: "ghcr.io/actions/actions-runner:latest".into(),
            desired_count: 1,
            min_count: 0,
            max_count: 10,
            autoscaling_enabled: true,
            queue_scale_factor: 1,
            idle_timeout_minutes: 5,
            cpu_limit: 2.0,
            memory_limit_mb: 4096,
            runner_group_id: 1,
        }
    }

    #[test]
    fn validates_runner_pool_invariants() {
        assert!(pool().validate().is_ok());
        let mut invalid = pool();
        invalid.desired_count = 11;
        invalid.max_count = 10;
        assert!(invalid.validate().is_err());
        let mut invalid = pool();
        invalid.name = "Bad Pool".into();
        assert!(invalid.validate().is_err());

        let mut invalid = pool();
        invalid.labels = vec!["bad,label".into()];
        assert!(invalid.validate().is_err());

        let mut invalid = pool();
        invalid.image = " runner:latest".into();
        assert!(invalid.validate().is_err());
    }
}
