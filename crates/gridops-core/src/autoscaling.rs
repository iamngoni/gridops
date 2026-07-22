use sqlx::Row as _;
use sqlx::SqlitePool;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryCapacity {
    pub repository_id: i64,
    pub installation_id: i64,
    pub owner: String,
    pub name: String,
    pub queued: i64,
    pub active: i64,
    pub busy: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCapacity {
    pub provider: String,
    pub queued: i64,
    pub active: i64,
    pub busy: i64,
}

pub fn runner_arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        "arm" => "arm",
        architecture => architecture,
    }
}

pub fn runner_system_labels(provider: &str) -> [&'static str; 3] {
    match provider {
        "tart" => ["self-hosted", "macOS", "ARM64"],
        _ => ["self-hosted", "linux", runner_arch_label()],
    }
}

pub fn effective_runner_labels(provider: &str, configured: &[String]) -> Vec<String> {
    let mut labels = runner_system_labels(provider)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    for label in configured {
        if !is_runner_system_label(label)
            && !labels
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(label))
        {
            labels.push(label.clone());
        }
    }
    labels
}

pub fn runner_supports_system_label(provider: &str, label: &str) -> bool {
    runner_system_labels(provider)
        .iter()
        .any(|system| system.eq_ignore_ascii_case(label))
}

pub fn compatible_runner_provider(
    providers: &[String],
    requested: &[String],
    configured: &[String],
) -> Option<String> {
    providers
        .iter()
        .find(|provider| provider_supports_labels(provider, requested, configured))
        .cloned()
}

pub fn provider_supports_labels(
    provider: &str,
    requested: &[String],
    configured: &[String],
) -> bool {
    requested.iter().all(|requested| {
        if requested.eq_ignore_ascii_case("self-hosted") {
            return true;
        }
        if is_runner_system_label(requested) {
            return runner_supports_system_label(provider, requested);
        }
        configured
            .iter()
            .any(|label| label.eq_ignore_ascii_case(requested))
    })
}

pub fn is_runner_system_label(label: &str) -> bool {
    [
        "self-hosted",
        "linux",
        "macos",
        "windows",
        "x64",
        "arm64",
        "arm",
        "x86",
    ]
    .iter()
    .any(|system| system.eq_ignore_ascii_case(label))
}

pub fn scale_up_target(desired: i64, busy: i64, queued: i64, factor: i64, maximum: i64) -> i64 {
    maximum.min(desired.max(busy.saturating_add(queued.saturating_mul(factor))))
}

pub async fn assigned_queued_jobs(
    database: &SqlitePool,
    pool_id: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM workflow_jobs wj
          JOIN workflow_runs wr ON wr.id=wj.run_id
          JOIN repositories repo ON repo.id=wr.repository_id
          WHERE wj.status='queued' AND ?=(
            SELECT candidate.id FROM runner_pools candidate
            WHERE candidate.autoscaling_enabled=1 AND candidate.paused=0 AND (
              EXISTS (SELECT 1 FROM runner_pool_repositories candidate_repo
                WHERE candidate_repo.pool_id=candidate.id AND candidate_repo.repository_id=repo.id) OR
              candidate.repository_id=repo.id OR
              (candidate.scope='organization' AND candidate.installation_id=repo.installation_id)
            ) AND EXISTS (
              SELECT 1 FROM json_each(candidate.providers) provider
              WHERE NOT EXISTS (
                SELECT 1 FROM json_each(wj.labels) requested
                WHERE NOT (
                  lower(CAST(requested.value AS TEXT))='self-hosted' OR
                  (lower(CAST(requested.value AS TEXT)) NOT IN ('linux','macos','windows','x64','arm64','arm','x86') AND EXISTS (
                    SELECT 1 FROM json_each(candidate.labels) assigned
                    WHERE lower(CAST(assigned.value AS TEXT))=lower(CAST(requested.value AS TEXT))
                  )) OR
                  (lower(CAST(provider.value AS TEXT))='docker' AND lower(CAST(requested.value AS TEXT)) IN ('linux',lower(?))) OR
                  (lower(CAST(provider.value AS TEXT))='tart' AND lower(CAST(requested.value AS TEXT)) IN ('macos','arm64'))
                )
              )
            )
            ORDER BY CASE WHEN candidate.scope='repository' THEN 0 ELSE 1 END,
              candidate.created_at,candidate.id
            LIMIT 1
          )"#,
    )
    .bind(pool_id)
    .bind(runner_arch_label())
    .fetch_one(database)
    .await
}

pub async fn provider_capacities(
    database: &SqlitePool,
    pool_id: &str,
    providers: &[String],
    configured_labels: &[String],
) -> Result<Vec<ProviderCapacity>, sqlx::Error> {
    let mut capacities = providers
        .iter()
        .map(|provider| ProviderCapacity {
            provider: provider.clone(),
            queued: 0,
            active: 0,
            busy: 0,
        })
        .collect::<Vec<_>>();
    for (_, requested) in queued_job_assignments(database, pool_id).await? {
        if let Some(provider) = compatible_runner_provider(providers, &requested, configured_labels)
            && let Some(capacity) = capacities
                .iter_mut()
                .find(|capacity| capacity.provider == provider)
        {
            capacity.queued = capacity.queued.saturating_add(1);
        }
    }
    let rows = sqlx::query(
        r#"SELECT provider,COUNT(*) AS active,
          COUNT(CASE WHEN busy=1 THEN 1 END) AS busy
          FROM runners WHERE pool_id=? AND deleted_at IS NULL
            AND status IN ('starting','online','idle','busy','paused','stopped')
          GROUP BY provider"#,
    )
    .bind(pool_id)
    .fetch_all(database)
    .await?;
    for row in rows {
        let provider = row.get::<String, _>("provider");
        if let Some(capacity) = capacities
            .iter_mut()
            .find(|capacity| capacity.provider == provider)
        {
            capacity.active = row.get("active");
            capacity.busy = row.get("busy");
        }
    }
    Ok(capacities)
}

pub async fn next_runner_provider(
    database: &SqlitePool,
    pool_id: &str,
    repository_id: Option<i64>,
    providers: &[String],
    configured_labels: &[String],
    factor: i64,
) -> Result<Option<String>, sqlx::Error> {
    let assignments = queued_job_assignments(database, pool_id).await?;
    let mut capacities = providers
        .iter()
        .map(|provider| ProviderCapacity {
            provider: provider.clone(),
            queued: 0,
            active: 0,
            busy: 0,
        })
        .collect::<Vec<_>>();
    for (queued_repository_id, requested) in &assignments {
        if repository_id.is_none_or(|repository_id| repository_id == *queued_repository_id)
            && let Some(provider) =
                compatible_runner_provider(providers, requested, configured_labels)
            && let Some(capacity) = capacities
                .iter_mut()
                .find(|capacity| capacity.provider == provider)
        {
            capacity.queued = capacity.queued.saturating_add(1);
        }
    }
    let rows = sqlx::query(
        r#"SELECT provider,COUNT(*) AS active,
          COUNT(CASE WHEN busy=1 THEN 1 END) AS busy
          FROM runners WHERE pool_id=? AND deleted_at IS NULL
            AND status IN ('starting','online','idle','busy','paused','stopped')
            AND (? IS NULL OR target_repository_id=?)
          GROUP BY provider"#,
    )
    .bind(pool_id)
    .bind(repository_id)
    .bind(repository_id)
    .fetch_all(database)
    .await?;
    for row in rows {
        let provider = row.get::<String, _>("provider");
        if let Some(capacity) = capacities
            .iter_mut()
            .find(|capacity| capacity.provider == provider)
        {
            capacity.active = row.get("active");
            capacity.busy = row.get("busy");
        }
    }
    if let Some(capacity) = capacities
        .iter()
        .enumerate()
        .filter(|(_, capacity)| provider_capacity_deficit(capacity, factor) > 0)
        .max_by(|(left_index, left), (right_index, right)| {
            provider_capacity_deficit(left, factor)
                .cmp(&provider_capacity_deficit(right, factor))
                .then_with(|| left.queued.cmp(&right.queued))
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(_, capacity)| capacity)
    {
        return Ok(Some(capacity.provider.clone()));
    }
    Ok(providers.first().cloned())
}

pub fn provider_capacity_deficit(capacity: &ProviderCapacity, factor: i64) -> i64 {
    capacity
        .busy
        .saturating_add(capacity.queued.saturating_mul(factor))
        .saturating_sub(capacity.active)
}

async fn queued_job_assignments(
    database: &SqlitePool,
    pool_id: &str,
) -> Result<Vec<(i64, Vec<String>)>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT repo.id AS repository_id,wj.labels FROM workflow_jobs wj
          JOIN workflow_runs wr ON wr.id=wj.run_id
          JOIN repositories repo ON repo.id=wr.repository_id
          WHERE wj.status='queued' AND ?=(
            SELECT candidate.id FROM runner_pools candidate
            WHERE candidate.autoscaling_enabled=1 AND candidate.paused=0 AND (
              EXISTS (SELECT 1 FROM runner_pool_repositories candidate_repo
                WHERE candidate_repo.pool_id=candidate.id AND candidate_repo.repository_id=repo.id) OR
              candidate.repository_id=repo.id OR
              (candidate.scope='organization' AND candidate.installation_id=repo.installation_id)
            ) AND EXISTS (
              SELECT 1 FROM json_each(candidate.providers) provider
              WHERE NOT EXISTS (
                SELECT 1 FROM json_each(wj.labels) requested
                WHERE NOT (
                  lower(CAST(requested.value AS TEXT))='self-hosted' OR
                  (lower(CAST(requested.value AS TEXT)) NOT IN ('linux','macos','windows','x64','arm64','arm','x86') AND EXISTS (
                    SELECT 1 FROM json_each(candidate.labels) assigned
                    WHERE lower(CAST(assigned.value AS TEXT))=lower(CAST(requested.value AS TEXT))
                  )) OR
                  (lower(CAST(provider.value AS TEXT))='docker' AND lower(CAST(requested.value AS TEXT)) IN ('linux',lower(?))) OR
                  (lower(CAST(provider.value AS TEXT))='tart' AND lower(CAST(requested.value AS TEXT)) IN ('macos','arm64'))
                )
              )
            )
            ORDER BY CASE WHEN candidate.scope='repository' THEN 0 ELSE 1 END,
              candidate.created_at,candidate.id
            LIMIT 1
          ) ORDER BY wj.created_at,wj.id"#,
    )
    .bind(pool_id)
    .bind(runner_arch_label())
    .fetch_all(database)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get("repository_id"),
                serde_json::from_str(row.get::<&str, _>("labels")).unwrap_or_default(),
            )
        })
        .collect())
}

pub async fn repository_capacities(
    database: &SqlitePool,
    pool_id: &str,
) -> Result<Vec<RepositoryCapacity>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT repo.id,repo.installation_id,repo.owner,repo.name,
          (SELECT COUNT(*) FROM workflow_jobs wj
            JOIN workflow_runs wr ON wr.id=wj.run_id
            WHERE wr.repository_id=repo.id AND wj.status='queued' AND ?=(
              SELECT candidate.id FROM runner_pools candidate
              WHERE candidate.autoscaling_enabled=1 AND candidate.paused=0 AND (
                EXISTS (SELECT 1 FROM runner_pool_repositories candidate_repo
                  WHERE candidate_repo.pool_id=candidate.id AND candidate_repo.repository_id=repo.id) OR
                candidate.repository_id=repo.id OR
                (candidate.scope='organization' AND candidate.installation_id=repo.installation_id)
              ) AND EXISTS (
                SELECT 1 FROM json_each(candidate.providers) provider
                WHERE NOT EXISTS (
                  SELECT 1 FROM json_each(wj.labels) requested
                  WHERE NOT (
                    lower(CAST(requested.value AS TEXT))='self-hosted' OR
                    (lower(CAST(requested.value AS TEXT)) NOT IN ('linux','macos','windows','x64','arm64','arm','x86') AND EXISTS (
                      SELECT 1 FROM json_each(candidate.labels) assigned
                      WHERE lower(CAST(assigned.value AS TEXT))=lower(CAST(requested.value AS TEXT))
                    )) OR
                    (lower(CAST(provider.value AS TEXT))='docker' AND lower(CAST(requested.value AS TEXT)) IN ('linux',lower(?))) OR
                    (lower(CAST(provider.value AS TEXT))='tart' AND lower(CAST(requested.value AS TEXT)) IN ('macos','arm64'))
                  )
                )
              )
              ORDER BY CASE WHEN candidate.scope='repository' THEN 0 ELSE 1 END,
                candidate.created_at,candidate.id
              LIMIT 1
            )) AS queued,
          (SELECT COUNT(*) FROM runners runner
            WHERE runner.pool_id=? AND runner.target_repository_id=repo.id
              AND runner.deleted_at IS NULL
              AND runner.status IN ('starting','online','idle','busy','paused','stopped')) AS active,
          (SELECT COUNT(*) FROM runners runner
            WHERE runner.pool_id=? AND runner.target_repository_id=repo.id
              AND runner.deleted_at IS NULL AND runner.busy=1
              AND runner.status IN ('starting','online','idle','busy','paused','stopped')) AS busy
          FROM runner_pool_repositories membership
          JOIN repositories repo ON repo.id=membership.repository_id
          WHERE membership.pool_id=? AND repo.archived=0
          ORDER BY membership.created_at,repo.id"#,
    )
    .bind(pool_id)
    .bind(runner_arch_label())
    .bind(pool_id)
    .bind(pool_id)
    .bind(pool_id)
    .fetch_all(database)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| RepositoryCapacity {
            repository_id: row.get("id"),
            installation_id: row.get("installation_id"),
            owner: row.get("owner"),
            name: row.get("name"),
            queued: row.get("queued"),
            active: row.get("active"),
            busy: row.get("busy"),
        })
        .collect())
}

pub fn repository_capacity_deficit(capacity: &RepositoryCapacity, factor: i64) -> i64 {
    capacity
        .busy
        .saturating_add(capacity.queued.saturating_mul(factor))
        .saturating_sub(capacity.active)
}

pub async fn next_runner_repository(
    database: &SqlitePool,
    pool_id: &str,
    factor: i64,
) -> Result<Option<RepositoryCapacity>, sqlx::Error> {
    let mut capacities = repository_capacities(database, pool_id).await?;
    capacities.sort_by(|left, right| {
        repository_capacity_deficit(right, factor)
            .cmp(&repository_capacity_deficit(left, factor))
            .then_with(|| right.queued.cmp(&left.queued))
            .then_with(|| left.active.cmp(&right.active))
            .then_with(|| left.repository_id.cmp(&right.repository_id))
    });
    Ok(capacities.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_database_path;
    use std::fs;

    #[test]
    fn effective_labels_include_supported_runner_system_labels() {
        let labels = effective_runner_labels(
            "docker",
            &["gridops".into(), "SELF-HOSTED".into(), "pool-a".into()],
        );
        assert!(labels.iter().any(|label| label == "self-hosted"));
        assert!(labels.iter().any(|label| label == "linux"));
        assert!(labels.iter().any(|label| label == runner_arch_label()));
        assert!(labels.iter().any(|label| label == "gridops"));
        assert!(labels.iter().any(|label| label == "pool-a"));
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.eq_ignore_ascii_case("self-hosted"))
                .count(),
            1
        );
    }

    #[test]
    fn unsupported_system_labels_are_not_treated_as_available() {
        assert!(runner_supports_system_label("docker", "self-hosted"));
        assert!(runner_supports_system_label("docker", "Linux"));
        assert!(runner_supports_system_label("docker", runner_arch_label()));
        assert!(!runner_supports_system_label("docker", "windows"));
        assert!(runner_supports_system_label("tart", "macOS"));
        assert!(runner_supports_system_label("tart", "arm64"));
        assert!(!runner_supports_system_label("tart", "linux"));
        assert!(
            !effective_runner_labels("docker", &["macOS".into()])
                .iter()
                .any(|label| label.eq_ignore_ascii_case("macos"))
        );
    }

    #[test]
    fn mixed_pools_choose_the_provider_that_matches_job_system_labels() {
        let providers = vec!["docker".into(), "tart".into()];
        let configured = vec!["gridops".into(), "release".into()];
        assert_eq!(
            compatible_runner_provider(
                &providers,
                &["self-hosted".into(), "macOS".into(), "release".into()],
                &configured,
            ),
            Some("tart".into())
        );
        assert_eq!(
            compatible_runner_provider(
                &providers,
                &["self-hosted".into(), "linux".into()],
                &configured,
            ),
            Some("docker".into())
        );
        assert_eq!(
            compatible_runner_provider(
                &providers,
                &["self-hosted".into(), "windows".into()],
                &configured,
            ),
            None
        );
        assert_eq!(
            compatible_runner_provider(&providers, &["self-hosted".into()], &configured),
            Some("docker".into())
        );
    }

    #[test]
    fn scale_target_is_idempotent_for_the_same_queued_work() {
        assert_eq!(scale_up_target(0, 0, 1, 1, 10), 1);
        assert_eq!(scale_up_target(1, 0, 1, 1, 10), 1);
        assert_eq!(scale_up_target(1, 1, 1, 1, 10), 2);
        assert_eq!(scale_up_target(2, 1, 1, 1, 10), 2);
    }

    #[tokio::test]
    async fn provisioning_chooses_the_provider_with_an_unfilled_queue() -> anyhow::Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "gridops-provider-placement-test-{}",
            uuid::Uuid::new_v4()
        ));
        let database = connect_database_path(&directory.join("gridops.sqlite")).await?;
        sqlx::raw_sql(
            r#"
            INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
              VALUES (1,1,'octo-org','Organization','Organization','all',1,1);
            INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
              VALUES (10,1,'octo-org','gridops','octo-org/gridops',0,'master','https://github.com/octo-org/gridops',1,1,1);
            INSERT INTO runner_pools (id,installation_id,repository_id,name,scope,provider,providers,labels,image,docker_image,tart_image,autoscaling_enabled,created_at,updated_at)
              VALUES ('mixed',1,10,'mixed','repository','docker','["docker","tart"]','["mixed"]','runner:latest','runner:latest','macos-base',1,1,1);
            INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at)
              VALUES ('mixed',10,1);
            INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
              VALUES (20,10,'CI',1,'push','queued','abc123','https://github.com/octo-org/gridops/actions/runs/20',1,1,1,1);
            INSERT INTO workflow_jobs (id,run_id,name,status,labels,html_url,created_at,updated_at)
              VALUES
              (30,20,'linux','queued','["self-hosted","linux"]','https://github.com/octo-org/gridops/actions/runs/20/job/30',1,1),
              (31,20,'macos','queued','["self-hosted","macOS"]','https://github.com/octo-org/gridops/actions/runs/20/job/31',2,2);
            INSERT INTO runners (id,pool_id,target_repository_id,name,provider,status,created_at,updated_at)
              VALUES ('linux-runner','mixed',10,'mixed-linux','docker','online',1,1);
            "#,
        )
        .execute(&database)
        .await?;

        let providers = vec!["docker".into(), "tart".into()];
        assert_eq!(
            next_runner_provider(
                &database,
                "mixed",
                Some(10),
                &providers,
                &["mixed".into()],
                1,
            )
            .await?,
            Some("tart".into())
        );

        database.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
