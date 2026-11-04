//! Rollback in CD (SPEC-415, ADR-0263): what a deploy shipped, the releases Yulius keeps per
//! environment and part, which one a rollback goes to and whether it may, the steps of a drill,
//! and the rule that gates production deploys on a passed drill.
//!
//! A deploy names its releases with `::yulius-release [<part>=]<id>` lines and a release that
//! cannot be rolled back past with `::yulius-rollback-barrier [<part>:] <reason>`. A rollback
//! runs the environment's `rollback` command with the release in `YULIUS_CD_RELEASE`, then its
//! `verify`. These functions are pure: the application stores the records and runs the commands.

use std::fmt;

use crate::cd::{CdStage, CdTarget, valid_env_name};
use crate::time::Timestamp;

/// Most parts one environment names.
pub const MAX_PARTS: usize = 8;
/// Longest release id, in characters.
pub const MAX_RELEASE_CHARS: usize = 128;
/// Longest barrier reason kept, in characters.
pub const MAX_BARRIER_CHARS: usize = 300;
/// Distinct releases offered as rollback targets per environment and part.
pub const KEEP_RELEASES: usize = 10;
/// Release records kept per environment, stage and part (deploys and rollbacks).
pub const KEEP_RELEASE_RECORDS: u32 = 20;
/// The drill environment of a production-stage table without `drill`.
pub const DEFAULT_DRILL: &str = "dev";

/// What a CD run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CdRunKind {
    /// The `deploy` then `verify` commands.
    Deploy,
    /// The `rollback` then `verify` commands, to one kept release.
    Rollback,
    /// Yulius's drill after a dev-stage deploy: back to the previous release and forward again.
    Drill,
}

impl CdRunKind {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
            Self::Drill => "drill",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            "drill" => Some(Self::Drill),
            _ => None,
        }
    }
}

impl fmt::Display for CdRunKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `id` can be a release id: 1-128 letters, digits and `._+:@/-`, starting with a letter
/// or a digit (it reaches a command as an environment variable, never spliced into a script).
#[must_use]
pub fn valid_release(id: &str) -> bool {
    id.chars().count() <= MAX_RELEASE_CHARS
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+:@/-".contains(c))
}

/// Whether `name` can name a part: the syntax of an environment's name.
#[must_use]
pub fn valid_part(name: &str) -> bool {
    valid_env_name(name)
}

/// One release a deploy shipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shipped {
    /// The part (empty for an environment of one part).
    pub part: String,
    /// Its release id.
    pub release: String,
    /// Why rolling back past it is unsafe, when the deploy said so.
    pub barrier: Option<String>,
}

/// The releases a passed deploy of an environment with `parts` (empty: one part) shipped at
/// `sha`, read from its output `lines`, and a note for each marker it ignored. A one-part
/// environment whose output names no release shipped its commit. The last marker of a part wins.
#[must_use]
pub fn shipped<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    parts: &[String],
    sha: &str,
) -> (Vec<Shipped>, Vec<String>) {
    let mut out: Vec<Shipped> = Vec::new();
    let mut barriers: Vec<(Option<String>, String)> = Vec::new();
    let mut ignored = Vec::new();
    let known = |part: &str| {
        if parts.is_empty() {
            None
        } else {
            parts.iter().any(|p| p == part).then(|| part.to_owned())
        }
    };
    for line in lines {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("::yulius-release ") {
            let rest = rest.trim();
            let (part, id) = match rest.split_once('=') {
                Some((part, id)) => match known(part.trim()) {
                    Some(part) => (part, id.trim()),
                    None => {
                        ignored.push(format!("a release of an unknown part '{}'", clip(part)));
                        continue;
                    }
                },
                None if parts.is_empty() => (String::new(), rest),
                None => {
                    ignored.push(format!(
                        "a release without its part ({}=<id>)",
                        parts.join("|")
                    ));
                    continue;
                }
            };
            if !valid_release(id) {
                ignored.push(format!("an invalid release id '{}'", clip(id)));
                continue;
            }
            out.retain(|s| s.part != part);
            out.push(Shipped {
                part,
                release: id.to_owned(),
                barrier: None,
            });
        } else if let Some(rest) = line
            .strip_prefix("::yulius-rollback-barrier")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            let rest = rest.trim();
            let (part, reason) = match rest.split_once(':') {
                Some((part, reason)) if known(part.trim()).is_some() => {
                    (known(part.trim()), reason.trim())
                }
                _ => (None, rest),
            };
            if reason.is_empty() {
                ignored.push("a rollback barrier without its reason".to_owned());
                continue;
            }
            let reason: String = reason.chars().take(MAX_BARRIER_CHARS).collect();
            barriers.push((part, reason));
        }
    }
    if parts.is_empty() && out.is_empty() {
        out.push(Shipped {
            part: String::new(),
            release: sha.to_owned(),
            barrier: None,
        });
    }
    for (part, reason) in barriers {
        for shipped in &mut out {
            if part.as_ref().is_none_or(|p| *p == shipped.part) {
                shipped.barrier = Some(reason.clone());
            }
        }
    }
    (out, ignored)
}

fn clip(text: &str) -> String {
    text.chars().take(40).collect()
}

/// How a release record came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseKind {
    /// A passed deploy shipped it.
    Deploy,
    /// A passed rollback went to it.
    Rollback,
}

impl ReleaseKind {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            _ => None,
        }
    }
}

/// One release record of an environment's part (table `cd_releases`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecord {
    /// The part (empty: the environment's only one).
    pub part: String,
    /// Release id.
    pub release: String,
    /// The commit it was deployed from.
    pub sha: String,
    /// Deploy or rollback.
    pub kind: ReleaseKind,
    /// A login, `task:<n>` or `yulius`.
    pub requested_by: String,
    /// When the run that recorded it ended.
    pub at: Timestamp,
    /// Why rolling back past it is unsafe (deploys only).
    pub barrier: Option<String>,
    /// The run that recorded it.
    pub run_id: String,
}

/// The records of one environment's part, newest first.
#[derive(Debug, Clone, Copy)]
pub struct History<'a>(pub &'a [ReleaseRecord]);

impl<'a> History<'a> {
    /// What is deployed now: the newest record.
    #[must_use]
    pub fn current(&self) -> Option<&'a ReleaseRecord> {
        self.0.first()
    }

    fn deploys(&self) -> impl Iterator<Item = &'a ReleaseRecord> {
        self.0.iter().filter(|r| r.kind == ReleaseKind::Deploy)
    }

    /// The releases deploys shipped, each once (its newest deploy), newest first, at most
    /// [`KEEP_RELEASES`]: what a rollback may go to.
    #[must_use]
    pub fn releases(&self) -> Vec<&'a ReleaseRecord> {
        let mut out: Vec<&ReleaseRecord> = Vec::new();
        for record in self.deploys() {
            if out.len() == KEEP_RELEASES {
                break;
            }
            if !out.iter().any(|r| r.release == record.release) {
                out.push(record);
            }
        }
        out
    }

    /// Where the current release's newest deploy stands among the deploys (0: the newest);
    /// `None` when there is no current release or its deploy is not kept.
    fn current_position(&self) -> Option<usize> {
        let current = self.current()?;
        self.deploys().position(|r| r.release == current.release)
    }

    /// The release before the current one: the newest deploy after the current release's deploy
    /// that shipped another release, so repeated rollbacks walk back (RBK-013).
    #[must_use]
    pub fn previous(&self) -> Option<&'a ReleaseRecord> {
        let current = self.current()?;
        let from = self.current_position()?;
        let previous = self
            .deploys()
            .skip(from)
            .find(|r| r.release != current.release)?;
        self.releases()
            .into_iter()
            .find(|r| r.release == previous.release)
    }

    /// The kept release `id`.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&'a ReleaseRecord> {
        self.releases().into_iter().find(|r| r.release == id)
    }

    /// Why going to `target` is refused: a barrier between it and the current release, when
    /// `target` is older (RBK-022). Rolling forward is never blocked.
    #[must_use]
    pub fn blocked(&self, target: &str) -> Option<String> {
        // Without the current release's deploy, every barrier down to the target counts.
        let from = self.current_position().unwrap_or(0);
        let to = self.deploys().position(|r| r.release == target)?;
        if to <= from {
            return None;
        }
        self.deploys().skip(from).take(to - from).find_map(|r| {
            r.barrier.as_ref().map(|reason| {
                format!(
                    "rolling back past {} is blocked: {reason}",
                    short_release(&r.release)
                )
            })
        })
    }
}

/// A release id as people read it: a commit shortened to 7, else as it is.
#[must_use]
pub fn short_release(id: &str) -> &str {
    if id.len() == 40 && id.chars().all(|c| c.is_ascii_hexdigit()) {
        &id[..7]
    } else {
        id
    }
}

/// One step of a rollback or a drill: go to `release` of `part`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackStep {
    /// The part (empty: the environment's only one).
    pub part: String,
    /// The release id.
    pub release: String,
    /// The commit it was deployed from.
    pub sha: String,
}

impl RollbackStep {
    /// A step to `record`.
    #[must_use]
    pub fn to(record: &ReleaseRecord) -> Self {
        Self {
            part: record.part.clone(),
            release: record.release.clone(),
            sha: record.sha.clone(),
        }
    }
}

/// The drill's steps for one part (RBK-030): back to the release before the one it runs now and
/// forward to that one again; when a barrier forbids going back (on any deploy of the release it
/// runs, review H2), only that release again; nothing without a previous release. Every part is
/// drilled, shipped by the deploy or not (review M2): the drill proves the scripts at its commit.
#[must_use]
pub fn drill_steps(history: History<'_>) -> Vec<RollbackStep> {
    let (Some(now), Some(previous)) = (history.current(), history.previous()) else {
        return Vec::new();
    };
    if history.blocked(&previous.release).is_some() {
        return vec![RollbackStep::to(now)];
    }
    vec![RollbackStep::to(previous), RollbackStep::to(now)]
}

/// The barrier an earlier deploy of `release` of `part` printed, among `records`: a release
/// deployed again keeps it (its migration ran the first time), so retention, which keeps a
/// release's newest deploy, never drops the barrier in front of an older release it keeps
/// (review round 2).
#[must_use]
pub fn earlier_barrier(records: &[ReleaseRecord], part: &str, release: &str) -> Option<String> {
    records
        .iter()
        .filter(|r| r.kind == ReleaseKind::Deploy && r.part == part && r.release == release)
        .find_map(|r| r.barrier.clone())
}

/// Which of one part's records, newest first as `(kind, release)`, retention keeps (RBK-012,
/// review M3): the newest [`KEEP_RELEASE_RECORDS`], the newest deploy of each of the newest
/// [`KEEP_RELEASES`] distinct releases deployed, and the newest deploy of the release run now, so
/// rollbacks and repeated deploys never push out what a rollback goes to.
#[must_use]
pub fn kept(records: &[(ReleaseKind, &str)]) -> Vec<bool> {
    let newest = usize::try_from(KEEP_RELEASE_RECORDS).unwrap_or(usize::MAX);
    let mut keep: Vec<bool> = (0..records.len()).map(|i| i < newest).collect();
    let mut releases: Vec<&str> = Vec::new();
    for (i, (kind, release)) in records.iter().enumerate() {
        if *kind == ReleaseKind::Deploy
            && releases.len() < KEEP_RELEASES
            && !releases.contains(release)
        {
            releases.push(release);
            keep[i] = true;
        }
    }
    if let Some((_, current)) = records.first()
        && let Some(i) = records
            .iter()
            .position(|(kind, release)| *kind == ReleaseKind::Deploy && release == current)
    {
        keep[i] = true;
    }
    keep
}

/// The newest finished drill of the environment that proves a production one, as the gate reads
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrillSeen {
    /// It passed.
    pub passed: bool,
    /// The commit it drilled.
    pub sha: String,
    /// That commit has the tree of the commit being deployed.
    pub same_tree: bool,
}

/// Why a production deploy is refused for its rollback (RBK-031); `None`: it may go on. Only a
/// production-stage table with a deploy command is gated (RBK-032).
#[must_use]
pub fn production_gate(target: &CdTarget, drill: Option<&DrillSeen>) -> Option<String> {
    if target.stage != CdStage::Prod || target.deploy.is_none() {
        return None;
    }
    let name = &target.name;
    if target.rollback.is_none() {
        return Some(format!(
            "rollback is not configured: production deploys need a proven rollback; add \
             `rollback` (a command redeploying the release in $YULIUS_CD_RELEASE) and \
             `migrations` to [cd.{name}] and [cd.{}] of yulius.toml",
            target.drill_env()
        ));
    }
    let drill_env = target.drill_env();
    let again =
        format!("deploy {drill_env} at this commit first: its rollback drill follows by itself");
    match drill {
        None => Some(format!(
            "rollback is not proven yet: no rollback drill of {drill_env} has run ({drill_env} \
             needs [cd.{drill_env}] rollback and two deploys); {again}"
        )),
        Some(seen) if !seen.passed => Some(format!(
            "rollback is not proven: the last rollback drill of {drill_env} (at {}) did not pass; \
             fix it, then {again}",
            short_release(&seen.sha)
        )),
        Some(seen) if !seen.same_tree => Some(format!(
            "rollback is not proven for this commit: the last rollback drill of {drill_env} ran \
             at {}; {again}",
            short_release(&seen.sha)
        )),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(release: &str, kind: ReleaseKind, barrier: Option<&str>) -> ReleaseRecord {
        ReleaseRecord {
            part: String::new(),
            release: release.into(),
            sha: format!("sha-{release}"),
            kind,
            requested_by: "alice".into(),
            at: Timestamp(0),
            barrier: barrier.map(str::to_owned),
            run_id: format!("run-{release}"),
        }
    }

    fn deploy(release: &str) -> ReleaseRecord {
        record(release, ReleaseKind::Deploy, None)
    }

    fn rolled(release: &str) -> ReleaseRecord {
        record(release, ReleaseKind::Rollback, None)
    }

    #[test]
    fn release_ids_are_plain_words() {
        for good in [
            "v1.4.2",
            "1.4.2+build.7",
            "registry/app@sha256:ab12",
            &"a".repeat(128),
        ] {
            assert!(valid_release(good), "{good}");
        }
        for bad in [
            "",
            "-x",
            "a b",
            "a;b",
            "$(id)",
            "a\"b",
            &"a".repeat(129),
            "ünï",
        ] {
            assert!(!valid_release(bad), "{bad}");
        }
    }

    /// RBK-010/011: markers name releases per part; a one-part deploy without one ships its commit.
    #[test]
    fn a_deploy_names_what_it_shipped() {
        let sha = "c".repeat(40);
        let (one, ignored) = shipped(["building", "done"], &[], &sha);
        assert_eq!(
            one,
            vec![Shipped {
                part: String::new(),
                release: sha.clone(),
                barrier: None
            }]
        );
        assert!(ignored.is_empty());

        let (one, _) = shipped(
            [
                "::yulius-release v1",
                "  ::yulius-release v2  ",
                "::yulius-rollback-barrier dropped column x",
            ],
            &[],
            &sha,
        );
        assert_eq!(
            one,
            vec![Shipped {
                part: String::new(),
                release: "v2".into(),
                barrier: Some("dropped column x".into())
            }],
            "the last marker wins; a barrier marks it"
        );

        let parts = vec!["engine".to_owned(), "infra".to_owned()];
        let (two, ignored) = shipped(
            [
                "::yulius-release engine=1.4.2",
                "::yulius-release infra=infra-77",
                "::yulius-release web=1",
                "::yulius-release 9",
                "::yulius-release engine=bad id",
                "::yulius-rollback-barrier infra: users.email is now NOT NULL",
            ],
            &parts,
            &sha,
        );
        assert_eq!(
            two,
            vec![
                Shipped {
                    part: "engine".into(),
                    release: "1.4.2".into(),
                    barrier: None
                },
                Shipped {
                    part: "infra".into(),
                    release: "infra-77".into(),
                    barrier: Some("users.email is now NOT NULL".into())
                },
            ]
        );
        assert_eq!(ignored.len(), 3, "{ignored:?}");
        let (none, _) = shipped(["nothing changed"], &parts, &sha);
        assert!(
            none.is_empty(),
            "a part deploy that shipped nothing records nothing"
        );
        let (all, _) = shipped(
            [
                "::yulius-release engine=2",
                "::yulius-release infra=3",
                "::yulius-rollback-barrier schema v9",
            ],
            &parts,
            &sha,
        );
        assert!(
            all.iter()
                .all(|s| s.barrier.as_deref() == Some("schema v9")),
            "a barrier without a part marks every part"
        );
        let (empty, ignored) = shipped(["::yulius-rollback-barrier   "], &[], &sha);
        assert_eq!(empty[0].barrier, None);
        assert_eq!(ignored.len(), 1);
    }

    /// RBK-012/013: the current release, the kept releases and the previous one, which walks back
    /// over repeated rollbacks.
    #[test]
    fn previous_walks_back_over_rollbacks() {
        let records = [deploy("v3"), deploy("v2"), deploy("v2"), deploy("v1")];
        let history = History(&records);
        assert_eq!(history.current().map(|r| r.release.as_str()), Some("v3"));
        let kept: Vec<&str> = history
            .releases()
            .iter()
            .map(|r| r.release.as_str())
            .collect();
        assert_eq!(kept, ["v3", "v2", "v1"], "each release once");
        assert_eq!(history.previous().map(|r| r.release.as_str()), Some("v2"));

        let records = [rolled("v2"), deploy("v3"), deploy("v2"), deploy("v1")];
        let history = History(&records);
        assert_eq!(history.current().map(|r| r.release.as_str()), Some("v2"));
        assert_eq!(
            history.previous().map(|r| r.release.as_str()),
            Some("v1"),
            "not back to the broken v3"
        );

        let records = [
            rolled("v1"),
            rolled("v2"),
            deploy("v3"),
            deploy("v2"),
            deploy("v1"),
        ];
        assert_eq!(
            History(&records).previous(),
            None,
            "nothing before the oldest"
        );
        assert_eq!(History(&[]).previous(), None);
        let many: Vec<ReleaseRecord> = (0..15).map(|i| deploy(&format!("v{i}"))).collect();
        assert_eq!(History(&many).releases().len(), KEEP_RELEASES);
    }

    /// RBK-022: a barrier blocks going back past it, never forward.
    #[test]
    fn a_barrier_blocks_only_going_back_past_it() {
        let records = [
            deploy("v4"),
            record(
                "v3",
                ReleaseKind::Deploy,
                Some("orders.total split into two columns"),
            ),
            deploy("v2"),
            deploy("v1"),
        ];
        let history = History(&records);
        assert_eq!(
            history.blocked("v3"),
            None,
            "back to the barrier's own release is safe"
        );
        let blocked = history.blocked("v2").unwrap_or_default();
        assert!(
            blocked.contains("past v3") && blocked.contains("two columns"),
            "{blocked}"
        );
        assert!(history.blocked("v1").is_some());
        assert_eq!(history.blocked("v4"), None);
        let back = [rolled("v3"), deploy("v4"), records[1].clone(), deploy("v2")];
        assert_eq!(History(&back).blocked("v4"), None, "forward again");
        assert!(History(&back).blocked("v2").is_some());
        assert_eq!(
            history.blocked("v9"),
            None,
            "unknown targets are refused elsewhere"
        );
    }

    /// RBK-030: the drill goes back and forward; behind a barrier it only redeploys the new one.
    #[test]
    fn drill_steps_go_back_and_forward() {
        let records = [deploy("v2"), deploy("v1")];
        let steps: Vec<String> = drill_steps(History(&records))
            .into_iter()
            .map(|s| s.release)
            .collect();
        assert_eq!(steps, ["v1", "v2"]);
        let first = [deploy("v1")];
        assert!(
            drill_steps(History(&first)).is_empty(),
            "nothing to go back to"
        );
        let barrier = [record("v2", ReleaseKind::Deploy, Some("x")), deploy("v1")];
        let steps: Vec<String> = drill_steps(History(&barrier))
            .into_iter()
            .map(|s| s.release)
            .collect();
        assert_eq!(steps, ["v2"]);
        let after_rollback = [rolled("v1"), deploy("v2"), deploy("v1")];
        assert!(
            drill_steps(History(&after_rollback)).is_empty(),
            "nothing before the release run now"
        );
    }

    fn prod(deploy: Option<&str>, rollback: Option<&str>) -> CdTarget {
        CdTarget {
            deploy: deploy.map(str::to_owned),
            rollback: rollback.map(str::to_owned),
            migrations: rollback.map(|_| "expand and contract".to_owned()),
            ..CdTarget::new("prod", CdStage::Prod)
        }
    }

    /// RBK-031/032: production needs rollback configured and a passed drill of its tree.
    #[test]
    fn production_waits_for_a_proven_rollback() {
        let seen = |passed, same_tree| DrillSeen {
            passed,
            sha: "a".repeat(40),
            same_tree,
        };
        let unconfigured = production_gate(&prod(Some("deploy.sh"), None), Some(&seen(true, true)));
        assert!(
            unconfigured.is_some_and(|why| why.contains("not configured")
                && why.contains("[cd.prod]")
                && why.contains("[cd.dev]"))
        );
        let target = prod(Some("deploy.sh"), Some("rollback.sh"));
        assert!(
            production_gate(&target, None)
                .is_some_and(|why| why.contains("no rollback drill of dev"))
        );
        assert!(
            production_gate(&target, Some(&seen(false, true)))
                .is_some_and(|why| why.contains("did not pass"))
        );
        assert!(
            production_gate(&target, Some(&seen(true, false)))
                .is_some_and(|why| why.contains("ran at aaaaaaa"))
        );
        assert_eq!(production_gate(&target, Some(&seen(true, true))), None);
        assert_eq!(
            production_gate(&prod(None, None), None),
            None,
            "promotion only deploys nothing of Yulius's"
        );
        let dev = CdTarget {
            deploy: Some("d".into()),
            ..CdTarget::new("dev", CdStage::Dev)
        };
        assert_eq!(production_gate(&dev, None), None);
        let named = CdTarget {
            drill: Some("staging".into()),
            ..target
        };
        assert!(production_gate(&named, None).is_some_and(|why| why.contains("drill of staging")));
    }

    /// Review H2: a release deployed again keeps the barrier of its earlier deploy; the drill
    /// does not go back past it.
    #[test]
    fn a_barrier_on_an_earlier_deploy_of_the_same_release_holds() {
        let records = [
            deploy("v3"),
            record("v3", ReleaseKind::Deploy, Some("dropped column x")),
            deploy("v2"),
        ];
        let history = History(&records);
        assert!(history.blocked("v2").is_some());
        let steps: Vec<String> = drill_steps(history)
            .into_iter()
            .map(|s| s.release)
            .collect();
        assert_eq!(steps, ["v3"], "only the release deployed now again");
    }

    /// Review M2: a part this deploy did not ship is drilled from what it runs now.
    #[test]
    fn a_part_not_shipped_now_is_drilled_too() {
        let records = [rolled("e2"), deploy("e3"), deploy("e2"), deploy("e1")];
        let steps: Vec<String> = drill_steps(History(&records))
            .into_iter()
            .map(|s| s.release)
            .collect();
        assert_eq!(
            steps,
            ["e1", "e2"],
            "back to the release before e2, forward to e2"
        );
    }

    /// Review M3: without the current release's deploy there is no previous release.
    #[test]
    fn a_current_release_without_its_deploy_has_no_previous() {
        let records = [rolled("v9"), deploy("v2"), deploy("v1")];
        assert_eq!(History(&records).previous(), None);
    }

    /// Review M3: retention keeps the newest records and the deploys of the newest distinct
    /// releases and of the current one, whatever rollbacks and repeated deploys came after.
    #[test]
    fn retention_keeps_the_releases_a_rollback_needs() {
        let mut records: Vec<(ReleaseKind, String)> = Vec::new();
        for _ in 0..30 {
            records.push((ReleaseKind::Rollback, "v1".into()));
        }
        for n in (1..=15).rev() {
            records.push((ReleaseKind::Deploy, format!("v{n}")));
            records.push((ReleaseKind::Deploy, format!("v{n}")));
        }
        let view: Vec<(ReleaseKind, &str)> =
            records.iter().map(|(k, r)| (*k, r.as_str())).collect();
        let kept = kept(&view);
        let kept_deploys: Vec<&str> = view
            .iter()
            .zip(&kept)
            .filter(|((kind, _), keep)| **keep && *kind == ReleaseKind::Deploy)
            .map(|((_, release), _)| *release)
            .collect();
        assert_eq!(
            kept_deploys,
            [
                "v15", "v14", "v13", "v12", "v11", "v10", "v9", "v8", "v7", "v6", "v1"
            ],
            "the newest deploy of the 10 newest releases and of the current one"
        );
        assert_eq!(
            kept.iter().take(20).filter(|k| **k).count(),
            20,
            "the newest 20"
        );
        assert_eq!(kept.iter().filter(|k| **k).count(), 31);
    }

    /// Review round 2: a release deployed again keeps the barrier of its first deploy, so
    /// retention never keeps an older release while dropping the barrier in front of it.
    #[test]
    fn a_barrier_survives_redeploys_and_retention() {
        let mut records: Vec<ReleaseRecord> = vec![deploy("v2")];
        let mut add = |records: &mut Vec<ReleaseRecord>, mut new: ReleaseRecord| {
            if new.barrier.is_none() {
                new.barrier = earlier_barrier(records, &new.part, &new.release);
            }
            records.insert(0, new);
            let view: Vec<(ReleaseKind, &str)> = records
                .iter()
                .map(|r| (r.kind, r.release.as_str()))
                .collect();
            let keep = kept(&view);
            let mut i = 0;
            records.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        };
        add(
            &mut records,
            record("v3", ReleaseKind::Deploy, Some("dropped column x")),
        );
        for _ in 0..20 {
            add(&mut records, deploy("v3"));
        }
        let history = History(&records);
        assert!(history.find("v2").is_some(), "v2 is still kept");
        assert!(
            history
                .blocked("v2")
                .is_some_and(|why| why.contains("dropped column x")),
            "{records:?}"
        );
        assert_eq!(earlier_barrier(&records, "", "v9"), None);
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [CdRunKind::Deploy, CdRunKind::Rollback, CdRunKind::Drill] {
            assert_eq!(CdRunKind::parse(kind.as_str()), Some(kind));
        }
        for kind in [ReleaseKind::Deploy, ReleaseKind::Rollback] {
            assert_eq!(ReleaseKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(short_release(&"ab".repeat(20)), "abababa");
        assert_eq!(short_release("v1.2"), "v1.2");
    }
}
