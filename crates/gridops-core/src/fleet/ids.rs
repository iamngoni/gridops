//! Strong identifiers and bounded counters for fleet records.
//!
//! These types deliberately do not wrap legacy pool, user, or connection text
//! identifiers. Nil UUIDs and zero/out-of-range lifecycle counters are rejected
//! at deserialization so database and protocol layers receive valid values.

use std::{fmt, num::NonZeroU64, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// The largest counter accepted by fleet records while remaining safe for
/// signed `SQLite` INTEGER columns and JavaScript-safe API conversion policies.
pub const MAX_COUNTER: u64 = i64::MAX as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("fleet UUID must not be nil")]
pub struct NilFleetUuid;

#[derive(Debug, Error)]
pub enum FleetIdParseError {
    #[error("fleet identifier is not a UUID: {0}")]
    Malformed(#[from] uuid::Error),
    #[error(transparent)]
    Nil(#[from] NilFleetUuid),
}

macro_rules! fleet_uuid {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);

        impl $name {
            /// Construct an identifier after rejecting the nil UUID sentinel.
            pub fn try_new(value: Uuid) -> Result<Self, NilFleetUuid> {
                if value.is_nil() {
                    Err(NilFleetUuid)
                } else {
                    Ok(Self(value))
                }
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl TryFrom<Uuid> for $name {
            type Error = NilFleetUuid;

            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl FromStr for $name {
            type Err = FleetIdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self::try_new(Uuid::parse_str(value)?)?)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

fleet_uuid!(HostId);
fleet_uuid!(BackendId);
fleet_uuid!(WorkloadId);
fleet_uuid!(PlacementId);
fleet_uuid!(CommandId);
fleet_uuid!(AllocationId);
fleet_uuid!(ResourceDomainId);
fleet_uuid!(ProfileId);
fleet_uuid!(OperationId);
fleet_uuid!(CiTargetId);
fleet_uuid!(SessionId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{kind} must be between 1 and {MAX_COUNTER}, got {value}")]
pub struct CounterError {
    kind: CounterKind,
    value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterKind {
    HostEpoch,
    Generation,
    Revision,
}

impl fmt::Display for CounterKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::HostEpoch => "host epoch",
            Self::Generation => "generation",
            Self::Revision => "revision",
        };
        formatter.write_str(name)
    }
}

macro_rules! bounded_counter {
    ($name:ident, $kind:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(NonZeroU64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, CounterError> {
                Self::try_from(value)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }

        impl TryFrom<u64> for $name {
            type Error = CounterError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                if value == 0 || value > MAX_COUNTER {
                    return Err(CounterError {
                        kind: CounterKind::$kind,
                        value,
                    });
                }
                // SAFETY: zero is rejected immediately above.
                let value = NonZeroU64::new(value).ok_or(CounterError {
                    kind: CounterKind::$kind,
                    value,
                })?;
                Ok(Self(value))
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.get()
            }
        }
    };
}

bounded_counter!(HostEpoch, HostEpoch);
bounded_counter!(Generation, Generation);
bounded_counter!(Revision, Revision);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetTextError {
    #[error("{kind} must not be empty")]
    Empty { kind: &'static str },
    #[error("{kind} exceeds {max} characters")]
    TooLong { kind: &'static str, max: usize },
    #[error("{kind} contains control characters")]
    Control { kind: &'static str },
}

/// A bounded, non-empty text identity used only where an upstream API defines
/// a text key. Legacy IDs remain text and are never coerced into UUIDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UpstreamText(String);

impl UpstreamText {
    pub fn new(value: impl Into<String>) -> Result<Self, FleetTextError> {
        let value = value.into();
        if value.is_empty() {
            return Err(FleetTextError::Empty {
                kind: "upstream text",
            });
        }
        if value.chars().count() > 128 {
            return Err(FleetTextError::TooLong {
                kind: "upstream text",
                max: 128,
            });
        }
        if value.chars().any(char::is_control) {
            return Err(FleetTextError::Control {
                kind: "upstream text",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UpstreamText {
    type Error = FleetTextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UpstreamText> for String {
    fn from(value: UpstreamText) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonNilUuid(Uuid);

impl NonNilUuid {
    pub fn try_new(value: Uuid) -> Result<Self, NilFleetUuid> {
        if value.is_nil() {
            Err(NilFleetUuid)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Serialize for NonNilUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NonNilUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(Uuid::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn fleet_ids_reject_nil_and_malformed_values() {
        assert!(HostId::try_new(Uuid::nil()).is_err());
        assert!("not-a-uuid".parse::<HostId>().is_err());
        assert!(
            serde_json::from_str::<HostId>("\"00000000-0000-0000-0000-000000000000\"").is_err()
        );
    }

    #[test]
    fn counters_are_nonzero_and_i64_safe() {
        assert!(HostEpoch::new(0).is_err());
        assert!(Revision::new(MAX_COUNTER).is_ok());
        assert!(Generation::new(MAX_COUNTER + 1).is_err());
        assert_eq!(u64::from(HostEpoch::new(7).expect("valid test counter")), 7);
    }

    #[test]
    fn upstream_text_rejects_unsafe_values() {
        assert!(UpstreamText::new("").is_err());
        assert!(UpstreamText::new("workspace\nname").is_err());
        assert!(serde_json::from_str::<UpstreamText>("\"\"").is_err());
    }
}
