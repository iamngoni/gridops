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
    pub deferred_runner_cleanup: i64,
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
        if self.scope == "repository" && self.repository_id.is_none() {
            return Err("Repository scope requires a repository.".into());
        }
        if self.scope == "organization" && self.repository_id.is_some() {
            return Err("Organization pools cannot target one repository.".into());
        }
        if !matches!(self.scope.as_str(), "repository" | "organization") {
            return Err("Runner pool scope is invalid.".into());
        }
        validate_pool_configuration(
            &self.name,
            &self.mode,
            &self.labels,
            &self.image,
            self.desired_count,
            self.min_count,
            self.max_count,
            self.queue_scale_factor,
            self.idle_timeout_minutes,
            self.cpu_limit,
            self.memory_limit_mb,
            self.runner_group_id,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRunnerPool {
    pub name: String,
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

impl UpdateRunnerPool {
    pub fn validate(&self) -> Result<(), String> {
        validate_pool_configuration(
            &self.name,
            &self.mode,
            &self.labels,
            &self.image,
            self.desired_count,
            self.min_count,
            self.max_count,
            self.queue_scale_factor,
            self.idle_timeout_minutes,
            self.cpu_limit,
            self.memory_limit_mb,
            self.runner_group_id,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_pool_configuration(
    name: &str,
    mode: &str,
    labels: &[String],
    image: &str,
    desired_count: i64,
    min_count: i64,
    max_count: i64,
    queue_scale_factor: i64,
    idle_timeout_minutes: i64,
    cpu_limit: f64,
    memory_limit_mb: i64,
    runner_group_id: i64,
) -> Result<(), String> {
    if name.len() < 2 || name.len() > 48 || !valid_pool_name(name) {
        return Err("Use 2-48 lowercase letters, numbers, and hyphens for the pool name.".into());
    }
    if !matches!(mode, "ephemeral" | "persistent") {
        return Err("Runner pool mode is invalid.".into());
    }
    if min_count < 0 || desired_count < min_count || desired_count > max_count || max_count > 100 {
        return Err("Capacity must satisfy 0 <= minimum <= desired <= maximum <= 100.".into());
    }
    if !(0.25..=64.0).contains(&cpu_limit) || !(256..=262_144).contains(&memory_limit_mb) {
        return Err("Runner resource limits are outside the supported range.".into());
    }
    if !(1..=20).contains(&queue_scale_factor) || !(1..=1_440).contains(&idle_timeout_minutes) {
        return Err("Autoscaling settings are outside the supported range.".into());
    }
    if labels.len() > 20
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 64
                || label.contains([',', '\n', '\r'])
                || label.trim() != label
        })
    {
        return Err("Use at most 20 runner labels of 1-64 characters each.".into());
    }
    if image.trim() != image || image.is_empty() || image.len() > 300 {
        return Err("Runner image must contain 1-300 non-padding characters.".into());
    }
    if runner_group_id <= 0 {
        return Err("Runner group ID must be positive.".into());
    }
    Ok(())
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

    #[test]
    fn validates_runner_pool_updates() {
        let original = pool();
        let update = UpdateRunnerPool {
            name: original.name,
            mode: original.mode,
            labels: original.labels,
            image: original.image,
            desired_count: original.desired_count,
            min_count: original.min_count,
            max_count: original.max_count,
            autoscaling_enabled: original.autoscaling_enabled,
            queue_scale_factor: original.queue_scale_factor,
            idle_timeout_minutes: original.idle_timeout_minutes,
            cpu_limit: original.cpu_limit,
            memory_limit_mb: original.memory_limit_mb,
            runner_group_id: original.runner_group_id,
        };
        assert!(update.validate().is_ok());
    }
}
