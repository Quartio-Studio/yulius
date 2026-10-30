# 0240. A run's strays are reaped after a bound, and a sandbox gets half of the egress proxy

## Status

Accepted. Specified in [SPEC-353](../../.ai/specs/SPEC-353-leftover-agent-shells-pile-up-in-a-sandb.md);
task #335. Amends SPEC-130 DMD-001: the namespace init's orphans are still no background shells,
but they are no longer ignored. Leaves the Superagent's fingerprint file to SPEC-352 (task #334).

## Context

On 2026-10-30 the quartio-v2 sandbox held about 60 pairs of `sh -c "if command -v bash …"` and
`bash /tmp/.tmpXXXXXX/.jobs/job-<hex>.sh`, each spinning `while [ ! -f go ]; do sleep 0.05; done`,
plus a headless Chrome and other old commands. They came from quartio-v2's own unit tests, which
start detached jobs and leak them whenever a test fails or is killed before it releases them. When
their parent exits, the sandbox's PID-namespace init (bwrap) adopts them. The background-shell
probe skips whatever that init adopted, so that a preview server does not keep a task Working
(DMD-001). Nothing else looked at them, so they lived as long as the session, which is days for a
Superagent or for a task waiting on an armed merge.

The same evening the Superagent of that sandbox could not run a single command: its fingerprint
request to Yulius failed, and it has no fallback file. A sandbox reaches Yulius through one relayed
port that serves both the internal API and the egress proxy. The relay takes 256 connections per
sandbox; the proxy takes 256 for all sandboxes and keeps a tunnel up to 24 h. Both refuse silently
past their limit. Only a restart freed them.

## Decision

- **Strays are listed.** The probe reports, besides background shells, every subtree the run's
  own namespace init adopted (an init that is a child of the probe's root), other than the agent,
  which is the init's earliest-started child. Each stray carries its processes' start times, so a
  reused pid is never mistaken for it.
- **An idle task's strays are reaped after 2 hours.** The first time a stray is seen, Yulius logs a
  warning. Once a stray has been seen for `stray_reap_after_ms` (2 h), Yulius kills its subtree
  with `SIGKILL` at the next probe while the task is idle, signalling each process only while its
  start time matches. Processes under the agent are never strays, so a live turn's commands and
  the agent's own background shells are untouched.
- **A sandbox gets at most half of the egress proxy** (`max_per_sandbox`, 128 of 256). A sandbox
  past its share gets `503` saying that its own processes hold its share, while other sandboxes
  keep theirs, and half of its relay stays free for its own calls to Yulius (fingerprints, hooks,
  MCP).
- **Limits are logged.** A refusal at the proxy's limit or share, and a connection the relay drops
  at its limit, is a warning, at most once a minute per limit and sandbox.
- **The fingerprint message names its cause**: a refused token means the run ended (continue from
  Yulius), an unreachable Yulius means a restart or used-up connections, and the fallback file's
  state is given. The message never blames a restart for a refused token.

## Alternatives

- **Reap strays at the end of every turn.** Rejected: an agent starts a preview server with
  `(server &)` for a person to look at, and DMD-001 keeps it running on purpose.
- **Never reap and only show strays.** Rejected: a busy loop then burns CPU until someone notices,
  which on 2026-10-30 took days and put the host at load 7.
- **A shorter bound (30 minutes) or one based on CPU use.** Rejected: a person may review a preview
  over a long break, and a sleeping process can still hold connections and memory. 2 h bounds the
  waste to hours.
- **Serving the internal API on its own port, apart from the proxy.** Rejected for now: it changes
  every run's environment, the relay's forwarded ports and the containment. The share protects the
  internal API with one counter.
- **A larger relay or proxy limit.** Rejected: a leak grows past any limit, and file descriptors
  are the server's.
- **Reaping in the ptyhost.** Rejected: the ptyhost sees no task state (idle or working), and the
  server already probes idle runs.

## Consequences

- A preview server an idle task left running stops after 2 hours. The agent starts it again when
  it needs it, and the log says what was reaped.
- One sandbox can hold at most 128 proxy tunnels at once. Parallel package installs past that get a
  `503 … try again` that npm and cargo retry.
- An unsandboxed run (no PID namespace) reports no strays: its orphans go to the host's init.
  Production runs are contained.
- The leaking tests are quartio-v2's to fix, and an issue on that repository says so.
