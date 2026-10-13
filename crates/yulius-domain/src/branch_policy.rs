//! The branch policy Yulius's merge gate enforces (SPEC-069, ADR 0051): which pull requests may
//! merge, by their title, head and base.
//!
//! Into the integration branch: a typed short-lived head (`feat/42-task-cancellation`) and a
//! conventional one-line title of at most [`MAX_TITLE_CHARS`] (`feat(tasks): add cancellation`).
//! Into the production branch: only the integration branch of the same repository, titled
//! `release: promote <integration> to <production>` with an optional ` - <label>`.
//!
//! Choice: the grammar is small, so it is checked by hand instead of through a regex dependency.

/// Conventional types of the standard preset.
pub const STANDARD_TYPES: [&str; 10] = [
    "feat", "fix", "refactor", "perf", "docs", "test", "build", "ci", "chore", "revert",
];
/// Longest pull request title, in characters.
pub const MAX_TITLE_CHARS: usize = 100;
/// Longest head branch name, in characters.
pub const MAX_BRANCH_CHARS: usize = 80;
/// Most types a project may configure.
pub const MAX_TYPES: usize = 20;
/// Longest configured type.
pub const MAX_TYPE_CHARS: usize = 20;

/// A project's branch policy (`yulius.toml` `[branch_policy]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPolicy {
    /// The allowed conventional types; `None` when the policy is off (preset `none`).
    types: Option<Vec<String>>,
}

impl Default for BranchPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

/// The facts of a pull request the policy reads.
#[derive(Debug, Clone, Copy)]
pub struct PullFacts<'a> {
    /// Title.
    pub title: &'a str,
    /// Head branch.
    pub head: &'a str,
    /// Base branch.
    pub base: &'a str,
    /// The head is in another repository (a fork) or its repository is gone.
    pub from_fork: bool,
}

/// Why the policy refuses a pull request; the message states the rule in words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyViolation {
    /// A title with a line break or longer than [`MAX_TITLE_CHARS`].
    #[error(
        "a pull request title is one line of at most {MAX_TITLE_CHARS} characters; this one has {chars} characters{lines}"
    )]
    TitleShape {
        /// Characters in the title.
        chars: usize,
        /// ` on several lines` or empty.
        lines: &'static str,
    },
    /// A base that is neither the integration nor the production branch.
    #[error(
        "pull requests go into `{integration}` (changes) or `{production}` (promotion); this one goes into `{base}`"
    )]
    Base {
        /// The pull request's base.
        base: String,
        /// Integration branch.
        integration: String,
        /// Production branch.
        production: String,
    },
    /// A promotion from another branch or repository.
    #[error(
        "`{production}` accepts only promotions from this repository's `{integration}` branch; this one comes from `{head}`{fork}"
    )]
    PromotionHead {
        /// The pull request's head.
        head: String,
        /// ` in another repository` or empty.
        fork: &'static str,
        /// Integration branch.
        integration: String,
        /// Production branch.
        production: String,
    },
    /// A promotion without the release title.
    #[error(
        "a promotion is titled `release: promote {integration} to {production}`, optionally followed by ` - <label>`; this one is `{title}`"
    )]
    PromotionTitle {
        /// The title.
        title: String,
        /// Integration branch.
        integration: String,
        /// Production branch.
        production: String,
    },
    /// A head that is not a typed short-lived branch.
    #[error(
        "changes come from a typed short-lived branch `<type>/<words>` of at most {MAX_BRANCH_CHARS} characters, like `feat/42-task-cancellation` (lowercase letters and digits in hyphen-separated words, at least one letter; types: {types}); this one is `{head}`"
    )]
    Branch {
        /// The head.
        head: String,
        /// The allowed types, comma-separated.
        types: String,
    },
    /// A title that is not conventional.
    #[error(
        "a pull request title is conventional, `<type>[(scope)][!]: <description>`, like `feat(tasks): add cancellation` (scope in lowercase letters, digits and hyphens; types: {types}); this one is `{title}`"
    )]
    Title {
        /// The title.
        title: String,
        /// The allowed types, comma-separated.
        types: String,
    },
}

impl PolicyViolation {
    /// Whether the title is what is wrong (the fix is to edit it).
    #[must_use]
    pub fn is_title(&self) -> bool {
        matches!(
            self,
            Self::TitleShape { .. } | Self::PromotionTitle { .. } | Self::Title { .. }
        )
    }
}

/// Lowercase letters and digits in hyphen-separated words (`[a-z0-9]+(-[a-z0-9]+)*`).
fn is_words(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether `text` is an acceptable configured type: lowercase ASCII letters, bounded.
#[must_use]
pub fn valid_type(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TYPE_CHARS && text.bytes().all(|b| b.is_ascii_lowercase())
}

impl BranchPolicy {
    /// This repository's rules (the default).
    #[must_use]
    pub fn standard() -> Self {
        Self {
            types: Some(STANDARD_TYPES.iter().map(|t| (*t).to_owned()).collect()),
        }
    }

    /// No checks (preset `none`).
    #[must_use]
    pub fn none() -> Self {
        Self { types: None }
    }

    /// The standard rules with other conventional types; `None` when a type is invalid, the list
    /// is empty or longer than [`MAX_TYPES`].
    #[must_use]
    pub fn with_types(types: Vec<String>) -> Option<Self> {
        (!types.is_empty() && types.len() <= MAX_TYPES && types.iter().all(|t| valid_type(t)))
            .then_some(Self { types: Some(types) })
    }

    /// Whether the policy checks anything.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.types.is_some()
    }

    /// The allowed types (empty when the policy is off).
    #[must_use]
    pub fn types(&self) -> &[String] {
        self.types.as_deref().unwrap_or_default()
    }

    fn allows(&self, kind: &str) -> bool {
        self.types().iter().any(|t| t == kind)
    }

    fn type_list(&self) -> String {
        self.types().join(", ")
    }

    /// The `type(scope)!` head of `title` when it is a conventional title this policy accepts
    /// (one line and length aside); with the policy off, never.
    #[must_use]
    pub fn conventional_head<'t>(&self, title: &'t str) -> Option<&'t str> {
        let (head, description) = title.split_once(": ")?;
        if !description.starts_with(|c: char| !c.is_whitespace()) {
            return None;
        }
        let bare = head.strip_suffix('!').unwrap_or(head);
        let kind = match bare.split_once('(') {
            Some((kind, scope)) => is_words(scope.strip_suffix(')')?).then_some(kind)?,
            None => bare,
        };
        self.allows(kind).then_some(head)
    }

    /// The type of a typed branch name this policy accepts (`feat` of `feat/42-x`).
    #[must_use]
    pub fn branch_type<'b>(&self, branch: &'b str) -> Option<&'b str> {
        let (kind, rest) = branch.split_once('/')?;
        (self.allows(kind)
            && branch.chars().count() <= MAX_BRANCH_CHARS
            && is_words(rest)
            && rest.bytes().any(|b| b.is_ascii_lowercase()))
        .then_some(kind)
    }

    /// Checks a pull request from `pull.head` into `pull.base`.
    ///
    /// # Errors
    /// The first rule the pull request breaks.
    pub fn check(
        &self,
        pull: &PullFacts<'_>,
        integration: &str,
        production: &str,
    ) -> Result<(), PolicyViolation> {
        if !self.is_enforced() {
            return Ok(());
        }
        let chars = pull.title.chars().count();
        let multiline = pull.title.contains(['\n', '\r']);
        if chars > MAX_TITLE_CHARS || multiline {
            return Err(PolicyViolation::TitleShape {
                chars,
                lines: if multiline { " on several lines" } else { "" },
            });
        }
        if pull.base == production {
            if pull.head != integration || pull.from_fork {
                return Err(PolicyViolation::PromotionHead {
                    head: pull.head.to_owned(),
                    fork: if pull.from_fork {
                        " in another repository"
                    } else {
                        ""
                    },
                    integration: integration.to_owned(),
                    production: production.to_owned(),
                });
            }
            let release = format!("release: promote {integration} to {production}");
            let valid = match pull.title.strip_prefix(&release) {
                Some("") => true,
                Some(rest) => rest
                    .strip_prefix(" - ")
                    .is_some_and(|label| label.starts_with(|c: char| !c.is_whitespace())),
                None => false,
            };
            if !valid {
                return Err(PolicyViolation::PromotionTitle {
                    title: pull.title.to_owned(),
                    integration: integration.to_owned(),
                    production: production.to_owned(),
                });
            }
            return Ok(());
        }
        if pull.base != integration {
            return Err(PolicyViolation::Base {
                base: pull.base.to_owned(),
                integration: integration.to_owned(),
                production: production.to_owned(),
            });
        }
        if self.branch_type(pull.head).is_none() {
            return Err(PolicyViolation::Branch {
                head: pull.head.to_owned(),
                types: self.type_list(),
            });
        }
        if self.conventional_head(pull.title).is_none() {
            return Err(PolicyViolation::Title {
                title: pull.title.to_owned(),
                types: self.type_list(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull<'a>(head: &'a str, base: &'a str, title: &'a str) -> PullFacts<'a> {
        PullFacts {
            title,
            head,
            base,
            from_fork: false,
        }
    }

    fn check(facts: &PullFacts<'_>) -> Result<(), PolicyViolation> {
        BranchPolicy::standard().check(facts, "dev", "main")
    }

    const OK_HEAD: &str = "feat/42-task-cancellation";
    const OK_TITLE: &str = "feat(tasks): add cancellation";

    #[test]
    fn work_branch_and_conventional_title_are_accepted() {
        assert_eq!(check(&pull(OK_HEAD, "dev", OK_TITLE)), Ok(()));
        assert_eq!(
            check(&pull(
                "fix/event-replay",
                "dev",
                "fix(events)!: replace protocol"
            )),
            Ok(())
        );
        assert_eq!(check(&pull("docs/1-readme", "dev", "docs: x")), Ok(()));
        let longest = format!("fix: {}", "x".repeat(MAX_TITLE_CHARS - 5));
        assert_eq!(check(&pull(OK_HEAD, "dev", &longest)), Ok(()));
        let branch = format!("feat/{}", "x".repeat(MAX_BRANCH_CHARS - 5));
        assert_eq!(check(&pull(&branch, "dev", OK_TITLE)), Ok(()));
    }

    #[test]
    fn wrong_routes_names_and_titles_are_rejected() {
        let long_branch = format!("feat/{}", "x".repeat(MAX_BRANCH_CHARS));
        let long_title = format!("fix: {}", "x".repeat(MAX_TITLE_CHARS));
        for (head, base, title) in [
            (OK_HEAD, "main", OK_TITLE),
            (OK_HEAD, "prod", OK_TITLE),
            ("main", "dev", OK_TITLE),
            ("dev", "dev", OK_TITLE),
            ("feat/user/nested", "dev", OK_TITLE),
            ("feat/123", "dev", OK_TITLE),
            ("feat/4-123", "dev", OK_TITLE),
            ("feat/Bad_name", "dev", OK_TITLE),
            ("feat/bad--name", "dev", OK_TITLE),
            ("feat/-bad", "dev", OK_TITLE),
            ("wip/thing", "dev", OK_TITLE),
            (long_branch.as_str(), "dev", OK_TITLE),
            (OK_HEAD, "dev", "WIP: fix it"),
            (OK_HEAD, "dev", "Checkout API"),
            (OK_HEAD, "dev", "fix: x\ny"),
            (OK_HEAD, "dev", long_title.as_str()),
            (OK_HEAD, "dev", "fix:  leading space"),
            (OK_HEAD, "dev", "fix: "),
            (OK_HEAD, "dev", "fix(Bad Scope): x"),
            (OK_HEAD, "dev", "fix(): x"),
            (OK_HEAD, "dev", "Fix #223: x"),
            (OK_HEAD, "dev", "wip(x): y"),
        ] {
            assert!(
                check(&pull(head, base, title)).is_err(),
                "{head} -> {base}: {title:?} was accepted"
            );
        }
    }

    #[test]
    fn only_same_repository_dev_can_promote() {
        let release = "release: promote dev to main";
        assert_eq!(check(&pull("dev", "main", release)), Ok(()));
        assert_eq!(
            check(&pull(
                "dev",
                "main",
                "release: promote dev to main - October"
            )),
            Ok(())
        );
        let fork = PullFacts {
            from_fork: true,
            ..pull("dev", "main", release)
        };
        assert!(matches!(
            check(&fork),
            Err(PolicyViolation::PromotionHead {
                fork: " in another repository",
                ..
            })
        ));
        for title in [
            "Promote dev to main",
            "release: promote dev to main -",
            "release: promote dev to main - ",
            "release: promote dev to mainline",
            "feat: promote",
        ] {
            assert!(
                matches!(
                    check(&pull("dev", "main", title)),
                    Err(PolicyViolation::PromotionTitle { .. })
                ),
                "{title:?}"
            );
        }
        assert!(matches!(
            check(&pull(OK_HEAD, "main", release)),
            Err(PolicyViolation::PromotionHead { .. })
        ));
    }

    #[test]
    fn the_branches_of_the_profile_decide_the_routes() {
        let policy = BranchPolicy::standard();
        assert_eq!(
            policy.check(&pull(OK_HEAD, "quartio", OK_TITLE), "quartio", "prod"),
            Ok(())
        );
        assert_eq!(
            policy.check(
                &pull("quartio", "prod", "release: promote quartio to prod"),
                "quartio",
                "prod"
            ),
            Ok(())
        );
        assert!(
            policy
                .check(&pull(OK_HEAD, "dev", OK_TITLE), "quartio", "prod")
                .is_err()
        );
    }

    #[test]
    fn violations_state_the_rule_in_words() {
        let err = check(&pull(OK_HEAD, "dev", "Checkout API")).err();
        let text = err.as_ref().map(ToString::to_string).unwrap_or_default();
        assert!(text.contains("conventional"), "{text}");
        assert!(text.contains("`Checkout API`"), "{text}");
        assert!(err.is_some_and(|e| e.is_title()));
        let text = check(&pull("topic", "dev", OK_TITLE))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(text.contains("typed short-lived branch"), "{text}");
        let text = check(&pull(OK_HEAD, "dev", &"x".repeat(130)))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            text.contains("at most 100 characters; this one has 130"),
            "{text}"
        );
    }

    #[test]
    fn configured_types_replace_the_standard_ones_and_none_checks_nothing() {
        let custom = BranchPolicy::with_types(vec!["story".into(), "bug".into()]);
        let custom = custom.unwrap_or_default();
        assert_eq!(
            custom.check(
                &pull("story/12-login", "dev", "story: log in"),
                "dev",
                "main"
            ),
            Ok(())
        );
        assert!(
            custom
                .check(&pull(OK_HEAD, "dev", OK_TITLE), "dev", "main")
                .is_err()
        );
        assert_eq!(BranchPolicy::with_types(Vec::new()), None);
        assert_eq!(BranchPolicy::with_types(vec!["Feat".into()]), None);
        assert_eq!(BranchPolicy::with_types(vec!["x".repeat(21)]), None);
        let none = BranchPolicy::none();
        assert!(!none.is_enforced());
        assert_eq!(
            none.check(&pull("topic", "prod", "anything\ngoes"), "dev", "main"),
            Ok(())
        );
        assert_eq!(none.conventional_head("feat: x"), None);
    }

    #[test]
    fn heads_and_types_are_read_for_the_title_generator() {
        let policy = BranchPolicy::standard();
        assert_eq!(
            policy.conventional_head("fix(api)!: limit"),
            Some("fix(api)!")
        );
        assert_eq!(policy.conventional_head("Note: not a type"), None);
        assert_eq!(policy.branch_type("docs/1-readme"), Some("docs"));
        assert_eq!(policy.branch_type("topic"), None);
    }
}
