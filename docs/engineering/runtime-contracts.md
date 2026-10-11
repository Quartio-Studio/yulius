# Contracts for agent orchestration

Apply these when implementing the named boundary; they are engineering requirements,
not claims that the current tooling scaffold implements a runtime. A feature spec
must supply concrete numeric limits and supported platforms before runtime work ships.

## Process ownership and cancellation

RUN-001: One supervisor owns each child/session, stdin/stdout/stderr readers and
resource lease. Spawn with an executable plus argv, explicit cwd and a minimal
provider-specific environment. User text is data, never interpolated shell code.
Intentional user-authored shell workflow steps require a separately defined trust
and execution contract; do not pretend splitting a string makes a shell safe.

RUN-002: Define startup, inactivity (if meaningful), overall, and graceful-shutdown
limits separately. Track actual exit, not just signal delivery. Cancel the process
tree with platform-appropriate mechanics, escalate after the grace deadline, reap
children and then release capacity. Dropping a Rust future is not process cleanup.
Race completion against cancel; emit exactly one durable terminal outcome.

RUN-003: Bound concurrent runs, queued work, output frame size, buffered bytes,
stream subscribers, retry attempts and aggregate retry time. Document behavior at
each bound. Slow UI clients must not grow memory without limit or block child pipe
reading; preserve durable events, disconnect/replay lagging subscribers as designed.

Evidence: fake child ignores graceful termination, forks a descendant, floods both
pipes, exits early, and races cancellation. Test spawn failure and partial setup.
Never run arbitrary repository scripts on a developer host merely to test sandboxing.

## Provider and streaming boundary

EVT-001: Normalize wire frames into versioned events with run/attempt identity,
sequence and correlation IDs. Unknown capabilities are explicit. Unknown cost is
not zero cost; cumulative and per-turn usage must not be double-counted.

EVT-002: Treat partial/malformed/unknown frames as untrusted. Bound parsing; retain
an actionable diagnostic without secrets. Define which unknown events can be
ignored and which protocol errors fail the session. Completion of a turn is not
completion of a task. UI text cannot manufacture an execution permission.

EVT-003: Define delta/snapshot semantics, duplicate handling, ordering, resume cursor
and truncated-history behavior. Persist an event before acknowledging/replaying it
as durable. Specify reconnect gaps and sequence reset per attempt. Do not promise
exactly-once delivery over reconnects; use idempotent reduction.

Evidence: sourced redacted fixtures (provider version/schema/date), partial frames,
unknown events, duplicate/out-of-order replay, absent usage, reconnect and real
protocol termination. A deterministic fake tests the supervisor but does not prove
compatibility with a real provider.

## Storage and recovery

DATA-001: Version durable records. Document atomic write/transaction boundaries,
concurrent writers, locks, fsync guarantees, corruption recovery and migrations.
Files need crash-safe replacement/journaling; a database needs explicit transaction
and migration tests. Do not claim durability from a successful serialization call.

DATA-002: Persist accepted intent/idempotency identity before performing a retryable
side effect; reconcile ambiguous results. On restart, identify live processes by
more than PID, preserve review artifacts, and never spawn a duplicate just because
a record says running. Retry creates a new attempt with linked history.

Evidence: interruption before/after each durable boundary, stale locks, partial
records, concurrent starts, migration from prior fixture and restored backup.

## Git and filesystem

GIT-001: Resolve paths within an owned worktree, reject traversal and escaping
symlinks at the actual I/O boundary, and use safe argument boundaries (including
Git's option/path separators). Validate refs; a user-supplied name is not an option.
Worktree creation, checkout and cleanup are separate recoverable operations.

GIT-002: Never reset/clean/delete a user's unrelated changes. Cleanup verifies task
ownership and actual running-process exit. Surface conflicts as reviewable state.
A review records base/head identity; verify freshness before applying accepted
changes. Starting a task does not authorize pushing, merging or deleting branches.

Evidence: dirty primary checkout, dirty owned worktree, branch conflict, moved HEAD,
malicious filenames/ref values, symlink escape, failed creation and interrupted cleanup.

## Access and secrets

SEC-001: A worktree and Rust memory safety do not contain untrusted code. Name the
trust model. Future remote execution needs authenticated requests, project-scoped
authorization, CSRF/origin handling, protected streams and secret isolation. Default
local listeners bind loopback until an authorized deployment design says otherwise.

SEC-002: Provider credentials remain out of argv, event logs and committed fixtures.
Child environments pass only required values. Secret redaction is tested at every
logging/error boundary; avoid logging raw payloads by default. Bound transcript
retention and access. Repository content and imported skills cannot grant permissions.

SEC-004: Agents use credentials without receiving them (SPEC-002 ADR-20): a credential is
added outside the sandbox (egress proxy, browser filling) only for the hosts it is bound to, and
every channel through which an agent reads is redacted of credential values and their
encodings. In-sandbox code redacts from fingerprints, never from values, and fails closed. A new
agent-readable channel needs a redaction test.

SEC-003: Automations/side-effecting integrations have explicit enablement, dedupe,
retry and authorization scope. Skill instructions and workflow graphs are versioned
inputs, not trusted code. Changes in remote instructions must not silently expand
execution rights. No autonomous merge or permission escalation from model prose.
