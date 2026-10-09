//! Transactional inventory reconciliation and observational heartbeat writes.
//!
//! The service authenticates and rechecks the complete session tuple inside a
//! writer transaction.  Agent observations update only observed state and
//! probe evidence; policy budgets, grants, allocations, and work authority
//! remain server-owned.

use std::collections::{HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::{Row as _, Sqlite, Transaction};
use thiserror::Error;
use uuid::Uuid;

use super::{
    AuthenticatedHost, RegistryError, RegistryService, audit_and_event,
    ensure_authenticated_generation,
    handshake::{CurrentInventorySession, advance_rotated_session_credential},
};
use crate::fleet::{
    Architecture, BackendReadiness, ExecutionOs, RuntimeKind,
    ids::{BackendId, ControlPlaneIncarnation, HostEpoch, ResourceDomainId, SessionId},
    protocol::{
        inventory::{
            DomainKind, HeartbeatRequest, HeartbeatResponse, HelperState, InventoryIdMapping,
            InventoryPayloadError, InventoryReport, InventoryRequest, InventoryResponse,
            ObservationLease, ObservationSessionState, ObservedBackend, ObservedDomain,
            ObservedInteractiveSession, ObservedSample, SampleCoverage,
        },
        primitives::{BrowserCounter, BrowserUint53, Sha256Digest, WireTimestamp},
    },
};

const OBSERVATION_LEASE_MILLIS: i64 = 90_000;
const HEARTBEAT_INTERVAL_MILLIS: u64 = 15_000;

/// Safe errors suitable for mapping to a typed fleet envelope.  No bearer,
/// verifier, raw inventory JSON, or local runtime path is retained here.
#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("inventory payload is invalid: {0}")]
    Payload(#[from] InventoryPayloadError),
    #[error("fleet control plane is unavailable")]
    DependencyUnavailable,
    #[error("authenticated inventory identity does not match the current host")]
    IdentityConflict,
    #[error("handshake identity conflicts with the durable session")]
    HandshakeConflict,
    #[error("credential generation conflicts with the durable session")]
    CredentialConflict,
    #[error("an observation session is already live for this host")]
    SessionConflict,
    #[error("inventory sequence is stale or was reused with a different body")]
    SequenceConflict,
    #[error("inventory mapping is owned by another host or has changed meaning")]
    MappingConflict,
    #[error("inventory domain graph is too deep")]
    DomainDepth,
    #[error("inventory must contain exactly one physical root domain")]
    RootConflict,
    #[error("heartbeat sequence is stale or was reused with a different body")]
    HeartbeatConflict,
    #[error("JSON encoding failed")]
    Json(#[from] serde_json::Error),
}

impl RegistryService {
    /// Persist one complete inventory for the authenticated observation lease.
    pub async fn submit_inventory(
        &self,
        authenticated: &AuthenticatedHost,
        request: InventoryRequest,
    ) -> Result<InventoryResponse, InventoryError> {
        request.report.validate_shape()?;
        let digest = canonical_inventory_digest(&request.report)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let session = load_current_session(&mut tx, authenticated, &request.report, now).await?;
        let sequence = request.report.inventory_sequence.get();
        let sequence_i64 = i64::try_from(sequence).map_err(|_| {
            RegistryError::InvalidRequest("inventory sequence exceeds SQLite integer")
        })?;
        if sequence_i64 <= session.current_inventory_sequence {
            if sequence_i64 == session.current_inventory_sequence
                && session.current_inventory_digest.as_deref() == Some(digest.as_str())
            {
                let row = sqlx::query(
                    "SELECT digest,received_at,receipt_lease_expires_at,receipt_state,inventory_revision,
                            domain_mappings_json,backend_mappings_json,interactive_mappings_json
                       FROM fleet_inventory_snapshots
                      WHERE host_id=? AND authority_session_id=? AND inventory_sequence=?",
                )
                .bind(session.host_id.to_string())
                .bind(session.authority_session_id.to_string())
                .bind(sequence_i64)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(InventoryError::SequenceConflict)?;
                let stored_digest: String = row.get("digest");
                if stored_digest != digest {
                    return Err(InventoryError::SequenceConflict);
                }
                let domain_mapping_json: Option<String> = row.get("domain_mappings_json");
                let backend_mapping_json: Option<String> = row.get("backend_mappings_json");
                let interactive_mapping_json: Option<String> = row.get("interactive_mappings_json");
                let (domain_ids, backend_ids, interactive_session_ids) = (
                    decode_mappings(domain_mapping_json.as_deref())?,
                    decode_mappings(backend_mapping_json.as_deref())?,
                    decode_mappings(interactive_mapping_json.as_deref())?,
                );
                let response = inventory_response(
                    &request,
                    session.host_id,
                    session.host_epoch,
                    session.control_plane_incarnation,
                    stored_digest,
                    row.get("received_at"),
                    row.get::<Option<i64>, _>("inventory_revision")
                        .ok_or(InventoryError::SequenceConflict)?,
                    parse_session_state(
                        row.get::<Option<String>, _>("receipt_state")
                            .as_deref()
                            .ok_or(InventoryError::SequenceConflict)?,
                    )?,
                    row.get::<Option<i64>, _>("receipt_lease_expires_at")
                        .ok_or(InventoryError::SequenceConflict)?,
                    domain_ids,
                    backend_ids,
                    interactive_session_ids,
                )?;
                tx.commit().await?;
                return Ok(response);
            }
            return Err(InventoryError::SequenceConflict);
        }
        validate_root_and_depth(&request.report)?;
        let (domain_map, backend_map, interactive_map) =
            reconcile_inventory(&mut tx, &session, &request.report, now).await?;
        insert_samples(&mut tx, &session, &request.report.samples, &domain_map, now).await?;
        let snapshot_json = canonical_inventory_json(&request.report)?;
        let revision: i64 =
            sqlx::query_scalar("SELECT inventory_revision FROM fleet_hosts WHERE id=?")
                .bind(session.host_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        let next_revision = revision
            .checked_add(1)
            .ok_or(RegistryError::InvalidRequest(
                "inventory revision overflowed",
            ))?;
        let lease_expires =
            now.checked_add(OBSERVATION_LEASE_MILLIS)
                .ok_or(RegistryError::InvalidRequest(
                    "observation lease overflowed",
                ))?;
        let domain_ids = sorted_id_mappings(domain_map.clone())?;
        let backend_ids = sorted_id_mappings(backend_map.clone())?;
        let interactive_session_ids = sorted_id_mappings(interactive_map.clone())?;
        let domain_mappings_json = serde_json::to_string(&domain_ids)?;
        let backend_mappings_json = serde_json::to_string(&backend_ids)?;
        let interactive_mappings_json = serde_json::to_string(&interactive_session_ids)?;
        sqlx::query(
            "INSERT INTO fleet_inventory_snapshots
             (host_id,authority_session_id,host_epoch,control_plane_incarnation,inventory_sequence,digest,snapshot_json,received_at,
              receipt_lease_expires_at,receipt_state,inventory_revision,domain_mappings_json,backend_mappings_json,interactive_mappings_json)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(session.host_id.to_string())
        .bind(session.authority_session_id.to_string())
        .bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?)
        .bind(session.control_plane_incarnation.to_string())
        .bind(sequence_i64)
        .bind(&digest)
        .bind(snapshot_json)
        .bind(now)
        .bind(lease_expires)
        .bind("reconciling")
        .bind(next_revision)
        .bind(domain_mappings_json)
        .bind(backend_mappings_json)
        .bind(interactive_mappings_json)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_revision=?,inventory_digest=?,inventory_epoch=?,inventory_session_id=?,inventory_boot_id=?,
                inventory_complete=1,last_inventory_at=?,updated_at=? WHERE id=?",
        )
        .bind(next_revision)
        .bind(&digest)
        .bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?)
        .bind(session.authority_session_id.to_string())
        .bind(session.boot_id.as_str())
        .bind(now)
        .bind(now)
        .bind(session.host_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET state='reconciling',last_seen_at=?,lease_expires_at=?,current_inventory_sequence=?,current_inventory_digest=?,current_inventory_received_at=? WHERE session_id=? AND host_id=?",
        )
        .bind(now)
        .bind(lease_expires)
        .bind(sequence_i64)
        .bind(&digest)
        .bind(now)
        .bind(session.authority_session_id.to_string())
        .bind(session.host_id.to_string())
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            None,
            Some(session.host_id),
            "fleet.observation.inventory_received",
            "host",
            &session.host_id.to_string(),
            json!({"inventory_sequence": sequence, "inventory_revision": next_revision}),
            now,
        )
        .await?;
        tx.commit().await?;
        inventory_response(
            &request,
            session.host_id,
            session.host_epoch,
            session.control_plane_incarnation,
            digest,
            now,
            next_revision,
            ObservationSessionState::Reconciling,
            lease_expires,
            domain_ids,
            backend_ids,
            interactive_session_ids,
        )
    }

    /// Persist partial measurements and renew only the observation lease.
    pub async fn heartbeat(
        &self,
        authenticated: &AuthenticatedHost,
        request: HeartbeatRequest,
    ) -> Result<HeartbeatResponse, InventoryError> {
        request.validate_shape()?;
        let digest = canonical_heartbeat_digest(&request)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let report = InventoryReport {
            host_epoch: request.host_epoch,
            control_plane_incarnation: request.control_plane_incarnation,
            authority_session_id: request.authority_session_id,
            boot_id: request.boot_id.clone(),
            inventory_sequence: request.heartbeat_sequence,
            observed_at: request.observed_at,
            domains: Vec::new(),
            backends: Vec::new(),
            interactive_sessions: Vec::new(),
            samples: request.samples.clone(),
        };
        let session = load_current_session(&mut tx, authenticated, &report, now).await?;
        let sequence_i64 = i64::try_from(request.heartbeat_sequence.get()).map_err(|_| {
            RegistryError::InvalidRequest("heartbeat sequence exceeds SQLite integer")
        })?;
        if sequence_i64 <= session.current_heartbeat_sequence {
            if sequence_i64 == session.current_heartbeat_sequence
                && session.current_heartbeat_digest.as_deref() == Some(digest.as_str())
            {
                let received = session
                    .current_heartbeat_received_at
                    .ok_or(InventoryError::HeartbeatConflict)?;
                let receipt_state = session
                    .current_heartbeat_state
                    .ok_or(InventoryError::HeartbeatConflict)?;
                let response = heartbeat_response(
                    &request,
                    &session,
                    received,
                    session
                        .current_heartbeat_lease_expires_at
                        .ok_or(InventoryError::HeartbeatConflict)?,
                    receipt_state,
                )?;
                tx.commit().await?;
                return Ok(response);
            }
            return Err(InventoryError::HeartbeatConflict);
        }
        let known_domains: HashMap<String, ResourceDomainId> =
            sqlx::query("SELECT local_key,domain_id FROM fleet_observed_domains WHERE host_id=?")
                .bind(session.host_id.to_string())
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .filter_map(|row| {
                    let key: String = row.get("local_key");
                    let id: Result<ResourceDomainId, _> = row.get::<String, _>("domain_id").parse();
                    id.ok().map(|id| (key, id))
                })
                .collect();
        insert_samples(&mut tx, &session, &request.samples, &known_domains, now).await?;
        let lease_expires =
            now.checked_add(OBSERVATION_LEASE_MILLIS)
                .ok_or(RegistryError::InvalidRequest(
                    "observation lease overflowed",
                ))?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET last_seen_at=?,lease_expires_at=?,current_heartbeat_sequence=?,current_heartbeat_digest=?,current_heartbeat_received_at=?,current_heartbeat_lease_expires_at=?,current_heartbeat_state=? WHERE session_id=? AND host_id=?",
        )
        .bind(now).bind(lease_expires).bind(sequence_i64).bind(&digest).bind(now)
        .bind(lease_expires).bind(state_name(session.state))
        .bind(session.authority_session_id.to_string()).bind(session.host_id.to_string())
        .execute(&mut *tx).await?;
        sqlx::query("UPDATE fleet_hosts SET last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(now)
            .bind(now)
            .bind(session.host_id.to_string())
            .execute(&mut *tx)
            .await?;
        audit_and_event(
            &mut tx,
            None,
            Some(session.host_id),
            "fleet.observation.heartbeat_received",
            "host",
            &session.host_id.to_string(),
            json!({"heartbeat_sequence": request.heartbeat_sequence.get()}),
            now,
        )
        .await?;
        tx.commit().await?;
        heartbeat_response(&request, &session, now, lease_expires, session.state)
    }
}

async fn load_current_session(
    tx: &mut Transaction<'_, Sqlite>,
    authenticated: &AuthenticatedHost,
    report: &InventoryReport,
    now: i64,
) -> Result<CurrentInventorySession, InventoryError> {
    ensure_authenticated_generation(tx, authenticated, now).await?;
    if report.host_epoch != authenticated.epoch || report.boot_id.as_str().is_empty() {
        return Err(InventoryError::IdentityConflict);
    }
    let control = sqlx::query("SELECT incarnation FROM fleet_control_plane WHERE singleton=1")
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(InventoryError::DependencyUnavailable)?;
    let expected_incarnation: ControlPlaneIncarnation = control
        .get::<String, _>("incarnation")
        .parse()
        .map_err(|_| RegistryError::InvalidRequest("control-plane incarnation is malformed"))?;
    if expected_incarnation != report.control_plane_incarnation {
        return Err(InventoryError::IdentityConflict);
    }
    let row = sqlx::query(
        "SELECT session_id,host_epoch,boot_id,control_plane_incarnation,current_credential_id,current_credential_generation,
                state,lease_expires_at,current_inventory_sequence,current_inventory_digest,current_inventory_received_at,
                current_heartbeat_sequence,current_heartbeat_digest,current_heartbeat_received_at,
                current_heartbeat_lease_expires_at,current_heartbeat_state
           FROM fleet_observation_sessions WHERE host_id=? AND session_id=? AND ended_at IS NULL",
    ).bind(authenticated.host_id.to_string()).bind(report.authority_session_id.to_string())
    .fetch_optional(&mut **tx).await?.ok_or(InventoryError::IdentityConflict)?;
    let host = sqlx::query(
        "SELECT agent_session_id,boot_id,authority_incarnation FROM fleet_hosts WHERE id=?",
    )
    .bind(authenticated.host_id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(InventoryError::IdentityConflict)?;
    let report_session_id = report.authority_session_id.to_string();
    let report_incarnation = report.control_plane_incarnation.to_string();
    if host.get::<Option<String>, _>("agent_session_id").as_deref()
        != Some(report_session_id.as_str())
        || host.get::<Option<String>, _>("boot_id").as_deref() != Some(report.boot_id.as_str())
        || host
            .get::<Option<String>, _>("authority_incarnation")
            .as_deref()
            != Some(report_incarnation.as_str())
    {
        return Err(InventoryError::IdentityConflict);
    }
    if row.get::<i64, _>("host_epoch")
        != i64::try_from(report.host_epoch.get())
            .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?
        || row.get::<String, _>("boot_id") != report.boot_id.as_str()
        || row.get::<String, _>("control_plane_incarnation")
            != report.control_plane_incarnation.to_string()
    {
        return Err(InventoryError::IdentityConflict);
    }
    if row.get::<String, _>("current_credential_id") != authenticated.credential_id.to_string()
        || row.get::<i64, _>("current_credential_generation")
            != i64::try_from(authenticated.generation.get())
                .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?
    {
        advance_rotated_session_credential(
            tx,
            authenticated.host_id,
            report_session_id.as_str(),
            row.get("current_credential_id"),
            row.get("current_credential_generation"),
            authenticated,
            now,
        )
        .await?;
    }
    let state = session_state(row.get::<String, _>("state").as_str());
    if state == ObservationSessionState::Conflicted {
        return Err(InventoryError::SessionConflict);
    }
    if row.get::<i64, _>("lease_expires_at") <= now {
        return Err(InventoryError::SessionConflict);
    }
    let current_inventory_sequence: i64 = row.get("current_inventory_sequence");
    let current_inventory_digest: Option<String> = row.get("current_inventory_digest");
    let current_inventory_received_at: Option<i64> = row.get("current_inventory_received_at");
    if (current_inventory_sequence == 0
        && (current_inventory_digest.is_some() || current_inventory_received_at.is_some()))
        || (current_inventory_sequence > 0
            && (current_inventory_digest.is_none() || current_inventory_received_at.is_none()))
    {
        return Err(InventoryError::SequenceConflict);
    }
    let current_heartbeat_sequence: i64 = row.get("current_heartbeat_sequence");
    let current_heartbeat_digest: Option<String> = row.get("current_heartbeat_digest");
    let current_heartbeat_received_at: Option<i64> = row.get("current_heartbeat_received_at");
    let current_heartbeat_lease_expires_at: Option<i64> =
        row.get("current_heartbeat_lease_expires_at");
    let current_heartbeat_state = row
        .get::<Option<String>, _>("current_heartbeat_state")
        .as_deref()
        .map(parse_session_state)
        .transpose()?;
    if (current_heartbeat_sequence == 0
        && (current_heartbeat_digest.is_some()
            || current_heartbeat_received_at.is_some()
            || current_heartbeat_lease_expires_at.is_some()
            || current_heartbeat_state.is_some()))
        || (current_heartbeat_sequence > 0
            && (current_heartbeat_digest.is_none()
                || current_heartbeat_received_at.is_none()
                || current_heartbeat_lease_expires_at.is_none()
                || current_heartbeat_state.is_none()))
    {
        return Err(InventoryError::HeartbeatConflict);
    }
    Ok(CurrentInventorySession {
        host_id: authenticated.host_id,
        host_epoch: authenticated.epoch,
        authority_session_id: report.authority_session_id,
        boot_id: report.boot_id.clone(),
        control_plane_incarnation: report.control_plane_incarnation,
        state,
        lease_expires_at: row.get("lease_expires_at"),
        current_inventory_sequence,
        current_inventory_digest,
        current_inventory_received_at,
        current_heartbeat_sequence,
        current_heartbeat_digest,
        current_heartbeat_received_at,
        current_heartbeat_lease_expires_at,
        current_heartbeat_state,
    })
}

fn session_state(value: &str) -> ObservationSessionState {
    match value {
        "observing" => ObservationSessionState::Observing,
        "ready" => ObservationSessionState::Ready,
        "conflicted" => ObservationSessionState::Conflicted,
        _ => ObservationSessionState::Reconciling,
    }
}

fn parse_session_state(value: &str) -> Result<ObservationSessionState, InventoryError> {
    match value {
        "observing" => Ok(ObservationSessionState::Observing),
        "reconciling" => Ok(ObservationSessionState::Reconciling),
        "ready" => Ok(ObservationSessionState::Ready),
        "conflicted" => Ok(ObservationSessionState::Conflicted),
        _ => Err(InventoryError::MappingConflict),
    }
}

fn state_name(value: ObservationSessionState) -> &'static str {
    match value {
        ObservationSessionState::Observing => "observing",
        ObservationSessionState::Reconciling => "reconciling",
        ObservationSessionState::Ready => "ready",
        ObservationSessionState::Conflicted => "conflicted",
    }
}

fn decode_mappings<T>(value: Option<&str>) -> Result<Vec<InventoryIdMapping<T>>, InventoryError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(value.ok_or(InventoryError::SequenceConflict)?)
        .map_err(InventoryError::from)
}

fn validate_root_and_depth(report: &InventoryReport) -> Result<(), InventoryError> {
    let roots = report
        .domains
        .iter()
        .filter(|domain| domain.parent_local_key.is_none())
        .collect::<Vec<_>>();
    if roots.len() != 1 || roots[0].kind != DomainKind::Physical {
        return Err(InventoryError::RootConflict);
    }
    let by_key: HashMap<&str, &ObservedDomain> = report
        .domains
        .iter()
        .map(|domain| (domain.local_key.as_str(), domain))
        .collect();
    for domain in &report.domains {
        let mut current = domain;
        let mut depth = 1;
        let mut seen = HashSet::new();
        while let Some(parent) = &current.parent_local_key {
            if !seen.insert(parent.as_str()) {
                return Err(InventoryError::Payload(
                    InventoryPayloadError::InvalidDomainGraph,
                ));
            }
            current = by_key
                .get(parent.as_str())
                .copied()
                .ok_or(InventoryError::Payload(
                    InventoryPayloadError::InvalidDomainGraph,
                ))?;
            depth += 1;
            if depth > 64 {
                return Err(InventoryError::DomainDepth);
            }
        }
    }
    Ok(())
}

async fn reconcile_inventory(
    tx: &mut Transaction<'_, Sqlite>,
    session: &CurrentInventorySession,
    report: &InventoryReport,
    now: i64,
) -> Result<
    (
        HashMap<String, ResourceDomainId>,
        HashMap<String, BackendId>,
        HashMap<String, SessionId>,
    ),
    InventoryError,
> {
    report
        .domains
        .iter()
        .find(|domain| domain.parent_local_key.is_none())
        .ok_or(InventoryError::RootConflict)?;
    let root_id: ResourceDomainId = sqlx::query_scalar::<_, String>(
        "SELECT id FROM host_resource_domains WHERE host_id=? AND parent_domain_id IS NULL",
    )
    .bind(session.host_id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(InventoryError::MappingConflict)?
    .parse()
    .map_err(|_| RegistryError::InvalidRequest("root domain ID is malformed"))?;
    ensure_domain_owner(tx, session.host_id, root_id).await?;
    let mut domain_map: HashMap<String, ResourceDomainId> = HashMap::new();
    let mut pending = report.domains.clone();
    let by_key: HashMap<&str, &ObservedDomain> = report
        .domains
        .iter()
        .map(|domain| (domain.local_key.as_str(), domain))
        .collect();
    pending.sort_by_key(|domain| {
        let mut depth = 0;
        let mut key = domain.local_key.as_str();
        while let Some(parent) = by_key
            .get(key)
            .and_then(|current| current.parent_local_key.as_ref())
        {
            depth += 1;
            key = parent.as_str();
        }
        depth
    });
    for domain in pending {
        let id = if let Some(mapping) = sqlx::query("SELECT domain_id,parent_local_key,kind,runtime_identity FROM fleet_observed_domains WHERE host_id=? AND local_key=?")
            .bind(session.host_id.to_string()).bind(domain.local_key.as_str()).fetch_optional(&mut **tx).await? {
            let mapped: ResourceDomainId = mapping.get::<String, _>("domain_id").parse().map_err(|_| RegistryError::InvalidRequest("domain mapping ID is malformed"))?;
            if let Some(known) = domain.known_id { if known != mapped { return Err(InventoryError::MappingConflict); } }
            if mapping.get::<String, _>("kind") != domain_kind(domain.kind) { return Err(InventoryError::MappingConflict); }
            if mapping.get::<Option<String>, _>("parent_local_key")
                != domain.parent_local_key.as_ref().map(|value| value.as_str().to_owned())
                || mapping.get::<Option<String>, _>("runtime_identity")
                    != domain.runtime_identity.as_ref().map(|value| value.as_str().to_owned())
            {
                return Err(InventoryError::MappingConflict);
            }
            mapped
        } else if domain.parent_local_key.is_none() {
            if domain.known_id.is_some_and(|known| known != root_id) { return Err(InventoryError::MappingConflict); }
            root_id
        } else if domain.known_id.is_some() {
            return Err(InventoryError::MappingConflict);
        } else {
            ResourceDomainId::try_new(Uuid::new_v4()).map_err(|_| RegistryError::InvalidRequest("domain ID generation failed"))?
        };
        if let Some(parent) = &domain.parent_local_key {
            let parent_id = *domain_map
                .get(parent.as_str())
                .ok_or(InventoryError::MappingConflict)?;
            let existing_parent: Option<String> = sqlx::query_scalar(
                "SELECT parent_domain_id FROM host_resource_domains WHERE id=? AND host_id=?",
            )
            .bind(id.to_string())
            .bind(session.host_id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
            if existing_parent.as_deref() != Some(parent_id.to_string().as_str()) {
                if existing_parent.is_some() {
                    return Err(InventoryError::MappingConflict);
                }
                sqlx::query("INSERT INTO host_resource_domains (id,host_id,parent_domain_id,name,cpu_millis,memory_mib,disk_bytes,revision) VALUES (?,?,?, ?,0,0,0,1)")
                    .bind(id.to_string()).bind(session.host_id.to_string()).bind(parent_id.to_string()).bind(domain.name.as_str()).execute(&mut **tx).await?;
            }
        }
        let parent_key = domain
            .parent_local_key
            .as_ref()
            .map(|value| value.as_str().to_owned());
        sqlx::query(
            "INSERT INTO fleet_observed_domains (host_id,local_key,domain_id,parent_local_key,kind,observed_name,runtime_identity,cpu_millis,memory_mib,disk_bytes,session_id,host_epoch,control_plane_incarnation,inventory_sequence,received_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(host_id,local_key) DO UPDATE SET parent_local_key=excluded.parent_local_key,observed_name=excluded.observed_name,runtime_identity=excluded.runtime_identity,cpu_millis=excluded.cpu_millis,memory_mib=excluded.memory_mib,disk_bytes=excluded.disk_bytes,session_id=excluded.session_id,host_epoch=excluded.host_epoch,control_plane_incarnation=excluded.control_plane_incarnation,inventory_sequence=excluded.inventory_sequence,received_at=excluded.received_at",
        ).bind(session.host_id.to_string()).bind(domain.local_key.as_str()).bind(id.to_string()).bind(parent_key).bind(domain_kind(domain.kind)).bind(domain.name.as_str()).bind(domain.runtime_identity.as_ref().map(|value| value.as_str())).bind(opt_u64(domain.capacity.cpu_millis)).bind(opt_u64(domain.capacity.memory_mib)).bind(opt_u64(domain.capacity.disk_bytes)).bind(session.authority_session_id.to_string()).bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?).bind(session.control_plane_incarnation.to_string()).bind(i64::try_from(report.inventory_sequence.get()).map_err(|_| RegistryError::InvalidRequest("sequence exceeds SQLite integer"))?).bind(now).execute(&mut **tx).await?;
        domain_map.insert(domain.local_key.as_str().to_owned(), id);
    }
    sqlx::query("UPDATE host_backends SET readiness='unknown',reason_code='stale_inventory',last_probed_at=NULL,probe_epoch=NULL,probe_session_id=NULL,probe_boot_id=NULL,probe_incarnation=NULL,updated_at=? WHERE host_id=?")
        .bind(now).bind(session.host_id.to_string()).execute(&mut **tx).await?;
    sqlx::query(
        "UPDATE host_sessions SET helper_state='unknown',last_seen_at=NULL WHERE host_id=?",
    )
    .bind(session.host_id.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE fleet_observed_interactive_sessions SET observed_helper_state='unknown',authority_session_id=?,boot_id=?,host_epoch=?,control_plane_incarnation=? WHERE host_id=?")
        .bind(session.authority_session_id.to_string())
        .bind(session.boot_id.as_str())
        .bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?)
        .bind(session.control_plane_incarnation.to_string())
        .bind(session.host_id.to_string())
        .execute(&mut **tx)
        .await?;
    let mut backend_map = HashMap::new();
    for backend in &report.backends {
        let domain_id = *domain_map
            .get(backend.domain_local_key.as_str())
            .ok_or(InventoryError::MappingConflict)?;
        let id = upsert_backend(tx, session, report, backend, domain_id, now).await?;
        backend_map.insert(backend.local_key.as_str().to_owned(), id);
    }
    let mut interactive_map = HashMap::new();
    for helper in &report.interactive_sessions {
        let id = upsert_interactive(tx, session, report, helper, now).await?;
        interactive_map.insert(helper.local_key.as_str().to_owned(), id);
    }
    Ok((domain_map, backend_map, interactive_map))
}

async fn ensure_domain_owner(
    tx: &mut Transaction<'_, Sqlite>,
    host_id: crate::fleet::ids::HostId,
    id: ResourceDomainId,
) -> Result<(), InventoryError> {
    let owner: Option<String> =
        sqlx::query_scalar("SELECT host_id FROM host_resource_domains WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
    if owner.as_deref() != Some(host_id.to_string().as_str()) {
        return Err(InventoryError::MappingConflict);
    }
    Ok(())
}

async fn upsert_backend(
    tx: &mut Transaction<'_, Sqlite>,
    session: &CurrentInventorySession,
    report: &InventoryReport,
    backend: &ObservedBackend,
    domain_id: ResourceDomainId,
    now: i64,
) -> Result<BackendId, InventoryError> {
    let id = if let Some(mapping) = sqlx::query("SELECT backend_id,domain_local_key,runtime_kind,execution_os,architecture,runtime_identity FROM fleet_observed_backends WHERE host_id=? AND local_key=?").bind(session.host_id.to_string()).bind(backend.local_key.as_str()).fetch_optional(&mut **tx).await? {
        let mapped: BackendId = mapping.get::<String, _>("backend_id").parse().map_err(|_| RegistryError::InvalidRequest("backend mapping ID is malformed"))?;
        if backend.known_id.is_some_and(|known| known != mapped) || mapping.get::<String, _>("domain_local_key") != backend.domain_local_key.as_str() || mapping.get::<String, _>("runtime_kind") != runtime_name(backend.runtime_kind) || mapping.get::<String, _>("execution_os") != os_name(backend.execution_os) || mapping.get::<String, _>("architecture") != arch_name(backend.architecture) || mapping.get::<Option<String>, _>("runtime_identity") != backend.runtime_identity.as_ref().map(|value| value.as_str().to_owned()) { return Err(InventoryError::MappingConflict); }
        mapped
    } else if backend.known_id.is_some() {
        return Err(InventoryError::MappingConflict);
    } else { BackendId::try_new(Uuid::new_v4()).map_err(|_| RegistryError::InvalidRequest("backend ID generation failed"))? };
    let existing = sqlx::query("SELECT domain_id,runtime_kind,execution_os,architecture FROM host_backends WHERE id=? AND host_id=?").bind(id.to_string()).bind(session.host_id.to_string()).fetch_optional(&mut **tx).await?;
    if let Some(row) = existing {
        if row.get::<String, _>("domain_id") != domain_id.to_string()
            || row.get::<String, _>("runtime_kind") != runtime_name(backend.runtime_kind)
            || row.get::<String, _>("execution_os") != os_name(backend.execution_os)
            || row.get::<String, _>("architecture") != arch_name(backend.architecture)
        {
            return Err(InventoryError::MappingConflict);
        }
    } else {
        let caps = serde_json::to_string(backend.capabilities.report())?;
        sqlx::query("INSERT INTO host_backends (id,host_id,domain_id,runtime_kind,execution_os,architecture,readiness,reason_code,enabled,revision,capabilities_json,config_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?, ?,0,1,?,'{}',?,?)")
            .bind(id.to_string()).bind(session.host_id.to_string()).bind(domain_id.to_string()).bind(runtime_name(backend.runtime_kind)).bind(os_name(backend.execution_os)).bind(arch_name(backend.architecture)).bind(readiness_name(backend.readiness)).bind(backend.reason.as_ref().map(|value| value.as_str())).bind(caps).bind(now).bind(now).execute(&mut **tx).await?;
    }
    let caps = serde_json::to_string(backend.capabilities.report())?;
    sqlx::query("UPDATE host_backends SET readiness=?,reason_code=?,capabilities_json=?,last_probed_at=?,probe_epoch=?,probe_session_id=?,probe_boot_id=?,probe_incarnation=?,updated_at=? WHERE id=? AND host_id=?")
        .bind(readiness_name(backend.readiness)).bind(backend.reason.as_ref().map(|value| value.as_str())).bind(&caps).bind(now).bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?).bind(session.authority_session_id.to_string()).bind(session.boot_id.as_str()).bind(session.control_plane_incarnation.to_string()).bind(now).bind(id.to_string()).bind(session.host_id.to_string()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO fleet_observed_backends (host_id,local_key,backend_id,domain_local_key,runtime_kind,execution_os,architecture,capabilities_json,observed_readiness,reason,runtime_identity,session_id,host_epoch,control_plane_incarnation,inventory_sequence,received_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(host_id,local_key) DO UPDATE SET domain_local_key=excluded.domain_local_key,capabilities_json=excluded.capabilities_json,observed_readiness=excluded.observed_readiness,reason=excluded.reason,runtime_identity=excluded.runtime_identity,session_id=excluded.session_id,host_epoch=excluded.host_epoch,control_plane_incarnation=excluded.control_plane_incarnation,inventory_sequence=excluded.inventory_sequence,received_at=excluded.received_at")
        .bind(session.host_id.to_string()).bind(backend.local_key.as_str()).bind(id.to_string()).bind(backend.domain_local_key.as_str()).bind(runtime_name(backend.runtime_kind)).bind(os_name(backend.execution_os)).bind(arch_name(backend.architecture)).bind(caps).bind(readiness_name(backend.readiness)).bind(backend.reason.as_ref().map(|value| value.as_str())).bind(backend.runtime_identity.as_ref().map(|value| value.as_str())).bind(session.authority_session_id.to_string()).bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?).bind(session.control_plane_incarnation.to_string()).bind(i64::try_from(report.inventory_sequence.get()).map_err(|_| RegistryError::InvalidRequest("sequence exceeds SQLite integer"))?).bind(now).execute(&mut **tx).await?;
    Ok(id)
}

async fn upsert_interactive(
    tx: &mut Transaction<'_, Sqlite>,
    session: &CurrentInventorySession,
    report: &InventoryReport,
    helper: &ObservedInteractiveSession,
    now: i64,
) -> Result<SessionId, InventoryError> {
    let id = if let Some(mapping) = sqlx::query("SELECT session_id,os_user_id FROM fleet_observed_interactive_sessions WHERE host_id=? AND local_key=?").bind(session.host_id.to_string()).bind(helper.local_key.as_str()).fetch_optional(&mut **tx).await? {
        let mapped: SessionId = mapping.get::<String, _>("session_id").parse().map_err(|_| RegistryError::InvalidRequest("session mapping ID is malformed"))?;
        if helper.known_id.is_some_and(|known| known != mapped)
            || mapping.get::<String, _>("os_user_id") != helper.os_user_id.as_str()
        { return Err(InventoryError::MappingConflict); } mapped
    } else if helper.known_id.is_some() {
        return Err(InventoryError::MappingConflict);
    } else {
        let key_exists = sqlx::query_scalar::<_, String>(
            "SELECT id FROM host_sessions WHERE host_id=? AND session_key=?",
        )
        .bind(session.host_id.to_string())
        .bind(helper.local_key.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .is_some();
        if key_exists {
            return Err(InventoryError::MappingConflict);
        }
        SessionId::try_new(Uuid::new_v4())
            .map_err(|_| RegistryError::InvalidRequest("session ID generation failed"))?
    };
    let owner: Option<String> = sqlx::query_scalar("SELECT host_id FROM host_sessions WHERE id=?")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await?;
    if owner.is_none() {
        sqlx::query("INSERT INTO host_sessions (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,revision) VALUES (?,?,?,?,?,0,1,1)").bind(id.to_string()).bind(session.host_id.to_string()).bind(helper.local_key.as_str()).bind(helper.os_user_id.as_str()).bind(helper_state_name(helper.helper_state)).execute(&mut **tx).await?;
    } else if owner.as_deref() != Some(session.host_id.to_string().as_str()) {
        return Err(InventoryError::MappingConflict);
    }
    let existing_user: Option<String> =
        sqlx::query_scalar("SELECT os_user_id FROM host_sessions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
    if existing_user.as_deref() != Some(helper.os_user_id.as_str()) {
        return Err(InventoryError::MappingConflict);
    }
    sqlx::query("UPDATE host_sessions SET helper_state=?,last_seen_at=? WHERE id=? AND host_id=?")
        .bind(helper_state_name(helper.helper_state))
        .bind(now)
        .bind(id.to_string())
        .bind(session.host_id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO fleet_observed_interactive_sessions (host_id,local_key,session_id,os_user_id,observed_helper_state,authority_session_id,boot_id,host_epoch,control_plane_incarnation,inventory_sequence,received_at) VALUES (?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(host_id,local_key) DO UPDATE SET observed_helper_state=excluded.observed_helper_state,authority_session_id=excluded.authority_session_id,boot_id=excluded.boot_id,host_epoch=excluded.host_epoch,control_plane_incarnation=excluded.control_plane_incarnation,inventory_sequence=excluded.inventory_sequence,received_at=excluded.received_at").bind(session.host_id.to_string()).bind(helper.local_key.as_str()).bind(id.to_string()).bind(helper.os_user_id.as_str()).bind(helper_state_name(helper.helper_state)).bind(session.authority_session_id.to_string()).bind(session.boot_id.as_str()).bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?).bind(session.control_plane_incarnation.to_string()).bind(i64::try_from(report.inventory_sequence.get()).map_err(|_| RegistryError::InvalidRequest("sequence exceeds SQLite integer"))?).bind(now).execute(&mut **tx).await?;
    Ok(id)
}

async fn insert_samples(
    tx: &mut Transaction<'_, Sqlite>,
    session: &CurrentInventorySession,
    samples: &[ObservedSample],
    domains: &HashMap<String, ResourceDomainId>,
    now: i64,
) -> Result<(), InventoryError> {
    for sample in samples {
        let domain_id = domains
            .get(sample.domain_local_key.as_str())
            .ok_or(InventoryError::MappingConflict)?;
        sqlx::query("INSERT INTO host_samples (host_id,domain_id,host_epoch,observed_at,received_at,cpu_used_millis,memory_used_mib,disk_free_bytes,uptime_seconds,coverage,authority_session_id,boot_id,control_plane_incarnation) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(session.host_id.to_string()).bind(domain_id.to_string()).bind(i64::try_from(session.host_epoch.get()).map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?).bind(sample.observed_at.millis()).bind(now).bind(opt_u64(sample.cpu_used_millis)).bind(opt_u64(sample.memory_used_mib)).bind(opt_u64(sample.disk_free_bytes)).bind(opt_u64(sample.uptime_seconds)).bind(coverage_name(sample.coverage)).bind(session.authority_session_id.to_string()).bind(session.boot_id.as_str()).bind(session.control_plane_incarnation.to_string()).execute(&mut **tx).await?;
    }
    Ok(())
}

fn canonical_inventory_json(report: &InventoryReport) -> Result<String, InventoryError> {
    Ok(String::from_utf8(canonical_inventory_bytes(report)?)
        .map_err(|_| RegistryError::InvalidRequest("canonical inventory is not UTF-8"))?)
}
fn canonical_inventory_digest(report: &InventoryReport) -> Result<String, InventoryError> {
    Ok(String::from(Sha256Digest::of(&canonical_inventory_bytes(
        report,
    )?)))
}
fn canonical_inventory_bytes(report: &InventoryReport) -> Result<Vec<u8>, InventoryError> {
    let mut value = serde_json::to_value(report)?;
    if let Some(items) = value.get_mut("domains").and_then(Value::as_array_mut) {
        items.sort_by_key(|item| {
            item.get("local_key")
                .and_then(Value::as_str)
                .map_or("", |value| value)
                .to_owned()
        });
    }
    if let Some(items) = value.get_mut("backends").and_then(Value::as_array_mut) {
        items.sort_by_key(|item| {
            item.get("local_key")
                .and_then(Value::as_str)
                .map_or("", |value| value)
                .to_owned()
        });
    }
    if let Some(items) = value
        .get_mut("interactive_sessions")
        .and_then(Value::as_array_mut)
    {
        items.sort_by_key(|item| {
            item.get("local_key")
                .and_then(Value::as_str)
                .map_or("", |value| value)
                .to_owned()
        });
    }
    if let Some(items) = value.get_mut("samples").and_then(Value::as_array_mut) {
        items.sort_by_key(|item| {
            item.get("domain_local_key")
                .and_then(Value::as_str)
                .map_or("", |value| value)
                .to_owned()
        });
    }
    Ok(serde_json::to_vec(&value)?)
}
fn canonical_heartbeat_digest(request: &HeartbeatRequest) -> Result<String, InventoryError> {
    Ok(String::from(Sha256Digest::of(&serde_json::to_vec(
        request,
    )?)))
}

#[allow(clippy::too_many_arguments)]
fn inventory_response(
    request: &InventoryRequest,
    host_id: crate::fleet::ids::HostId,
    epoch: HostEpoch,
    incarnation: ControlPlaneIncarnation,
    digest: String,
    received: i64,
    revision: i64,
    state: ObservationSessionState,
    lease_expires: i64,
    domain_ids: Vec<InventoryIdMapping<ResourceDomainId>>,
    backend_ids: Vec<InventoryIdMapping<BackendId>>,
    interactive_session_ids: Vec<InventoryIdMapping<SessionId>>,
) -> Result<InventoryResponse, InventoryError> {
    Ok(InventoryResponse {
        request_id: request.request_id,
        host_id,
        host_epoch: epoch,
        control_plane_incarnation: incarnation,
        authority_session_id: request.report.authority_session_id,
        inventory_sequence: request.report.inventory_sequence,
        inventory_revision: BrowserCounter::new(
            u64::try_from(revision)
                .map_err(|_| RegistryError::InvalidRequest("inventory revision is malformed"))?,
        )
        .map_err(|_| RegistryError::InvalidRequest("inventory revision is malformed"))?,
        digest: Sha256Digest::try_from(digest)
            .map_err(|_| RegistryError::InvalidRequest("inventory digest is malformed"))?,
        domain_ids,
        backend_ids,
        interactive_session_ids,
        received_at: WireTimestamp::from_millis(received)
            .map_err(|_| RegistryError::InvalidRequest("receipt timestamp is malformed"))?,
        state,
        lease: lease(lease_expires)?,
    })
}
fn heartbeat_response(
    request: &HeartbeatRequest,
    session: &CurrentInventorySession,
    received: i64,
    lease_expires: i64,
    state: ObservationSessionState,
) -> Result<HeartbeatResponse, InventoryError> {
    Ok(HeartbeatResponse {
        request_id: request.request_id,
        host_id: session.host_id,
        host_epoch: session.host_epoch,
        control_plane_incarnation: session.control_plane_incarnation,
        authority_session_id: session.authority_session_id,
        heartbeat_sequence: request.heartbeat_sequence,
        received_at: WireTimestamp::from_millis(received)
            .map_err(|_| RegistryError::InvalidRequest("receipt timestamp is malformed"))?,
        state,
        lease: lease(lease_expires)?,
    })
}

fn sorted_id_mappings<T>(
    mappings: HashMap<String, T>,
) -> Result<Vec<InventoryIdMapping<T>>, InventoryError> {
    let mut mappings = mappings
        .into_iter()
        .map(|(key, id)| {
            Ok(InventoryIdMapping {
                local_key: crate::fleet::ids::UpstreamText::new(key)
                    .map_err(|_| RegistryError::InvalidRequest("inventory key is malformed"))?,
                server_id: id,
            })
        })
        .collect::<Result<Vec<_>, InventoryError>>()?;
    mappings.sort_by(|left, right| left.local_key.as_str().cmp(right.local_key.as_str()));
    Ok(mappings)
}

fn lease(expires: i64) -> Result<ObservationLease, InventoryError> {
    Ok(ObservationLease {
        expires_at: WireTimestamp::from_millis(expires)
            .map_err(|_| RegistryError::InvalidRequest("lease timestamp is malformed"))?,
        heartbeat_ms: BrowserUint53::new(HEARTBEAT_INTERVAL_MILLIS)
            .map_err(|_| RegistryError::InvalidRequest("heartbeat interval is malformed"))?,
    })
}
fn opt_u64(value: Option<BrowserUint53>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value.get()).ok())
}
fn domain_kind(value: DomainKind) -> &'static str {
    match value {
        DomainKind::Physical => "physical",
        DomainKind::VirtualMachine => "virtual_machine",
        DomainKind::Container => "container",
    }
}
fn runtime_name(value: RuntimeKind) -> &'static str {
    match value {
        RuntimeKind::Docker => "docker",
        RuntimeKind::TartVm => "tart_vm",
        RuntimeKind::NativeProcess => "native_process",
    }
}
fn os_name(value: ExecutionOs) -> &'static str {
    match value {
        ExecutionOs::Linux => "linux",
        ExecutionOs::Macos => "macos",
        ExecutionOs::Windows => "windows",
    }
}
fn arch_name(value: Architecture) -> &'static str {
    match value {
        Architecture::X64 => "x64",
        Architecture::Arm64 => "arm64",
    }
}
fn readiness_name(value: BackendReadiness) -> &'static str {
    match value {
        BackendReadiness::Ready => "ready",
        BackendReadiness::Reconciling => "reconciling",
        BackendReadiness::Unavailable => "unavailable",
        BackendReadiness::Unsupported => "unsupported",
        BackendReadiness::Unknown => "unknown",
    }
}
fn helper_state_name(value: HelperState) -> &'static str {
    match value {
        HelperState::Ready => "ready",
        HelperState::Unavailable => "unavailable",
        HelperState::Unknown => "unknown",
    }
}
fn coverage_name(value: SampleCoverage) -> &'static str {
    match value {
        SampleCoverage::Complete => "complete",
        SampleCoverage::Partial => "partial",
        SampleCoverage::Unknown => "unknown",
    }
}
