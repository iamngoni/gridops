//! The fix agent: provider connections, the tool loop, and the sandbox tools
//! it uses to diagnose and repair a failed GitHub Actions job.
//!
//! This crate holds no database or GitHub state. Callers decrypt credentials,
//! supply a [`sandbox::Sandbox`] that runs commands in an isolated container,
//! and turn the [`session::AgentOutcome`] into a pull request or a diagnosis.
//! The sandbox never sees model credentials or GitHub tokens: the model is
//! called from here, and the caller writes commits through the GitHub API.

pub mod catalog;
pub mod changes;
pub mod connection;
pub mod prompt;
pub mod provider_auth;
pub mod sandbox;
pub mod session;
pub mod subscriptions;
pub mod tools;

/// Ceiling for one model request. Long agent turns with large contexts can
/// take minutes; anything slower is treated as stalled.
pub const PROVIDER_HTTP_TIMEOUT_SECONDS: u64 = 400;
