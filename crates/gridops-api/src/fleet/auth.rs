//! Host bearer authentication is independent of the browser cookie boundary.
//! The registry rechecks credential generation inside each write transaction.

use std::collections::HashMap;

use axum::{
    extract::{FromRequestParts, Path},
    http::{HeaderMap, header, request::Parts},
};
use gridops_core::fleet::{
    HostId,
    protocol::browser::ErrorCode,
    registry::{AuthenticatedHost, RegistryError, RegistryService},
};

use super::{boundary::context, error::FleetApiError};
use crate::state::AppState;

pub(super) struct FleetAgent(AuthenticatedHost);

impl FleetAgent {
    pub(super) const fn authenticated(&self) -> &AuthenticatedHost {
        &self.0
    }
}

impl FromRequestParts<AppState> for FleetAgent {
    type Rejection = FleetApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = context(parts);
        let Path(path) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| context.error(ErrorCode::InvalidRequest))?;
        let host_id: HostId = path
            .get("host_id")
            .ok_or_else(|| context.error(ErrorCode::InvalidRequest))?
            .parse()
            .map_err(|_| context.error(ErrorCode::InvalidRequest))?;
        let credential =
            bearer(&parts.headers).ok_or_else(|| context.error(ErrorCode::Unauthenticated))?;
        let registry = RegistryService::new(state.database.clone(), state.vault.clone());
        let host = registry
            .authenticate(host_id, credential)
            .await
            .map_err(|error| {
                context.error(if matches!(error, RegistryError::Database(_)) {
                    ErrorCode::DependencyUnavailable
                } else {
                    ErrorCode::Unauthenticated
                })
            })?;
        Ok(Self(host))
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    if headers.contains_key(header::COOKIE) {
        return None;
    }
    let mut authorizations = headers.get_all(header::AUTHORIZATION).iter();
    let value = authorizations.next()?.to_str().ok()?;
    if authorizations.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer")
        && token.len() == 43
        && token
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch)))
    .then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_bearer_rejects_cookie_ambiguity_and_malformed_headers() -> anyhow::Result<()> {
        let token = "A".repeat(43);
        let mut headers = HeaderMap::new();
        assert!(bearer(&headers).is_none());
        for value in [
            "Basic fixture".to_owned(),
            format!("Bearer {token} "),
            format!("Bearer  {token}"),
            format!("Bearer {token}="),
            "Bearer sentinel".to_owned(),
        ] {
            headers.insert(header::AUTHORIZATION, value.parse()?);
            assert!(bearer(&headers).is_none());
        }
        headers.insert(header::AUTHORIZATION, format!("bEaReR {token}").parse()?);
        assert_eq!(bearer(&headers), Some(token.as_str()));
        headers.append(header::AUTHORIZATION, format!("Bearer {token}").parse()?);
        assert!(bearer(&headers).is_none());
        headers.remove(header::AUTHORIZATION);
        headers.insert(header::AUTHORIZATION, format!("Bearer {token}").parse()?);
        headers.insert(header::COOKIE, "gridops_session=sentinel".parse()?);
        assert!(bearer(&headers).is_none());
        Ok(())
    }
}
