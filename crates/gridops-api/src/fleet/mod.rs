//! Fleet transport is isolated from legacy routes and host runtime execution.
//! Only implemented registry operations are exposed; enrollment grants no start authority.

mod auth;
mod boundary;
mod credentials;
mod enrollment;
mod error;
mod inventory;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use gridops_core::fleet::protocol::{browser::ErrorCode, primitives::DEFAULT_JSON_BYTES};

use self::error::RequestContext;
use crate::state::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/fleet/enrollments", post(enrollment::issue))
        .route(
            "/api/v1/fleet/enrollments/{enrollment_id}",
            get(enrollment::status),
        )
        .route(
            "/api/v1/fleet/enrollments/{enrollment_id}/revoke",
            post(enrollment::revoke),
        )
        .route("/api/v1/fleet/agent/enroll", post(enrollment::consume))
        .route(
            "/api/v1/fleet/agent/hosts/{host_id}/handshake",
            post(inventory::handshake),
        )
        .route(
            "/api/v1/fleet/agent/hosts/{host_id}/inventory",
            post(inventory::submit),
        )
        .route(
            "/api/v1/fleet/agent/hosts/{host_id}/heartbeat",
            post(inventory::heartbeat),
        )
        .route(
            "/api/v1/fleet/hosts/{host_id}/approve",
            post(enrollment::approve),
        )
        .route(
            "/api/v1/fleet/hosts/{host_id}/recover",
            post(enrollment::recover),
        )
        .route(
            "/api/v1/fleet/hosts/{host_id}/credentials/rotate",
            post(credentials::request),
        )
        .route(
            "/api/v1/fleet/agent/hosts/{host_id}/credentials/rotate",
            post(credentials::exchange),
        )
        .route(
            "/api/v1/fleet/agent/hosts/{host_id}/credentials/ack",
            post(credentials::acknowledge),
        )
        .route("/api/v1/fleet", any(not_found))
        .route("/api/v1/fleet/", any(not_found))
        .route("/api/v1/fleet/{*path}", any(not_found))
        .layer(DefaultBodyLimit::max(DEFAULT_JSON_BYTES))
        .layer(middleware::from_fn(boundary::envelope))
}

pub(crate) fn operation_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/operations/{operation_id}",
            get(credentials::status),
        )
        .route("/api/v1/operations", any(not_found))
        .route("/api/v1/operations/", any(not_found))
        .route("/api/v1/operations/{operation_id}/", any(not_found))
        .route("/api/v1/operations/{operation_id}/{*path}", any(not_found))
        .layer(middleware::from_fn(boundary::envelope))
}

async fn not_found(context: RequestContext) -> Response {
    context.error(ErrorCode::NotFound).into_response()
}
