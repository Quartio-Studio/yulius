//! Whether GitHub accepts the instance's own sign-in address (SPEC-466, ADR-0302). After a move
//! to another URL GitHub sign-in fails until the new callback URL is in the GitHub App, which only
//! a person can add: GitHub has no API for it. The instance asks GitHub's token endpoint with its
//! own App credentials, keeps the answer, and says where the App's settings are and what to paste.
//! GitHub is asked at start, on demand, and while someone reads an answer that is not
//! "registered"; never in a loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::host_move::{MOVED, MoveReport};
use crate::ports::github::GitHubAppConfig;

/// An answer that is not "registered" is asked for again when it is read this long after.
const RECHECK_MS: i64 = 30_000;
/// A sign-in start reuses an answer this young, so visitors cannot make the instance ask GitHub
/// more often.
const REUSE_MS: i64 = 5_000;

/// GitHub's answer about a callback URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackState {
    /// GitHub accepts it: sign-in works.
    Registered,
    /// The App does not hold it: GitHub refuses every sign-in from this address.
    Missing,
    /// GitHub gave neither answer; why.
    Unknown(String),
}

/// GitHub's answer for a public URL, and what a person registers in the App for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackFacts {
    /// The answer.
    pub state: CallbackState,
    /// When GitHub gave it; `None` before its first answer.
    pub checked_at: Option<Timestamp>,
    /// The callback URL to register.
    pub callback_url: String,
    /// The homepage to set.
    pub homepage_url: String,
    /// The App's settings page on GitHub.
    pub settings_url: String,
}

/// Settings → Instance, card "Address" (SPEC-466 MVC-010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressView {
    /// The instance's public URL.
    pub public_url: String,
    /// GitHub's answer; `None` without a GitHub App.
    pub github: Option<CallbackFacts>,
    /// What the first start after a move recorded; `None` when the instance never moved.
    pub moved: Option<MoveReport>,
}

/// The kept answer for the instance's own callback.
#[derive(Debug, Default)]
pub(crate) struct CallbackKept {
    /// The last answer, when it was given, and its number.
    answer: Mutex<Option<(CallbackState, Timestamp, u64)>>,
    /// One check at a time.
    asking: tokio::sync::Mutex<()>,
    /// A background check is on its way.
    refreshing: AtomicBool,
}

/// The sign-in callback of the instance at `public_url`.
#[must_use]
pub fn callback_url(public_url: &str) -> String {
    format!(
        "{}/api/auth/github/callback",
        public_url.trim_end_matches('/')
    )
}

/// A public URL as a move names it: `https://host`, nothing after it.
#[must_use]
pub fn public_url_ok(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|host| {
        !host.is_empty()
            && !host.starts_with(['-', '.'])
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
    })
}

/// The App's settings page: an organization's or a person's. Names GitHub would not have given
/// lead to the list of the person's Apps instead of into a made-up address.
#[must_use]
pub fn settings_url(owner_kind: Option<&str>, owner_login: Option<&str>, slug: &str) -> String {
    let plain = |name: &str| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    match (owner_kind, owner_login) {
        _ if !plain(slug) => "https://github.com/settings/apps".to_owned(),
        (Some("org"), Some(owner)) if plain(owner) => {
            format!("https://github.com/organizations/{owner}/settings/apps/{slug}")
        }
        _ => format!("https://github.com/settings/apps/{slug}"),
    }
}

fn facts(
    public_url: &str,
    settings_url: String,
    state: CallbackState,
    checked_at: Option<Timestamp>,
) -> CallbackFacts {
    CallbackFacts {
        state,
        checked_at,
        callback_url: callback_url(public_url),
        homepage_url: public_url.trim_end_matches('/').to_owned(),
        settings_url,
    }
}

impl App {
    /// The App's credentials and its settings page; `None` without an App.
    async fn callback_app(&self) -> AppResult<Option<(GitHubAppConfig, String)>> {
        let Some(instance) = self.deps.store.instance().await? else {
            return Ok(None);
        };
        let Some(sealed) = instance.github_app else {
            return Ok(None);
        };
        let app: GitHubAppConfig =
            serde_json::from_slice(&self.deps.crypto.open(&sealed)?).map_err(AppError::internal)?;
        let settings = settings_url(
            instance.owner_kind.as_deref(),
            instance.owner_login.as_deref(),
            &app.slug,
        );
        Ok(Some((app, settings)))
    }

    fn kept_answer(&self) -> Option<(CallbackState, Timestamp, u64)> {
        self.github_callback
            .answer
            .lock()
            .ok()
            .and_then(|kept| kept.clone())
    }

    /// Asks GitHub now whether it accepts the callback of `public_url` (the instance's own
    /// without one, whose kept answer this replaces); `None` without a GitHub App. One check runs
    /// at a time: a demand for the instance's own URL that waited for another takes that one's
    /// answer, as does one that finds an answer younger than `reuse_ms` (SPEC-466 MVC-003).
    ///
    /// # Errors
    /// `Invalid` for a `public_url` that is not `https://host`; store failures. GitHub giving no
    /// answer is [`CallbackState::Unknown`], not an error.
    pub async fn check_github_callback(
        &self,
        public_url: Option<&str>,
        reuse_ms: i64,
    ) -> AppResult<Option<CallbackFacts>> {
        // Read before anything is awaited: an answer given from here on is news to this demand.
        let before = self.kept_answer().map(|(.., number)| number);
        if public_url.is_some_and(|url| !public_url_ok(url)) {
            return Err(AppError::Invalid(
                "public_url must be `https://host`".into(),
            ));
        }
        let Some((app, settings)) = self.callback_app().await? else {
            return Ok(None);
        };
        let own = public_url.is_none();
        let url = public_url.unwrap_or(&self.config.public_url);
        let _one = self.github_callback.asking.lock().await;
        if own
            && let Some((state, at, number)) = self.kept_answer()
            && (Some(number) != before || self.now().0.saturating_sub(at.0) < reuse_ms)
        {
            return Ok(Some(facts(url, settings, state, Some(at))));
        }
        let asked = self
            .deps
            .github
            .callback_registered(&app, &callback_url(url))
            .await;
        let state = match asked {
            Ok(true) => CallbackState::Registered,
            Ok(false) => CallbackState::Missing,
            Err(err) => CallbackState::Unknown(err.to_string()),
        };
        let at = self.now();
        if own && let Ok(mut kept) = self.github_callback.answer.lock() {
            if kept.as_ref().map(|(was, ..)| was) != Some(&state) {
                tracing::info!(state = ?state, "GitHub's answer about the sign-in callback");
            }
            let number = kept.as_ref().map_or(0, |(.., number)| *number);
            *kept = Some((state.clone(), at, number.wrapping_add(1)));
        }
        Ok(Some(facts(url, settings, state, Some(at))))
    }

    /// Asks GitHub in the background, unless a background check is already on its way: at start
    /// and when the App was stored (SPEC-466 MVC-003).
    pub fn start_github_callback_check(self: &Arc<Self>) {
        if self.github_callback.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = app.check_github_callback(None, REUSE_MS).await {
                tracing::warn!(error = %err, "could not ask GitHub about the sign-in callback");
            }
            app.github_callback
                .refreshing
                .store(false, Ordering::SeqCst);
        });
    }

    /// The kept answer. Reading one that is not "registered" and older than [`RECHECK_MS`] (or
    /// none at all) asks GitHub again in the background: re-checked while someone looks.
    fn kept_callback(self: &Arc<Self>) -> Option<(CallbackState, Timestamp)> {
        let kept = self.kept_answer();
        let stale = match &kept {
            None => true,
            Some((CallbackState::Registered, ..)) => false,
            Some((_, at, _)) => self.now().0.saturating_sub(at.0) >= RECHECK_MS,
        };
        if stale {
            self.start_github_callback_check();
        }
        kept.map(|(state, at, _)| (state, at))
    }

    /// Whether GitHub is known to refuse the instance's callback, for the sign-in page (SPEC-466
    /// MVC-012). Call it only for an instance with a GitHub App.
    #[must_use]
    pub fn github_callback_missing(self: &Arc<Self>) -> bool {
        matches!(self.kept_callback(), Some((CallbackState::Missing, _)))
    }

    /// Whether a GitHub sign-in that starts now would end on GitHub's error page: the kept answer
    /// is "missing" and GitHub, asked again, still says so (SPEC-466 MVC-013). Any other answer
    /// never blocks sign-in.
    pub async fn github_sign_in_blocked(&self) -> bool {
        let missing = |state: &CallbackState| *state == CallbackState::Missing;
        self.kept_answer()
            .is_some_and(|(state, ..)| missing(&state))
            && self
                .check_github_callback(None, REUSE_MS)
                .await
                .is_ok_and(|facts| facts.is_some_and(|facts| missing(&facts.state)))
    }

    fn address_view(&self, github: Option<CallbackFacts>) -> AddressView {
        let moved = std::fs::read(self.config.data_dir.join(MOVED))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok());
        AddressView {
            public_url: self.config.public_url.clone(),
            github,
            moved,
        }
    }

    /// The instance's address: GitHub's kept answer and the last move (superadmin).
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins; store failures.
    pub async fn instance_address(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<AddressView> {
        self.require_superadmin(ctx)?;
        let github = self.callback_app().await?.map(|(_, settings)| {
            let (state, at) = match self.kept_callback() {
                Some((state, at)) => (state, Some(at)),
                None => (
                    CallbackState::Unknown("GitHub has not answered yet".into()),
                    None,
                ),
            };
            facts(&self.config.public_url, settings, state, at)
        });
        Ok(self.address_view(github))
    }

    /// [`App::instance_address`] after asking GitHub now ("Check again").
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins; store failures.
    pub async fn check_instance_address(&self, ctx: &AuthContext) -> AppResult<AddressView> {
        self.require_superadmin(ctx)?;
        let github = self.check_github_callback(None, 0).await?;
        Ok(self.address_view(github))
    }
}

#[cfg(test)]
mod tests {
    use super::{callback_url, public_url_ok, settings_url};

    #[test]
    fn the_settings_page_is_the_organization_s_or_the_person_s() {
        assert_eq!(
            settings_url(Some("org"), Some("Quartio-Studio"), "yulius-quartio"),
            "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-quartio"
        );
        assert_eq!(
            settings_url(Some("user"), Some("octocat"), "yulius-octocat"),
            "https://github.com/settings/apps/yulius-octocat"
        );
        assert_eq!(
            settings_url(None, None, "yulius"),
            "https://github.com/settings/apps/yulius"
        );
        // Never an address made of what GitHub would not have named.
        assert_eq!(
            settings_url(Some("org"), Some("a/../b"), "yulius"),
            "https://github.com/settings/apps/yulius"
        );
        assert_eq!(
            settings_url(Some("org"), Some("acme"), "x/../../y?z"),
            "https://github.com/settings/apps"
        );
    }

    #[test]
    fn a_public_url_is_https_and_a_host() {
        assert_eq!(
            callback_url("https://yulius.example/"),
            "https://yulius.example/api/auth/github/callback"
        );
        for good in ["https://yulius.example", "https://1-2-3-4.sslip.io"] {
            assert!(public_url_ok(good), "{good}");
        }
        for bad in [
            "http://yulius.example",
            "https://",
            "https://yulius.example/",
            "https://yulius.example/x",
            "https://user@yulius.example",
            "https://yulius.example:8443",
            "https://-x.example",
            "yulius.example",
        ] {
            assert!(!public_url_ok(bad), "{bad}");
        }
    }
}
