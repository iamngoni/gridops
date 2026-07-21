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

pub fn runner_arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        "arm" => "arm",
        architecture => architecture,
    }
}

pub fn runner_system_labels() -> [&'static str; 3] {
    ["self-hosted", "linux", runner_arch_label()]
}

pub fn effective_runner_labels(configured: &[String]) -> Vec<String> {
    let mut labels = runner_system_labels()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    for label in configured {
        if !labels
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(label))
        {
            labels.push(label.clone());
        }
    }
    labels
}

pub fn runner_supports_system_label(label: &str) -> bool {
    runner_system_labels()
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
            ) AND NOT EXISTS (
              SELECT 1 FROM json_each(wj.labels) requested
              WHERE lower(CAST(requested.value AS TEXT)) NOT IN ('self-hosted','linux',?)
                AND NOT EXISTS (
                  SELECT 1 FROM json_each(candidate.labels) assigned
                  WHERE lower(CAST(assigned.value AS TEXT))=lower(CAST(requested.value AS TEXT))
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
              ) AND NOT EXISTS (
                SELECT 1 FROM json_each(wj.labels) requested
                WHERE lower(CAST(requested.value AS TEXT)) NOT IN ('self-hosted','linux',?)
                  AND NOT EXISTS (
                    SELECT 1 FROM json_each(candidate.labels) assigned
                    WHERE lower(CAST(assigned.value AS TEXT))=lower(CAST(requested.value AS TEXT))
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

    #[test]
    fn effective_labels_include_supported_runner_system_labels() {
        let labels =
            effective_runner_labels(&["gridops".into(), "SELF-HOSTED".into(), "pool-a".into()]);
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
        assert!(runner_supports_system_label("self-hosted"));
        assert!(runner_supports_system_label("Linux"));
        assert!(runner_supports_system_label(runner_arch_label()));
        assert!(!runner_supports_system_label("windows"));
    }

    #[test]
    fn scale_target_is_idempotent_for_the_same_queued_work() {
        assert_eq!(scale_up_target(0, 0, 1, 1, 10), 1);
        assert_eq!(scale_up_target(1, 0, 1, 1, 10), 1);
        assert_eq!(scale_up_target(1, 1, 1, 1, 10), 2);
        assert_eq!(scale_up_target(2, 1, 1, 1, 10), 2);
    }
}
