mod auth;
mod error;
mod github_app;
mod oauth;
mod resources;
mod state;
mod webhooks;

use std::time::Duration;

use anyhow::Result;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    routing::{delete, get, post},
};
use gridops_core::{Config, GitHubClient, Vault, connect_database};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gridops=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    config.validate_api()?;
    let database = connect_database(&config).await?;
    let vault = Vault::from_config(&config)?;
    let github = GitHubClient::new(config.clone())?;
    let state = AppState::new(config.clone(), database, vault, github)?;
    state.validate_api().await?;
    let request_id = HeaderName::from_static("x-request-id");
    let origin: HeaderValue = config.base_url().origin().ascii_serialization().parse()?;

    let app = Router::new()
        .route("/api/health", get(resources::health))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/github", get(oauth::begin))
        .route("/api/v1/auth/github/callback", get(oauth::callback))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/webhooks/github", post(webhooks::receive))
        .route("/auth/github", get(oauth::begin))
        .route("/auth/github/callback", get(oauth::callback))
        .route("/auth/logout", post(auth::logout))
        .route(
            "/auth/github-app/manifest/callback",
            get(github_app::manifest_callback),
        )
        .route("/api/webhooks/github", post(webhooks::receive))
        .route("/api/v1/overview", get(resources::overview))
        .route("/api/v1/search", get(resources::search))
        .route("/api/v1/repositories", get(resources::repositories))
        .route("/api/v1/repositories/sync", post(oauth::sync))
        .route(
            "/api/v1/runner-pools",
            get(resources::runner_pools).post(resources::create_runner_pool),
        )
        .route(
            "/api/v1/runner-pools/options",
            get(resources::runner_pool_options),
        )
        .route(
            "/api/v1/runner-pools/{pool_id}",
            delete(resources::delete_runner_pool),
        )
        .route(
            "/api/v1/runner-pools/{pool_id}/action",
            post(resources::runner_pool_action),
        )
        .route("/api/v1/runners", get(resources::runners))
        .route(
            "/api/v1/runners/{runner_id}/action",
            post(resources::runner_action),
        )
        .route(
            "/api/v1/runners/{runner_id}/logs",
            get(resources::runner_logs),
        )
        .route(
            "/api/v1/runners/{runner_id}/logs/stream",
            get(resources::runner_log_stream),
        )
        .route("/api/v1/workflow-runs", get(resources::workflow_runs))
        .route(
            "/api/v1/workflow-runs/{run_id}",
            get(resources::workflow_run),
        )
        .route(
            "/api/v1/workflow-runs/{run_id}/logs",
            get(resources::workflow_run_logs),
        )
        .route(
            "/api/v1/workflow-runs/{run_id}/action",
            post(resources::workflow_run_action),
        )
        .route(
            "/api/workflow-runs/{run_id}/logs",
            get(resources::workflow_run_logs),
        )
        .route("/api/v1/webhooks", get(resources::webhook_deliveries))
        .route(
            "/api/v1/webhooks/{delivery_id}/retry",
            post(webhooks::retry),
        )
        .route("/api/v1/audit", get(resources::audit_events))
        .route("/api/v1/log-streams", get(resources::log_targets))
        .route(
            "/api/v1/log-streams/{stream_id}/logs",
            get(resources::archived_logs),
        )
        .route(
            "/api/v1/settings",
            get(resources::settings).put(resources::save_settings),
        )
        .route(
            "/api/v1/github-app/manifest",
            post(github_app::create_manifest),
        )
        .route("/api/v1/backups/database", get(resources::database_backup))
        .route("/api/backups/database", get(resources::database_backup))
        .with_state(state)
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
        ]))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(origin)
                .allow_credentials(true)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([
                    header::CONTENT_TYPE,
                    HeaderName::from_static("x-csrf-token"),
                ]),
        );

    let listener = tokio::net::TcpListener::bind(config.api_bind()).await?;
    tracing::info!(address = %config.api_bind(), "GridOps Rust API listening");
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
