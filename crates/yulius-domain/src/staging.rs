//! Merge-first staging (SPEC-575 MFS-003 to MFS-008): the requests to change a project's shared
//! staging target, decided purely.
//!
//! - **Admission**: a request names a revision of the integration branch. A task gets at most
//!   one successful staging deployment per environment; a repeated request answers with the one
//!   already recorded.
//! - **Verdict**: a request waits for the full gate's verdict on exactly its revision.
//! - **Pick**: staging only moves forward along the integration branch's first-parent history.
//!   Yulius deploys the newest ready revision above the last verified one, never passes over an
//!   older request still testing, and counts each older request included in that deploy.
//! - **Outcome**: a passed deploy (deploy and verify) is verified. A failed one stays with its
//!   task, and a lost one is retried a bounded number of times.
//!
//! Revisions are ordered by their *position*: the number of first-parent commits of the
//! integration branch up to and including them, computed by the caller. A revision without a
//! position is not on that history and is never deployed.

use crate::ci::RunState;

/// Most deploy attempts of one request: a run lost to a restart or an infrastructure error is
/// tried again until this many attempts ended (SPEC-575 MFS-008).
pub const MAX_DEPLOY_ATTEMPTS: u32 = 3;

/// Where a staging request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StagingState {
    /// Waiting for the full gate's verdict on its revision.
    Testing,
    /// Its failing tests belong to earlier revisions. A verified descendant serves it.
    Inherited,
    /// Its full gate passed: waiting for its turn on staging.
    Ready,
    /// Its revision is being deployed and verified.
    Deploying,
    /// Its revision was deployed and verified on staging.
    Verified,
    /// A verified deploy of a later revision contains it.
    Included,
    /// The full gate failed on its revision: staging did not change.
    GateFailed,
    /// Its deploy or verify failed (or every attempt was lost).
    DeployFailed,
    /// A newer revision of the same task replaced it before it deployed.
    Superseded,
}

impl StagingState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 9] = [
        Self::Testing,
        Self::Inherited,
        Self::Ready,
        Self::Deploying,
        Self::Verified,
        Self::Included,
        Self::GateFailed,
        Self::DeployFailed,
        Self::Superseded,
    ];

    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Testing => "testing",
            Self::Inherited => "inherited",
            Self::Ready => "ready",
            Self::Deploying => "deploying",
            Self::Verified => "verified",
            Self::Included => "included",
            Self::GateFailed => "gate_failed",
            Self::DeployFailed => "deploy_failed",
            Self::Superseded => "superseded",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }

    /// The work Yulius owns until the request's outcome reaches its idle agent.
    #[must_use]
    pub fn task_wait_label(self) -> &'static str {
        match self {
            Self::Testing => "Waiting for the merged revision’s full suite",
            Self::Inherited => "Waiting for staging to verify the fix",
            Self::Ready => "Waiting for the staging deployment to start",
            Self::Deploying => "Waiting for staging to deploy and verify",
            Self::Verified
            | Self::Included
            | Self::GateFailed
            | Self::DeployFailed
            | Self::Superseded => "Waiting for the staging outcome to reach the agent",
        }
    }

    /// Still on its way to staging.
    #[must_use]
    pub fn is_pending(self) -> bool {
        matches!(
            self,
            Self::Testing | Self::Inherited | Self::Ready | Self::Deploying
        )
    }

    /// Its revision is on staging, deployed and verified (its own deploy or a later one).
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Verified | Self::Included)
    }
}

/// A request as the decisions below see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// Its id.
    pub id: String,
    /// The task it delivers (`None`: a person's request for the integration head).
    pub task: Option<String>,
    /// The revision (commit SHA) it deploys.
    pub revision: String,
    /// Its state.
    pub state: StagingState,
    /// Its revision's first-parent position on the integration branch (`None`: not on it).
    pub position: Option<u64>,
    /// Deploy attempts ended so far.
    pub attempts: u32,
}

/// What a new request for a revision does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// The same request already exists: answer with it, record nothing.
    Existing(String),
    /// The same revision's deploy failed: that request is ready again for one more deploy.
    Retry(String),
    /// Refused, with why.
    Refused(String),
    /// A new request in `state`, superseding these older requests of the same task.
    New {
        /// `Testing`, or `Included` when staging already serves a verified revision containing
        /// it.
        state: StagingState,
        /// Pending requests of the same task for other revisions, replaced by this one.
        supersede: Vec<String>,
    },
}

/// Decides a request for `revision` given the requests of the same requester (`own`: the task's,
/// or a person's requests for the same revision) and whether a verified deploy of this
/// environment already contains it (`contained`; also deploys from before merge-first)
/// (SPEC-575 MFS-005).
#[must_use]
pub fn admit(revision: &str, own: &[Facts], contained: bool) -> Admission {
    if let Some(same) = own
        .iter()
        .find(|r| r.revision == revision && r.state != StagingState::Superseded)
    {
        return match same.state {
            StagingState::DeployFailed => Admission::Retry(same.id.clone()),
            _ => Admission::Existing(same.id.clone()),
        };
    }
    if let Some(done) = own.iter().find(|r| r.state.is_success()) {
        return Admission::Refused(format!(
            "this task already has its staging deployment: revision {} is {} there; a later \
             change reaches staging with a task of its own",
            short(&done.revision),
            done.state.as_str()
        ));
    }
    if let Some(busy) = own.iter().find(|r| r.state == StagingState::Deploying) {
        return Admission::Refused(format!(
            "revision {} of this task is being deployed: wait for its outcome (yulius wait), \
             then ask again if it failed",
            short(&busy.revision)
        ));
    }
    Admission::New {
        state: if contained {
            StagingState::Included
        } else {
            StagingState::Testing
        },
        supersede: own
            .iter()
            .filter(|r| {
                matches!(
                    r.state,
                    StagingState::Testing | StagingState::Inherited | StagingState::Ready
                )
            })
            .map(|r| r.id.clone())
            .collect(),
    }
}

/// The state a testing request moves to on the full gate's newest verdict at its revision, or
/// `None` while it has none (no run, queued, running or cancelled) (SPEC-575 MFS-003, MFS-007).
#[must_use]
pub fn after_verdict(verdict: Option<RunState>) -> Option<StagingState> {
    match verdict? {
        RunState::Passed => Some(StagingState::Ready),
        RunState::Failed | RunState::Error => Some(StagingState::GateFailed),
        RunState::Queued | RunState::Running | RunState::Cancelled | RunState::Skipped => None,
    }
}

/// What the staging queue does next.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pick {
    /// Pending requests whose revision the verified staging revision already contains: included
    /// now, with nothing deployed.
    pub included: Vec<String>,
    /// The request to deploy now (none while a deploy runs or nothing is ready in order).
    pub deploy: Option<String>,
    /// Older ready requests the deploy contains: included once it verifies.
    pub carried: Vec<String>,
}

/// Plans the queue of one environment (SPEC-575 MFS-006) from its requests and the position of
/// its last verified revision (`current`; `None`: nothing verified yet).
#[must_use]
pub fn pick(requests: &[Facts], current: Option<u64>) -> Pick {
    let mut out = Pick::default();
    let mut ahead: Vec<&Facts> = Vec::new();
    let mut included = Vec::new();
    let mut first_inherited: Option<u64> = None;
    for request in requests {
        if !matches!(
            request.state,
            StagingState::Testing | StagingState::Inherited | StagingState::Ready
        ) {
            continue;
        }
        let Some(position) = request.position else {
            continue;
        };
        if current.is_some_and(|at| position <= at) {
            included.push((position, request.id.clone()));
        } else if request.state == StagingState::Inherited {
            first_inherited =
                Some(first_inherited.map_or(position, |earlier| earlier.min(position)));
        } else {
            ahead.push(request);
        }
    }
    included.sort_by_key(|(position, _)| *position);
    out.included = included.into_iter().map(|(_, id)| id).collect();
    if requests.iter().any(|r| r.state == StagingState::Deploying) {
        return out;
    }
    ahead.sort_by_key(|r| r.position);
    let ready: Vec<&Facts> = ahead
        .iter()
        .take_while(|r| r.state == StagingState::Ready)
        .copied()
        .collect();
    // The first passing descendant answers an inherited request even if another green
    // revision has arrived meanwhile. Include every ready request at that same position.
    let count = first_inherited
        .and_then(|at| {
            ready
                .iter()
                .find(|r| r.position.is_some_and(|pos| pos >= at))
        })
        .map_or(ready.len(), |fix| {
            ready
                .iter()
                .take_while(|r| r.position <= fix.position)
                .count()
        });
    let ready = &ready[..count];
    if let Some((last, older)) = ready.split_last() {
        // Requests at the deployed revision's own position (two tasks, one landing) are carried.
        let target = ahead
            .iter()
            .take(ready.len())
            .rev()
            .find(|r| r.position == last.position && r.task.is_some())
            .unwrap_or(last);
        out.deploy = Some(target.id.clone());
        out.carried = older
            .iter()
            .chain(std::iter::once(last))
            .filter(|r| r.id != target.id)
            .map(|r| r.id.clone())
            .collect();
    }
    out
}

/// How a deploy run of a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployEnd {
    /// Deployed and verified.
    Passed,
    /// The deploy or verify script failed.
    Failed,
    /// The run ended in error or was lost to a restart: nothing says the scripts failed.
    Lost,
    /// A person cancelled the run: their decision stands, nothing deploys it again.
    Cancelled,
}

/// The request's state after its deploy run ended, `attempts` counting this one (SPEC-575
/// MFS-007, MFS-008).
#[must_use]
pub fn after_deploy(end: DeployEnd, attempts: u32) -> StagingState {
    match end {
        DeployEnd::Passed => StagingState::Verified,
        DeployEnd::Failed | DeployEnd::Cancelled => StagingState::DeployFailed,
        DeployEnd::Lost if attempts < MAX_DEPLOY_ATTEMPTS => StagingState::Ready,
        DeployEnd::Lost => StagingState::DeployFailed,
    }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, task: &str, revision: &str, state: StagingState, position: u64) -> Facts {
        Facts {
            id: id.to_owned(),
            task: Some(task.to_owned()),
            revision: revision.to_owned(),
            state,
            position: Some(position),
            attempts: 0,
        }
    }

    #[test]
    fn states_round_trip_their_wire_names() {
        for state in StagingState::ALL {
            assert_eq!(StagingState::parse(state.as_str()), Some(state));
        }
        assert_eq!(StagingState::parse("done"), None);
        let pending: Vec<_> = StagingState::ALL
            .into_iter()
            .filter(|s| s.is_pending())
            .collect();
        assert_eq!(
            pending,
            [
                StagingState::Testing,
                StagingState::Inherited,
                StagingState::Ready,
                StagingState::Deploying
            ]
        );
        let success: Vec<_> = StagingState::ALL
            .into_iter()
            .filter(|s| s.is_success())
            .collect();
        assert_eq!(success, [StagingState::Verified, StagingState::Included]);
    }

    /// MFS-005: a duplicate request answers with the first; a failed deploy of the same
    /// revision is retried; a new revision supersedes the task's waiting one; a task with a
    /// successful deployment gets no second one.
    #[test]
    fn a_task_gets_one_successful_staging_deployment() {
        assert_eq!(
            admit("aaa", &[], false),
            Admission::New {
                state: StagingState::Testing,
                supersede: Vec::new()
            }
        );
        assert_eq!(
            admit("aaa", &[], true),
            Admission::New {
                state: StagingState::Included,
                supersede: Vec::new()
            },
            "already on staging from an earlier deploy: never deployed twice"
        );
        let testing = req("r1", "t", "aaa", StagingState::Testing, 3);
        assert_eq!(
            admit("aaa", std::slice::from_ref(&testing), false),
            Admission::Existing("r1".into())
        );
        let failed_gate = req("r1", "t", "aaa", StagingState::GateFailed, 3);
        assert_eq!(
            admit("aaa", std::slice::from_ref(&failed_gate), false),
            Admission::Existing("r1".into()),
            "a failed gate is fixed by a new revision, never retried until green"
        );
        assert_eq!(
            admit("bbb", &[failed_gate], false),
            Admission::New {
                state: StagingState::Testing,
                supersede: Vec::new()
            }
        );
        let failed_deploy = req("r1", "t", "aaa", StagingState::DeployFailed, 3);
        assert_eq!(
            admit("aaa", &[failed_deploy], false),
            Admission::Retry("r1".into())
        );
        let ready = req("r2", "t", "bbb", StagingState::Ready, 4);
        assert_eq!(
            admit("ccc", &[testing.clone(), ready], false),
            Admission::New {
                state: StagingState::Testing,
                supersede: vec!["r1".into(), "r2".into()]
            }
        );
        for done in [StagingState::Verified, StagingState::Included] {
            let verified = req("r1", "t", "aaa", done, 3);
            assert_eq!(
                admit("aaa", std::slice::from_ref(&verified), false),
                Admission::Existing("r1".into())
            );
            let Admission::Refused(why) = admit("bbb", &[verified], false) else {
                panic!("a second deployment of {done:?}");
            };
            assert!(why.contains("already has its staging deployment"), "{why}");
        }
        let deploying = req("r1", "t", "aaa", StagingState::Deploying, 3);
        assert!(matches!(
            admit("bbb", &[deploying], false),
            Admission::Refused(why) if why.contains("being deployed")
        ));
        let superseded = req("r1", "t", "aaa", StagingState::Superseded, 3);
        assert!(matches!(
            admit("aaa", &[superseded], false),
            Admission::New { .. }
        ));
    }

    /// MFS-003, MFS-007: only a verdict moves a testing request.
    #[test]
    fn the_full_gates_verdict_decides_a_testing_request() {
        assert_eq!(after_verdict(None), None);
        assert_eq!(
            after_verdict(Some(RunState::Passed)),
            Some(StagingState::Ready)
        );
        for bad in [RunState::Failed, RunState::Error] {
            assert_eq!(after_verdict(Some(bad)), Some(StagingState::GateFailed));
        }
        for none in [
            RunState::Queued,
            RunState::Running,
            RunState::Cancelled,
            RunState::Skipped,
        ] {
            assert_eq!(after_verdict(Some(none)), None, "{none:?}");
        }
    }

    /// MFS-006: the newest ready revision above the last verified one deploys, carrying the
    /// older ready ones; an older request still testing holds every newer one back; a request
    /// already contained is included at once; one deploy at a time.
    #[test]
    fn staging_moves_forward_in_revision_order() {
        let a = req("a", "t1", "aaa", StagingState::Ready, 5);
        let b = req("b", "t2", "bbb", StagingState::Ready, 7);
        assert_eq!(
            pick(&[b.clone(), a.clone()], Some(4)),
            Pick {
                included: Vec::new(),
                deploy: Some("b".into()),
                carried: vec!["a".into()],
            }
        );
        let a_testing = Facts {
            state: StagingState::Testing,
            ..a.clone()
        };
        assert_eq!(
            pick(&[a_testing.clone(), b.clone()], Some(4)),
            Pick::default(),
            "never past an older request still testing"
        );
        let old = req("old", "t0", "000", StagingState::Testing, 3);
        assert_eq!(
            pick(&[old, a.clone()], Some(4)).included,
            ["old"],
            "contained in what staging verified"
        );
        let deploying = req("d", "t3", "ddd", StagingState::Deploying, 6);
        assert_eq!(pick(&[a.clone(), deploying], Some(4)).deploy, None);
        let failed = req("f", "t4", "fff", StagingState::GateFailed, 6);
        assert_eq!(
            pick(&[a.clone(), failed, b.clone()], None),
            Pick {
                included: Vec::new(),
                deploy: Some("b".into()),
                carried: vec!["a".into()],
            },
            "a failed gate holds nobody back"
        );
        let off_branch = Facts {
            position: None,
            ..b.clone()
        };
        assert_eq!(pick(&[off_branch], None), Pick::default());
        // Two tasks landing in one revision: one deploys, the other is carried.
        let twin = req("twin", "t5", "bbb", StagingState::Ready, 7);
        let both = pick(&[b.clone(), twin], Some(4));
        assert_eq!(both.deploy.as_deref(), Some("twin"));
        assert_eq!(both.carried, ["b"]);
        // A person's request at the same revision leaves the deploy to a task's.
        let person = Facts {
            task: None,
            id: "p".into(),
            ..b.clone()
        };
        let mixed = pick(&[b, person], Some(4));
        assert_eq!(mixed.deploy.as_deref(), Some("b"));
        assert_eq!(mixed.carried, ["p"]);
    }

    #[test]
    fn the_verified_descendant_serves_inherited_requests_in_revision_order() {
        let requests = [
            req("later", "b", "bbb", StagingState::Inherited, 4),
            req("fix", "c", "ccc", StagingState::Ready, 5),
            req("first", "a", "aaa", StagingState::Inherited, 3),
            req("newer", "d", "ddd", StagingState::Ready, 6),
        ];
        let before = pick(&requests, Some(2));
        assert_eq!(before.deploy.as_deref(), Some("fix"));
        assert!(before.included.is_empty());
        assert_eq!(pick(&requests, Some(5)).included, ["first", "later", "fix"]);
    }

    /// MFS-007, MFS-008: a passed deploy verifies, a failed one stays failed, a lost one is
    /// retried until its attempts run out.
    #[test]
    fn a_lost_deploy_is_retried_a_bounded_number_of_times() {
        assert_eq!(after_deploy(DeployEnd::Passed, 1), StagingState::Verified);
        assert_eq!(
            after_deploy(DeployEnd::Failed, 1),
            StagingState::DeployFailed
        );
        assert_eq!(
            after_deploy(DeployEnd::Cancelled, 1),
            StagingState::DeployFailed,
            "a person's cancel is never undone"
        );
        for attempts in 1..MAX_DEPLOY_ATTEMPTS {
            assert_eq!(after_deploy(DeployEnd::Lost, attempts), StagingState::Ready);
        }
        assert_eq!(
            after_deploy(DeployEnd::Lost, MAX_DEPLOY_ATTEMPTS),
            StagingState::DeployFailed
        );
    }
}
