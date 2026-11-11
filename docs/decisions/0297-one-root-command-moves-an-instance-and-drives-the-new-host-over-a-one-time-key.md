# 0297. One root command moves an instance and drives the new host over a one-time key

## Status

Accepted. Specified in
[SPEC-460](../../.ai/specs/SPEC-460-run-migration-one-command-moves-the-whol.md); task #442
(continues #439). Builds on [0222](0222-an-instance-export-freezes-the-whole-unit-and-streams-an-exact-size-tar.md)
(the export's freeze, stream and import) and on `yulius host setup` (SPEC-203).

## Context

- `yulius instance export --move | ssh new yulius instance import` moves the data directory and
  nothing else. Moving production on 2026-11-11 took nine groups of hand steps around it: checking
  and provisioning the new host, a data disk, the unit and its keys, the proxy, the firewall, the
  URL, the GitHub App, the proof, the tasks and the way back. Each is a way to break production.
- The server runs as the unprivileged service user and is frozen while its data is read, so the
  move cannot be driven from inside it.
- A remote command given to `ssh` passes through the remote user's shell, and the project forbids
  interpolating paths, names or repository content into a shell.
- GitHub has no API for an App's callback URLs or homepage (only `PATCH /app/hook/config`); its
  token endpoint does say whether a `redirect_uri` is registered for a client id and secret.

## Decision

- **One command on the old host**: `sudo yulius instance migrate <[user@]host>` plans, prepares
  the target, copies, cuts over and finishes, printing `host setup`'s kind of checklist. People
  are asked only in the plan, before the target changes and before anything freezes.
- **A one-time key, fixed remote commands**: the flow makes an SSH key, the operator's own
  sign-in installs it once, and it is removed at the end or at an abort. The target is reached
  through four constant command strings; every value travels as JSON or as a stream on standard
  input.
- **The target's steps are run by the same build**: the running binary is copied to the target
  first and executes each step there over `host_setup::Host` (`yulius instance migrate-target`),
  so both hosts' steps are Rust code tested against fake hosts, and the build is the identical
  one by construction.
- **Idempotent steps, little state**: a step reads the facts, acts if needed and returns a line.
  Only the move's identity and the cutover's progress are stored, so a rerun resumes without a
  record that could disagree with the hosts.
- **The new instance's start is the point of no automatic return**: before it, any failure or
  signal thaws the old instance. It is started only after both hosts computed the same inventory
  of the copy (entries and bytes per folder, one fingerprint over every entry's metadata in the
  old host's ids). If it does not come up healthy it is stopped and disabled before the old one
  is thawed. The old unit is disabled only once the new instance is healthy.
- **The host is matched, not described**: the old host's manual package list, third-party apt
  sources, `/usr/local`, unowned `/opt` entries, root's Rust toolchain, the unit with its drop-ins
  and key files, Caddy's configuration and ufw's rules are carried as they are, with the release
  codename, the public URL and the host name replaced. Sandboxes copy the host system, so a
  matching host is what keeps them working.
- **GitHub**: Yulius never changes the App's settings with a person's session. The plan gives one
  deep link and the values before the cutover; the instance asks GitHub's token endpoint whether
  its callback is registered (slice C of the spec).
- **Tasks come back by a mark**: the move marks the copy; the first start that finds the mark
  queues the tasks whose runs were lost with the old host.

## Alternatives

- **A guide and several commands** (preflight, export, import, apply profile, verify): what
  #439 first sketched. Every seam between commands is a hand step again; rejected by the request
  ("the goal is ONE command").
- **A host profile inside the archive, applied by the import**: the import would install units
  and run package managers from an input it otherwise distrusts (ADR-0222), and the target could
  not be checked before the freeze. The move reads the old host live instead.
- **Remote shell commands built on the old host**: least code, but every path and name would be
  interpolated into a shell.
- **An agent or daemon on the target with its own port and protocol**: more to secure and to
  remove than one key for an SSH the operator already trusts.
- **Describing the host as a list of supported packages**: it would miss what an operator
  installed by hand, which is exactly what broke sandboxes on the new host.
- **`rsync` for the copy**: its quick check compares size and time, its ids cannot be remapped
  by range, and the import's checks would be bypassed. The export's own archive stays the
  transport; the warm copy of slice B extends it.
- **`git fsck`, row counts and sandbox starts as the proof**: they recheck the old host's own
  state at many minutes frozen. The copy is proven equal to a source that worked, and the new
  instance's start, the contained doctor and the Docker smoke prove the host.
- **Driving GitHub's settings page with a stored session**: forbidden by the request.

## Consequences

- The old host's root gets root on the target for the length of the move, and the target ends as
  the instance's host with every secret; the key is one-time and the flow removes it.
- A release without a package, another proxy than Caddy and another firewall than ufw are named
  in the plan as `manual`, not carried.
- The first slice freezes for the whole copy, as the move by hand did; slice B shortens it.
- A copy that was started and then rolled back is no longer a clean copy and must be removed on
  the target before the next run.
- `yulius instance export` and `import` keep working by themselves, for a copy to keep or a move
  without SSH between the hosts.
