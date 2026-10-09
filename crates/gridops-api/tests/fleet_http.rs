//! Runs the actual API binary against isolated `SQLite` files. These checks
//! exercise route/middleware order, one-time enrollment and redacted failures.

use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gridops_core::{
    connect_database_path, crypto::hash_token, fleet::protocol::browser::FleetErrorResponse,
};
use hmac::{Hmac, Mac as _};
use reqwest::{Client, Response, StatusCode, header};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::SqlitePool;
use uuid::Uuid;

const SESSION_SECRET: &str = "fleet-http-test-session-key";
const SESSION_TOKEN: &str = "fleet-http-test-session-token";

#[path = "fleet_http/inventory.rs"]
mod inventory;

struct Fixture {
    _process: ApiProcess,
    directory: PathBuf,
    database: SqlitePool,
    client: Client,
    origin: String,
    cookie: String,
}

struct ApiProcess(Child);

impl Drop for ApiProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Fixture {
    async fn new() -> Result<Self> {
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let directory = std::env::temp_dir().join(format!("gridops-fleet-http-{}", Uuid::new_v4()));
        fs::create_dir(&directory)?;
        let path = directory.join("db.sqlite");
        let log = fs::File::create(directory.join("api.log"))?;
        let origin = format!("http://127.0.0.1:{port}");
        let process = ApiProcess(
            Command::new(env!("CARGO_BIN_EXE_gridops-api"))
                .env("GRIDOPS_BASE_URL", &origin)
                .env("GRIDOPS_FLEET_ALLOW_LOOPBACK_HTTP", "true")
                .env("GRIDOPS_API_BIND", format!("127.0.0.1:{port}"))
                .env("GRIDOPS_DATABASE_PATH", &path)
                .env("GRIDOPS_SESSION_SECRET", SESSION_SECRET)
                .env(
                    "GRIDOPS_ENCRYPTION_KEY",
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                )
                .env("GRIDOPS_MANAGER_URL", "http://127.0.0.1:9")
                .env("GITHUB_CLIENT_ID", "fleet-http-client")
                .env("GITHUB_CLIENT_SECRET", "fleet-http-synthetic-secret")
                .env("GRIDOPS_GITHUB_WEBHOOK_ACTIVE", "false")
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .spawn()?,
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(SESSION_SECRET.as_bytes())?;
        mac.update(SESSION_TOKEN.as_bytes());
        let cookie = format!(
            "gridops_session={SESSION_TOKEN}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        let client = Client::new();
        for attempt in 0..100 {
            if client
                .get(format!("{origin}/api/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            if attempt == 99 {
                bail!(
                    "fleet API did not start; see {}",
                    directory.join("api.log").display()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let fixture = Self {
            _process: process,
            database: connect_database_path(&path).await?,
            directory,
            client,
            origin,
            cookie,
        };
        fixture.seed().await?;
        Ok(fixture)
    }

    async fn seed(&self) -> Result<()> {
        let now = gridops_core::now_millis();
        sqlx::query("INSERT INTO users (id,github_id,login,role,access_token,last_login_at,created_at,updated_at) VALUES ('fleet-user',1,'fleet-admin','admin','',?,?,?)")
            .bind(now).bind(now).bind(now).execute(&self.database).await?;
        sqlx::query("INSERT INTO sessions (id,token_hash,user_id,expires_at,last_seen_at,created_at) VALUES ('fleet-session',?,'fleet-user',?,?,?)")
            .bind(hash_token(SESSION_TOKEN)).bind(now+3_600_000).bind(now).bind(now).execute(&self.database).await?;
        Ok(())
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/fleet{path}", self.origin)
    }

    fn admin_post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(self.url(path))
            .header(header::COOKIE, &self.cookie)
            .header(header::ORIGIN, &self.origin)
    }

    async fn issue(&self, key: &str) -> Result<Value> {
        let response = self
            .admin_post("/enrollments")
            .header("idempotency-key", key)
            .json(&json!({"targetIds":[]}))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        Ok(response.json().await?)
    }

    async fn consume(&self, code: &str) -> Result<Response> {
        Ok(self
            .client
            .post(self.url("/agent/enroll"))
            .json(&json!({
                "code":code, "name":"ThinkPad", "host_os":"windows", "architecture":"x64",
                "hardware_fingerprint":"synthetic-hardware",
            }))
            .send()
            .await?)
    }
}

async fn error(response: Response, status: StatusCode, code: &str) -> Result<()> {
    assert_eq!(response.status(), status);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .with_context(|| format!("missing cache policy on {code} response"))?,
        "no-store"
    );
    let request_id = response
        .headers()
        .get("x-request-id")
        .context("missing request ID")?
        .to_str()?
        .to_owned();
    let body: FleetErrorResponse = response.json().await?;
    assert_eq!(serde_json::to_value(body.code)?, code);
    assert_eq!(body.request_id.to_string(), request_id);
    assert!(body.details.is_none());
    Ok(())
}

async fn other_admin_cookie(f: &Fixture) -> Result<String> {
    let now = gridops_core::now_millis();
    let token = "fleet-http-other-admin-token";
    sqlx::query("INSERT INTO users (id,github_id,login,role,access_token,last_login_at,created_at,updated_at) VALUES ('other-admin',2,'other-admin','admin','',?,?,?)")
        .bind(now).bind(now).bind(now).execute(&f.database).await?;
    sqlx::query("INSERT INTO sessions (id,token_hash,user_id,expires_at,last_seen_at,created_at) VALUES ('other-session',?,'other-admin',?,?,?)")
        .bind(hash_token(token)).bind(now+3_600_000).bind(now).bind(now).execute(&f.database).await?;
    let mut mac = Hmac::<Sha256>::new_from_slice(SESSION_SECRET.as_bytes())?;
    mac.update(token.as_bytes());
    Ok(format!(
        "gridops_session={token}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}

#[tokio::test]
async fn rotation_replays_are_actor_bound_and_remain_metadata_only_after_recovery() -> Result<()> {
    let f = Fixture::new().await?;
    let issued = f.issue("rotation-actor-enroll").await?;
    let enrolled: Value = f
        .consume(issued["code"].as_str().context("code")?)
        .await?
        .json()
        .await?;
    let host = enrolled["host_id"].as_str().context("host")?;
    let path = format!("/hosts/{host}/credentials/rotate");
    let created = f
        .admin_post(&path)
        .header("idempotency-key", "actor-bound-rotation")
        .json(&json!({"expectedGeneration":1}))
        .send()
        .await?;
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let original: Value = created.json().await?;
    let second_cookie = other_admin_cookie(&f).await?;
    error(
        f.client
            .post(f.url(&path))
            .header(header::COOKIE, second_cookie)
            .header(header::ORIGIN, &f.origin)
            .header("idempotency-key", "actor-bound-rotation")
            .json(&json!({"expectedGeneration":1}))
            .send()
            .await?,
        StatusCode::CONFLICT,
        "idempotency_conflict",
    )
    .await?;
    error(
        f.admin_post(&path)
            .header("idempotency-key", "actor-bound-rotation")
            .json(&json!({"expectedGeneration":2}))
            .send()
            .await?,
        StatusCode::CONFLICT,
        "idempotency_conflict",
    )
    .await?;

    let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
        .bind(host)
        .fetch_one(&f.database)
        .await?;
    let recovery = f
        .admin_post(&format!("/hosts/{host}/recover"))
        .header("idempotency-key", "actor-bound-recovery")
        .json(&json!({"expectedRevision":revision}))
        .send()
        .await?;
    assert_eq!(recovery.status(), StatusCode::CREATED);
    let recovery: Value = recovery.json().await?;
    let recovered = f
        .consume(recovery["code"].as_str().context("recovery code")?)
        .await?;
    assert_eq!(recovered.status(), StatusCode::CREATED);
    let _recovered: Value = recovered.json().await?;

    for retired in [false, true] {
        if retired {
            sqlx::query("UPDATE fleet_hosts SET lifecycle_state='retired',retired_at=? WHERE id=?")
                .bind(gridops_core::now_millis())
                .bind(host)
                .execute(&f.database)
                .await?;
        }
        let before: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM host_credential_rotations),(SELECT COUNT(*) FROM host_credentials),(SELECT COUNT(*) FROM audit_events),(SELECT COUNT(*) FROM fleet_events)")
            .fetch_one(&f.database).await?;
        let replay = f
            .admin_post(&path)
            .header("idempotency-key", "actor-bound-rotation")
            .json(&json!({"expectedGeneration":1}))
            .send()
            .await?;
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        assert_eq!(replay.headers()[header::CACHE_CONTROL], "no-store");
        let replay: Value = replay.json().await?;
        assert_eq!(replay, original);
        assert!(replay.get("credential").is_none());
        let after: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM host_credential_rotations),(SELECT COUNT(*) FROM host_credentials),(SELECT COUNT(*) FROM audit_events),(SELECT COUNT(*) FROM fleet_events)")
            .fetch_one(&f.database).await?;
        assert_eq!(after, before);
    }
    sqlx::query("UPDATE users SET role='member' WHERE id='fleet-user'")
        .execute(&f.database)
        .await?;
    error(
        f.admin_post(&path)
            .header("idempotency-key", "actor-bound-rotation")
            .json(&json!({"expectedGeneration":1}))
            .send()
            .await?,
        StatusCode::FORBIDDEN,
        "forbidden",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn enrollment_is_one_time_with_metadata_only_retries_and_secret_free_logs() -> Result<()> {
    let f = Fixture::new().await?;
    let issued = f.issue("issue-once").await?;
    let code = issued["code"].as_str().context("missing one-time code")?;
    let enrollment_id = issued["enrollmentId"]
        .as_str()
        .context("missing enrollment ID")?;
    let replay = f
        .admin_post("/enrollments")
        .header("idempotency-key", "issue-once")
        .json(&json!({"targetIds":[]}))
        .send()
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value = replay.json().await?;
    assert_eq!(replay["status"], "replay");
    assert!(replay.get("code").is_none());
    assert!(!replay.to_string().contains(code));

    let response = f.consume(code).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let enrolled: Value = response.json().await?;
    let credential = enrolled["credential"]
        .as_str()
        .context("missing host credential")?;
    let host_id = enrolled["host_id"].as_str().context("missing host ID")?;
    error(
        f.consume(code).await?,
        StatusCode::GONE,
        "enrollment_expired",
    )
    .await?;

    let response = f
        .client
        .get(f.url(&format!("/enrollments/{enrollment_id}")))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let status: Value = response.json().await?;
    assert_eq!(status["consumedHostId"], host_id);
    assert!(!status.to_string().contains(code));
    assert!(!status.to_string().contains(credential));
    let host: (String, String, String) = sqlx::query_as(
        "SELECT host_os,lifecycle_state,enrollment_state FROM fleet_hosts WHERE id=?",
    )
    .bind(host_id)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(
        host,
        (
            "windows".to_owned(),
            "paused".to_owned(),
            "pending".to_owned()
        )
    );
    for query in [
        "SELECT COUNT(*) FROM fleet_hosts",
        "SELECT COUNT(*) FROM host_credentials",
        "SELECT COUNT(*) FROM host_resource_domains",
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&f.database).await?;
        assert_eq!(count, 1);
    }
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    assert!(!logs.contains(code));
    assert!(!logs.contains(credential));
    assert!(!logs.contains(SESSION_TOKEN));
    Ok(())
}

#[tokio::test]
async fn revoked_expired_and_issuer_revoked_enrollments_create_no_host() -> Result<()> {
    let f = Fixture::new().await?;
    let revoked = f.issue("revoked-code").await?;
    let code = revoked["code"].as_str().context("missing code")?;
    let id = revoked["enrollmentId"].as_str().context("missing ID")?;
    for _ in 0..2 {
        let response = f
            .admin_post(&format!("/enrollments/{id}/revoke"))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    error(
        f.consume(code).await?,
        StatusCode::GONE,
        "enrollment_expired",
    )
    .await?;

    // Seed an already expired issuance; issued identity and expiry are immutable.
    let expired_code = URL_SAFE_NO_PAD.encode([57_u8; 32]);
    let now = gridops_core::now_millis();
    sqlx::query("INSERT INTO host_enrollments (id,code_verifier,scope_json,issued_by,created_at,expires_at) VALUES (?,?,'{\"target_ids\":[]}','fleet-user',?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(hash_token(&expired_code))
        .bind(now - 60_000)
        .bind(now - 1)
        .execute(&f.database)
        .await?;
    error(
        f.consume(&expired_code).await?,
        StatusCode::GONE,
        "enrollment_expired",
    )
    .await?;

    let denied = f.issue("issuer-revoked-code").await?;
    sqlx::query("UPDATE users SET role='member' WHERE id='fleet-user'")
        .execute(&f.database)
        .await?;
    error(
        f.consume(denied["code"].as_str().context("missing code")?)
            .await?,
        StatusCode::FORBIDDEN,
        "forbidden",
    )
    .await?;
    for query in [
        "SELECT COUNT(*) FROM fleet_hosts",
        "SELECT COUNT(*) FROM host_credentials",
        "SELECT COUNT(*) FROM host_resource_domains",
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&f.database).await?;
        assert_eq!(count, 0);
    }
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    assert!(!logs.contains(revoked["code"].as_str().context("missing code")?));
    assert!(!logs.contains(&expired_code));
    assert!(!logs.contains(denied["code"].as_str().context("missing code")?));
    Ok(())
}

#[tokio::test]
async fn fleet_cookie_writes_validate_origin_auth_key_shape_and_body_limits() -> Result<()> {
    let f = Fixture::new().await?;
    let body = json!({"targetIds":[]});
    error(
        f.client
            .post(f.url("/enrollments"))
            .header(header::ORIGIN, &f.origin)
            .header("idempotency-key", "anonymous")
            .json(&body)
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    for origin in [None, Some("https://unrelated.example"), Some("null")] {
        let mut request = f
            .client
            .post(f.url("/enrollments"))
            .header(header::COOKIE, &f.cookie)
            .header("idempotency-key", "bad-origin")
            .json(&body);
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        error(request.send().await?, StatusCode::FORBIDDEN, "forbidden").await?;
    }
    error(
        f.admin_post("/enrollments").json(&body).send().await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    error(
        f.admin_post("/enrollments")
            .header("idempotency-key", "a".repeat(129))
            .json(&body)
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    error(
        f.admin_post("/enrollments")
            .header("idempotency-key", "unknown-field")
            .json(&json!({"targetIds":[],"isAdmin":true}))
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    error(
        f.admin_post("/enrollments")
            .header("idempotency-key", "invalid-json")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-request-id", "untrusted-sentinel")
            .body("{")
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    error(
        f.admin_post("/enrollments")
            .header("idempotency-key", "too-large")
            .header(header::CONTENT_TYPE, "application/json")
            .body(" ".repeat(1_048_577))
            .send()
            .await?,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
    )
    .await?;
    error(
        f.client
            .get(f.url("/enrollments/not-a-uuid"))
            .header(header::COOKIE, &f.cookie)
            .send()
            .await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    for path in ["", "/", "/missing", "/agent/missing"] {
        error(
            f.client.get(f.url(path)).send().await?,
            StatusCode::NOT_FOUND,
            "not_found",
        )
        .await?;
    }
    for path in [
        "/api/v1/operations",
        "/api/v1/operations/",
        "/api/v1/operations/missing/extra",
    ] {
        error(
            f.client.get(format!("{}{path}", f.origin)).send().await?,
            StatusCode::NOT_FOUND,
            "not_found",
        )
        .await?;
    }
    error(
        f.client.delete(f.url("/enrollments")).send().await?,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await?;
    let issued = f.issue("permission-check").await?;
    let id = issued["enrollmentId"].as_str().context("missing ID")?;
    error(
        f.client
            .get(f.url(&format!("/enrollments/{id}")))
            .header(header::COOKIE, &f.cookie)
            .bearer_auth("A".repeat(43))
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    sqlx::query("UPDATE users SET role='member' WHERE id='fleet-user'")
        .execute(&f.database)
        .await?;
    error(
        f.client
            .get(f.url(&format!("/enrollments/{id}")))
            .header(header::COOKIE, &f.cookie)
            .send()
            .await?,
        StatusCode::FORBIDDEN,
        "forbidden",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn enrollment_database_failure_is_redacted_and_rolls_back_consumption() -> Result<()> {
    let f = Fixture::new().await?;
    let issued = f.issue("rollback").await?;
    let code = issued["code"].as_str().context("missing code")?;
    sqlx::query("CREATE TRIGGER fail_enrollment_host BEFORE INSERT ON fleet_hosts BEGIN SELECT RAISE(ABORT,'database-sentinel-do-not-return'); END")
        .execute(&f.database).await?;
    let response = f.consume(code).await?;
    error(
        response,
        StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
    )
    .await?;
    let consumed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM host_enrollments WHERE consumed_at IS NOT NULL")
            .fetch_one(&f.database)
            .await?;
    assert_eq!(consumed, 0);
    sqlx::query("DROP TRIGGER fail_enrollment_host")
        .execute(&f.database)
        .await?;
    assert_eq!(f.consume(code).await?.status(), StatusCode::CREATED);
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    assert!(!logs.contains(code));
    assert!(!logs.contains("database-sentinel-do-not-return"));
    Ok(())
}

#[tokio::test]
async fn recovery_issues_a_code_and_replaces_the_credential_only_when_consumed() -> Result<()> {
    let f = Fixture::new().await?;
    let issued = f.issue("recoverable-host").await?;
    let code = issued["code"].as_str().context("missing code")?;
    let enrolled: Value = f.consume(code).await?.json().await?;
    let host = enrolled["host_id"].as_str().context("missing host")?;
    let old_credential = enrolled["credential"]
        .as_str()
        .context("missing credential")?;
    let response = f
        .admin_post(&format!("/hosts/{host}/recover"))
        .header("idempotency-key", "recover-lost-response")
        .json(&json!({"expectedRevision":1}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let recovery: Value = response.json().await?;
    assert!(recovery.get("credential").is_none());
    assert_eq!(recovery["hostId"], host);
    let recovery_code = recovery["code"].as_str().context("missing recovery code")?;
    let before: (i64,i64) = sqlx::query_as("SELECT epoch,(SELECT COUNT(*) FROM host_credentials c WHERE c.host_id=h.id AND c.revoked_at IS NOT NULL) FROM fleet_hosts h WHERE h.id=?")
        .bind(host).fetch_one(&f.database).await?;
    assert_eq!(before, (1, 0));
    let replay = f
        .admin_post(&format!("/hosts/{host}/recover"))
        .header("idempotency-key", "recover-lost-response")
        .json(&json!({"expectedRevision":1}))
        .send()
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value = replay.json().await?;
    assert!(replay.get("code").is_none());
    assert!(!replay.to_string().contains(recovery_code));
    let response = f.consume(recovery_code).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let recovered: Value = response.json().await?;
    assert_eq!(recovered["host_id"], host);
    assert_eq!(recovered["credential_generation"], 2);
    let new_credential = recovered["credential"]
        .as_str()
        .context("missing replacement credential")?;
    let authority: (i64, String, i64) = sqlx::query_as(
        "SELECT epoch,lifecycle_state,inventory_complete FROM fleet_hosts WHERE id=?",
    )
    .bind(host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(authority, (2, "paused".to_owned(), 0));
    let unknown = Uuid::new_v4().to_string();
    let endpoint = f.url(&format!("/agent/hosts/{host}/credentials/ack"));
    error(
        f.client
            .post(&endpoint)
            .bearer_auth(old_credential)
            .json(&json!({"operation_id":unknown}))
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    error(
        f.client
            .post(&endpoint)
            .bearer_auth(new_credential)
            .json(&json!({"operation_id":unknown}))
            .send()
            .await?,
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await?;
    error(
        f.consume(recovery_code).await?,
        StatusCode::GONE,
        "enrollment_expired",
    )
    .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fleet_hosts")
        .fetch_one(&f.database)
        .await?;
    assert_eq!(count, 1);
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    for secret in [code, recovery_code, old_credential, new_credential] {
        assert!(!logs.contains(secret));
    }
    Ok(())
}

#[tokio::test]
async fn rotation_uses_separate_agent_auth_and_replay_does_not_extend_overlap() -> Result<()> {
    let f = Fixture::new().await?;
    let issued = f.issue("rotatable-host").await?;
    let code = issued["code"].as_str().context("missing code")?;
    let enrolled: Value = f.consume(code).await?.json().await?;
    let host = enrolled["host_id"].as_str().context("missing host")?;
    let old = enrolled["credential"]
        .as_str()
        .context("missing credential")?;
    let response = f
        .admin_post(&format!("/hosts/{host}/credentials/rotate"))
        .header("idempotency-key", "rotate-once")
        .json(&json!({"expectedGeneration":1}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let location = response
        .headers()
        .get(header::LOCATION)
        .context("missing operation location")?
        .to_str()?
        .to_owned();
    let accepted: Value = response.json().await?;
    let operation = accepted["operationId"]
        .as_str()
        .context("missing operation")?;
    assert_eq!(accepted["statusUrl"], location);
    let status_url = format!("{}{location}", f.origin);
    let response = f
        .client
        .get(&status_url)
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let status: Value = response.json().await?;
    assert_eq!(status["status"], "queued");

    let replay = f
        .admin_post(&format!("/hosts/{host}/credentials/rotate"))
        .header("idempotency-key", "rotate-once")
        .json(&json!({"expectedGeneration":1}))
        .send()
        .await?;
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(replay.json::<Value>().await?, accepted);

    let next = URL_SAFE_NO_PAD.encode([7_u8; 32]);
    let exchange = f.url(&format!("/agent/hosts/{host}/credentials/rotate"));
    let body = json!({"operation_id":operation,"next_secret":next});
    error(
        f.client
            .post(&exchange)
            .header(header::COOKIE, &f.cookie)
            .json(&body)
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    let wrong_host = f.url(&format!(
        "/agent/hosts/{}/credentials/rotate",
        Uuid::new_v4()
    ));
    error(
        f.client
            .post(wrong_host)
            .bearer_auth(old)
            .json(&body)
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    // A failed audit/event insert must roll back both the next verifier and
    // the old generation's overlap. A retry then performs exactly one exchange.
    let events_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fleet_events")
        .fetch_one(&f.database)
        .await?;
    sqlx::query("CREATE TRIGGER fail_rotation_event BEFORE INSERT ON fleet_events BEGIN SELECT RAISE(ABORT,'private-rotation-database-sentinel'); END")
        .execute(&f.database).await?;
    error(
        f.client
            .post(&exchange)
            .bearer_auth(old)
            .json(&body)
            .send()
            .await?,
        StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
    )
    .await?;
    let credentials: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COUNT(overlap_expires_at) FROM host_credentials WHERE host_id=?",
    )
    .bind(host)
    .fetch_one(&f.database)
    .await?;
    assert_eq!(credentials, (1, 0));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM host_credential_rotations WHERE id=?")
            .bind(operation)
            .fetch_one(&f.database)
            .await?;
    assert_eq!(phase, "requested");
    let events_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fleet_events")
        .fetch_one(&f.database)
        .await?;
    assert_eq!(events_after, events_before);
    sqlx::query("DROP TRIGGER fail_rotation_event")
        .execute(&f.database)
        .await?;
    let response = f
        .client
        .post(&exchange)
        .bearer_auth(old)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let exchanged: Value = response.json().await?;
    assert!(!exchanged.to_string().contains(&next));
    let response = f
        .client
        .post(&exchange)
        .bearer_auth(old)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let replay: Value = response.json().await?;
    assert_eq!(exchanged, replay);
    let changed = json!({"operation_id":operation,"next_secret":URL_SAFE_NO_PAD.encode([8_u8;32])});
    error(
        f.client
            .post(&exchange)
            .bearer_auth(old)
            .json(&changed)
            .send()
            .await?,
        StatusCode::CONFLICT,
        "idempotency_conflict",
    )
    .await?;
    let acknowledge = f.url(&format!("/agent/hosts/{host}/credentials/ack"));
    error(
        f.client
            .post(&acknowledge)
            .bearer_auth(old)
            .json(&json!({"operation_id":operation}))
            .send()
            .await?,
        StatusCode::CONFLICT,
        "revision_conflict",
    )
    .await?;
    let response = f
        .client
        .post(&acknowledge)
        .bearer_auth(&next)
        .json(&json!({"operation_id":operation}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let acknowledged: Value = response.json().await?;
    let replay = f
        .client
        .post(&acknowledge)
        .bearer_auth(&next)
        .json(&json!({"operation_id":operation}))
        .send()
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.json::<Value>().await?, acknowledged);
    error(
        f.client
            .post(&exchange)
            .bearer_auth(old)
            .json(&body)
            .send()
            .await?,
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
    )
    .await?;
    let status: Value = f
        .client
        .get(status_url)
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(status["status"], "succeeded");
    sqlx::query("UPDATE users SET role='member' WHERE id='fleet-user'")
        .execute(&f.database)
        .await?;
    error(
        f.client
            .get(format!("{}{location}", f.origin))
            .header(header::COOKIE, &f.cookie)
            .send()
            .await?,
        StatusCode::FORBIDDEN,
        "forbidden",
    )
    .await?;
    let logs = fs::read_to_string(f.directory.join("api.log"))?;
    for secret in [code, old, next.as_str()] {
        assert!(!logs.contains(secret));
    }
    Ok(())
}
