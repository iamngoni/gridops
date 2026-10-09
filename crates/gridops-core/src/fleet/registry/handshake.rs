//! Authenticated observation-session establishment.
//!
//! A handshake binds a private credential proof to durable host identity and a
//! server-owned observation lease.  It never writes work authority or host
//! readiness.  Conflicting live sessions are fenced and quarantined in the
//! same transaction before the conflict is returned.

use serde_json::json;
use sqlx::{Row as _, Sqlite, Transaction};

use super::{
    AuthenticatedHost, RegistryError, RegistryService, audit_and_event,
    ensure_authenticated_generation,
};
use crate::fleet::{
    ids::{ControlPlaneIncarnation, HostEpoch},
    protocol::{
        inventory::{
            HandshakeRequest, HandshakeResponse, ObservationLease, ObservationSessionState,
            ProtocolNegotiation,
        },
        primitives::{BrowserUint53, ProtocolVersion, Sha256Digest, WireTimestamp},
    },
};

const OBSERVATION_LEASE_MILLIS: i64 = 90_000;
const HEARTBEAT_INTERVAL_MILLIS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentInventorySession {
    pub host_id: crate::fleet::ids::HostId,
    pub host_epoch: HostEpoch,
    pub authority_session_id: crate::fleet::ids::AuthoritySessionId,
    pub boot_id: crate::fleet::ids::UpstreamText,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub state: crate::fleet::protocol::inventory::ObservationSessionState,
    pub lease_expires_at: i64,
    pub current_inventory_sequence: i64,
    pub current_inventory_digest: Option<String>,
    pub current_inventory_received_at: Option<i64>,
    pub current_heartbeat_sequence: i64,
    pub current_heartbeat_digest: Option<String>,
    pub current_heartbeat_received_at: Option<i64>,
    pub current_heartbeat_lease_expires_at: Option<i64>,
    pub current_heartbeat_state: Option<crate::fleet::protocol::inventory::ObservationSessionState>,
}

impl RegistryService {
    /// Establish or replay one durable observation session.
    pub async fn handshake(
        &self,
        authenticated: &AuthenticatedHost,
        request: HandshakeRequest,
    ) -> Result<HandshakeResponse, super::inventory::InventoryError> {
        let body_digest = handshake_digest(&request)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_authenticated_generation(&mut tx, authenticated, now).await?;
        let host_id = authenticated.host_id;
        let host = sqlx::query(
            "SELECT epoch,enrollment_state,lifecycle_state,agent_session_id,boot_id,authority_incarnation,authority_expires_at
               FROM fleet_hosts WHERE id=?",
        )
        .bind(host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RegistryError::NotFound)?;
        let epoch = epoch_from_row(&host, "epoch")?;
        if request
            .expected_host_epoch
            .is_some_and(|expected| expected != epoch)
        {
            return Err(super::inventory::InventoryError::IdentityConflict);
        }
        let control = sqlx::query(
            "SELECT incarnation,protocol_min,protocol_max FROM fleet_control_plane WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(super::inventory::InventoryError::DependencyUnavailable)?;
        let incarnation: ControlPlaneIncarnation = control
            .get::<String, _>("incarnation")
            .parse()
            .map_err(|_| RegistryError::InvalidRequest("control-plane incarnation is malformed"))?;
        let protocol_min: u16 = u16::try_from(control.get::<i64, _>("protocol_min"))
            .map_err(|_| RegistryError::InvalidRequest("protocol range is malformed"))?;
        let protocol_max: u16 = u16::try_from(control.get::<i64, _>("protocol_max"))
            .map_err(|_| RegistryError::InvalidRequest("protocol range is malformed"))?;
        let compatible =
            request.supported_protocol.includes(1) && (protocol_min..=protocol_max).contains(&1);
        if !compatible {
            let received_at = WireTimestamp::from_millis(now)
                .map_err(|_| RegistryError::InvalidRequest("server clock is outside wire range"))?;
            tx.commit().await?;
            return Ok(HandshakeResponse {
                request_id: request.request_id,
                host_id,
                host_epoch: epoch,
                control_plane_incarnation: incarnation,
                agent_session_id: request.agent_session_id,
                boot_id: request.boot_id,
                negotiated_protocol: ProtocolNegotiation::Incompatible,
                state: ObservationSessionState::Conflicted,
                lease: None,
                received_at,
            });
        }
        let existing = sqlx::query(
            "SELECT session_id,boot_id,host_epoch,control_plane_incarnation,initial_body_digest,
                    state,lease_expires_at,current_credential_id,current_credential_generation,
                    handshake_received_at,handshake_lease_expires_at,handshake_state
               FROM fleet_observation_sessions
              WHERE host_id=? AND ended_at IS NULL",
        )
        .bind(host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = existing {
            let existing_session: String = row.get("session_id");
            let existing_boot: String = row.get("boot_id");
            let existing_epoch: i64 = row.get("host_epoch");
            let existing_incarnation: String = row.get("control_plane_incarnation");
            let same_identity = existing_session == request.agent_session_id.to_string()
                && existing_boot == request.boot_id.as_str()
                && existing_epoch
                    == i64::try_from(epoch.get()).map_err(|_| {
                        RegistryError::InvalidRequest("epoch exceeds SQLite integer")
                    })?
                && existing_incarnation == incarnation.to_string();
            if same_identity {
                let existing_digest: String = row.get("initial_body_digest");
                if existing_digest != body_digest {
                    return Err(super::inventory::InventoryError::HandshakeConflict);
                }
                if row.get::<String, _>("current_credential_id")
                    != authenticated.credential_id.to_string()
                    || row.get::<i64, _>("current_credential_generation")
                        != i64::try_from(authenticated.generation.get()).map_err(|_| {
                            RegistryError::InvalidRequest("generation exceeds SQLite integer")
                        })?
                {
                    advance_rotated_session_credential(
                        &mut tx,
                        host_id,
                        &existing_session,
                        row.get("current_credential_id"),
                        row.get("current_credential_generation"),
                        authenticated,
                        now,
                    )
                    .await?;
                }
                let response = handshake_response_from_row(
                    &row,
                    request.request_id,
                    host_id,
                    epoch,
                    incarnation,
                    &request,
                )?;
                tx.commit().await?;
                return Ok(response);
            }
            let lease_expires_at: i64 = row.get("lease_expires_at");
            let work_authority_expires_at: Option<i64> = host.get("authority_expires_at");
            let same_authority_lineage = existing_epoch
                == i64::try_from(epoch.get())
                    .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?
                && existing_incarnation == incarnation.to_string();
            if same_authority_lineage
                && (lease_expires_at > now
                    || work_authority_expires_at.is_some_and(|expires| expires > now))
            {
                sqlx::query(
                    "UPDATE fleet_observation_sessions SET state='conflicted',ended_at=? WHERE session_id=?",
                )
                .bind(now)
                .bind(&existing_session)
                .execute(&mut *tx)
                .await?;
                clear_host_observation(&mut tx, host_id, now).await?;
                sqlx::query(
                    "UPDATE fleet_hosts SET integrity_state='quarantined',lifecycle_state='paused',updated_at=? WHERE id=?",
                )
                .bind(now)
                .bind(host_id.to_string())
                .execute(&mut *tx)
                .await?;
                audit_and_event(
                    &mut tx,
                    None,
                    Some(host_id),
                    "fleet.observation.session_conflict",
                    "host",
                    &host_id.to_string(),
                    json!({"session_id": existing_session}),
                    now,
                )
                .await?;
                tx.commit().await?;
                return Err(super::inventory::InventoryError::SessionConflict);
            }
            sqlx::query("UPDATE fleet_observation_sessions SET ended_at=? WHERE session_id=?")
                .bind(now)
                .bind(&existing_session)
                .execute(&mut *tx)
                .await?;
            clear_host_observation(&mut tx, host_id, now).await?;
        }
        let session_id = request.agent_session_id.to_string();
        let epoch_i64 = i64::try_from(epoch.get())
            .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?;
        let generation_i64 = i64::try_from(authenticated.generation.get())
            .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?;
        sqlx::query(
            "INSERT INTO fleet_observation_sessions
             (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,negotiated_protocol,
              initial_credential_id,initial_credential_generation,current_credential_id,current_credential_generation,
             initial_request_id,initial_body_digest,state,created_at,last_seen_at,lease_expires_at,
             handshake_received_at,handshake_lease_expires_at,handshake_state)
             VALUES (?,?,?,?,?,1,?,?,?,?,?,?,'observing',?,?,?, ?, ?, 'observing')",
        )
        .bind(&session_id)
        .bind(host_id.to_string())
        .bind(epoch_i64)
        .bind(request.boot_id.as_str())
        .bind(incarnation.to_string())
        .bind(authenticated.credential_id.to_string())
        .bind(generation_i64)
        .bind(authenticated.credential_id.to_string())
        .bind(generation_i64)
        .bind(request.request_id.to_string())
        .bind(&body_digest)
        .bind(now)
        .bind(now)
        .bind(now.checked_add(OBSERVATION_LEASE_MILLIS).ok_or(RegistryError::InvalidRequest("observation lease overflowed"))?)
        .bind(now)
        .bind(now.checked_add(OBSERVATION_LEASE_MILLIS).ok_or(RegistryError::InvalidRequest("observation lease overflowed"))?)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET agent_session_id=?,boot_id=?,agent_version=?,authority_incarnation=?,
                authority_expires_at=NULL,inventory_complete=0,last_inventory_at=NULL,updated_at=? WHERE id=?",
        )
        .bind(&session_id)
        .bind(request.boot_id.as_str())
        .bind(request.agent_version.as_str())
        .bind(incarnation.to_string())
        .bind(now)
        .bind(host_id.to_string())
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            None,
            Some(host_id),
            "fleet.observation.session_established",
            "host",
            &host_id.to_string(),
            json!({"session_id": session_id, "protocol": 1}),
            now,
        )
        .await?;
        tx.commit().await?;
        let received_at = WireTimestamp::from_millis(now)
            .map_err(|_| RegistryError::InvalidRequest("server clock is outside wire range"))?;
        Ok(HandshakeResponse {
            request_id: request.request_id,
            host_id,
            host_epoch: epoch,
            control_plane_incarnation: incarnation,
            agent_session_id: request.agent_session_id,
            boot_id: request.boot_id,
            negotiated_protocol: ProtocolNegotiation::Supported {
                version: ProtocolVersion::new(1)
                    .map_err(|_| RegistryError::InvalidRequest("protocol version invalid"))?,
            },
            state: ObservationSessionState::Observing,
            lease: Some(ObservationLease {
                expires_at: WireTimestamp::from_millis(now + OBSERVATION_LEASE_MILLIS)
                    .map_err(|_| RegistryError::InvalidRequest("lease timestamp invalid"))?,
                heartbeat_ms: BrowserUint53::new(HEARTBEAT_INTERVAL_MILLIS)
                    .map_err(|_| RegistryError::InvalidRequest("heartbeat interval invalid"))?,
            }),
            received_at,
        })
    }
}

fn handshake_digest(request: &HandshakeRequest) -> Result<String, RegistryError> {
    let bytes = serde_json::to_vec(request)?;
    Ok(String::from(Sha256Digest::of(&bytes)))
}

fn epoch_from_row(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<HostEpoch, RegistryError> {
    HostEpoch::new(
        u64::try_from(row.get::<i64, _>(column))
            .map_err(|_| RegistryError::InvalidRequest("host epoch is malformed"))?,
    )
    .map_err(|_| RegistryError::InvalidRequest("host epoch is malformed"))
}

fn handshake_response_from_row(
    row: &sqlx::sqlite::SqliteRow,
    request_id: crate::fleet::ids::OperationId,
    host_id: crate::fleet::ids::HostId,
    epoch: HostEpoch,
    incarnation: ControlPlaneIncarnation,
    request: &HandshakeRequest,
) -> Result<HandshakeResponse, RegistryError> {
    let expires_at: i64 = row
        .get::<Option<i64>, _>("handshake_lease_expires_at")
        .ok_or(RegistryError::InvalidRequest(
            "handshake receipt lease is incomplete",
        ))?;
    let received_at =
        WireTimestamp::from_millis(row.get::<Option<i64>, _>("handshake_received_at").ok_or(
            RegistryError::InvalidRequest("handshake receipt timestamp is incomplete"),
        )?)
        .map_err(|_| RegistryError::InvalidRequest("session receipt timestamp invalid"))?;
    Ok(HandshakeResponse {
        request_id,
        host_id,
        host_epoch: epoch,
        control_plane_incarnation: incarnation,
        agent_session_id: request.agent_session_id,
        boot_id: request.boot_id.clone(),
        negotiated_protocol: ProtocolNegotiation::Supported {
            version: ProtocolVersion::new(1)
                .map_err(|_| RegistryError::InvalidRequest("protocol version invalid"))?,
        },
        state: match row
            .get::<Option<String>, _>("handshake_state")
            .ok_or(RegistryError::InvalidRequest(
                "handshake receipt state is incomplete",
            ))?
            .as_str()
        {
            "observing" => ObservationSessionState::Observing,
            "ready" => ObservationSessionState::Ready,
            "conflicted" => ObservationSessionState::Conflicted,
            _ => ObservationSessionState::Reconciling,
        },
        lease: Some(ObservationLease {
            expires_at: WireTimestamp::from_millis(expires_at)
                .map_err(|_| RegistryError::InvalidRequest("lease timestamp invalid"))?,
            heartbeat_ms: BrowserUint53::new(HEARTBEAT_INTERVAL_MILLIS)
                .map_err(|_| RegistryError::InvalidRequest("heartbeat interval invalid"))?,
        }),
        received_at,
    })
}

/// Advance a session only after the durable registry has acknowledged the exact
/// predecessor-to-next-generation rotation for this host and epoch.
pub(crate) async fn advance_rotated_session_credential(
    tx: &mut Transaction<'_, Sqlite>,
    host_id: crate::fleet::ids::HostId,
    session_id: &str,
    current_credential_id: String,
    current_generation: i64,
    authenticated: &AuthenticatedHost,
    now: i64,
) -> Result<(), super::inventory::InventoryError> {
    let authenticated_generation = i64::try_from(authenticated.generation.get())
        .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?;
    let authenticated_epoch = i64::try_from(authenticated.epoch.get())
        .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?;
    let relation = sqlx::query(
        "SELECT old_credential_id,old_generation,old_host_epoch,next_generation
           FROM host_credential_rotations
          WHERE host_id=? AND old_credential_id=? AND old_generation=? AND old_host_epoch=?
            AND next_generation=? AND phase IN ('exchanged','acknowledged')
            AND revoked_at IS NULL",
    )
    .bind(host_id.to_string())
    .bind(&current_credential_id)
    .bind(current_generation)
    .bind(authenticated_epoch)
    .bind(authenticated_generation)
    .fetch_optional(&mut **tx)
    .await?;
    if relation.is_none()
        || authenticated.credential_id.to_string() == current_credential_id
        || current_generation >= authenticated_generation
    {
        return Err(super::inventory::InventoryError::CredentialConflict);
    }
    let changed = sqlx::query(
        "UPDATE fleet_observation_sessions
            SET current_credential_id=?,current_credential_generation=?
          WHERE host_id=? AND session_id=? AND current_credential_id=? AND current_credential_generation=?
            AND ended_at IS NULL",
    )
    .bind(authenticated.credential_id.to_string())
    .bind(authenticated_generation)
    .bind(host_id.to_string())
    .bind(session_id)
    .bind(&current_credential_id)
    .bind(current_generation)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(super::inventory::InventoryError::CredentialConflict);
    }
    audit_and_event(
        tx,
        None,
        Some(host_id),
        "fleet.observation.session_credential_advanced",
        "host",
        &host_id.to_string(),
        json!({"session_id": session_id, "generation": authenticated_generation}),
        now,
    )
    .await?;
    Ok(())
}

pub(crate) async fn clear_host_observation(
    tx: &mut Transaction<'_, Sqlite>,
    host_id: crate::fleet::ids::HostId,
    now: i64,
) -> Result<(), RegistryError> {
    sqlx::query(
        "UPDATE fleet_hosts SET authority_expires_at=NULL,inventory_complete=0,last_inventory_at=NULL,
         inventory_epoch=NULL,inventory_session_id=NULL,inventory_boot_id=NULL,inventory_digest=NULL,
         updated_at=? WHERE id=?",
    )
    .bind(now)
    .bind(host_id.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE host_backends SET readiness='unknown',reason_code='stale_inventory',last_probed_at=NULL,
         probe_epoch=NULL,probe_session_id=NULL,probe_boot_id=NULL,probe_incarnation=NULL,updated_at=? WHERE host_id=?",
    )
    .bind(now)
    .bind(host_id.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE host_sessions SET helper_state='unknown',last_seen_at=NULL WHERE host_id=?",
    )
    .bind(host_id.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE fleet_observed_interactive_sessions
            SET observed_helper_state='unknown'
          WHERE host_id=?",
    )
    .bind(host_id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
