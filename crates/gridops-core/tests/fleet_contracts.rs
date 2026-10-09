//! Shared fixtures establish Rust/browser parsing parity, including unknown
//! fields and dependent-state rejection. They do not prove gateway permissions.

use gridops_core::fleet::protocol::browser::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    schema: String,
    valid: bool,
    body: Value,
}

fn accepts<T: DeserializeOwned + Serialize>(body: Value) -> anyhow::Result<bool> {
    let Ok(value) = serde_json::from_value::<T>(body) else {
        return Ok(false);
    };
    let encoded = serde_json::to_vec(&value)?;
    serde_json::from_slice::<T>(&encoded)?;
    Ok(true)
}

#[test]
fn rust_and_browser_use_identical_examples_and_rejections() -> anyhow::Result<()> {
    let fixtures: Vec<Fixture> =
        serde_json::from_str(include_str!("../../../tests/fixtures/fleet-contracts.json"))?;
    for fixture in fixtures {
        let valid = match fixture.schema.as_str() {
            "host" => accepts::<HostSummary>(fixture.body)?,
            "backend" => accepts::<BackendSummary>(fixture.body)?,
            "resource" => accepts::<ResourceAmountDto>(fixture.body)?,
            "accepted" => accepts::<OperationAccepted>(fixture.body)?,
            "status" => accepts::<OperationStatus>(fixture.body)?,
            "outcome" => accepts::<PlacementOutcome>(fixture.body)?,
            "action" => accepts::<HostActionRequest>(fixture.body)?,
            "submit" => accepts::<SubmitWorkloadRequest>(fixture.body)?,
            "event" => accepts::<FleetEventDto>(fixture.body)?,
            "error" => accepts::<FleetErrorResponse>(fixture.body)?,
            "page" => accepts::<Page<HostSummary>>(fixture.body)?,
            "log" => accepts::<LogChunkDto>(fixture.body)?,
            "logMetadata" => accepts::<LogMetadata>(fixture.body)?,
            other => anyhow::bail!("fixture has unknown schema {other}"),
        };
        assert_eq!(valid, fixture.valid, "{}", fixture.name);
    }
    Ok(())
}

#[test]
fn browser_log_bytes_and_resource_integer_conversion_are_checked() -> anyhow::Result<()> {
    use gridops_core::fleet::{ResourceAmount, protocol::primitives::LogText};
    let full = LogText::try_from("😀".repeat(65536))?;
    assert_eq!(full.as_str().len(), 262_144);
    assert!(!format!("{full:?}").contains('😀'));
    assert!(LogText::try_from("😀".repeat(65537)).is_err());
    let amount = ResourceAmount::new(1, 1, 9_007_199_254_740_992)?;
    assert!(ResourceAmountDto::try_from(amount).is_err());
    Ok(())
}

#[test]
fn server_factories_and_error_mapping_preserve_browser_contracts() -> anyhow::Result<()> {
    let id = "00000001-0000-4000-8000-000000000001".parse()?;
    let accepted = OperationAccepted::new(id);
    assert_eq!(accepted.operation_id(), id);
    assert_eq!(
        serde_json::to_value(&accepted)?["statusUrl"],
        "/api/v1/operations/00000001-0000-4000-8000-000000000001"
    );
    let queued = PlacementOutcome::try_from(PlacementOutcomeData::Queued {
        operation_id: id,
        workload_id: "00000002-0000-4000-8000-000000000001".parse()?,
        placement_id: (),
        explanation: PlacementExplanation {
            selected: None,
            rejections: BoundedItems::new(Vec::new())?,
        },
    })?;
    assert!(matches!(queued.data(), PlacementOutcomeData::Queued { .. }));
    let bounded = BoundedItems::<u8, 100>::new(vec![0; 100])?;
    assert_eq!(bounded.as_slice().len(), 100);
    assert!(BoundedItems::<u8, 100>::new(vec![0; 101]).is_err());
    for (code, status) in [
        (ErrorCode::InvalidRequest, 400),
        (ErrorCode::ContractRange, 400),
        (ErrorCode::Unauthenticated, 401),
        (ErrorCode::Forbidden, 403),
        (ErrorCode::RevisionConflict, 409),
        (ErrorCode::IdempotencyConflict, 409),
        (ErrorCode::EnrollmentExpired, 410),
        (ErrorCode::GrantExpired, 410),
        (ErrorCode::CursorExpired, 410),
        (ErrorCode::CursorGap, 410),
        (ErrorCode::PayloadTooLarge, 413),
        (ErrorCode::RateLimited, 429),
        (ErrorCode::DependencyUnavailable, 503),
        (ErrorCode::NotReady, 503),
    ] {
        assert_eq!(code.status(), status);
    }
    Ok(())
}
