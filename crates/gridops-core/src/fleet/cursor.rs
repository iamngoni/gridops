//! Authenticated, scoped fleet cursors with bounded retention.
//! Signatures protect pagination state, not authorization. Every cursor read
//! must still recheck current grants and project only visible records.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    ids::{ControlPlaneIncarnation, NonNilUuid, UpstreamText},
    protocol::primitives::{ByteOffset, MAX_CURSOR_BYTES},
};
use crate::Vault;

const DOMAIN: &str = "gridops:fleet-cursor:v1\0";
pub const MAX_CURSOR_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStream {
    Hosts,
    Backends,
    Runners,
    Samples,
    FleetEvents,
    HostEvents,
    Commands,
    AgentEvents,
    Logs,
    Artifacts,
    ExternalRunners,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CursorPosition {
    Row { after: ByteOffset },
    Entity { after: NonNilUuid },
    LegacyEntity { after: UpstreamText },
    Bytes { offset: ByteOffset },
}

/// A digest built from the CURRENT authenticated principal and normalized
/// query, never reconstructed from the token's own claims. Agent principal
/// keys include host/epoch/authority session/credential generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorScope([u8; 32]);

impl CursorScope {
    /// Caller identity is loaded by the browser authentication middleware.
    pub fn for_browser(
        principal: &super::authorization::Principal,
        resource_key: &str,
        normalized_query: &str,
    ) -> Result<Self, CursorError> {
        let super::authorization::Principal::AuthenticatedUser { user_id } = principal else {
            return Err(CursorError::Invalid);
        };
        Self::new(&format!("user:{user_id}"), resource_key, normalized_query)
    }

    /// Every field which fences agent authority participates in cursor scope.
    pub fn for_agent(
        session: &super::protocol::agent::ExpectedAgentSession,
        resource_key: &str,
        normalized_query: &str,
    ) -> Result<Self, CursorError> {
        let key = format!(
            "agent:{}:{}:{}:{}:{}",
            session.host_id,
            session.host_epoch.get(),
            session.control_plane_incarnation,
            session.authority_session_id,
            session.credential_generation.get()
        );
        Self::new(&key, resource_key, normalized_query)
    }

    fn new(
        principal_key: &str,
        resource_key: &str,
        normalized_query: &str,
    ) -> Result<Self, CursorError> {
        if principal_key.is_empty()
            || resource_key.is_empty()
            || principal_key.len() > 1024
            || resource_key.len() > 1024
            || normalized_query.len() > 4096
        {
            return Err(CursorError::Invalid);
        }
        let bytes = serde_json::to_vec(&(principal_key, resource_key, normalized_query))
            .map_err(|_| CursorError::Invalid)?;
        Ok(Self(Sha256::digest(bytes).into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    version: u8,
    incarnation: ControlPlaneIncarnation,
    scope_digest: [u8; 32],
    stream: CursorStream,
    position: CursorPosition,
    issued_at_ms: i64,
    expires_at_ms: i64,
}

/// Retention bounds come from durable storage, never the cursor payload.
#[derive(Debug, Clone, Copy)]
pub enum RetainedFrom {
    /// The first retained event row. `after = first - 1` is still resumable.
    Row(ByteOffset),
    /// The first retained byte. This is inclusive, unlike an after-row cursor.
    Byte(ByteOffset),
    /// Stable entity-key pagination has expiry but no event replay window.
    Catalog,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CursorError {
    #[error("invalid fleet cursor")]
    Invalid,
    #[error("fleet cursor expired")]
    Expired,
    #[error("fleet cursor is outside retained history")]
    RetentionGap,
    #[error("cursor signing unavailable")]
    Signing,
}

pub struct CursorCodec<'a> {
    vault: &'a Vault,
}

impl<'a> CursorCodec<'a> {
    pub const fn new(vault: &'a Vault) -> Self {
        Self { vault }
    }

    /// `expires_at_ms` must already be capped to the backing store's retention.
    #[allow(clippy::too_many_arguments)] // Explicit scope/clock inputs prevent ambient authority.
    pub fn issue(
        &self,
        scope: &CursorScope,
        incarnation: ControlPlaneIncarnation,
        stream: CursorStream,
        position: CursorPosition,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<String, CursorError> {
        check_times(now_ms, expires_at_ms)?;
        check_position(stream, &position)?;
        let claims = Claims {
            version: 1,
            incarnation,
            scope_digest: scope.0,
            stream,
            position,
            issued_at_ms: now_ms,
            expires_at_ms,
        };
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| CursorError::Invalid)?);
        let signature = self
            .vault
            .sign(&format!("{DOMAIN}{payload}"))
            .map_err(|_| CursorError::Signing)?;
        let token = format!("v1.{payload}.{signature}");
        if token.len() > MAX_CURSOR_BYTES {
            return Err(CursorError::Invalid);
        }
        Ok(token)
    }

    #[allow(clippy::too_many_arguments)] // Current authority and retention must be supplied separately.
    pub fn verify(
        &self,
        token: &str,
        scope: &CursorScope,
        incarnation: ControlPlaneIncarnation,
        stream: CursorStream,
        now_ms: i64,
        retained_from: RetainedFrom,
    ) -> Result<CursorPosition, CursorError> {
        if token.len() > MAX_CURSOR_BYTES {
            return Err(CursorError::Invalid);
        }
        let mut parts = token.split('.');
        let version = parts.next().ok_or(CursorError::Invalid)?;
        let payload = parts.next().ok_or(CursorError::Invalid)?;
        let signature = parts.next().ok_or(CursorError::Invalid)?;
        if version != "v1" || parts.next().is_some() || signature.len() != 43 || payload.is_empty()
        {
            return Err(CursorError::Invalid);
        }
        // Authenticate exact encoded bytes before parsing any attacker-controlled JSON.
        if !self.vault.verify(&format!("{DOMAIN}{payload}"), signature) {
            return Err(CursorError::Invalid);
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| CursorError::Invalid)?;
        if URL_SAFE_NO_PAD.encode(&decoded) != payload {
            return Err(CursorError::Invalid);
        }
        let claims: Claims = serde_json::from_slice(&decoded).map_err(|_| CursorError::Invalid)?;
        check_times(claims.issued_at_ms, claims.expires_at_ms)?;
        check_position(claims.stream, &claims.position)?;
        if claims.version != 1
            || claims.incarnation != incarnation
            || claims.scope_digest != scope.0
            || claims.stream != stream
            || now_ms < claims.issued_at_ms
        {
            return Err(CursorError::Invalid);
        }
        if now_ms >= claims.expires_at_ms {
            return Err(CursorError::Expired);
        }
        let gap = match (&claims.position, retained_from) {
            (CursorPosition::Row { after }, RetainedFrom::Row(first)) => {
                after.get().checked_add(1).ok_or(CursorError::Invalid)? < first.get()
            }
            (CursorPosition::Bytes { offset }, RetainedFrom::Byte(first)) => {
                offset.get() < first.get()
            }
            (
                CursorPosition::Entity { .. } | CursorPosition::LegacyEntity { .. },
                RetainedFrom::Catalog,
            ) => false,
            _ => return Err(CursorError::Invalid),
        };
        if gap {
            return Err(CursorError::RetentionGap);
        }
        Ok(claims.position)
    }
}

fn check_position(stream: CursorStream, position: &CursorPosition) -> Result<(), CursorError> {
    let compatible = match stream {
        CursorStream::Logs | CursorStream::Artifacts => {
            matches!(position, CursorPosition::Bytes { .. })
        }
        CursorStream::Hosts | CursorStream::Backends | CursorStream::ExternalRunners => {
            matches!(position, CursorPosition::Entity { .. })
        }
        CursorStream::Runners => matches!(position, CursorPosition::LegacyEntity { .. }),
        CursorStream::Samples
        | CursorStream::FleetEvents
        | CursorStream::HostEvents
        | CursorStream::Commands
        | CursorStream::AgentEvents => matches!(position, CursorPosition::Row { .. }),
    };
    if compatible {
        Ok(())
    } else {
        Err(CursorError::Invalid)
    }
}

fn check_times(issued: i64, expires: i64) -> Result<(), CursorError> {
    let ttl = expires.checked_sub(issued).ok_or(CursorError::Invalid)?;
    if issued < 0 || ttl <= 0 || ttl > MAX_CURSOR_LIFETIME_MS {
        return Err(CursorError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_and_retention_are_checked_after_signature() -> Result<(), Box<dyn std::error::Error>> {
        let vault = Vault::test_with_secret(b"cursor-fixture-key");
        let codec = CursorCodec::new(&vault);
        let incarnation = "10000000-0000-4000-8000-000000000001".parse()?;
        let scope = CursorScope::new("user:fixture", "host:fixture", "time:ascending")?;
        let position = CursorPosition::Row {
            after: ByteOffset::new(9)?,
        };
        let token = codec.issue(
            &scope,
            incarnation,
            CursorStream::FleetEvents,
            position.clone(),
            100,
            1000,
        )?;
        let verify = |token: &str, scope: &CursorScope, now, floor| {
            codec.verify(
                token,
                scope,
                incarnation,
                CursorStream::FleetEvents,
                now,
                RetainedFrom::Row(ByteOffset::new(floor).map_err(|_| CursorError::Invalid)?),
            )
        };
        assert_eq!(verify(&token, &scope, 101, 10)?, position);
        assert_eq!(
            verify(&token, &scope, 101, 11),
            Err(CursorError::RetentionGap)
        );
        assert_eq!(verify(&token, &scope, 1000, 1), Err(CursorError::Expired));
        assert_eq!(verify(&token, &scope, 99, 1), Err(CursorError::Invalid));
        for other in [
            CursorScope::new("user:other", "host:fixture", "time:ascending")?,
            CursorScope::new("user:fixture", "host:other", "time:ascending")?,
            CursorScope::new("user:fixture", "host:fixture", "time:descending")?,
        ] {
            assert_eq!(verify(&token, &other, 101, 1), Err(CursorError::Invalid));
        }
        assert_eq!(
            codec.verify(
                &token,
                &scope,
                incarnation,
                CursorStream::Logs,
                101,
                RetainedFrom::Row(ByteOffset::new(1)?)
            ),
            Err(CursorError::Invalid)
        );
        let different_incarnation = "20000000-0000-4000-8000-000000000001".parse()?;
        assert_eq!(
            codec.verify(
                &token,
                &scope,
                different_incarnation,
                CursorStream::FleetEvents,
                101,
                RetainedFrom::Row(ByteOffset::new(1)?)
            ),
            Err(CursorError::Invalid)
        );
        let rotated = Vault::test_with_secret(b"rotated-fixture-key");
        assert_eq!(
            CursorCodec::new(&rotated).verify(
                &token,
                &scope,
                incarnation,
                CursorStream::FleetEvents,
                101,
                RetainedFrom::Row(ByteOffset::new(1)?)
            ),
            Err(CursorError::Invalid)
        );
        for bad in [
            String::new(),
            format!("{token}.extra"),
            token.replacen("v1.", "v2.", 1),
            "x".repeat(MAX_CURSOR_BYTES + 1),
        ] {
            assert_eq!(verify(&bad, &scope, 101, 1), Err(CursorError::Invalid));
        }
        let mut tampered = token.clone().into_bytes();
        tampered[5] = if tampered[5] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            verify(&String::from_utf8(tampered)?, &scope, 101, 1),
            Err(CursorError::Invalid)
        );
        assert!(
            codec
                .issue(
                    &scope,
                    incarnation,
                    CursorStream::FleetEvents,
                    position,
                    100,
                    100 + MAX_CURSOR_LIFETIME_MS + 1
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn byte_retention_uses_inclusive_offset_and_agent_scope_binds_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let vault = Vault::test_with_secret(b"cursor-fixture-key");
        let codec = CursorCodec::new(&vault);
        let incarnation = "10000000-0000-4000-8000-000000000001".parse()?;
        let scope = CursorScope::new(
            "host:fixture:epoch1:session1:credential1",
            "stream:fixture",
            "offset",
        )?;
        let token = codec.issue(
            &scope,
            incarnation,
            CursorStream::Logs,
            CursorPosition::Bytes {
                offset: ByteOffset::new(4096)?,
            },
            1,
            2,
        )?;
        assert!(
            codec
                .verify(
                    &token,
                    &scope,
                    incarnation,
                    CursorStream::Logs,
                    1,
                    RetainedFrom::Byte(ByteOffset::new(4096)?)
                )
                .is_ok()
        );
        assert_eq!(
            codec.verify(
                &token,
                &scope,
                incarnation,
                CursorStream::Logs,
                1,
                RetainedFrom::Byte(ByteOffset::new(4097)?)
            ),
            Err(CursorError::RetentionGap)
        );
        let changed = CursorScope::new(
            "host:fixture:epoch1:session2:credential1",
            "stream:fixture",
            "offset",
        )?;
        assert_eq!(
            codec.verify(
                &token,
                &changed,
                incarnation,
                CursorStream::Logs,
                1,
                RetainedFrom::Byte(ByteOffset::new(0)?)
            ),
            Err(CursorError::Invalid)
        );
        Ok(())
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::fleet::{
        authorization::Principal,
        ids::{Generation, HostEpoch},
        protocol::{agent::ExpectedAgentSession, primitives::WireTimestamp},
    };

    #[test]
    fn typed_agent_scope_binds_every_authority_axis_and_browser_principal()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = ExpectedAgentSession {
            host_id: "10000000-0000-4000-8000-000000000001".parse()?,
            host_epoch: HostEpoch::new(1)?,
            control_plane_incarnation: "20000000-0000-4000-8000-000000000001".parse()?,
            authority_session_id: "30000000-0000-4000-8000-000000000001".parse()?,
            credential_generation: Generation::new(1)?,
            authority_expires_at: WireTimestamp::from_millis(90_000)?,
        };
        let expected = CursorScope::for_agent(&current, "host:events", "after:row")?;
        for field in ["host", "epoch", "incarnation", "session", "credential"] {
            let mut altered = current.clone();
            match field {
                "host" => altered.host_id = "90000000-0000-4000-8000-000000000001".parse()?,
                "epoch" => altered.host_epoch = HostEpoch::new(2)?,
                "incarnation" => {
                    altered.control_plane_incarnation =
                        "90000000-0000-4000-8000-000000000001".parse()?;
                }
                "session" => {
                    altered.authority_session_id =
                        "90000000-0000-4000-8000-000000000001".parse()?;
                }
                "credential" => altered.credential_generation = Generation::new(2)?,
                _ => return Err("unexpected test axis".into()),
            }
            assert_ne!(
                expected,
                CursorScope::for_agent(&altered, "host:events", "after:row")?,
                "{field}"
            );
        }
        assert!(CursorScope::for_browser(&Principal::Reconciler, "fleet", "id").is_err());
        let browser = CursorScope::for_browser(
            &Principal::AuthenticatedUser {
                user_id: "admin".into(),
            },
            "fleet",
            "id",
        )?;
        assert_ne!(browser, expected);
        assert!(
            CursorScope::for_browser(
                &Principal::AuthenticatedUser {
                    user_id: "admin".into()
                },
                "",
                "id"
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn catalog_cursors_preserve_legacy_ids_and_reject_incompatible_positions()
    -> Result<(), Box<dyn std::error::Error>> {
        let vault = Vault::test_with_secret(b"cursor-fixture-key");
        let codec = CursorCodec::new(&vault);
        let scope = CursorScope::new("user:fixture", "pool:fixture", "id")?;
        let incarnation = "10000000-0000-4000-8000-000000000001".parse()?;
        let legacy = CursorPosition::LegacyEntity {
            after: UpstreamText::new("legacy-runner")?,
        };
        let token = codec.issue(
            &scope,
            incarnation,
            CursorStream::Runners,
            legacy.clone(),
            0,
            100,
        )?;
        assert_eq!(
            codec.verify(
                &token,
                &scope,
                incarnation,
                CursorStream::Runners,
                1,
                RetainedFrom::Catalog
            )?,
            legacy
        );
        assert!(
            codec
                .issue(&scope, incarnation, CursorStream::Logs, legacy, 0, 100)
                .is_err()
        );
        let entity = CursorPosition::Entity {
            after: NonNilUuid::try_new(uuid::Uuid::new_v4())?,
        };
        let token = codec.issue(
            &scope,
            incarnation,
            CursorStream::Hosts,
            entity.clone(),
            0,
            100,
        )?;
        assert_eq!(
            codec.verify(
                &token,
                &scope,
                incarnation,
                CursorStream::Hosts,
                1,
                RetainedFrom::Catalog
            )?,
            entity
        );
        assert!(
            codec
                .issue(&scope, incarnation, CursorStream::Runners, entity, 0, 100)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn even_signed_malformed_claims_and_noncanonical_payloads_are_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let vault = Vault::test_with_secret(b"cursor-fixture-key");
        let codec = CursorCodec::new(&vault);
        let scope = CursorScope::new("user:fixture", "fleet", "row")?;
        let incarnation = "10000000-0000-4000-8000-000000000001".parse()?;
        let claims = Claims {
            version: 1,
            incarnation,
            scope_digest: scope.0,
            stream: CursorStream::FleetEvents,
            position: CursorPosition::Row {
                after: ByteOffset::new(0)?,
            },
            issued_at_ms: 0,
            expires_at_ms: 100,
        };
        let mut unknown = serde_json::to_value(&claims)?;
        unknown["unsafe"] = true.into();
        for payload in [
            URL_SAFE_NO_PAD.encode(b"not JSON"),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&unknown)?),
            format!("{}=", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?)),
        ] {
            let signature = vault.sign(&format!("{DOMAIN}{payload}"))?;
            let token = format!("v1.{payload}.{signature}");
            assert_eq!(
                codec.verify(
                    &token,
                    &scope,
                    incarnation,
                    CursorStream::FleetEvents,
                    1,
                    RetainedFrom::Row(ByteOffset::new(0)?)
                ),
                Err(CursorError::Invalid)
            );
        }
        Ok(())
    }
}
