//! Request-correlated authority renewal using a monotonic clock.
//! Network replies consume a pending request. Suspend invalidates the caller's
//! pending requests when its platform clock cannot reliably include sleep.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::primitives::{AuthorityDurationMs, PollDurationMs};
use crate::fleet::ids::{
    AuthoritySessionId, ControlPlaneIncarnation, HostEpoch, HostId, OperationId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityRenewal {
    pub request_id: OperationId,
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub authority_ttl_ms: AuthorityDurationMs,
}

/// Captured by the authenticated transport before it sends a request.
#[derive(Debug)]
pub struct PendingRenewal {
    request_id: OperationId,
    session: super::agent::ExpectedAgentSession,
    started: Instant,
    request_deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RenewalError {
    #[error("authority response does not match its originating request")]
    Mismatch,
    #[error("authority request or grant has expired")]
    Expired,
}

impl PendingRenewal {
    pub fn new(
        request_id: OperationId,
        session: super::agent::ExpectedAgentSession,
        started: Instant,
        timeout_ms: PollDurationMs,
    ) -> Result<Self, RenewalError> {
        let request_deadline = started
            .checked_add(Duration::from_millis(timeout_ms.get()))
            .ok_or(RenewalError::Expired)?;
        Ok(Self {
            request_id,
            session,
            started,
            request_deadline,
        })
    }

    /// The deadline is anchored to send time, never to receipt or wall time.
    pub fn accept(
        self,
        reply: &AuthorityRenewal,
        received: Instant,
    ) -> Result<Instant, RenewalError> {
        if self.request_id != reply.request_id
            || self.session.host_id != reply.host_id
            || self.session.host_epoch != reply.host_epoch
            || self.session.control_plane_incarnation != reply.control_plane_incarnation
            || self.session.authority_session_id != reply.authority_session_id
        {
            return Err(RenewalError::Mismatch);
        }
        let deadline = self
            .started
            .checked_add(Duration::from_millis(reply.authority_ttl_ms.get()))
            .ok_or(RenewalError::Expired)?;
        if received < self.started || received >= self.request_deadline || received >= deadline {
            return Err(RenewalError::Expired);
        }
        Ok(deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{ids::Generation, protocol::primitives::WireTimestamp};

    #[test]
    fn delayed_and_unrelated_responses_cannot_renew_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = super::super::agent::ExpectedAgentSession {
            host_id: HostId::try_new(uuid::Uuid::new_v4())?,
            host_epoch: HostEpoch::new(1)?,
            control_plane_incarnation: ControlPlaneIncarnation::try_new(uuid::Uuid::new_v4())?,
            authority_session_id: AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
            credential_generation: Generation::new(1)?,
            authority_expires_at: WireTimestamp::from_millis(0)?,
        };
        let reply = AuthorityRenewal {
            request_id: OperationId::try_new(uuid::Uuid::new_v4())?,
            host_id: session.host_id,
            host_epoch: session.host_epoch,
            control_plane_incarnation: session.control_plane_incarnation,
            authority_session_id: session.authority_session_id,
            authority_ttl_ms: AuthorityDurationMs::new(90_000)?,
        };
        let start = Instant::now();
        let timeout = PollDurationMs::new(25_000)?;
        let pending = || PendingRenewal::new(reply.request_id, session.clone(), start, timeout);
        assert_eq!(
            pending()?.accept(&reply, start + Duration::from_secs(10))?,
            start + Duration::from_secs(90)
        );
        for delay in [25, 30, 90] {
            assert_eq!(
                pending()?.accept(&reply, start + Duration::from_secs(delay)),
                Err(RenewalError::Expired)
            );
        }
        let mut wrong = reply.clone();
        wrong.request_id = OperationId::try_new(uuid::Uuid::new_v4())?;
        assert_eq!(
            pending()?.accept(&wrong, start),
            Err(RenewalError::Mismatch)
        );
        let mut expired = reply.clone();
        expired.authority_ttl_ms = AuthorityDurationMs::new(1000)?;
        assert_eq!(
            pending()?.accept(&expired, start + Duration::from_secs(1)),
            Err(RenewalError::Expired)
        );
        Ok(())
    }
}
