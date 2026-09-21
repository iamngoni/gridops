use std::{
    env, fs,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gridops_core::{
    compatible_runner_provider, connect_database_path, crypto::hash_token,
    models::CreateRunnerPool, runner_system_labels, scale_up_target,
};
use hmac::{Hmac, Mac as _};
use reqwest::{Client, StatusCode, header};
use serde_json::Value;
use sha2::Sha256;
use sqlx::{Row as _, SqlitePool};
use tokio::time::sleep;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const SESSION_SECRET: &str = "integration-session-secret";
const WEBHOOK_SECRET: &str = "integration-webhook-secret";
const ENCRYPTION_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

struct ApiFixture {
    child: Child,
    client: Client,
    base_url: String,
    database_path: PathBuf,
}

impl ApiFixture {
    async fn start() -> Result<Self> {
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let database_path =
            env::temp_dir().join(format!("gridops-api-test-{}.sqlite", Uuid::new_v4()));
        let base_url = format!("http://127.0.0.1:{port}");
        let binary = env!("CARGO_BIN_EXE_gridops-api");
        let child = Command::new(binary)
            .env("GRIDOPS_BASE_URL", &base_url)
            .env("GRIDOPS_API_BIND", format!("127.0.0.1:{port}"))
            .env("GRIDOPS_DATABASE_PATH", &database_path)
            .env("GRIDOPS_SESSION_SECRET", SESSION_SECRET)
            .env("GRIDOPS_ENCRYPTION_KEY", ENCRYPTION_KEY)
            .env("GITHUB_CLIENT_ID", "integration-client")
            .env("GITHUB_CLIENT_SECRET", "integration-secret")
            .env("GITHUB_WEBHOOK_SECRET", WEBHOOK_SECRET)
            .env("GRIDOPS_WEBHOOK_ACTIVE", "false")
            .env("GRIDOPS_MANAGER_URL", "http://127.0.0.1:9")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start the API fixture")?;
        let client = Client::builder().build()?;
        let fixture = Self {
            child,
            client,
            base_url,
            database_path,
        };
        fixture.wait_until_healthy().await?;
        fixture.seed_identity("member").await?;
        Ok(fixture)
    }

    async fn wait_until_healthy(&self) -> Result<()> {
        for _ in 0..60 {
            if let Ok(response) = self
                .client
                .get(format!("{}/api/health", self.base_url))
                .send()
                .await
                && response.status().is_success()
            {
                return Ok(());
            }
            sleep(Duration::from_millis(50)).await;
        }
        Err(anyhow!("API fixture did not become healthy"))
    }

    async fn database(&self) -> Result<SqlitePool> {
        connect_database_path(&self.database_path).await
    }

    async fn seed_identity(&self, role: &str) -> Result<()> {
        let database = self.database().await?;
        let now = gridops_core::now_millis();
        sqlx::query("INSERT INTO users (id,github_id,login,role,access_token,last_login_at,created_at,updated_at) VALUES ('user-1',1,'fixture',?,?, ?,?,?)")
            .bind(role)
            .bind("")
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&database)
            .await?;
        sqlx::query("INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at) VALUES (42,42,'fixture','Organization','Organization','selected',?,?)")
            .bind(now)
            .bind(now)
            .execute(&database)
            .await?;
        sqlx::query("INSERT INTO user_installations (user_id,installation_id,permission,created_at) VALUES ('user-1',42,'admin',?)")
            .bind(now)
            .execute(&database)
            .await?;
        let token = "fixture-session-token";
        sqlx::query("INSERT INTO sessions (id,token_hash,user_id,expires_at,last_seen_at,created_at) VALUES ('session-1',?,?, ?,?,?)")
            .bind(hash_token(token))
            .bind("user-1")
            .bind(now + 3_600_000)
            .bind(now)
            .bind(now)
            .execute(&database)
            .await?;
        Ok(())
    }

    fn session_cookie() -> Result<String> {
        let token = "fixture-session-token";
        let mut mac = HmacSha256::new_from_slice(SESSION_SECRET.as_bytes())?;
        mac.update(token.as_bytes());
        Ok(format!(
            "gridops_session={token}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        ))
    }

    fn webhook_signature(body: &str) -> Result<String> {
        let mut mac = HmacSha256::new_from_slice(WEBHOOK_SECRET.as_bytes())?;
        mac.update(body.as_bytes());
        Ok(format!(
            "sha256={}",
            hex::encode(mac.finalize().into_bytes())
        ))
    }

    async fn post_webhook(
        &self,
        delivery: &str,
        body: &str,
        signature: &str,
    ) -> Result<reqwest::Response> {
        Ok(self
            .client
            .post(format!("{}/api/v1/webhooks/github", self.base_url))
            .header("x-github-delivery", delivery)
            .header("x-github-event", "ping")
            .header("x-hub-signature-256", signature)
            .body(body.to_owned())
            .send()
            .await?)
    }
}

impl Drop for ApiFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.database_path);
        let _ = fs::remove_file(self.database_path.with_extension("sqlite-shm"));
        let _ = fs::remove_file(self.database_path.with_extension("sqlite-wal"));
    }
}

#[tokio::test]
async fn api_routes_enforce_authentication_and_admin_boundaries() -> Result<()> {
    let fixture = ApiFixture::start().await?;

    let unauthenticated = fixture
        .client
        .get(format!("{}/api/v1/backups/database", fixture.base_url))
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let member_settings = fixture
        .client
        .get(format!("{}/api/v1/backups/database", fixture.base_url))
        .header(header::COOKIE, ApiFixture::session_cookie()?)
        .send()
        .await?;
    assert_eq!(member_settings.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn webhook_signature_idempotency_and_rejection_are_persistent() -> Result<()> {
    let fixture = ApiFixture::start().await?;
    let body = r#"{"zen":"fixture"}"#;
    let signature = ApiFixture::webhook_signature(body)?;

    let accepted = fixture.post_webhook("delivery-1", body, &signature).await?;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert_eq!(accepted.json::<Value>().await?["accepted"], true);

    let duplicate = fixture.post_webhook("delivery-1", body, &signature).await?;
    assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
    assert_eq!(duplicate.json::<Value>().await?["duplicate"], true);

    let rejected = fixture
        .post_webhook("delivery-2", body, "sha256=invalid")
        .await?;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let database = fixture.database().await?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM webhook_deliveries WHERE id='delivery-2'",
    )
    .fetch_one(&database)
    .await?;
    assert_eq!(status, "rejected");
    let deliveries = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM webhook_deliveries WHERE id='delivery-1'",
    )
    .fetch_one(&database)
    .await?;
    assert_eq!(deliveries, 1);
    Ok(())
}

#[tokio::test]
async fn webhook_retry_requires_admin_and_reprocesses_verified_delivery() -> Result<()> {
    let fixture = ApiFixture::start().await?;
    let database = fixture.database().await?;
    let now = gridops_core::now_millis();
    sqlx::query("INSERT INTO webhook_deliveries (id,event,signature_valid,status,payload,received_at) VALUES ('retry-1','ping',1,'failed',?,?)")
        .bind(r#"{"zen":"retry"}"#)
        .bind(now)
        .execute(&database)
        .await?;

    let denied = fixture
        .client
        .post(format!(
            "{}/api/v1/webhooks/retry-1/retry",
            fixture.base_url
        ))
        .header(header::COOKIE, ApiFixture::session_cookie()?)
        .header(header::ORIGIN, &fixture.base_url)
        .send()
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    sqlx::query("UPDATE users SET role='admin' WHERE id='user-1'")
        .execute(&database)
        .await?;
    let retried = fixture
        .client
        .post(format!(
            "{}/api/v1/webhooks/retry-1/retry",
            fixture.base_url
        ))
        .header(header::COOKIE, ApiFixture::session_cookie()?)
        .header(header::ORIGIN, &fixture.base_url)
        .send()
        .await?;
    assert_eq!(retried.status(), StatusCode::OK);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM webhook_deliveries WHERE id='retry-1'")
            .fetch_one(&database)
            .await?,
        "processed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_events WHERE action='webhook.retried'"
        )
        .fetch_one(&database)
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn backup_route_is_authenticated_and_migrations_preserve_contract_columns() -> Result<()> {
    let fixture = ApiFixture::start().await?;
    let unauthenticated = fixture
        .client
        .get(format!("{}/api/v1/backups/database", fixture.base_url))
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let database = fixture.database().await?;
    let columns = sqlx::query("SELECT name FROM pragma_table_info('runner_pools') WHERE name IN ('provider','providers','configuration_version','provision_circuit_open') ORDER BY name")
        .fetch_all(&database)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert_eq!(
        columns,
        [
            "configuration_version",
            "provider",
            "providers",
            "provision_circuit_open"
        ]
    );

    sqlx::query("UPDATE users SET role='admin' WHERE id='user-1'")
        .execute(&database)
        .await?;
    let backup = fixture
        .client
        .get(format!("{}/api/v1/backups/database", fixture.base_url))
        .header(header::COOKIE, ApiFixture::session_cookie()?)
        .send()
        .await?;
    assert_eq!(backup.status(), StatusCode::OK);
    assert_eq!(
        backup.bytes().await?.get(..16),
        Some(&b"SQLite format 3\0"[..])
    );
    Ok(())
}

#[test]
fn shared_provisioning_contract_keeps_provider_labels_and_scale_targets_consistent() -> Result<()> {
    assert_eq!(scale_up_target(2, 1, 5, 1, 10), 6);
    assert_eq!(scale_up_target(2, 1, 5, 2, 10), 10);
    let docker_labels = runner_system_labels("docker")
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let tart_labels = runner_system_labels("tart")
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        compatible_runner_provider(
            &["docker".into(), "tart".into()],
            &docker_labels,
            &["gridops".into()]
        ),
        Some("docker".into())
    );
    assert_eq!(
        compatible_runner_provider(
            &["docker".into(), "tart".into()],
            &tart_labels,
            &["gridops".into()]
        ),
        Some("tart".into())
    );
    assert_eq!(
        compatible_runner_provider(
            &["docker".into()],
            &["runs-on:macos".into()],
            &["gridops".into()]
        ),
        None
    );

    let input = CreateRunnerPool {
        installation_id: 42,
        repository_id: None,
        repository_ids: vec![7],
        bitbucket_connection_ids: vec![],
        name: "fixture-pool".into(),
        scope: "repository".into(),
        mode: "ephemeral".into(),
        provider: "docker".into(),
        providers: vec!["docker".into(), "tart".into()],
        labels: vec!["gridops".into()],
        image: "fixture-image".into(),
        docker_image: "fixture-image".into(),
        tart_image: "fixture-tart".into(),
        macos_runtime: "vm".into(),
        desired_count: 2,
        min_count: 0,
        max_count: 4,
        autoscaling_enabled: true,
        queue_scale_factor: 1,
        idle_timeout_minutes: 5,
        cpu_limit: 2.0,
        memory_limit_mb: 2_048,
        runner_group_id: 1,
    };
    input.validate().map_err(|error| anyhow!(error))?;
    Ok(())
}
