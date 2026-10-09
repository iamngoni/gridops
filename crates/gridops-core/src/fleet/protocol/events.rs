//! Bounded typed agent event batches.
//!
//! A batch binds every event to one host epoch and authority session.  The
//! parser checks the serialized byte limit before deserialization; semantic
//! validation then checks item count, identity homogeneity, and typed event
//! ownership.  A valid batch is still only evidence until server persistence.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    agent::{CommandScope, LocalResourceIdentity},
    outcomes::{AgentCommandResult, DelayedStartExclusionClaim, LocalAbsenceClaim},
    primitives::{MAX_BATCH_BYTES, MAX_EVENT_ITEMS, Sha256Digest, WireTimestamp, parse_json},
};
use crate::fleet::ids::{
    AuthoritySessionId, BatchId, CommandId, ControlPlaneIncarnation, EventId, HostEpoch, HostId,
    ResultId,
};

const MAX_ACK_IDS: usize = MAX_EVENT_ITEMS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEventKind {
    Heartbeat,
    CommandAccepted {
        command_id: CommandId,
        request_hash: Sha256Digest,
    },
    CommandResult {
        result: Box<AgentCommandResult>,
    },
    LocalAbsenceReported {
        claim: LocalAbsenceClaim,
    },
    DelayedStartsExcluded {
        claim: DelayedStartExclusionClaim,
    },
    ResourceObserved {
        resource: LocalResourceIdentity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvent {
    pub event_id: EventId,
    pub observed_at: WireTimestamp,
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub scope: Option<CommandScope>,
    pub kind: AgentEventKind,
}

/// Untrusted transport data.  Callers must construct [`AgentEventBatch`] via
/// `TryFrom` or [`AgentEventBatch::parse`] before treating the batch as checked
/// for size, identity homogeneity, and nested result scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventBatchData {
    pub batch_id: BatchId,
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AgentEventBatchData", into = "AgentEventBatchData")]
pub struct AgentEventBatch(AgentEventBatchData);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EventBatchError {
    #[error("event batch contains too many events")]
    TooManyEvents,
    #[error("event IDs in a batch must be unique")]
    DuplicateEvent,
    #[error("event is bound to a different host or authority session")]
    MixedAuthority,
    #[error("event result is bound to a different command identity")]
    ResultMismatch,
    #[error("command result scope is missing from its event")]
    MissingScope,
    #[error("event scope does not match its typed evidence")]
    ScopeMismatch,
    #[error("event batch exceeds the serialized byte limit")]
    PayloadTooLarge,
    #[error("event batch JSON is invalid")]
    InvalidJson,
}

impl TryFrom<AgentEventBatchData> for AgentEventBatch {
    type Error = EventBatchError;

    fn try_from(data: AgentEventBatchData) -> Result<Self, Self::Error> {
        if data.events.is_empty() || data.events.len() > MAX_EVENT_ITEMS {
            return Err(EventBatchError::TooManyEvents);
        }
        let encoded = serde_json::to_vec(&data).map_err(|_| EventBatchError::InvalidJson)?;
        if encoded.len() > MAX_BATCH_BYTES {
            return Err(EventBatchError::PayloadTooLarge);
        }
        let mut ids = HashSet::with_capacity(data.events.len());
        for event in &data.events {
            if !ids.insert(event.event_id) {
                return Err(EventBatchError::DuplicateEvent);
            }
            if event.host_id != data.host_id
                || event.host_epoch != data.host_epoch
                || event.control_plane_incarnation != data.control_plane_incarnation
                || event.authority_session_id != data.authority_session_id
            {
                return Err(EventBatchError::MixedAuthority);
            }
            match &event.kind {
                AgentEventKind::CommandResult { result } => {
                    if result.host_id != data.host_id
                        || result.host_epoch != data.host_epoch
                        || result.control_plane_incarnation != data.control_plane_incarnation
                        || result.authority_session_id != data.authority_session_id
                    {
                        return Err(EventBatchError::ResultMismatch);
                    }
                    let Some(scope) = &event.scope else {
                        return Err(EventBatchError::MissingScope);
                    };
                    if scope != &result.scope {
                        return Err(EventBatchError::ScopeMismatch);
                    }
                }
                AgentEventKind::CommandAccepted { .. } => {}
                AgentEventKind::LocalAbsenceReported { .. }
                | AgentEventKind::DelayedStartsExcluded { .. }
                | AgentEventKind::ResourceObserved { .. } => {
                    if !matches!(event.scope.as_ref(), Some(CommandScope::Placement { .. })) {
                        return Err(EventBatchError::ScopeMismatch);
                    }
                }
                AgentEventKind::Heartbeat => {
                    if matches!(event.scope.as_ref(), Some(CommandScope::Placement { .. })) {
                        return Err(EventBatchError::ScopeMismatch);
                    }
                }
            }
        }
        Ok(Self(data))
    }
}

impl AgentEventBatch {
    pub fn parse(bytes: &[u8]) -> Result<Self, EventBatchError> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(EventBatchError::PayloadTooLarge);
        }
        let data: AgentEventBatchData =
            parse_json(bytes, MAX_BATCH_BYTES).map_err(|error| match error {
                super::primitives::ContractError::PayloadTooLarge => {
                    EventBatchError::PayloadTooLarge
                }
                _ => EventBatchError::InvalidJson,
            })?;
        Self::try_from(data)
    }

    #[must_use]
    /// Returns the checked batch data.  The same data type can be built
    /// directly by callers, so retain the wrapper when crossing trust
    /// boundaries instead of treating an arbitrary `AgentEventBatchData` as
    /// validated.
    pub const fn data(&self) -> &AgentEventBatchData {
        &self.0
    }
}

impl From<AgentEventBatch> for AgentEventBatchData {
    fn from(value: AgentEventBatch) -> Self {
        value.0
    }
}

/// Server responses identify only durable event IDs.  They do not claim that
/// an event was persisted merely because an agent parsed or transmitted it.
/// Untrusted acknowledgement data.  Construct [`AgentEventAck`] through
/// `TryFrom` before using its ID partition as a checked server response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventAckData {
    pub batch_id: BatchId,
    pub accepted: Vec<EventId>,
    pub replayed: Vec<EventId>,
    pub result_ids: Vec<ResultId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AgentEventAckData", into = "AgentEventAckData")]
pub struct AgentEventAck(AgentEventAckData);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AckError {
    #[error("acknowledged IDs exceed the batch limit")]
    TooManyIds,
    #[error("acknowledged IDs must be unique")]
    Duplicate,
    #[error("accepted and replayed IDs must not overlap")]
    Overlap,
}

impl TryFrom<AgentEventAckData> for AgentEventAck {
    type Error = AckError;

    fn try_from(data: AgentEventAckData) -> Result<Self, Self::Error> {
        if data.accepted.len().saturating_add(data.replayed.len()) > MAX_ACK_IDS
            || data.result_ids.len() > MAX_ACK_IDS
        {
            return Err(AckError::TooManyIds);
        }
        let accepted: HashSet<_> = data.accepted.iter().copied().collect();
        let replayed: HashSet<_> = data.replayed.iter().copied().collect();
        if accepted.len() != data.accepted.len() || replayed.len() != data.replayed.len() {
            return Err(AckError::Duplicate);
        }
        if accepted.iter().any(|id| replayed.contains(id)) {
            return Err(AckError::Overlap);
        }
        let mut result_ids = HashSet::new();
        for id in &data.result_ids {
            if !result_ids.insert(*id) {
                return Err(AckError::Duplicate);
            }
        }
        Ok(Self(data))
    }
}

impl From<AgentEventAck> for AgentEventAckData {
    fn from(value: AgentEventAck) -> Self {
        value.0
    }
}

impl AgentEventAck {
    /// Returns the checked acknowledgement data without granting persistence
    /// or authorization semantics to any acknowledged ID.
    #[must_use]
    pub const fn data(&self) -> &AgentEventAckData {
        &self.0
    }
}
