//! The Claude account gate (SPEC-246): a person's start of an agent is refused while no
//! account is usable, with the reason and where to connect one, instead of a job that fails or
//! waits without saying why.

use yulius_domain::harness::Provider;
use yulius_domain::task::FailReason;

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AccountRecord, AccountStatus};

/// Where an instance administrator connects a Claude account (Subscriptions).
pub const CONNECT_PATH: &str = "/admin/accounts";

/// Whether the pool can run an agent of a provider (CLA-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentAccounts {
    /// An account has a credential and is `ok` or `limited`: a start runs or waits for capacity.
    Usable,
    /// Accounts are signed in, but every one must sign in again (or failed).
    NeedSignIn,
    /// No account of the provider has a credential.
    None,
}

/// The pool's own health rule (`account_views`): a credential, and `ok` or `limited`.
pub(crate) fn agent_accounts(accounts: &[AccountRecord], provider: Provider) -> AgentAccounts {
    let signed_in = accounts
        .iter()
        .filter(|a| a.provider == provider && a.credential.is_some());
    let mut any = false;
    for account in signed_in {
        if matches!(account.status, AccountStatus::Ok | AccountStatus::Limited) {
            return AgentAccounts::Usable;
        }
        any = true;
    }
    if any {
        AgentAccounts::NeedSignIn
    } else {
        AgentAccounts::None
    }
}

/// The refusal of a start for `state` (CLA-002), `None` when the start may go on.
pub(crate) fn refusal(state: AgentAccounts) -> Option<AppError> {
    let reason = match state {
        AgentAccounts::Usable => return None,
        AgentAccounts::None => "no Claude account is connected",
        AgentAccounts::NeedSignIn => "every Claude account must sign in again",
    };
    Some(AppError::AccountRequired(format!(
        "Connect a Claude account first: {reason}, so no agent can start. An instance \
         administrator connects one in Subscriptions ({CONNECT_PATH})."
    )))
}

/// What a task started by itself (a chain, a rule, an automation, a time) says while it waits for
/// an account (CLA-004); `None` while one is usable (it only waits for capacity).
pub(crate) fn waiting_notice(state: AgentAccounts) -> Option<&'static str> {
    match state {
        AgentAccounts::Usable => None,
        AgentAccounts::None => Some(
            "Yulius is holding this task: no Claude account is connected. An instance \
             administrator connects one in Subscriptions (/admin/accounts); the task then starts \
             by itself.",
        ),
        AgentAccounts::NeedSignIn => Some(
            "Yulius is holding this task: every Claude account must sign in again. An instance \
             administrator signs one in in Subscriptions (/admin/accounts); the task then starts \
             by itself.",
        ),
    }
}

/// Why a start that failed ended (CLA-005): no usable account, a precondition, or the spawn.
pub(crate) fn fail_reason(err: &AppError) -> FailReason {
    match err {
        AppError::AccountRequired(_) => FailReason::NoAccount,
        AppError::Conflict(_) | AppError::Capacity(_) | AppError::Invalid(_) => {
            FailReason::Precondition
        }
        _ => FailReason::SpawnError,
    }
}

impl App {
    /// Whether an agent of `provider` can be started now (CLA-001).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn agent_accounts(&self, provider: Provider) -> AppResult<AgentAccounts> {
        let accounts = self.deps.store.accounts().await?;
        Ok(agent_accounts(&accounts, provider))
    }

    /// Whether a Claude account is usable (`GET /api/me` `claude_connected`).
    ///
    /// # Errors
    /// Store failures.
    pub async fn claude_connected(&self) -> AppResult<bool> {
        Ok(self.agent_accounts(Provider::Anthropic).await? == AgentAccounts::Usable)
    }

    /// Refuses a person's start of an agent while no account of `provider` is usable (CLA-002).
    ///
    /// # Errors
    /// `AccountRequired` without a usable account; store failures.
    pub(crate) async fn ensure_agent_account(&self, provider: Provider) -> AppResult<()> {
        match refusal(self.agent_accounts(provider).await?) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Sealed;
    use yulius_domain::ids::AccountId;
    use yulius_domain::time::Timestamp;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn account(
        id: &str,
        credential: bool,
        status: AccountStatus,
    ) -> Result<AccountRecord, Box<dyn std::error::Error>> {
        Ok(AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: credential.then(|| Sealed(vec![1])),
            status,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
        })
    }

    #[test]
    fn an_account_with_a_credential_and_health_is_usable() -> TestResult {
        let ok = [account("a", true, AccountStatus::Ok)?];
        assert_eq!(
            agent_accounts(&ok, Provider::Anthropic),
            AgentAccounts::Usable
        );
        let limited = [
            account("a", true, AccountStatus::Relogin)?,
            account("b", true, AccountStatus::Limited)?,
        ];
        assert_eq!(
            agent_accounts(&limited, Provider::Anthropic),
            AgentAccounts::Usable,
            "a limited account waits for capacity, it is not missing"
        );
        Ok(())
    }

    #[test]
    fn no_signed_in_account_is_none_and_unhealthy_ones_need_a_sign_in() -> TestResult {
        assert_eq!(
            agent_accounts(&[], Provider::Anthropic),
            AgentAccounts::None
        );
        let pending = [account("a", false, AccountStatus::PendingLogin)?];
        assert_eq!(
            agent_accounts(&pending, Provider::Anthropic),
            AgentAccounts::None,
            "a sign-in never finished connects nothing"
        );
        let stale = [
            account("a", true, AccountStatus::Relogin)?,
            account("b", true, AccountStatus::Error)?,
        ];
        assert_eq!(
            agent_accounts(&stale, Provider::Anthropic),
            AgentAccounts::NeedSignIn
        );
        Ok(())
    }

    #[test]
    fn the_refusal_names_the_reason_and_where_to_connect() -> TestResult {
        assert_eq!(refusal(AgentAccounts::Usable), None);
        let none = refusal(AgentAccounts::None).ok_or("refused")?;
        assert_eq!(none.code(), "claude_account_required");
        let text = none.public_message();
        assert!(text.contains("no Claude account is connected"), "{text}");
        assert!(text.contains(CONNECT_PATH), "{text}");
        let stale = refusal(AgentAccounts::NeedSignIn)
            .ok_or("refused")?
            .public_message();
        assert!(stale.contains("must sign in again"), "{stale}");
        Ok(())
    }

    #[test]
    fn the_waiting_notice_tells_no_account_from_a_needed_sign_in() -> TestResult {
        assert_eq!(waiting_notice(AgentAccounts::Usable), None);
        let none = waiting_notice(AgentAccounts::None).ok_or("notice")?;
        assert!(none.contains("no Claude account is connected"), "{none}");
        let stale = waiting_notice(AgentAccounts::NeedSignIn).ok_or("notice")?;
        assert!(stale.contains("must sign in again"), "{stale}");
        for text in [none, stale] {
            assert!(text.contains("/admin/accounts") && text.contains("starts by itself"));
        }
        Ok(())
    }

    #[test]
    fn a_start_refused_for_want_of_an_account_fails_as_no_account() {
        let refused = refusal(AgentAccounts::None);
        assert_eq!(
            refused.as_ref().map(fail_reason),
            Some(FailReason::NoAccount)
        );
        assert_eq!(
            fail_reason(&AppError::Conflict("not cloned".into())),
            FailReason::Precondition
        );
        assert_eq!(
            fail_reason(&AppError::Internal("spawn".into())),
            FailReason::SpawnError
        );
    }
}
