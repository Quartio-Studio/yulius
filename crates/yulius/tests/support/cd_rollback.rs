//! Shared fixtures moved from `cd_rollback`.

use crate::support;
use crate::support::cd::{BoxResult, Fixture, TOKEN, TestResult, VALUE, fixture};
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use support::{Creds, call};
use yulius_app::ports::store_ext::AutomationStore as _;

/// Bob's (an administrator's) or alice's (a writer's) Superagent in `app`.
pub(crate) fn superagent(
    user: &str,
    login: &str,
) -> BoxResult<yulius_app::coordination::ProjectCaller> {
    Ok(yulius_app::coordination::ProjectCaller {
        sandbox: yulius_domain::ids::SandboxId::parse("s1")?,
        user: yulius_domain::ids::UserId::parse(user)?,
        task: None,
        channel: yulius_app::coordination::Channel::Superagent,
        actor: format!("cockpit:{login}"),
        detail: json!({"via": "cockpit", "shell": format!("sh-{login}")}),
    })
}
