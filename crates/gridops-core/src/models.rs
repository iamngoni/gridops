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
    pub role: String,
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
    pub webhook_active: bool,
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
    #[serde(default)]
    pub repository_ids: Vec<i64>,
    pub name: String,
    pub scope: String,
    pub mode: String,
    pub provider: String,
    #[serde(default)]
    pub providers: Vec<String>,
    pub labels: Vec<String>,
    pub image: String,
    #[serde(default)]
    pub docker_image: String,
    #[serde(default)]
    pub tart_image: String,
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
    pub fn selected_providers(&self) -> Vec<String> {
        selected_providers(&self.provider, &self.providers)
    }

    pub fn selected_docker_image(&self) -> String {
        selected_image("docker", &self.provider, &self.image, &self.docker_image)
    }

    pub fn selected_tart_image(&self) -> String {
        selected_image("tart", &self.provider, &self.image, &self.tart_image)
    }

    pub fn selected_repository_ids(&self) -> Vec<i64> {
        let mut repositories = self.repository_ids.clone();
        if let Some(repository_id) = self.repository_id {
            repositories.push(repository_id);
        }
        repositories.sort_unstable();
        repositories.dedup();
        repositories
    }

    pub fn validate(&self) -> Result<(), String> {
        let repositories = self.selected_repository_ids();
        if self.scope == "repository" && repositories.is_empty() {
            return Err("Repository scope requires at least one repository.".into());
        }
        if self.scope == "organization" && !repositories.is_empty() {
            return Err(
                "Organization pools use runner-group access instead of repository assignments."
                    .into(),
            );
        }
        if repositories.len() > 1_000
            || repositories.iter().any(|repository_id| *repository_id <= 0)
        {
            return Err("Repository assignments are invalid.".into());
        }
        if self.scope == "repository"
            && i64::try_from(repositories.len()).unwrap_or(i64::MAX) > self.max_count
        {
            return Err("Repository count cannot exceed maximum runner capacity.".into());
        }
        if !matches!(self.scope.as_str(), "repository" | "organization") {
            return Err("Runner pool scope is invalid.".into());
        }
        validate_pool_configuration(
            &self.name,
            &self.mode,
            &self.provider,
            &self.providers,
            &self.labels,
            &self.image,
            &self.docker_image,
            &self.tart_image,
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
    #[serde(default)]
    pub repository_ids: Option<Vec<i64>>,
    pub name: String,
    pub mode: String,
    pub provider: String,
    #[serde(default)]
    pub providers: Vec<String>,
    pub labels: Vec<String>,
    pub image: String,
    #[serde(default)]
    pub docker_image: String,
    #[serde(default)]
    pub tart_image: String,
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
    pub fn selected_providers(&self) -> Vec<String> {
        selected_providers(&self.provider, &self.providers)
    }

    pub fn selected_docker_image(&self) -> String {
        selected_image("docker", &self.provider, &self.image, &self.docker_image)
    }

    pub fn selected_tart_image(&self) -> String {
        selected_image("tart", &self.provider, &self.image, &self.tart_image)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.repository_ids.as_ref().is_some_and(|repositories| {
            repositories.is_empty()
                || repositories.len() > 1_000
                || repositories.iter().any(|repository_id| *repository_id <= 0)
                || {
                    let mut unique = repositories.clone();
                    unique.sort_unstable();
                    unique.dedup();
                    unique.len() != repositories.len()
                }
        }) {
            return Err("Choose between 1 and 1,000 unique repositories for the pool.".into());
        }
        if self.repository_ids.as_ref().is_some_and(|repositories| {
            i64::try_from(repositories.len()).unwrap_or(i64::MAX) > self.max_count
        }) {
            return Err("Repository count cannot exceed maximum runner capacity.".into());
        }
        validate_pool_configuration(
            &self.name,
            &self.mode,
            &self.provider,
            &self.providers,
            &self.labels,
            &self.image,
            &self.docker_image,
            &self.tart_image,
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
    provider: &str,
    providers: &[String],
    labels: &[String],
    image: &str,
    docker_image: &str,
    tart_image: &str,
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
    let selected = selected_providers(provider, providers);
    if selected.is_empty()
        || selected.len() > 2
        || selected
            .iter()
            .any(|provider| !matches!(provider.as_str(), "docker" | "tart"))
    {
        return Err("Choose one or more supported runner providers.".into());
    }
    let mut unique = selected.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != selected.len() || (!providers.is_empty() && selected[0] != provider) {
        return Err(
            "Runner providers must be unique and the primary provider must be first.".into(),
        );
    }
    let includes_tart = selected.iter().any(|provider| provider == "tart");
    if includes_tart && mode != "ephemeral" {
        return Err("Pools that include Tart must be ephemeral.".into());
    }
    if min_count < 0 || desired_count < min_count || desired_count > max_count || max_count > 100 {
        return Err("Capacity must satisfy 0 <= minimum <= desired <= maximum <= 100.".into());
    }
    if !(0.25..=64.0).contains(&cpu_limit) || !(256..=262_144).contains(&memory_limit_mb) {
        return Err("Runner resource limits are outside the supported range.".into());
    }
    if includes_tart && (cpu_limit.fract() != 0.0 || memory_limit_mb < 2_048) {
        return Err("Tart runners require whole CPU cores and at least 2048 MB of memory.".into());
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
    let selected_docker_image = selected_image("docker", provider, image, docker_image);
    let selected_tart_image = selected_image("tart", provider, image, tart_image);
    if selected.iter().any(|provider| provider == "docker") && !valid_image(&selected_docker_image)
    {
        return Err("Docker image must contain 1-300 non-padding characters.".into());
    }
    if includes_tart && !valid_image(&selected_tart_image) {
        return Err("Tart base VM must contain 1-300 non-padding characters.".into());
    }
    if runner_group_id <= 0 {
        return Err("Runner group ID must be positive.".into());
    }
    Ok(())
}

fn selected_providers(provider: &str, providers: &[String]) -> Vec<String> {
    if providers.is_empty() {
        vec![provider.to_owned()]
    } else {
        providers.to_vec()
    }
}

fn selected_image(kind: &str, provider: &str, image: &str, configured: &str) -> String {
    if configured.is_empty() && provider == kind {
        image.to_owned()
    } else {
        configured.to_owned()
    }
}

fn valid_image(image: &str) -> bool {
    image.trim() == image && !image.is_empty() && image.len() <= 300
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
            repository_ids: Vec::new(),
            name: "linux-general".into(),
            scope: "repository".into(),
            mode: "ephemeral".into(),
            provider: "docker".into(),
            providers: vec!["docker".into()],
            labels: vec!["docker".into()],
            image: "ghcr.io/actions/actions-runner:latest".into(),
            docker_image: "ghcr.io/actions/actions-runner:latest".into(),
            tart_image: "gridops-macos-tahoe-base".into(),
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

        let mut mixed = pool();
        mixed.providers = vec!["docker".into(), "tart".into()];
        assert!(mixed.validate().is_ok());
        mixed.mode = "persistent".into();
        assert!(mixed.validate().is_err());

        let mut invalid = pool();
        invalid.labels = vec!["bad,label".into()];
        assert!(invalid.validate().is_err());

        let mut invalid = pool();
        invalid.docker_image = " runner:latest".into();
        assert!(invalid.validate().is_err());

        let mut multi_repository = pool();
        multi_repository.repository_id = None;
        multi_repository.repository_ids = vec![2, 3];
        multi_repository.max_count = 1;
        assert!(multi_repository.validate().is_err());
        multi_repository.max_count = 2;
        assert!(multi_repository.validate().is_ok());
    }

    #[test]
    fn validates_runner_pool_updates() {
        let original = pool();
        let update = UpdateRunnerPool {
            repository_ids: None,
            name: original.name,
            mode: original.mode,
            provider: original.provider,
            providers: original.providers,
            labels: original.labels,
            image: original.image,
            docker_image: original.docker_image,
            tart_image: original.tart_image,
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

        let mut undersized = update;
        undersized.repository_ids = Some(vec![2, 3]);
        undersized.max_count = 1;
        assert!(undersized.validate().is_err());
    }
}
