# 0359. A CI job's containers and networks carry its label and go when it ends

## Status

Accepted. Specified in [SPEC-504](../../.ai/specs/SPEC-504-ci-leaves-nothing-behind-and-a-bad-yuliu.md)
(task #486), CLB-001 to CLB-006. Builds on [ADR-0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md)
(every Docker call of a sandbox goes through its front) and
[ADR-0344](0344-the-docker-front-gives-way-and-ends-connections-whose-client-is-gone.md) (the
front's gate). Volumes and images stay out of reach, as in SPEC-187 DKR-006.

## Context

On 16 Nov 2026 quartio-v2's sandbox engine held 22 `quartio-browser` test containers (38 min to
16 h old) and 13 `quartio-test-*` networks from test runs that were killed or cancelled
mid-test: superseded runs, server restarts, time limits. They took 18 of the engine's 31
networks; six CI jobs waited for networks (SPEC-497 HBA-003) while no job of the project ran, and
the engine never went idle, so it kept Docker's 31-network pools instead of the 4,096 an engine
gets at its next start (#1356). An operator restarted the front by hand.

A job's own processes end with it (its PID namespace), but its containers live in the engine and
outlive it. A test harness cleans up only when it gets to run its teardown, which a kill, a
cancellation or a time limit never lets it do. The upkeep pruned only networks no container used,
and these were in use by the leaked containers. Nothing knew which container a job had made: the
front copied bytes, and every job and agent of a sandbox shared one engine.

## Decision

- **Each CI job runs in its own cgroup**, `ci/ci-job-<job id>` below the sandbox's CI group
  (where CPU is not split, the sandbox's group), made by the server before the job starts and
  removed after it ends. It holds no limit of its own: the CI group's apply to it. A run cannot
  leave it (the cgroup tree is not writable from a run), and it outlives a server restart.
- **The front tells a CI job's calls by the caller's cgroup.** At accept it reads the connecting
  process's pid (`SO_PEERCRED`) and its `/proc/<pid>/cgroup` through a handle of the host's
  `/proc` opened before the front joined the sandbox's mount namespace. A process in a group
  named `ci-job-<id>` is that job's; anyone else's connection is only copied, never read.
- **The front labels what a job creates.** On a job's connection it reads each HTTP/1.1 request
  and sets `yulius.ci.job=<id>` in the body of `POST [/v…]/containers/create` and
  `POST [/v…]/networks/create`, with `yulius.ci.create=<a value of its own>` beside it
  (replacing labels of those names, keeping the others, framed by `Content-Length`); every other
  byte passes as it came. Each head is read line by line as it comes: the moment its bytes cannot
  be a request line or a header line ending in CRLF, what was read goes on and the connection
  turns into plain copying, as it does after a hijacking or upgraded call (attach, an exec start
  that is not detached, a BuildKit session, gRPC, any `Upgrade`) and after a body whose framing
  breaks, so nothing is held back for a line that will not make a request. A body that is not
  JSON or is over 4 MiB passes unlabelled, byte for byte. The engine's answers are never read.
- **A job's end removes what it made**, whatever the outcome: the executor tells the isolation
  after the job's process ended (passed, failed, cancelled, timed out), and so does the CI's
  recovery for jobs a restart ended. The server lists the running engine's containers and
  networks with that label through the engine's control socket, removes the containers forced
  without their volumes, then the networks; a network still used by another container stays.
- **The upkeep sweeps** every 30 s, and so first after a server starts: each running engine's
  containers and networks labelled with a job whose group has no process (gone or empty) are
  removed the same way; a group that cannot be read counts as running. Job groups emptied over
  10 minutes ago go too (a job group made again for the same job id is made fresh). One sweep
  runs at a time, in a task of its own, bounded by 60 s per engine; what it removed before a
  failure or the bound counts. The recovery after a restart removes in tasks of their own, so an
  engine that never answers does not hold CI's start.
- **A label inherited from an image is not a job's.** A container whose image carries the same
  `yulius.ci.create` value (an image committed from a job's container, run by anyone) is kept; a
  container a job made from such an image has a value of its own and goes. A container whose
  image cannot be read is kept: nothing then tells it from an agent's.
- **Nothing starts a stopped engine** for this; what a stopped engine holds is swept once it runs
  again. Images and volumes are never removed.
- **Measured:** each removal is a Docker event, audited as `sandbox.docker.ci_leftovers`
  (`<containers> <networks> <job-end|sweep>`), summed per week by `GET /api/analytics/docker`
  and shown on the Pipeline tab's Docker card.

## Alternatives

- **Cleaning up by hand or a periodic prune of all containers.** Rejected: it does not tell a
  job's containers from an agent's, and agents' containers are never to be touched.
- **An environment variable naming the job.** Rejected: a job's processes can unset it, reading
  another process's environment needs ptrace rights, and a restarted server cannot tell which
  jobs ended.
- **A socket per job.** Rejected: the front and the server would have to agree on each job's
  lifetime across restarts, and a job could still reach the shared socket through a path it
  finds.
- **Recording created ids from the engine's answers.** Rejected: answers can be chunked or
  streamed, and the record would be state of its own to keep across restarts; a label lives with
  the container.
- **Diffing the engine's containers before and after a job.** Rejected: agents and other jobs
  create containers at the same time.
- **Leaving it to each project's harness.** Rejected as the only defence: a killed job never runs
  its teardown. Harnesses keep their own cleanup for a clean state between tests in one job.

## Consequences

- A job's containers, Compose projects and networks go when it ends, also after a kill, a
  cancellation, a time limit or a server restart; an engine no longer fills with them, and goes
  idle again (taking #1356's larger pools at its next start).
- Containers a job's own container creates through a mounted socket are not labelled (their
  caller is in the engine's group); a reaper such as Ryuk only removes, so the common case is
  covered.
- Without cgroup delegation there are no job groups, so nothing is labelled or removed (and no
  limits apply either). A front started by an older binary labels nothing until it is replaced
  (SPEC-494 replaces outdated fronts).
- An agent can still create a container labelled `yulius.ci.job` itself; it is then treated as
  that job's.
- A container one job created and another job or an agent uses (a fixed name, the same Compose
  project) goes when its creator ends.
- Anonymous volumes of removed containers stay, as volumes are never removed; they count toward
  the sandbox's disk and are pruned only by a person or the engine's own prune.
- A job's own container whose image cannot be read (gone while the container stays, which Docker
  does not normally allow) stays.
- `yulius doctor --docker-smoke` proves it on a host: a CI job's container and network carry its
  label, an agent's beside it does not, and the job's end removes only the job's.
