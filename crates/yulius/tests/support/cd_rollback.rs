//! Shared fixtures moved from `cd_rollback`.

use crate::support::cd::BoxResult;
use serde_json::json;

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
