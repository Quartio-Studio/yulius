# 0003. Per-sandbox containment and namespace holder

## Status

Accepted; amended by [0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md)
(each sandbox runs its own Docker engine) and [0198](0198-sandbox-task-limits-are-sized-from-the-host-and-leave-sessions-a-reserve.md)
(task limits sized from the host). Recorded retroactively from SPEC-002 (ADR-03, ADR-12,
ADR-22); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Agents and users run arbitrary code and may need `sudo apt install`; worktrees isolate changes, not process capabilities.

## Decision

Started with bubblewrap (ADR-03/12). ADR-22 replaced it with one namespace holder per sandbox: user, network (slirp4netns egress) and mount namespaces with a per-sandbox overlay system layer, plus PID/IPC/UTS/cgroup namespaces, reduced capabilities, seccomp and cgroup v2 limits per run. Without prerequisites the `legacy` bubblewrap mode remains.

## Alternatives

Docker per sandbox; rootless Podman; gVisor; microVMs (no KVM); fuse-overlayfs over the live host.

## Consequences

`apt` persists per sandbox and sandboxes cannot reach each other or host loopback. Needs `/etc/subuid` and `Delegate=`; the base copy costs about 6 GB. Not a defence against kernel exploits.
