//! Exercises observation routes through the real API process and `SQLite`.
//! Agent capability reports never grant execution policy or command authority.

use super::*;
use gridops_core::fleet::protocol::inventory::{
    HandshakeResponse, HeartbeatResponse, InventoryResponse,
};

async fn enrolled(f: &Fixture) -> Result<(String, String)> {
    let issued = f.issue("inventory-enrollment").await?;
    let response = f.consume(issued["code"].as_str().context("code")?).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    Ok((
        body["host_id"].as_str().context("host ID")?.to_owned(),
        body["credential"]
            .as_str()
            .context("credential")?
            .to_owned(),
    ))
}

async fn control_plane(f: &Fixture) -> Result<String> {
    let incarnation = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO fleet_control_plane (singleton,incarnation,readiness,schema_version,protocol_min,protocol_max,updated_at) VALUES (1,?,'paused',1,1,1,?)")
        .bind(&incarnation).bind(gridops_core::now_millis()).execute(&f.database).await?;
    Ok(incarnation)
}

fn handshake_body() -> Value {
    json!({
        "request_id":Uuid::new_v4(),"agent_session_id":Uuid::new_v4(),
        "boot_id":"http-inventory-boot","supported_protocol":{"min":1,"max":1},
        "agent_version":"0.1.0","expected_host_epoch":1,
    })
}

fn agent_post(f: &Fixture, host: &str, secret: &str, action: &str) -> reqwest::RequestBuilder {
    f.client
        .post(f.url(&format!("/agent/hosts/{host}/{action}")))
        .bearer_auth(secret)
}

async fn receipt(response: Response) -> Result<Value> {
    let status = response.status();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers().contains_key("x-request-id"));
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(body)
}

fn inventory_body(handshake: &Value, incarnation: &str) -> Value {
    let backend = |key: &str, domain: &str, runtime: &str, os: &str| {
        json!({
            "local_key":key,"known_id":null,"domain_local_key":domain,
            "runtime_kind":runtime,"execution_os":os,"architecture":"x64",
            "readiness":"ready","reason":null,"runtime_identity":format!("runtime-{key}"),
            "capabilities":{
                "schema_version":1,"runtime_kind":runtime,"execution_os":os,"architecture":"x64",
                "supported_runners":[{"target_kind":"github_repository","mode":"ephemeral"}],
                "environments":[{"reference":"runner:http","readiness":"ready","version":null}],
                "resources":{"cpu":"reservation","memory":"reservation","disk":"estimate"},
                "interactive":"unsupported","docker_socket":"unsupported"
            }
        })
    };
    json!({
        "request_id":Uuid::new_v4(),
        "report":{
            "host_epoch":1,"control_plane_incarnation":incarnation,
            "authority_session_id":handshake["agent_session_id"],"boot_id":handshake["boot_id"],
            "inventory_sequence":1,"observed_at":"2026-10-09T10:00:00.000Z",
            "domains":[
                {"local_key":"root","known_id":null,"parent_local_key":null,"kind":"physical",
                 "name":"ThinkPad","runtime_identity":null,"capacity":{"cpu_millis":8000,"memory_mib":16384,"disk_bytes":null}},
                {"local_key":"linux-vm","known_id":null,"parent_local_key":"root","kind":"virtual_machine",
                 "name":"Linux VM","runtime_identity":"linux-vm-instance","capacity":{"cpu_millis":4000,"memory_mib":8192,"disk_bytes":null}}
            ],
            "backends":[backend("native","root","native_process","windows"),backend("docker","linux-vm","docker","linux")],
            "interactive_sessions":[],
            "samples":[{"domain_local_key":"root","observed_at":"2026-10-09T10:00:00.000Z","coverage":"partial",
                        "cpu_used_millis":null,"memory_used_mib":1024,"disk_free_bytes":null,"uptime_seconds":50}]
        }
    })
}

fn heartbeat_body(handshake: &Value, incarnation: &str) -> Value {
    json!({
        "request_id":Uuid::new_v4(),"host_epoch":1,"control_plane_incarnation":incarnation,
        "authority_session_id":handshake["agent_session_id"],"boot_id":handshake["boot_id"],
        "heartbeat_sequence":1,"observed_at":"2026-10-09T10:00:05.000Z","samples":[]
    })
}

#[tokio::test]
async fn windows_host_reports_linux_backend_with_stable_receipts_and_no_work_authority()
-> Result<()> {
    let f = Fixture::new().await?;
    let (host, secret) = enrolled(&f).await?;
    let incarnation = control_plane(&f).await?;
    let request = handshake_body();
    let handshake = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&request)
            .send()
            .await?,
    )
    .await?;
    let _: HandshakeResponse = serde_json::from_value(handshake.clone())?;
    assert_eq!(handshake["state"], "observing");
    assert_eq!(
        handshake,
        receipt(
            agent_post(&f, &host, &secret, "handshake")
                .json(&request)
                .send()
                .await?
        )
        .await?
    );

    let inventory = inventory_body(&handshake, &incarnation);
    let original = receipt(
        agent_post(&f, &host, &secret, "inventory")
            .json(&inventory)
            .send()
            .await?,
    )
    .await?;
    let typed: InventoryResponse = serde_json::from_value(original.clone())?;
    assert_eq!(typed.domain_ids.len(), 2);
    assert_eq!(typed.backend_ids.len(), 2);
    assert_eq!(
        original,
        receipt(
            agent_post(&f, &host, &secret, "inventory")
                .json(&inventory)
                .send()
                .await?
        )
        .await?
    );
    let state: (String, Option<i64>, i64, Option<i64>, i64) = sqlx::query_as("SELECT host_os,authority_expires_at,inventory_complete,last_inventory_at,inventory_revision FROM fleet_hosts WHERE id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(state.0, "windows");
    assert_eq!(state.1, None);
    assert_eq!(state.2, 1);
    assert_eq!(state.4, 1);
    let backends: Vec<(String, String, i64)> = sqlx::query_as("SELECT runtime_kind,execution_os,enabled FROM host_backends WHERE host_id=? ORDER BY runtime_kind")
        .bind(&host).fetch_all(&f.database).await?;
    assert_eq!(
        backends,
        vec![
            ("docker".into(), "linux".into(), 0),
            ("native_process".into(), "windows".into(), 0)
        ]
    );
    let root_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM host_resource_domains WHERE host_id=? AND parent_domain_id IS NULL",
    )
    .bind(&host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(root_count, 1);
    let budget: (i64, i64, i64) = sqlx::query_as("SELECT SUM(cpu_millis),SUM(memory_mib),SUM(disk_bytes) FROM host_resource_domains WHERE host_id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(budget, (0, 0, 0));

    let beat = heartbeat_body(&handshake, &incarnation);
    let heartbeat = receipt(
        agent_post(&f, &host, &secret, "heartbeat")
            .json(&beat)
            .send()
            .await?,
    )
    .await?;
    let _: HeartbeatResponse = serde_json::from_value(heartbeat.clone())?;
    assert_eq!(
        heartbeat,
        receipt(
            agent_post(&f, &host, &secret, "heartbeat")
                .json(&beat)
                .send()
                .await?
        )
        .await?
    );
    let unchanged: (Option<i64>, i64, Option<i64>) = sqlx::query_as("SELECT last_inventory_at,inventory_revision,authority_expires_at FROM fleet_hosts WHERE id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(unchanged, (state.3, 1, None));
    let samples: (i64, Option<i64>) =
        sqlx::query_as("SELECT COUNT(*),MAX(cpu_used_millis) FROM host_samples WHERE host_id=?")
            .bind(&host)
            .fetch_one(&f.database)
            .await?;
    assert_eq!(samples, (1, None));

    // Another endpoint advancing its lease cannot rewrite an earlier receipt.
    assert_eq!(
        original,
        receipt(
            agent_post(&f, &host, &secret, "inventory")
                .json(&inventory)
                .send()
                .await?
        )
        .await?
    );
    assert_eq!(
        handshake,
        receipt(
            agent_post(&f, &host, &secret, "handshake")
                .json(&request)
                .send()
                .await?
        )
        .await?
    );
    let mut next_inventory = inventory;
    next_inventory["request_id"] = json!(Uuid::new_v4());
    next_inventory["report"]["inventory_sequence"] = 2.into();
    receipt(
        agent_post(&f, &host, &secret, "inventory")
            .json(&next_inventory)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(
        heartbeat,
        receipt(
            agent_post(&f, &host, &secret, "heartbeat")
                .json(&beat)
                .send()
                .await?
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn acknowledged_rotation_continues_observations_without_resetting_identity() -> Result<()> {
    let f = Fixture::new().await?;
    let (host, old_secret) = enrolled(&f).await?;
    let incarnation = control_plane(&f).await?;
    let request = handshake_body();
    let handshake = receipt(
        agent_post(&f, &host, &old_secret, "handshake")
            .json(&request)
            .send()
            .await?,
    )
    .await?;
    let inventory = inventory_body(&handshake, &incarnation);
    let original = receipt(
        agent_post(&f, &host, &old_secret, "inventory")
            .json(&inventory)
            .send()
            .await?,
    )
    .await?;
    let before: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT initial_credential_id,lease_expires_at,current_inventory_sequence,current_heartbeat_sequence
         FROM fleet_observation_sessions WHERE host_id=? AND ended_at IS NULL")
        .bind(&host).fetch_one(&f.database).await?;

    let response = f
        .admin_post(&format!("/hosts/{host}/credentials/rotate"))
        .header("idempotency-key", "observing-rotation")
        .json(&json!({"expectedGeneration":1}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let accepted: Value = response.json().await?;
    let next_secret = URL_SAFE_NO_PAD.encode([29_u8; 32]);
    receipt(
        agent_post(&f, &host, &old_secret, "credentials/rotate")
            .json(&json!({"operation_id":accepted["operationId"],"next_secret":next_secret}))
            .send()
            .await?,
    )
    .await?;
    receipt(
        agent_post(&f, &host, &next_secret, "credentials/ack")
            .json(&json!({"operation_id":accepted["operationId"]}))
            .send()
            .await?,
    )
    .await?;

    // Rotation changes the accepted credential only; a replay cannot renew the
    // observation lease, reset sequence counters, or change its original receipt.
    let resumed = receipt(
        agent_post(&f, &host, &next_secret, "handshake")
            .json(&request)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(resumed, handshake);
    let after: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT initial_credential_id,lease_expires_at,current_inventory_sequence,current_heartbeat_sequence
         FROM fleet_observation_sessions WHERE host_id=? AND ended_at IS NULL")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(after, before);
    let generations: (i64, i64) = sqlx::query_as(
        "SELECT initial_credential_generation,current_credential_generation FROM fleet_observation_sessions WHERE host_id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(generations, (1, 2));
    assert_eq!(
        original,
        receipt(
            agent_post(&f, &host, &next_secret, "inventory")
                .json(&inventory)
                .send()
                .await?
        )
        .await?
    );
    let mut next_inventory = inventory.clone();
    next_inventory["request_id"] = json!(Uuid::new_v4());
    next_inventory["report"]["inventory_sequence"] = 2.into();
    receipt(
        agent_post(&f, &host, &next_secret, "inventory")
            .json(&next_inventory)
            .send()
            .await?,
    )
    .await?;
    receipt(
        agent_post(&f, &host, &next_secret, "heartbeat")
            .json(&heartbeat_body(&handshake, &incarnation))
            .send()
            .await?,
    )
    .await?;
    for (action, body) in [
        ("handshake", request),
        ("inventory", next_inventory),
        ("heartbeat", heartbeat_body(&handshake, &incarnation)),
    ] {
        error(
            agent_post(&f, &host, &old_secret, action)
                .json(&body)
                .send()
                .await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
    }
    let final_state: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT h.epoch,h.inventory_revision,h.authority_expires_at FROM fleet_hosts h WHERE id=?",
    )
    .bind(&host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(final_state, (1, 2, None));
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    for secret in [&old_secret, &next_secret] {
        assert!(!logs.contains(secret));
    }
    Ok(())
}

#[tokio::test]
async fn new_incarnation_reconnects_without_quarantining_obsolete_session() -> Result<()> {
    let f = Fixture::new().await?;
    let (host, secret) = enrolled(&f).await?;
    let incarnation = control_plane(&f).await?;
    let old_request = handshake_body();
    let old = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&old_request)
            .send()
            .await?,
    )
    .await?;
    let inventory = inventory_body(&old, &incarnation);
    receipt(
        agent_post(&f, &host, &secret, "inventory")
            .json(&inventory)
            .send()
            .await?,
    )
    .await?;
    let next_incarnation = Uuid::new_v4().to_string();
    sqlx::query("UPDATE fleet_control_plane SET incarnation=? WHERE singleton=1")
        .bind(&next_incarnation)
        .execute(&f.database)
        .await?;
    error(
        agent_post(&f, &host, &secret, "heartbeat")
            .json(&heartbeat_body(&old, &incarnation))
            .send()
            .await?,
        StatusCode::CONFLICT,
        "revision_conflict",
    )
    .await?;

    let next = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&handshake_body())
            .send()
            .await?,
    )
    .await?;
    assert_eq!(next["control_plane_incarnation"], next_incarnation);
    let state: (String, i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT integrity_state,epoch,inventory_complete,authority_expires_at FROM fleet_hosts WHERE id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(state, ("unverified".into(), 1, 0, None));
    let sessions: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COUNT(ended_at) FROM fleet_observation_sessions WHERE host_id=?",
    )
    .bind(&host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(sessions, (2, 1));
    error(
        agent_post(&f, &host, &secret, "inventory")
            .json(&inventory)
            .send()
            .await?,
        StatusCode::CONFLICT,
        "revision_conflict",
    )
    .await?;
    receipt(
        agent_post(&f, &host, &secret, "inventory")
            .json(&inventory_body(&next, &next_incarnation))
            .send()
            .await?,
    )
    .await?;
    let state: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT inventory_complete,inventory_revision,authority_expires_at FROM fleet_hosts WHERE id=?")
        .bind(&host).fetch_one(&f.database).await?;
    assert_eq!(state, (1, 2, None));
    Ok(())
}

#[tokio::test]
async fn observation_routes_enforce_agent_auth_and_strict_payloads_before_mutation() -> Result<()> {
    let f = Fixture::new().await?;
    let (host, secret) = enrolled(&f).await?;
    let body = handshake_body();
    for action in ["handshake", "inventory", "heartbeat"] {
        let path = format!("/agent/hosts/{host}/{action}");
        error(
            f.client.post(f.url(&path)).json(&body).send().await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
        error(
            f.admin_post(&path).json(&body).send().await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
        error(
            agent_post(&f, &Uuid::new_v4().to_string(), &secret, action)
                .json(&body)
                .send()
                .await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
        error(
            agent_post(&f, &host, &secret, action)
                .header(header::COOKIE, &f.cookie)
                .json(&body)
                .send()
                .await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
    }
    error(
        agent_post(&f, &host, &secret, "handshake")
            .json(&body)
            .send()
            .await?,
        StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
    )
    .await?;
    let incarnation = control_plane(&f).await?;
    let mut invalid = body.clone();
    invalid["work_authority"] = true.into();
    error(
        agent_post(&f, &host, &secret, "handshake")
            .json(&invalid)
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    invalid = body.clone();
    invalid["supported_protocol"] = json!({"min":2,"max":1});
    error(
        agent_post(&f, &host, &secret, "handshake")
            .json(&invalid)
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    invalid = body.clone();
    invalid["supported_protocol"] = json!({"min":2,"max":3});
    let incompatible = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&invalid)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(incompatible["negotiated_protocol"]["state"], "incompatible");
    assert!(incompatible["lease"].is_null());
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fleet_observation_sessions")
        .fetch_one(&f.database)
        .await?;
    assert_eq!(sessions, 0);
    let handshake = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&body)
            .send()
            .await?,
    )
    .await?;
    let mut report = inventory_body(&handshake, &incarnation);
    report["report"]["backends"][0]["enabled"] = true.into();
    error(
        agent_post(&f, &host, &secret, "inventory")
            .json(&report)
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    error(
        agent_post(&f, &host, &secret, "inventory")
            .header(header::CONTENT_TYPE, "application/json")
            .body(" ".repeat(1_048_577))
            .send()
            .await?,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
    )
    .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM host_backends WHERE host_id=?")
        .bind(&host)
        .fetch_one(&f.database)
        .await?;
    assert_eq!(count, 0);
    let incomplete: i64 =
        sqlx::query_scalar("SELECT inventory_complete FROM fleet_hosts WHERE id=?")
            .bind(&host)
            .fetch_one(&f.database)
            .await?;
    assert_eq!(incomplete, 0);
    Ok(())
}

#[tokio::test]
async fn approval_uses_current_receipt_after_sequence_jump_or_reconnect() -> Result<()> {
    for reconnect in [false, true] {
        let f = Fixture::new().await?;
        let (host, secret) = enrolled(&f).await?;
        let mut incarnation = control_plane(&f).await?;
        let mut handshake = receipt(
            agent_post(&f, &host, &secret, "handshake")
                .json(&handshake_body())
                .send()
                .await?,
        )
        .await?;
        let mut inventory = inventory_body(&handshake, &incarnation);
        inventory["report"]["inventory_sequence"] = 7.into();
        let mut observed = receipt(
            agent_post(&f, &host, &secret, "inventory")
                .json(&inventory)
                .send()
                .await?,
        )
        .await?;
        if reconnect {
            incarnation = Uuid::new_v4().to_string();
            sqlx::query("UPDATE fleet_control_plane SET incarnation=? WHERE singleton=1")
                .bind(&incarnation)
                .execute(&f.database)
                .await?;
            handshake = receipt(
                agent_post(&f, &host, &secret, "handshake")
                    .json(&handshake_body())
                    .send()
                    .await?,
            )
            .await?;
            observed = receipt(
                agent_post(&f, &host, &secret, "inventory")
                    .json(&inventory_body(&handshake, &incarnation))
                    .send()
                    .await?,
            )
            .await?;
        }
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
            .bind(&host)
            .fetch_one(&f.database)
            .await?;
        let body = json!({"expectedRevision":revision,
            "expectedInventoryRevision":observed["inventory_revision"],
            "expectedInventoryDigest":observed["digest"],"targetIds":[]});
        let path = format!("/hosts/{host}/approve");
        error(
            f.client
                .post(f.url(&path))
                .bearer_auth(&secret)
                .header(header::ORIGIN, &f.origin)
                .json(&body)
                .send()
                .await?,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        )
        .await?;
        error(
            f.client
                .post(f.url(&path))
                .header(header::COOKIE, &f.cookie)
                .header(header::ORIGIN, "https://untrusted.example")
                .json(&body)
                .send()
                .await?,
            StatusCode::FORBIDDEN,
            "forbidden",
        )
        .await?;
        let mut malformed = body.clone();
        malformed["allowNative"] = true.into();
        error(
            f.admin_post(&path).json(&malformed).send().await?,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await?;
        let mut stale = body.clone();
        stale["expectedInventoryDigest"] = URL_SAFE_NO_PAD.encode([91_u8; 32]).into();
        error(
            f.admin_post(&path).json(&stale).send().await?,
            StatusCode::CONFLICT,
            "revision_conflict",
        )
        .await?;

        sqlx::query("CREATE TRIGGER fail_approval_event BEFORE INSERT ON fleet_events WHEN NEW.kind='fleet.host.approved' BEGIN SELECT RAISE(ABORT,'private-approval-fault'); END")
            .execute(&f.database).await?;
        error(
            f.admin_post(&path).json(&body).send().await?,
            StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
        )
        .await?;
        let failed: (String, i64) =
            sqlx::query_as("SELECT enrollment_state,revision FROM fleet_hosts WHERE id=?")
                .bind(&host)
                .fetch_one(&f.database)
                .await?;
        assert_eq!(failed, ("pending".into(), revision));
        sqlx::query("DROP TRIGGER fail_approval_event")
            .execute(&f.database)
            .await?;
        let response = f.admin_post(&path).json(&body).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.bytes().await?.is_empty());
        let state: (String,String,Option<i64>,i64) = sqlx::query_as(
            "SELECT enrollment_state,lifecycle_state,authority_expires_at,revision FROM fleet_hosts WHERE id=?")
            .bind(&host).fetch_one(&f.database).await?;
        assert_eq!(
            state,
            ("approved".into(), "paused".into(), None, revision + 1)
        );
        let enabled: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM host_backends WHERE host_id=? AND enabled=1")
                .bind(&host)
                .fetch_one(&f.database)
                .await?;
        assert_eq!(enabled, 0);
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fleet_events WHERE kind='fleet.host.approved'",
        )
        .fetch_one(&f.database)
        .await?;
        assert_eq!(events, 1);
        error(
            f.admin_post(&path).json(&body).send().await?,
            StatusCode::CONFLICT,
            "revision_conflict",
        )
        .await?;
        let logs = fs::read_to_string(f.directory.join("api.log"))?;
        assert!(!logs.contains("private-approval-fault"));
    }
    Ok(())
}

#[tokio::test]
async fn observation_failure_rolls_back_children_and_returns_redacted_retryable_error() -> Result<()>
{
    let f = Fixture::new().await?;
    let (host, secret) = enrolled(&f).await?;
    let incarnation = control_plane(&f).await?;
    let handshake = receipt(
        agent_post(&f, &host, &secret, "handshake")
            .json(&handshake_body())
            .send()
            .await?,
    )
    .await?;
    sqlx::query("CREATE TRIGGER reject_inventory_snapshot BEFORE INSERT ON fleet_inventory_snapshots BEGIN SELECT RAISE(ABORT,'inventory-private-database-sentinel'); END")
        .execute(&f.database).await?;
    let body = inventory_body(&handshake, &incarnation);
    error(
        agent_post(&f, &host, &secret, "inventory")
            .json(&body)
            .send()
            .await?,
        StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
    )
    .await?;
    let rows: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM host_backends),
                (SELECT COUNT(*) FROM fleet_observed_domains),
                (SELECT COUNT(*) FROM fleet_inventory_snapshots),
                (SELECT COUNT(*) FROM host_samples)",
    )
    .fetch_one(&f.database)
    .await?;
    assert_eq!(rows, (0, 0, 0, 0));
    let domain_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM host_resource_domains WHERE host_id=?")
            .bind(&host)
            .fetch_one(&f.database)
            .await?;
    assert_eq!(domain_count, 1);
    let sequence: i64 = sqlx::query_scalar(
        "SELECT current_inventory_sequence FROM fleet_observation_sessions WHERE host_id=?",
    )
    .bind(&host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(sequence, 0);
    sqlx::query("DROP TRIGGER reject_inventory_snapshot")
        .execute(&f.database)
        .await?;
    receipt(
        agent_post(&f, &host, &secret, "inventory")
            .json(&body)
            .send()
            .await?,
    )
    .await?;
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    assert!(!logs.contains(&secret));
    assert!(!logs.contains("inventory-private-database-sentinel"));
    Ok(())
}
