//! GitHub-mirrored access decisions with a bounded-staleness cache (SPEC-001 ACC-007/008), the
//! superadmin's access to every sandbox (ACC-012), and whose GitHub token an action uses: a
//! person's own, or for a task its owner's (F 2026-10-11, SPEC-070).

use std::sync::atomic::Ordering;

use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, TaskRecord, UserRecord};
use crate::ports::github::RepoAccess;

/// The token a person's action on GitHub uses, and whether it is the GitHub App's
/// installation token rather than the person's own (SPEC-677).
pub(crate) struct ActingToken {
    /// The token.
    pub(crate) token: Secret,
    /// The App acts for the superadmin without a GitHub identity.
    pub(crate) through_app: bool,
}

/// Whether `user` reaches GitHub only through the GitHub App: the superadmin without a GitHub
/// identity (ACC-012, SPEC-677).
pub(crate) fn acts_through_app(user: &UserRecord) -> bool {
    user.is_superadmin && user.github_login.is_none()
}

/// Whether `message` is GitHub's refusal of the App itself, in the GitHub adapter's own words: a
/// missing permission (403), or a ruleset or branch protection that blocks the write (a 405 merge
/// or a 422 ref update). Yulius's own refusals that quote GitHub, and GitHub's other refusals (a
/// draft, a conflict, a failure that may pass), are not.
fn refuses_the_app(message: &str) -> bool {
    let adapter = message.starts_with("GitHub: ")
        || message.starts_with("pull request is not mergeable (HTTP 405): ");
    let lower = message.to_ascii_lowercase();
    adapter
        && (message.contains("(HTTP 403)")
            || lower.contains("rule violation")
            || lower.contains("branch protection or a ruleset blocks it"))
}

/// `err` of an action the GitHub App took for the superadmin on `branch`, said plainly when
/// GitHub refused the App (SPEC-677 SAP-004). GitHub's rules bind the App as they bind agents,
/// whose tokens are the App's (ADR-0114), so the refusal never suggests letting the App through.
/// The error keeps its kind; anything else passes unchanged. A 401 for the App's token is the
/// App's, never the person's session.
pub(crate) fn app_refusal(err: AppError, branch: &str) -> AppError {
    let said = |reason: &str| {
        let reason = reason.strip_prefix("GitHub: ").unwrap_or(reason);
        format!(
            "GitHub refused the Yulius GitHub App, which acts for the superadmin: {reason}. \
             GitHub keeps the App out of '{branch}' as it keeps agents out (a ruleset, or the \
             App's permissions), and Yulius never asks to let it through: a repository admin \
             signed in with GitHub can do this, or do it on GitHub."
        )
    };
    match err {
        AppError::Unauthenticated => AppError::Upstream(
            "GitHub rejected the credentials of the Yulius GitHub App, which acts for the \
             superadmin (HTTP 401); nothing changed on GitHub. Check the App's installation, or \
             do it on GitHub."
                .into(),
        ),
        AppError::Upstream(m) if refuses_the_app(&m) => AppError::Upstream(said(&m)),
        AppError::Conflict(m) if refuses_the_app(&m) => AppError::Conflict(said(&m)),
        AppError::Invalid(m) if refuses_the_app(&m) => AppError::Invalid(said(&m)),
        AppError::Forbidden(m) if refuses_the_app(&m) => AppError::Forbidden(said(&m)),
        other => other,
    }
}

impl App {
    /// Effective access of `user` to `sandbox`.
    ///
    /// The superadmin owns the instance and has admin access to every sandbox, with or without
    /// a GitHub link, without asking GitHub (ACC-012). A playground is its maker's, and a guest's
    /// whose live link lets them in, with write access (SPEC-099, SPEC-410). Everyone else
    /// mirrors GitHub: fresh cache
    /// entries are reused; otherwise GitHub is asked with the user's token. On a GitHub failure a
    /// cached value younger than the staleness bound is used, else the error is returned. Other
    /// users without a GitHub identity have no access.
    ///
    /// Every access path (REST, the event stream, terminal, shell and browser sockets and their
    /// re-checks, files, Git, tasks and analytics) decides through this function.
    ///
    /// # Errors
    /// `Upstream` when GitHub cannot be reached and no usable cache exists.
    pub async fn access_level(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<AccessLevel> {
        self.access_level_within(user, sandbox, self.config.permission_ttl_ms)
            .await
    }

    /// Like [`App::access_level`], but a cached decision younger than the staleness bound is
    /// used without asking GitHub: for internal calls that must answer within seconds on every
    /// command, such as the redaction in a Superagent (SPEC-147 SAM-003). A loss of access
    /// takes effect within the staleness bound, or at once when any other check refreshed it.
    ///
    /// # Errors
    /// As [`App::access_level`].
    pub(crate) async fn settled_access_level(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<AccessLevel> {
        self.access_level_within(user, sandbox, self.config.permission_stale_ms)
            .await
    }

    async fn access_level_within(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
        fresh_ms: i64,
    ) -> AppResult<AccessLevel> {
        if user.disabled_at.is_some() {
            return Ok(AccessLevel::None);
        }
        if user.is_superadmin {
            return Ok(AccessLevel::Admin);
        }
        // A playground has no GitHub repository to mirror: it is its maker's (SPEC-099 PLY-005),
        // and a guest works in the one their live link lets them into (SPEC-410 GST-003).
        if sandbox.is_playground() {
            if sandbox.created_by == user.id {
                return Ok(AccessLevel::Admin);
            }
            return Ok(match self.guest_link_of(user).await? {
                Some(link) if link.sandbox_id == sandbox.id && link.live_at(self.now()) => {
                    AccessLevel::Write
                }
                _ => AccessLevel::None,
            });
        }
        if user.github_login.is_none() {
            return Ok(AccessLevel::None);
        }
        let now = self.now();
        let store = &self.deps.store;
        let cached = store.cached_access(&user.id, &sandbox.id).await?;
        if let Some(c) = cached
            && now.millis_since(c.checked_at) < fresh_ms
        {
            return Ok(c.level);
        }
        match self.fetch_access(user, sandbox).await {
            Ok(level) => {
                store.put_access(&user.id, &sandbox.id, level, now).await?;
                Ok(level)
            }
            Err(err) => match cached {
                Some(c) if now.millis_since(c.checked_at) < self.config.permission_stale_ms => {
                    tracing::warn!(error = %err, "using stale permission cache");
                    Ok(c.level)
                }
                _ => Err(err),
            },
        }
    }

    async fn fetch_access(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<AccessLevel> {
        let token = self.user_github_token(user).await?;
        match self
            .deps
            .github
            .repo_access(&token, &sandbox.repo_owner, &sandbox.repo_name)
            .await?
        {
            RepoAccess::NoAccess => Ok(AccessLevel::None),
            RepoAccess::Access { permissions, .. } => Ok(AccessLevel::from_github(permissions)),
        }
    }

    /// The user's GitHub access token, refreshed when expired.
    ///
    /// # Errors
    /// `Unauthenticated` when the user never signed in with GitHub or the refresh failed.
    pub async fn user_github_token(&self, user: &UserRecord) -> AppResult<Secret> {
        let store = &self.deps.store;
        let record = store
            .github_token(&user.id)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        let now = self.now();
        let expired = record
            .expires_at
            .is_some_and(|exp| now.plus_millis(60_000) >= exp);
        if !expired {
            let bytes = self.deps.crypto.open(&record.access)?;
            return String::from_utf8(bytes)
                .map(Secret::new)
                .map_err(AppError::internal);
        }
        let refresh = record.refresh.as_ref().ok_or(AppError::Unauthenticated)?;
        let refresh = Secret::new(
            String::from_utf8(self.deps.crypto.open(refresh)?).map_err(AppError::internal)?,
        );
        let app = self.github_app().await?;
        let fresh = self
            .deps
            .github
            .refresh_token(&app, &refresh)
            .await
            .map_err(|_| AppError::Unauthenticated)?;
        self.save_user_token(user, &fresh).await?;
        Ok(fresh.access)
    }

    /// A GitHub token for an action that acts as the person on GitHub (merging, releasing).
    /// An account without a GitHub identity (the superadmin, ACC-012) is refused with a message
    /// naming `action` — never `Unauthenticated`, which would end the browser session.
    ///
    /// # Errors
    /// `Forbidden` without a GitHub identity; otherwise as [`App::user_github_token`].
    pub(crate) async fn person_github_token(
        &self,
        user: &UserRecord,
        action: &str,
    ) -> AppResult<Secret> {
        if user.github_login.is_none() {
            return Err(AppError::Forbidden(format!(
                "{action} acts as a person on GitHub, and the superadmin account has no GitHub \
                 identity. Do it on GitHub, or sign in with GitHub as an admitted user."
            )));
        }
        self.user_github_token(user).await
    }

    /// [`App::person_github_token`], except that the superadmin without a GitHub identity acts
    /// through the GitHub App's installation for `sandbox` (SPEC-677 SAP-001). The caller checks
    /// every other rule first; GitHub's own rules still bind the App.
    ///
    /// # Errors
    /// As [`App::person_github_token`]; for the App, as [`App::dlv_installation_token`].
    pub(crate) async fn person_or_app_token(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
        action: &str,
    ) -> AppResult<ActingToken> {
        if acts_through_app(user) {
            return Ok(ActingToken {
                token: self.dlv_installation_token(sandbox).await?,
                through_app: true,
            });
        }
        Ok(ActingToken {
            token: self.person_github_token(user, action).await?,
            through_app: false,
        })
    }

    /// The GitHub token of the person who owns `task` (SPEC-070; its creator until a hand-over),
    /// for what Yulius does on GitHub for the task (F 2026-10-11): its pushes and pull requests for
    /// it and the agent's delivery tools (never the agent's own Git and `gh`, ADR-0114), so GitHub
    /// shows that person as the actor. `None` when the owner has no usable token — no linked GitHub account (the
    /// superadmin, ACC-012), a disabled or deleted account, or a sign-in that can no longer be
    /// refreshed. The caller then acts as the GitHub App, and the task's terminal says so once
    /// (again after a hand-over).
    pub(crate) async fn owner_github_token(&self, task: &TaskRecord) -> Option<Secret> {
        let reason = match self.deps.store.user(&task.owner).await {
            Ok(Some(user)) if user.disabled_at.is_some() => "its owner's account is disabled",
            Ok(Some(user)) if user.github_login.is_none() => {
                "its owner has no linked GitHub account"
            }
            Ok(Some(user)) => match self.user_github_token(&user).await {
                Ok(token) => return Some(token),
                Err(err) => {
                    tracing::info!(task = %task.id, error = %err, "owner's GitHub token unusable");
                    "its owner's GitHub sign-in has expired (signing in with GitHub renews it)"
                }
            },
            Ok(None) => "its owner's account no longer exists",
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "reading the task owner failed");
                "its owner's account could not be read"
            }
        };
        let runtime = self.runtime(&task.id);
        if !runtime.acting_as_app_said.swap(true, Ordering::SeqCst) {
            runtime.hub.notice(format!(
                "This task acts on GitHub as the Yulius GitHub App, not as a person: {reason}."
            ));
        }
        None
    }

    /// [`App::owner_github_token`], else the App's installation token for `sandbox`'s owner:
    /// the token for Yulius's own GitHub calls for a task (pushes, pull requests, merges,
    /// promotions).
    ///
    /// # Errors
    /// Without a usable owner token: the App is not installed; GitHub failures.
    pub(crate) async fn task_github_token(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<Secret> {
        match self.owner_github_token(task).await {
            Some(token) => Ok(token),
            None => self.dlv_installation_token(sandbox).await,
        }
    }

    /// [`Self::require`] for the caller of a request, who may hold an API token: a token's scope
    /// only narrows what its person may do (SPEC-247 RMT-001), so a `read` token takes no action
    /// above View, whatever the request's method (a GET that reads the working tree is one).
    ///
    /// # Errors
    /// As [`Self::require`]; `Forbidden` for a token whose scope does not allow `action`.
    pub async fn require_for(
        &self,
        ctx: &crate::auth::AuthContext,
        sandbox_id: &SandboxId,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let found = self.require(&ctx.user, sandbox_id, action).await?;
        if !ctx.token_allows(action) {
            return Err(AppError::Forbidden(
                "this API token is read-only; make an operate token on Settings to do this".into(),
            ));
        }
        Ok(found)
    }

    /// Requires `action` on `sandbox`; invisible sandboxes yield `NotFound`.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` with insufficient access.
    pub async fn require(
        &self,
        user: &UserRecord,
        sandbox_id: &SandboxId,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let sandbox = self
            .deps
            .store
            .sandbox(sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let level = self.access_level(user, &sandbox).await?;
        if level == AccessLevel::None {
            return Err(AppError::NotFound);
        }
        // A guest works in their playground but never configures or administers it (GST-003).
        if matches!(action, Action::Configure | Action::Administer) && self.is_guest(user) {
            return Err(AppError::Forbidden(
                "a guest works on this playground's tasks; its settings are its admins'".into(),
            ));
        }
        if !level.allows(action) {
            return Err(AppError::Forbidden(format!(
                "this action needs {} access on GitHub; you have {}",
                action.required_level().as_str(),
                level.as_str()
            )));
        }
        Ok((sandbox, level))
    }

    /// Whether the user may view a sandbox (errors count as no).
    pub async fn can_view(&self, user: &UserRecord, sandbox: &SandboxId) -> bool {
        self.require(user, sandbox, Action::View).await.is_ok()
    }

    /// Whether the user may work in a sandbox: its files, terminals and editor (errors count as
    /// no).
    pub async fn can_work_in(&self, user: &UserRecord, sandbox: &SandboxId) -> bool {
        self.require(user, sandbox, Action::Work).await.is_ok()
    }

    /// Sandboxes the user can see with their levels. GitHub failures for one sandbox hide it
    /// rather than failing the whole list.
    ///
    /// # Errors
    /// Store failures.
    pub async fn visible_sandboxes(
        &self,
        user: &UserRecord,
    ) -> AppResult<Vec<(SandboxRecord, AccessLevel)>> {
        let mut out = Vec::new();
        for sandbox in self.deps.store.sandboxes().await? {
            match self.access_level(user, &sandbox).await {
                Ok(AccessLevel::None) => {}
                Ok(level) => out.push((sandbox, level)),
                Err(err) => {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, "permission lookup failed")
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod app_refusal_tests {
    use super::{AppError, acts_through_app, app_refusal};
    use crate::model::UserRecord;
    use yulius_domain::ids::UserId;
    use yulius_domain::time::Timestamp;

    const SAID: &str = "GitHub refused the Yulius GitHub App, which acts for the superadmin: ";

    fn account(
        login: Option<&str>,
        superadmin: bool,
    ) -> Result<UserRecord, Box<dyn std::error::Error>> {
        Ok(UserRecord {
            id: UserId::parse("u-1")?,
            github_login: login.map(str::to_owned),
            github_id: None,
            display_name: "Someone".into(),
            avatar_url: None,
            is_superadmin: superadmin,
            is_instance_admin: superadmin,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
    }

    #[test]
    fn only_the_superadmin_without_a_github_identity_acts_through_the_app()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(acts_through_app(&account(None, true)?));
        assert!(
            !acts_through_app(&account(Some("root"), true)?),
            "a linked superadmin keeps their own token"
        );
        assert!(
            !acts_through_app(&account(None, false)?),
            "a guest stays refused"
        );
        assert!(!acts_through_app(&account(Some("bob"), false)?));
        Ok(())
    }

    #[test]
    fn a_ruleset_or_permission_refusal_names_the_app_and_keeps_its_kind() {
        let ruleset = app_refusal(
            AppError::Conflict(
                "pull request is not mergeable (HTTP 405): Repository rule violations found".into(),
            ),
            "main",
        );
        let AppError::Conflict(text) = ruleset else {
            panic!("a conflict stays a conflict: {ruleset:?}");
        };
        assert_eq!(
            text,
            format!(
                "{SAID}pull request is not mergeable (HTTP 405): Repository rule violations \
                 found. GitHub keeps the App out of 'main' as it keeps agents out (a ruleset, or \
                 the App's permissions), and Yulius never asks to let it through: a repository \
                 admin signed in with GitHub can do this, or do it on GitHub."
            )
        );
        for err in [
            AppError::Upstream("GitHub: GitHub refused the request (HTTP 403): no".into()),
            AppError::Upstream(
                "GitHub: GitHub rejected the request as invalid (HTTP 422): Repository rule \
                 violations found"
                    .into(),
            ),
            AppError::Conflict(
                "pull request is not mergeable (HTTP 405): branch protection or a ruleset \
                 blocks it (required reviews or checks, or restricted updates) (GitHub: x)"
                    .into(),
            ),
        ] {
            let text = app_refusal(err, "main").to_string();
            assert!(text.starts_with(SAID), "{text}");
            assert!(!text.contains("GitHub: GitHub"), "{text}");
        }
    }

    #[test]
    fn other_refusals_and_failures_that_may_pass_are_unchanged() {
        for err in [
            AppError::Conflict("nothing was promoted: 'dev' moved since it was confirmed".into()),
            AppError::Upstream("GitHub: GitHub upstream error (HTTP 502)".into()),
            AppError::Conflict("the ref update is not a fast-forward".into()),
            // Yulius's own refusal quoting GitHub's 422 (merge.rs, the pull request's title).
            AppError::Conflict(
                "GitHub refused the title `x` Yulius gives pull request #1 before merging it \
                 (GitHub: GitHub rejected the request as invalid (HTTP 422): title)"
                    .into(),
            ),
            // GitHub's ordinary 405s: the App is not what is refused.
            AppError::Conflict("pull request is not mergeable (HTTP 405): it is a draft".into()),
            AppError::Conflict(
                "pull request is not mergeable (HTTP 405): it conflicts with 'main' (GitHub: x)"
                    .into(),
            ),
            AppError::Invalid(
                "GitHub: GitHub rejected the request as invalid (HTTP 422): sha".into(),
            ),
            AppError::NotFound,
        ] {
            assert_eq!(app_refusal(err.clone(), "main"), err);
        }
    }

    #[test]
    fn a_rejected_app_token_never_signs_the_person_out() {
        let err = app_refusal(AppError::Unauthenticated, "main");
        assert!(
            matches!(&err, AppError::Upstream(text) if text.contains("(HTTP 401)")),
            "{err:?}"
        );
    }
}
