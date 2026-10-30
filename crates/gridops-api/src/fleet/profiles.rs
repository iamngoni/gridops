//! Fleet targets and pool fleet settings. A pool runs on fleet hosts once it
//! has enabled platforms (one execution profile each). Only system administrators can read or change
//! these, because they decide which machines run which repositories' jobs.

use axum::{Json, extract::State};
use gridops_core::fleet::{
    FleetError, Principal,
    protocol::browser::ErrorCode,
    targets::{self, FleetSettings, FleetSettingsInput, TargetSummary},
    workload::{self, FleetWorkload},
};
use serde::{Deserialize, Serialize};

use super::{
    boundary::{FleetJson, FleetPath, FleetUser, FleetWriteUser},
    error::{FleetApiError, RequestContext},
    query_boundary::FleetQuery,
};
use crate::state::AppState;

#[derive(Serialize)]
pub(super) struct Targets {
    items: Vec<TargetSummary>,
}

#[derive(Serialize)]
pub(super) struct ProfileResponse {
    item: Option<FleetSettings>,
}

#[derive(Deserialize)]
pub(super) struct PoolPath {
    pool_id: String,
}

pub(super) async fn targets(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
) -> Result<Json<Targets>, FleetApiError> {
    require_admin(&state, &context, user.principal()).await?;
    let items = targets::list_targets(&state.database)
        .await
        .map_err(|error| fleet_error(context, &error))?;
    Ok(Json(Targets { items }))
}

pub(super) async fn profile(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
    FleetPath(path): FleetPath<PoolPath>,
) -> Result<Json<ProfileResponse>, FleetApiError> {
    require_admin(&state, &context, user.principal()).await?;
    let item = targets::pool_fleet(&state.database, &path.pool_id)
        .await
        .map_err(|error| fleet_error(context, &error))?;
    Ok(Json(ProfileResponse { item }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueueResponse {
    items: Vec<targets::QueuedRunner>,
    blocked_hosts: Vec<targets::BlockedHost>,
}

pub(super) async fn queue(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
    FleetPath(path): FleetPath<PoolPath>,
) -> Result<Json<QueueResponse>, FleetApiError> {
    require_admin(&state, &context, user.principal()).await?;
    let items = targets::pool_queue(&state.database, &path.pool_id)
        .await
        .map_err(|error| fleet_error(context, &error))?;
    let blocked_hosts = targets::pool_blocked_hosts(&state.database, &path.pool_id)
        .await
        .map_err(|error| fleet_error(context, &error))?;
    Ok(Json(QueueResponse {
        items,
        blocked_hosts,
    }))
}

pub(super) async fn save_profile(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    FleetPath(path): FleetPath<PoolPath>,
    FleetJson(body): FleetJson<FleetSettingsInput>,
) -> Result<Json<ProfileResponse>, FleetApiError> {
    require_admin(&state, &context, user.principal()).await?;
    let item = targets::save_pool_fleet(&state.database, &path.pool_id, &body)
        .await
        .map_err(|error| fleet_error(context, &error))?;
    record_profile_saved(&state, context, user.principal(), &path.pool_id, &body).await?;
    Ok(Json(ProfileResponse { item: Some(item) }))
}

/// Every other admin change to pool/runner/installation state is logged in
/// `audit_events` (pool settings, host enrollment/approval, GitHub app events);
/// a fleet execution profile save is exactly the kind of change you'd most
/// want in that trail, so it must be too.
async fn record_profile_saved(
    state: &AppState,
    context: RequestContext,
    principal: &Principal,
    pool_id: &str,
    body: &FleetSettingsInput,
) -> Result<(), FleetApiError> {
    let Principal::AuthenticatedUser { user_id } = principal else {
        return Ok(());
    };
    let login: Option<String> = sqlx::query_scalar("SELECT login FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&state.database)
        .await
        .map_err(|_| context.error(ErrorCode::DependencyUnavailable))?;
    sqlx::query(
        r#"INSERT INTO audit_events (id,actor_user_id,actor_label,action,target_type,target_id,metadata,created_at)
           VALUES (?,?,?,'fleet.pool.profile_saved','runner_pool',?,?,?)"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(login.unwrap_or_else(|| user_id.clone()))
    .bind(pool_id)
    .bind(
        serde_json::json!({
            "enabled": body.enabled,
            "placement": body.placement,
            "mode": body.mode,
            "platformCount": body.platforms.len(),
        })
        .to_string(),
    )
    .bind(gridops_core::now_millis())
    .execute(&state.database)
    .await
    .map_err(|_| context.error(ErrorCode::DependencyUnavailable))?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkloadParams {
    window: Option<String>,
}

/// Each machine's runners, CPU load and memory over the last day, week or
/// month, for the overview.
pub(super) async fn workload(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
    FleetQuery(params): FleetQuery<WorkloadParams>,
) -> Result<Json<FleetWorkload>, FleetApiError> {
    require_admin(&state, &context, user.principal()).await?;
    let (window, bucket) = match params.window.as_deref().unwrap_or("24h") {
        "24h" => (86_400_000, 10 * 60_000),
        "7d" => (7 * 86_400_000, 60 * 60_000),
        "30d" => (30 * 86_400_000, 6 * 60 * 60_000),
        _ => return Err(context.error(ErrorCode::InvalidRequest)),
    };
    workload::fleet_workload(&state.database, gridops_core::now_millis(), window, bucket)
        .await
        .map(Json)
        .map_err(|error| fleet_error(context, &error))
}

async fn require_admin(
    state: &AppState,
    context: &RequestContext,
    principal: &Principal,
) -> Result<(), FleetApiError> {
    let Principal::AuthenticatedUser { user_id } = principal else {
        return Err(context.error(ErrorCode::Forbidden));
    };
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&state.database)
        .await
        .map_err(|_| context.error(ErrorCode::DependencyUnavailable))?;
    if role.as_deref() == Some("admin") {
        Ok(())
    } else {
        Err(context.error(ErrorCode::Forbidden))
    }
}

fn fleet_error(context: RequestContext, error: &FleetError) -> FleetApiError {
    context.error(match error {
        FleetError::NotFound => ErrorCode::NotFound,
        FleetError::Forbidden => ErrorCode::Forbidden,
        FleetError::InvalidRequest(_) | FleetError::InvalidIdentifier(_) => {
            ErrorCode::InvalidRequest
        }
        FleetError::IdempotencyConflict | FleetError::StaleProfileRevision => {
            ErrorCode::RevisionConflict
        }
        _ => {
            tracing::error!(request_id = %context.id(), "fleet profile dependency failed");
            ErrorCode::DependencyUnavailable
        }
    })
}
