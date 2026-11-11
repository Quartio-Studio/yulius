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
/// So many answers in a row that say the same are asked for again after [`RECHECK_MS`]: someone
/// is adding the callback on GitHub and watches the page.
const QUICK_RECHECKS: u32 = 10;
/// After them the wait doubles with every such answer, this often at most (4 minutes).
const MAX_DOUBLINGS: u32 = 3;
/// A check started by a read reuses an answer this young: one a demand got meanwhile.
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

/// GitHub's last answer about the instance's own callback.
#[derive(Debug, Clone)]
struct Kept {
    state: CallbackState,
    /// When GitHub gave it.
    at: Timestamp,
    /// Which answer it is, so a demand that waited for another knows a new one from an old one.
    number: u64,
    /// How many answers before it said the same and were asked for by a read, not by a person.
    same: u32,
}

/// How long an answer that is not "registered" is kept before a read asks GitHub again: half a
/// minute while it is new, then ever longer, so a page left open does not make the instance ask
/// GitHub twice a minute for as long as nobody adds the callback.
fn recheck_after(same: u32) -> i64 {
    RECHECK_MS << same.saturating_sub(QUICK_RECHECKS).min(MAX_DOUBLINGS)
}

impl Kept {
    /// Whether a read at `now` asks GitHub again. An answer dated after `now` (the clock stepped
    /// back) is due too: nothing waits for the clock to catch up.
    fn stale(&self, now: Timestamp) -> bool {
        self.state != CallbackState::Registered
            && !(0..recheck_after(self.same)).contains(&now.0.saturating_sub(self.at.0))
    }

    /// Whether `state` says the same as this answer (an unknown answer for whatever reason).
    fn says(&self, state: &CallbackState) -> bool {
        std::mem::discriminant(&self.state) == std::mem::discriminant(state)
    }
}

/// The kept answer for the instance's own callback.
#[derive(Debug, Default)]
pub(crate) struct CallbackKept {
    /// The last answer.
    answer: Mutex<Option<Kept>>,
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
/// (it gives letters, digits and hyphens) lead to the list of the person's Apps instead of into a
/// made-up address.
#[must_use]
pub fn settings_url(owner_kind: Option<&str>, owner_login: Option<&str>, slug: &str) -> String {
    let plain = |name: &str| {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
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

    fn kept_answer(&self) -> Option<Kept> {
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
        self.ask_github(public_url, reuse_ms, false).await
    }

    /// [`App::check_github_callback`]; `by_a_read` when no person or command demanded it, which
    /// counts towards asking less often ([`recheck_after`]).
    async fn ask_github(
        &self,
        public_url: Option<&str>,
        reuse_ms: i64,
        by_a_read: bool,
    ) -> AppResult<Option<CallbackFacts>> {
        // Read before anything is awaited: an answer given from here on is news to this demand.
        let before = self.kept_answer().map(|kept| kept.number);
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
        let known = self.kept_answer();
        if own
            && let Some(kept) = &known
            && (Some(kept.number) != before
                || (0..reuse_ms).contains(&self.now().0.saturating_sub(kept.at.0)))
        {
            // A demand that takes another check's answer is a demand all the same: someone is
            // here, and the next reads ask quickly again.
            if !by_a_read
                && let Ok(mut answer) = self.github_callback.answer.lock()
                && let Some(answer) = answer.as_mut()
            {
                answer.same = 0;
            }
            return Ok(Some(facts(
                url,
                settings,
                kept.state.clone(),
                Some(kept.at),
            )));
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
            let was = kept.as_ref();
            // Only a sign-in that completed writes while GitHub is asked: it proved the
            // callback, and is the better witness than an answer that was on its way.
            if let Some(proven) = was
                && Some(proven.number) != known.as_ref().map(|known| known.number)
            {
                return Ok(Some(facts(
                    url,
                    settings,
                    proven.state.clone(),
                    Some(proven.at),
                )));
            }
            if was.map(|was| &was.state) != Some(&state) {
                tracing::info!(state = ?state, "GitHub's answer about the sign-in callback");
            }
            *kept = Some(Kept {
                number: was.map_or(1, |was| was.number.wrapping_add(1)),
                // A demand means someone is here: the next reads ask quickly again.
                same: match was {
                    Some(was) if by_a_read && was.says(&state) => was.same.saturating_add(1),
                    _ => 0,
                },
                state: state.clone(),
                at,
            });
        }
        Ok(Some(facts(url, settings, state, Some(at))))
    }

    /// Asks GitHub in the background, unless a background check is already on its way: at start,
    /// when the App was stored and when a read finds the answer due (SPEC-466 MVC-003).
    pub fn start_github_callback_check(self: &Arc<Self>) {
        /// Says that no background check is on its way any more, however it ended.
        struct Done(Arc<App>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0
                    .github_callback
                    .refreshing
                    .store(false, Ordering::SeqCst);
            }
        }
        if self.github_callback.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let done = Done(Arc::clone(self));
        tokio::spawn(async move {
            if let Err(err) = done.0.ask_github(None, REUSE_MS, true).await {
                tracing::warn!(error = %err, "could not ask GitHub about the sign-in callback");
            }
        });
    }

    /// Whether a check a read or the start began is still on its way. It is marked before the
    /// read that began it answers, so a test can tell that a read began none.
    #[must_use]
    pub fn github_callback_check_running(&self) -> bool {
        self.github_callback.refreshing.load(Ordering::SeqCst)
    }

    /// GitHub just exchanged a sign-in code for the instance's callback URL, so it is registered,
    /// whatever was known before: the pages say so at once, without another question.
    pub(crate) fn github_callback_proven(&self) {
        let at = self.now();
        if let Ok(mut kept) = self.github_callback.answer.lock() {
            let was = kept.as_ref();
            if was.map(|was| &was.state) != Some(&CallbackState::Registered) {
                tracing::info!("a GitHub sign-in completed: its callback is registered");
            }
            *kept = Some(Kept {
                state: CallbackState::Registered,
                at,
                number: was.map_or(1, |was| was.number.wrapping_add(1)),
                same: 0,
            });
        }
    }

    /// The kept answer. Reading one that is not "registered" and due ([`Kept::stale`]), or none
    /// at all, asks GitHub again in the background: re-checked while someone looks.
    fn kept_callback(self: &Arc<Self>) -> Option<(CallbackState, Timestamp)> {
        let kept = self.kept_answer();
        if kept.as_ref().is_none_or(|kept| kept.stale(self.now())) {
            self.start_github_callback_check();
        }
        kept.map(|kept| (kept.state, kept.at))
    }

    /// Whether GitHub is known to refuse the instance's callback, for the sign-in page (SPEC-466
    /// MVC-012). Call it only for an instance with a GitHub App.
    #[must_use]
    pub fn github_callback_missing(self: &Arc<Self>) -> bool {
        matches!(self.kept_callback(), Some((CallbackState::Missing, _)))
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
    use yulius_domain::time::Timestamp;

    use super::{CallbackState, Kept, callback_url, public_url_ok, recheck_after, settings_url};

    /// SPEC-466 MVC-003: an answer that is not "registered" is asked for again when it is read
    /// 30 s later, and ever less often once it stayed the same for a while: a page left open
    /// does not make the instance ask GitHub twice a minute for ever.
    #[test]
    fn a_standing_answer_is_asked_for_again_ever_less_often() {
        for same in 0..=10 {
            assert_eq!(recheck_after(same), 30_000, "{same}");
        }
        assert_eq!(recheck_after(11), 60_000);
        assert_eq!(recheck_after(12), 120_000);
        assert_eq!(recheck_after(13), 240_000);
        // Four minutes at most between two questions while someone keeps reading, however
        // long the answer stands.
        assert_eq!(recheck_after(14), 240_000);
        assert_eq!(recheck_after(u32::MAX), 240_000);

        let kept = |state: CallbackState, same| Kept {
            state,
            at: Timestamp(1_000_000),
            number: 1,
            same,
        };
        let at = |ms: i64| Timestamp(1_000_000 + ms);
        for state in [
            CallbackState::Missing,
            CallbackState::Unknown("GitHub: timed out".into()),
        ] {
            assert!(!kept(state.clone(), 0).stale(at(29_999)));
            assert!(kept(state.clone(), 0).stale(at(30_000)));
            assert!(!kept(state.clone(), 11).stale(at(59_999)));
            assert!(kept(state.clone(), 11).stale(at(60_000)));
            assert!(!kept(state.clone(), 400).stale(at(239_999)));
            assert!(kept(state, 400).stale(at(240_000)));
        }
        // An answer GitHub did not give says the same whatever the reason; another answer is news.
        let unknown = |why: &str| CallbackState::Unknown(why.into());
        assert!(kept(unknown("timed out"), 3).says(&unknown("connection failed")));
        assert!(kept(CallbackState::Missing, 3).says(&CallbackState::Missing));
        assert!(!kept(CallbackState::Missing, 3).says(&unknown("timed out")));
        assert!(!kept(CallbackState::Missing, 3).says(&CallbackState::Registered));
        // "Registered" is kept until the next start or demand.
        assert!(!kept(CallbackState::Registered, 0).stale(at(86_400_000)));
        // An answer dated in the future (the clock stepped back) is due, not kept until then.
        assert!(kept(CallbackState::Missing, 0).stale(Timestamp(0)));
        assert!(kept(CallbackState::Missing, 0).stale(at(-1)));
        assert!(!kept(CallbackState::Registered, 0).stale(Timestamp(0)));
    }

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
        for odd in ["..", ".", "a.b", "a_b", ""] {
            assert_eq!(
                settings_url(Some("org"), Some(odd), "yulius"),
                "https://github.com/settings/apps/yulius",
                "{odd:?}"
            );
            assert_eq!(
                settings_url(Some("org"), Some("acme"), odd),
                "https://github.com/settings/apps",
                "{odd:?}"
            );
        }
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
