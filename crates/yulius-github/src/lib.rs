//! GitHub adapter: App JWT and installation tokens, OAuth, permissions, repositories, rulesets,
//! pull requests and checks (SPEC-002 ADR-08).
//!
//! [`GitHubClient`] is an async REST client over `reqwest` + `rustls` (ring provider, installed
//! once per process). Every request sends `User-Agent: yulius`,
//! `X-GitHub-Api-Version: 2022-11-28` and `Accept: application/vnd.github+json` (the OAuth
//! token endpoint gets `Accept: application/json`, which it needs to answer JSON). Bodies are
//! bounded to [`http::MAX_RESPONSE_BYTES`]; listings to [`client::MAX_PAGES`] pages.
//!
//! Retries (SPEC-002 §9): only idempotent GETs, at most 3 attempts within 30 s, on 5xx
//! (500/502/503/504), rate limits (429, or 403 with `Retry-After`/exhausted quota) and
//! transport errors, honoring `Retry-After` and `x-ratelimit-reset`. A wait that would exceed
//! the budget is not taken. Mutations are never retried.
//!
//! Credentials are [`Secret`]s with redacted `Debug`/`Display`; error messages are scrubbed
//! of the request's tokens and of token-shaped strings (SEC-002).
//!
//! With the `fake` feature, [`fake::FakeGitHub`] is an in-process GitHub stand-in for tests.

mod app;
pub mod client;
mod config;
mod error;
pub mod http;
mod jwt;
mod oauth;
mod pulls;
mod repos;
mod secret;
mod time;
mod types;
mod validate;
mod wire;

#[cfg(feature = "fake")]
pub mod fake;

pub use app::{AUTH_CALLBACK_PATH, MANIFEST_CALLBACK_PATH, TOKEN_REFRESH_MARGIN_MS};
pub use client::GitHubClient;
pub use config::{
    AppCredentials, Clock, DEFAULT_API_BASE, DEFAULT_WEB_BASE, GitHubConfig, OwnerKind,
    RetryPolicy, SystemClock,
};
pub use error::GitHubError;
pub use reqwest::Url;
pub use secret::Secret;
pub use types::{
    ChecksSummary, CompareStatus, Comparison, CreateRepo, GitHubUser, InstallationToken,
    MergeMethod, MergeOutcome, NewPull, PullFilter, PullRequest, PullState, Repo, RepoAccess,
    RulesetSpec, UserToken,
};
pub use yulius_domain::permission::{AccessLevel, GitHubPermissions};
