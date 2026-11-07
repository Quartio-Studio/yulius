# 0283. A cancelled update's build stops when its request is withdrawn

## Status

Accepted. Specified in [SPEC-439](../../.ai/specs/SPEC-439-cancel-didnt-work-instantly-and-smoothly.md);
task #421 ("cancel didnt work instantly and smoothly"), #1117. Amends SPEC-002 ADR-18 (the update
helper's build).

## Context

The server asks the root update helper to build by writing `<data>/update/request.json`; a
root-owned path unit starts the helper, which reads the request once and runs `cargo build
--release`. A superadmin's **Cancel update** marks the run cancelled and removes the request file,
but the helper never read it again: the release build ran on for minutes at nice 10 on every
core, and a restart or a new update asked for meanwhile waited behind it. The 2-hour bound killed
cargo alone, leaving its `rustc` processes running.

## Decision

- **The request file is the cancel signal.** While cargo runs, the helper checks every 500 ms
  whether its request still stands. The request is withdrawn once the file is gone (a cancel or a
  failure removed it) or holds another request id (a new update replaced it); a file the helper
  cannot read (wrong owner, a link, too large) is not a withdrawal. It also checks once before cargo
  starts.
- **A withdrawn build is killed with its compilers.** cargo runs as the leader of its own process
  group; a withdrawal or the 2-hour bound kills the whole group with `SIGKILL`. Nothing is staged;
  the status says `failed`, "The build was cancelled.", and the helper goes on with the next
  request in the same run, if there is one.
- Only a build stops this way. An install or restart runs to its end: stopping it halfway could
  leave the service down or the database half restored.
- The instance page shows the cancel from the `UpdateView` the cancel answered with
  (`ctx.updates.apply`, ordered like reads, #502), with its button busy until then.

## Alternatives

- **A separate cancel file or a second systemd unit**: a new protocol for what the request file
  already says, and one more root-owned unit to install.
- **The server stops `yulius-update.service`**: the unprivileged server cannot, and it would also
  stop an install halfway.
- **Inotify on the request file**: no faster in a way a person notices than a 500 ms check, and it
  adds a dependency and a thread to a root helper.
- **Killing cargo alone**: its `rustc` and build-script processes would go on, and they keep the
  stderr pipe open, so the helper would still wait for them.

## Consequences

- A cancel frees the host within about a second, and a new update or a restart starts at once.
- The helper reads the request file twice a second while it builds; it parses only the `id` of a
  file that passes the same ownership and size checks as the request itself.
- A cancelled build leaves cargo's target directory as an interrupted build does; the next build
  reuses what was compiled.
