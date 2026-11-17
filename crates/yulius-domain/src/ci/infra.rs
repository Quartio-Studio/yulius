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

    /// The kind stored as `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
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
/// must come just before it ended, not from something it went on from (review round 1).
pub const TAIL_LINES: usize = 20;

/// The infrastructure failure `text` names, if any: Docker's words for a full address pool, else
/// a line naming `docker.sock` that says the connection ended.
fn named(text: &str) -> Option<Infra> {
    let mut socket = false;
    for line in text.lines() {
        let line = line.to_ascii_lowercase();
        if ADDRESS_POOL.iter().any(|words| line.contains(words)) {
            return Some(Infra::AddressPool);
        }
        if line.contains("docker.sock")
            && SOCKET_ENDS
                .iter()
                .any(|words| line.contains(words) || (*words == ": eof" && line.ends_with(" eof")))
        {
            socket = true;
        }
    }
    socket.then_some(Infra::DockerSocket)
}

/// Why the failure of a job that ended `end` is the infrastructure's, if it is, from this run's
/// own `output` (what it printed, the end kept), the failing tests' blocks `failures` it printed
/// ([`crate::ci::failures::FailureScanner::finish`]) and whether the kernel killed a process of
/// its sandbox for memory meanwhile (`oom_killed`):
///
/// - its process host was lost;
/// - a `SIGKILL` (or exit 137, a shell's report of one) while the kernel killed for memory;
/// - with failing tests kept: every one of them failed with Docker's words for a full address
///   pool or a `docker.sock` connection that ended (the first such test's kind); a test that
///   failed otherwise makes it the change's;
/// - with none kept: those words in the last [`TAIL_LINES`] lines of the output, so a Docker
///   error the job went on from, followed by its own failure, is not one.
///
/// # Examples
/// ```
/// use yulius_domain::ci::infra::{Infra, JobEnd, infrastructure};
/// let pool = "Error response from daemon: could not find an available, non-overlapping IPv4 \
///             address pool among the defaults to assign to the network";
/// assert_eq!(infrastructure(JobEnd::Exited(1), pool, None, false), Some(Infra::AddressPool));
/// let eof = "error during connect: Post \"http://%2Frun%2Fyulius%2Fdocker%2Fdocker.sock/v1.47/containers/create\": EOF";
/// assert_eq!(infrastructure(JobEnd::Exited(1), eof, None, false), Some(Infra::DockerSocket));
/// assert_eq!(infrastructure(JobEnd::Signalled(9), "", None, true), Some(Infra::OutOfMemory));
/// assert_eq!(infrastructure(JobEnd::Exited(101), "test x ... FAILED", None, false), None);
/// ```
#[must_use]
pub fn infrastructure(
    end: JobEnd,
    output: &str,
    failures: Option<&str>,
    oom_killed: bool,
) -> Option<Infra> {
    match end {
        JobEnd::Lost => return Some(Infra::HostLost),
        JobEnd::Signalled(SIGKILL) | JobEnd::Exited(137) if oom_killed => {
            return Some(Infra::OutOfMemory);
        }
        _ => {}
    }
    let tests = failures
        .map(crate::ci::failures::failed_test_names)
        .unwrap_or_default();
    if let Some(kept) = failures.filter(|_| !tests.is_empty()) {
        // Each kept block starts at its header and ends at a blank line.
        let mut kinds = kept
            .split("\n\n")
            .filter(|block| !block.trim().is_empty() && !block.starts_with("[yulius]"))
            .map(named);
        let first = kinds.next()??;
        return kinds.all(|kind| kind.is_some()).then_some(first);
    }
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(TAIL_LINES)..];
    // The last line naming the infrastructure, with no test's failure after it.
    let (at, kind) = tail
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| named(line).map(|kind| (at, kind)))?;
    let failed_after = tail[at + 1..]
        .iter()
        .any(|line| TEST_FAILED.iter().any(|words| line.contains(words)));
    (!failed_after).then_some(kind)
}

/// What test runners print for a failing test.
const TEST_FAILED: [&str; 4] = [" FAILED", "FAIL [", "panicked at", "test result: FAILED"];

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-497 OFC-001: Docker's own words and the socket's end are the infrastructure's, in any
    /// case; a test's failure is not, nor a SIGKILL without a kill for memory, nor a line about
    /// the socket that says nothing ended.
    #[test]
    fn only_the_infrastructures_own_words_count() {
        let exited = JobEnd::Exited(1);
        for (output, kind) in [
            (
                "Error response from daemon: all predefined address pools have been fully subnetted",
                Infra::AddressPool,
            ),
            (
                "ERROR: could not find an available, non-overlapping IPv4 address pool among the defaults to assign to the network",
                Infra::AddressPool,
            ),
            (
                "read unix @->/run/yulius/docker/docker.sock: read: connection reset by peer",
                Infra::DockerSocket,
            ),
            (
                "Cannot connect to the Docker daemon at unix:///run/yulius/docker/docker.sock. Is the docker daemon running?",
                Infra::DockerSocket,
            ),
            (
                "write unix @->/run/yulius/docker/docker.sock: write: broken pipe",
                Infra::DockerSocket,
            ),
        ] {
            assert_eq!(
                infrastructure(exited, output, None, false),
                Some(kind),
                "{output}"
            );
        }
        for output in [
            "test person_lang::a_person_added_in_english ... FAILED",
            "mounted /run/yulius/docker/docker.sock into the container",
            "thread 'main' panicked at 'connection reset by peer' (no socket named)",
        ] {
            assert_eq!(
                infrastructure(exited, output, None, false),
                None,
                "{output}"
            );
        }
        assert_eq!(infrastructure(JobEnd::Signalled(9), "", None, false), None);
        assert_eq!(
            infrastructure(JobEnd::Exited(137), "Killed", None, true),
            Some(Infra::OutOfMemory)
        );
        assert_eq!(
            infrastructure(JobEnd::Exited(1), "", None, true),
            None,
            "not killed"
        );
        assert_eq!(
            infrastructure(JobEnd::Lost, "", None, false),
            Some(Infra::HostLost)
        );
        // Review round 1: a Docker error the job went on from, then a test's failure, is the
        // change's, in the output and in the failing tests' blocks.
        let survived = format!(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.\n{}test flaky ... FAILED",
            "line\n".repeat(TAIL_LINES)
        );
        assert_eq!(infrastructure(exited, &survived, None, false), None);
        let close = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.\n\
                     test flaky ... FAILED";
        assert_eq!(infrastructure(exited, close, None, false), None);
        // A Docker error long before the end, then the job's own error with no test named: only
        // the last lines count (mutation of the tail).
        let early = format!(
            "write unix @->/run/yulius/docker/docker.sock: write: broken pipe\n{}error[E0425]: \
             cannot find value `x` in this scope",
            "   Compiling crate\n".repeat(TAIL_LINES)
        );
        assert_eq!(infrastructure(exited, &early, None, false), None);
        let blocks = "FAIL [   0.1s] (1/2) app tests::a\n  stderr\n  read unix @->/run/docker.sock: \
                      read: connection reset by peer\n\nFAIL [   0.1s] (2/2) app tests::b\n  \
                      assertion failed: left == right\n\n";
        assert_eq!(infrastructure(exited, "", Some(blocks), false), None);
        let both = "FAIL [   0.1s] (1/2) app tests::a\n  read unix @->/run/docker.sock: read: \
                    connection reset by peer\n\nFAIL [   0.1s] (2/2) app tests::b\n  Error \
                    response from daemon: all predefined address pools have been fully \
                    subnetted\n\n";
        assert_eq!(
            infrastructure(exited, "", Some(both), false),
            Some(Infra::DockerSocket)
        );
        for kind in Infra::ALL {
            assert_eq!(Infra::parse(kind.as_str()), Some(kind));
        }
    }
}
