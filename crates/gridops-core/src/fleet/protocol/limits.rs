//! Validated deployment limits for fleet transports and queues.
//! Operators may lower limits within the protocol caps; inconsistent heartbeat
//! thresholds cannot become a running configuration.

use serde::{Deserialize, Serialize};

use super::primitives::ContractError;

/// Raw configuration is never consumed by a transport before validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    pub json_bytes: u64,
    pub event_batch_items: u64,
    pub event_batch_bytes: u64,
    pub chunk_bytes: u64,
    pub poll_ms: u64,
    pub pending_commands: u64,
    pub page_size: u64,
    pub command_attempts: u64,
    pub log_spool_bytes: u64,
    pub enrollment_ms: u64,
    pub autoapproval_ms: u64,
    pub preparation_ms: u64,
    pub start_ms: u64,
    pub authority_ms: u64,
    pub heartbeat_ms: u64,
    pub stale_ms: u64,
    pub offline_ms: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            json_bytes: 1_048_576,
            event_batch_items: 100,
            event_batch_bytes: 262_144,
            chunk_bytes: 262_144,
            poll_ms: 25_000,
            pending_commands: 100,
            page_size: 25,
            command_attempts: 10,
            log_spool_bytes: 268_435_456,
            enrollment_ms: 600_000,
            autoapproval_ms: 600_000,
            preparation_ms: 1_200_000,
            start_ms: 60_000,
            authority_ms: 90_000,
            heartbeat_ms: 15_000,
            stale_ms: 45_000,
            offline_ms: 90_000,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LimitsConfig", into = "LimitsConfig")]
pub struct ProtocolLimits(LimitsConfig);

impl ProtocolLimits {
    pub const fn values(&self) -> &LimitsConfig {
        &self.0
    }
}

impl TryFrom<LimitsConfig> for ProtocolLimits {
    type Error = ContractError;
    fn try_from(config: LimitsConfig) -> Result<Self, Self::Error> {
        let bounded = [
            (config.json_bytes, 1, 2_097_152),
            (config.event_batch_items, 1, 100),
            (config.event_batch_bytes, 1, 262_144),
            (config.chunk_bytes, 1, 262_144),
            (config.poll_ms, 1, 25_000),
            (config.pending_commands, 1, 1000),
            (config.page_size, 1, 100),
            (config.command_attempts, 1, 10),
            (config.log_spool_bytes, 1, 2_147_483_648),
            (config.enrollment_ms, 60_000, 3_600_000),
            (config.autoapproval_ms, 1, 3_600_000),
            (config.preparation_ms, 1, 3_600_000),
            (config.start_ms, 1, 60_000),
            (config.authority_ms, 1, 90_000),
            (config.heartbeat_ms, 1, 15_000),
            (config.stale_ms, 1, 45_000),
            (config.offline_ms, 1, 90_000),
        ];
        if bounded
            .iter()
            .any(|(value, min, max)| !(min..=max).contains(&value))
            || config.heartbeat_ms >= config.stale_ms
            || config.stale_ms >= config.offline_ms
            || config.heartbeat_ms >= config.authority_ms
            || config.start_ms > config.authority_ms
        {
            return Err(ContractError::IntegerRange);
        }
        Ok(Self(config))
    }
}

impl From<ProtocolLimits> for LimitsConfig {
    fn from(value: ProtocolLimits) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_threshold_order_are_validated() -> Result<(), Box<dyn std::error::Error>> {
        let defaults = serde_json::to_value(ProtocolLimits::default())?;
        assert_eq!(
            serde_json::from_value::<ProtocolLimits>(defaults.clone())?,
            ProtocolLimits::default()
        );
        let object = defaults.as_object().ok_or("limits are not an object")?;
        for field in object.keys() {
            for invalid in [0, u64::MAX] {
                let mut candidate = defaults.clone();
                candidate[field] = invalid.into();
                assert!(
                    serde_json::from_value::<ProtocolLimits>(candidate).is_err(),
                    "{field}"
                );
            }
        }
        for (field, invalid) in [
            ("stale_ms", 15_000),
            ("offline_ms", 45_000),
            ("authority_ms", 15_000),
        ] {
            let mut candidate = defaults.clone();
            candidate[field] = invalid.into();
            assert!(serde_json::from_value::<ProtocolLimits>(candidate).is_err());
        }
        assert!(serde_json::from_str::<ProtocolLimits>(r#"{"unlimited":true}"#).is_err());
        Ok(())
    }
}
