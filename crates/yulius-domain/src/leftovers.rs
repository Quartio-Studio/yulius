//! Leftovers (SPEC-215, ADR-0170): paths under the data directory that no record owns, or whose
//! owner's state says they are disposable. Each kind has a rule (in the application) and a
//! policy here: whether the automatic rounds may delete it, or only a person.
//!
//! Pure: the caller scans, checks liveness and deletes.

/// A removed sandbox's disposable parts are kept this long after the removal: its tasks are
/// stopping (about 20 s), its holder retires after 60 s (SPEC-206), shells get their grace.
pub const REMOVED_KEEP_MS: i64 = 10 * 60 * 1000;
/// A run directory untouched this long belongs to no run: longer than any CI job or CD deploy
/// may take, and an agent run's directory is kept while its attempt is live whatever its age.
pub const ENDED_RUN_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
/// A CI checkout whose job no run of its sandbox used for this long belongs to a job (or lane)
/// that no longer exists.
pub const STALE_CHECKOUT_AFTER_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// A draft attachment older than this was never sent (the composer prunes the same age).
pub const DRAFT_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The automatic round's period after the one at start.
pub const AUTO_EVERY_MS: i64 = 60 * 60 * 1000;

/// What a leftover is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A removed sandbox's worktrees.
    RemovedWorktrees,
    /// A removed sandbox's CI checkouts, caches and logs.
    RemovedCi,
    /// A removed sandbox's browser profile.
    RemovedBrowser,
    /// A removed sandbox's CD sources and caches (`cd/<sid>`).
    RemovedCd,
    /// A removed sandbox's virtual display files (`displays/<sid>`).
    RemovedDisplay,
    /// A removed sandbox's system layer (`sysroots/<sid>`: packages, Docker storage).
    RemovedSystem,
    /// A removed sandbox's holder state (`runtime/sandboxes/<sid>`).
    RemovedRuntime,
    /// Everything else of a removed sandbox: clone, home, sessions, attachments, transcripts,
    /// playground origin and export (kept by PRJ-009 unless a person deletes them).
    RemovedSandbox,
    /// A rebuildable cache of a task that ended or is done (SPEC-008, LFT-005).
    TaskCache,
    /// A directory under `worktrees/` that no task owns.
    OrphanWorktree,
    /// Git's record of a worktree whose directory is gone (`.git/worktrees/<name>`).
    GitMetadata,
    /// A CI checkout of a job or lane no run used for [`STALE_CHECKOUT_AFTER_MS`].
    StaleCiCheckout,
    /// A draft attachment older than [`DRAFT_MAX_AGE_MS`].
    OldDraft,
    /// A run directory (`runs/<name>`) of no live run.
    EndedRun,
    /// Transcripts of a task that no longer exists.
    OrphanTranscripts,
    /// A Docker smoke test's sandbox whose process is gone.
    StaleSmoke,
    /// A sandbox-named directory with no sandbox record.
    Unowned,
    /// A base of sandboxes' systems that is not current and that no live holder mounts.
    UnusedBase,
    /// The host's own Docker data (docker-ce), which Yulius does not use: reported only.
    HostDocker,
}

impl Kind {
    /// Every kind, in the order the list shows them.
    pub const ALL: [Self; 19] = [
        Self::RemovedWorktrees,
        Self::RemovedCi,
        Self::RemovedBrowser,
        Self::RemovedCd,
        Self::RemovedDisplay,
        Self::RemovedSystem,
        Self::RemovedRuntime,
        Self::RemovedSandbox,
        Self::TaskCache,
        Self::OrphanWorktree,
        Self::GitMetadata,
        Self::StaleCiCheckout,
        Self::OldDraft,
        Self::EndedRun,
        Self::OrphanTranscripts,
        Self::StaleSmoke,
        Self::Unowned,
        Self::UnusedBase,
        Self::HostDocker,
    ];

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RemovedWorktrees => "removed-worktrees",
            Self::RemovedCi => "removed-ci",
            Self::RemovedBrowser => "removed-browser",
            Self::RemovedCd => "removed-cd",
            Self::RemovedDisplay => "removed-display",
            Self::RemovedSystem => "removed-system",
            Self::RemovedRuntime => "removed-runtime",
            Self::RemovedSandbox => "removed-sandbox",
            Self::TaskCache => "task-cache",
            Self::OrphanWorktree => "orphan-worktree",
            Self::GitMetadata => "git-metadata",
            Self::StaleCiCheckout => "stale-ci-checkout",
            Self::OldDraft => "old-draft",
            Self::EndedRun => "ended-run",
            Self::OrphanTranscripts => "orphan-transcripts",
            Self::StaleSmoke => "stale-smoke",
            Self::Unowned => "unowned",
            Self::UnusedBase => "unused-base",
            Self::HostDocker => "host-docker",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The automatic rounds may delete it once its rule holds. Others wait for a person: they
    /// may hold work (a worktree no task owns), PRJ-009 keeps them (everything of a removed
    /// sandbox), or nothing tells whose they are.
    #[must_use]
    pub const fn auto(self) -> bool {
        !matches!(
            self,
            Self::RemovedSandbox | Self::OrphanWorktree | Self::Unowned | Self::HostDocker
        )
    }

    /// Yulius may delete it at all; the host's Docker data is only reported.
    #[must_use]
    pub const fn deletable(self) -> bool {
        !matches!(self, Self::HostDocker)
    }

    /// Its files may belong to a sandbox's mapped ids: the isolation removes them.
    #[must_use]
    pub const fn mapped(self) -> bool {
        matches!(
            self,
            Self::RemovedSystem | Self::RemovedRuntime | Self::StaleSmoke | Self::UnusedBase
        )
    }
}

/// A stable identifier of the leftover at `path` (relative to the data directory, or absolute
/// outside it): 64-bit FNV-1a in hex, the same across scans and processes.
#[must_use]
pub fn item_id(path: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether a removed sandbox's disposable parts may go: removed at `removed_ms`, now `now_ms`.
#[must_use]
pub fn removed_long_enough(removed_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(removed_ms) >= REMOVED_KEEP_MS
}

/// The process id of a Docker smoke test's sandbox name (`docker-smoke-<pid>`).
#[must_use]
pub fn smoke_pid(name: &str) -> Option<u32> {
    name.strip_prefix("docker-smoke-")?.parse().ok()
}

/// The job key of a CI checkout's directory name (`<key>[.<n>][@<lane>]`): the definition's
/// name, a matrix position and a lane past the first.
#[must_use]
pub fn checkout_job_key(name: &str) -> &str {
    let name = name.split_once('@').map_or(name, |(key, _)| key);
    match name.rsplit_once('.') {
        Some((key, index)) if !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) => key,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_only_listed_ones_wait_for_a_person() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("nope"), None);
        let manual: Vec<Kind> = Kind::ALL.into_iter().filter(|k| !k.auto()).collect();
        assert_eq!(
            manual,
            [
                Kind::RemovedSandbox,
                Kind::OrphanWorktree,
                Kind::Unowned,
                Kind::HostDocker
            ]
        );
        assert!(!Kind::HostDocker.deletable());
        assert!(Kind::RemovedSystem.mapped() && !Kind::RemovedWorktrees.mapped());
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(item_id("sysroots/s1"), item_id("sysroots/s1"));
        assert_ne!(item_id("sysroots/s1"), item_id("sysroots/s2"));
        assert_eq!(item_id("a").len(), 16);
    }

    #[test]
    fn a_removed_sandbox_waits_ten_minutes() {
        assert!(!removed_long_enough(0, REMOVED_KEEP_MS - 1));
        assert!(removed_long_enough(0, REMOVED_KEEP_MS));
    }

    #[test]
    fn smoke_names_and_checkout_keys_are_read() {
        assert_eq!(smoke_pid("docker-smoke-4242"), Some(4242));
        assert_eq!(smoke_pid("docker-smoke-x"), None);
        assert_eq!(smoke_pid("s1"), None);
        assert_eq!(checkout_job_key("gate"), "gate");
        assert_eq!(checkout_job_key("gate.2"), "gate");
        assert_eq!(checkout_job_key("gate@1"), "gate");
        assert_eq!(checkout_job_key("gate.2@1"), "gate");
        assert_eq!(checkout_job_key("node.js"), "node.js");
        assert_eq!(checkout_job_key("v1.2"), "v1");
    }
}
