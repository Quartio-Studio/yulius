//! A CI job that failed for a reason outside the change it tests (SPEC-497 OFC-001): the host or
//! its Docker engine let it down. Such a job runs again once by itself, saying so; a second
//! infrastructure failure ends it as an error. A test that fails is never one of these: only what
//! the infrastructure says about itself is read.
//!
//! Pure: the executor gives the job's output (the end it keeps and its failing tests' blocks),
//! how it ended and whether the kernel killed a process of its sandbox for memory meanwhile.

/// Why a job's failure is the infrastructure's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Infra {
    /// Docker had no free address pool for a new network.
    AddressPool,
    /// The Docker socket closed or reset a connection (EOF, reset, broken pipe, refused).
    DockerSocket,
    /// The kernel killed it for memory.
    OutOfMemory,
    /// Its process host was lost (a restart, a crashed host).
    HostLost,
}

impl Infra {
    /// Every kind, in the order they are told.
    pub const ALL: [Self; 4] = [
        Self::AddressPool,
        Self::DockerSocket,
        Self::OutOfMemory,
        Self::HostLost,
    ];

    /// Its name where it is stored and counted.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AddressPool => "address_pool",
            Self::DockerSocket => "docker_socket",
            Self::OutOfMemory => "out_of_memory",
            Self::HostLost => "host_lost",
        }
    }

    /// What happened, for a person.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::AddressPool => "Docker had no free address pool for a new network",
            Self::DockerSocket => "the Docker socket ended a connection",
            Self::OutOfMemory => "a process of it was killed for memory",
            Self::HostLost => "its process host was lost",
        }
    }
}

/// How a job's process ended, as far as the classification needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEnd {
    /// It exited by itself with this code (not 0).
    Exited(i32),
    /// A signal ended it.
    Signalled(i32),
    /// Its process host was lost.
    Lost,
}

/// `SIGKILL`, which the OOM killer sends.
const SIGKILL: i32 = 9;

/// Lines Docker prints when it has no address range left for a network.
const ADDRESS_POOL: [&str; 2] = [
    "could not find an available, non-overlapping ipv4 address pool",
    "all predefined address pools have been fully subnetted",
];

/// What a client prints when the engine's socket ends a connection, on a line naming the socket.
const SOCKET_ENDS: [&str; 5] = [
    ": eof",
    "connection reset by peer",
    "broken pipe",
    "connection refused",
    "cannot connect to the docker daemon",
];

/// The output lines read at the end of a job that failed with no failing test kept: the words
/// must be among the last few lines it printed, not something it went on from (review round 1;
/// round 2: the last lines only, a missed rerun costs a cycle, a wrong one breaks the rule that a
/// failing test is never run again).
pub const TAIL_LINES: usize = 3;

/// The infrastructure failure `line` names, if any: Docker's words for a full address pool, or
/// `docker.sock` with words for a connection that ended.
fn named(line: &str) -> Option<Infra> {
    let line = line.to_ascii_lowercase();
    if ADDRESS_POOL.iter().any(|words| line.contains(words)) {
        return Some(Infra::AddressPool);
    }
    (line.contains("docker.sock")
        && SOCKET_ENDS
            .iter()
            .any(|words| line.contains(words) || (*words == ": eof" && line.ends_with(" eof"))))
    .then_some(Infra::DockerSocket)
}

/// Why the failure of a job that ended `end` is the infrastructure's, if it is, from this run's
/// own `output` (what it printed, the end kept), whether any failing test's block was kept
/// (`tests_failed`, [`crate::ci::failures::FailureScanner::finish`]) and whether the kernel killed
/// a process of its sandbox for memory meanwhile (`oom_killed`):
///
/// - its process host was lost;
/// - a `SIGKILL` (or exit 137, a shell's report of one) while the kernel killed for memory;
/// - with no failing test kept: Docker's words for a full address pool or a `docker.sock`
///   connection that ended, in the last [`TAIL_LINES`] lines Yulius did not print itself, with
///   no line after them that reads as a failure in any test runner's words.
///
/// A job with a failing test kept is the change's, whatever its blocks say: a test's message can
/// quote Docker's words (review round 2). So is one whose last lines read as a failure.
///
/// # Examples
/// ```
/// use yulius_domain::ci::infra::{Infra, JobEnd, infrastructure};
/// let pool = "Error response from daemon: could not find an available, non-overlapping IPv4 \
///             address pool among the defaults to assign to the network";
/// assert_eq!(infrastructure(JobEnd::Exited(1), pool, false, false), Some(Infra::AddressPool));
/// let eof = "error during connect: Post \"http://%2Frun%2Fyulius%2Fdocker%2Fdocker.sock/v1.47/containers/create\": EOF";
/// assert_eq!(infrastructure(JobEnd::Exited(1), eof, false, false), Some(Infra::DockerSocket));
/// assert_eq!(infrastructure(JobEnd::Signalled(9), "", false, true), Some(Infra::OutOfMemory));
/// assert_eq!(infrastructure(JobEnd::Exited(1), pool, true, false), None);
/// ```
#[must_use]
pub fn infrastructure(
    end: JobEnd,
    output: &str,
    tests_failed: bool,
    oom_killed: bool,
) -> Option<Infra> {
    match end {
        JobEnd::Lost => return Some(Infra::HostLost),
        JobEnd::Signalled(SIGKILL) | JobEnd::Exited(137) if oom_killed => {
            return Some(Infra::OutOfMemory);
        }
        _ => {}
    }
    if tests_failed {
        return None;
    }
    let lines: Vec<&str> = output
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("[yulius]"))
        .collect();
    let tail = &lines[lines.len().saturating_sub(TAIL_LINES)..];
    // The last line naming the infrastructure, with nothing that reads as a failure after it.
    let (at, kind) = tail
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| named(line).map(|kind| (at, kind)))?;
    let failed_after = tail[at + 1..].iter().any(|line| {
        let line = line.to_ascii_lowercase();
        FAILURE_WORDS.iter().any(|words| line.contains(words))
    });
    (!failed_after).then_some(kind)
}

/// Words, lower-case, of a failure in any test runner or tool (Rust's, Go's, pytest's, jest's,
/// TAP's): a line after the infrastructure's that has one makes the failure the change's.
const FAILURE_WORDS: [&str; 7] = [
    "fail",
    "panic",
    "assert",
    "not ok",
    "timeout",
    "timed out",
    "error[",
];

#[cfg(test)]
mod tests {
    use super::*;

    const POOL: &str = "Error response from daemon: could not find an available, non-overlapping \
                        IPv4 address pool among the defaults to assign to the network";
    const SOCKET: &str = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the \
                          docker daemon running?";

    /// SPEC-497 OFC-001: Docker's own words and the socket's end are the infrastructure's when
    /// they end the run, in any case; a SIGKILL only with a kill for memory; a lost host.
    #[test]
    fn the_infrastructures_own_words_at_the_end_count() {
        let exited = JobEnd::Exited(1);
        for (output, kind) in [
            (POOL.to_owned(), Infra::AddressPool),
            (
                "Error response from daemon: all predefined address pools have been fully subnetted"
                    .to_owned(),
                Infra::AddressPool,
            ),
            (
                "read unix @->/run/yulius/docker/docker.sock: read: connection reset by peer"
                    .to_owned(),
                Infra::DockerSocket,
            ),
            (SOCKET.to_owned(), Infra::DockerSocket),
            (
                "write unix @->/run/yulius/docker/docker.sock: write: broken pipe".to_owned(),
                Infra::DockerSocket,
            ),
            // A make or shell report after it reads as no test's failure.
            (format!("{POOL}\nmake: *** [Makefile:4: up] Error 1"), Infra::AddressPool),
            // Yulius's own lines are not the job's.
            (format!("{POOL}\n[yulius] the job's own note"), Infra::AddressPool),
        ] {
            assert_eq!(infrastructure(exited, &output, false, false), Some(kind), "{output}");
        }
        assert_eq!(infrastructure(JobEnd::Signalled(9), "", false, false), None);
        assert_eq!(
            infrastructure(JobEnd::Exited(137), "Killed", false, true),
            Some(Infra::OutOfMemory)
        );
        assert_eq!(
            infrastructure(JobEnd::Exited(1), "", false, true),
            None,
            "not killed"
        );
        assert_eq!(
            infrastructure(JobEnd::Lost, "", false, false),
            Some(Infra::HostLost)
        );
    }

    /// SPEC-497 OFC-001, review rounds 1 and 2: a Docker error the job went on from, then its own
    /// failure in any runner's words, is the change's; so is any job with a failing test kept,
    /// whatever its message quotes; and words long before the end count for nothing.
    #[test]
    fn a_failure_of_the_change_is_never_the_infrastructures() {
        let exited = JobEnd::Exited(1);
        for after in [
            "test flaky ... FAILED",
            "--- FAIL: TestTotal (0.00s)",
            "FAILED tests/test_x.py::test_total - assert 1 == 2",
            "FAIL src/x.test.ts",
            "not ok 3 - totals",
            "        TIMEOUT [ 60.001s] app tests::slow",
            "thread 'main' panicked at src/lib.rs:3:5:",
            "AssertionError: expected 1",
            "error[E0425]: cannot find value `x` in this scope",
        ] {
            for infra in [POOL, SOCKET] {
                let output = format!("{infra}\n{after}");
                assert_eq!(
                    infrastructure(exited, &output, false, false),
                    None,
                    "{output}"
                );
            }
        }
        // A failing test kept: never, even when its message quotes Docker's words.
        assert_eq!(infrastructure(exited, POOL, true, false), None);
        // Words before the last lines: the job went on.
        let early = format!("{SOCKET}\n{}", "   Compiling crate\n".repeat(TAIL_LINES));
        assert_eq!(infrastructure(exited, &early, false, false), None);
        // Words that say nothing ended.
        for output in [
            "mounted /run/yulius/docker/docker.sock into the container",
            "thread 'main' stopped: 'connection reset by peer' (no socket named)",
        ] {
            assert_eq!(
                infrastructure(exited, output, false, false),
                None,
                "{output}"
            );
        }
    }
}
