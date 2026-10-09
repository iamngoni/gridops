//! SQL row loading used by the fleet service and admission transaction.

use sqlx::{Row as _, Sqlite, Transaction};

#[derive(Debug, Clone)]
pub(crate) struct OperationRow {
    pub(crate) id: String,
    pub(crate) principal_scope: String,
    pub(crate) request_hash: String,
    pub(crate) status: String,
    pub(crate) reason_code: Option<String>,
}

pub(crate) async fn operation_by_key(
    tx: &mut Transaction<'_, Sqlite>,
    principal_scope: &str,
    idempotency_key: &str,
) -> Result<Option<OperationRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id,principal_scope,request_hash,status,reason_code
           FROM fleet_operations
          WHERE principal_scope = ? AND idempotency_key = ?",
    )
    .bind(principal_scope)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.as_ref().map(operation_from_row))
}

pub(crate) async fn operation_by_id(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<OperationRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id,principal_scope,request_hash,status,reason_code
           FROM fleet_operations
          WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.as_ref().map(operation_from_row))
}

pub(crate) async fn intent_for_operation(
    tx: &mut Transaction<'_, Sqlite>,
    operation_id: &str,
) -> Result<Option<IntentRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id,workload_id,generation,kind,pool_id,profile_id,target_id,agent_run_id,
                expected_profile_revision
           FROM workload_intents WHERE id = ?",
    )
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| IntentRow {
        id: row.get("id"),
        workload_id: row.get("workload_id"),
        generation: row.get("generation"),
        kind: row.get("kind"),
        pool_id: row.get("pool_id"),
        profile_id: row.get("profile_id"),
        target_id: row.get("target_id"),
        agent_run_id: row.get("agent_run_id"),
        expected_profile_revision: row.get("expected_profile_revision"),
    }))
}

#[derive(Debug, Clone)]
pub(crate) struct IntentRow {
    pub(crate) id: String,
    pub(crate) workload_id: String,
    pub(crate) generation: i64,
    pub(crate) kind: String,
    pub(crate) pool_id: String,
    pub(crate) profile_id: String,
    pub(crate) target_id: String,
    pub(crate) agent_run_id: Option<String>,
    pub(crate) expected_profile_revision: i64,
}

fn operation_from_row(row: &sqlx::sqlite::SqliteRow) -> OperationRow {
    OperationRow {
        id: row.get("id"),
        principal_scope: row.get("principal_scope"),
        request_hash: row.get("request_hash"),
        status: row.get("status"),
        reason_code: row.get("reason_code"),
    }
}
