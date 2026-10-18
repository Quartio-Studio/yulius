# 0175. Environment runs run contained, from a mirror of origin only Yulius writes

## Status

Accepted. Specified in [SPEC-219](../../.ai/specs/SPEC-219-fix-656-delivery-environment-runs-must-r.md);
task #205, issue #656. Applies [ADR-0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md)
to delivery environment runs and closes SPEC-002 R-09 for them. Leaves the rest of #54 part B
([ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md), Consequences) as it is.

## Context

A delivery profile's environment `command` (DLV-003) ran on the host with the server's privileges:
Yulius checked out the stage branch's head in the sandbox clone and ran `/bin/sh -c <command>`
there, with `HOME` the sandbox's home. The clone's `.git` is the agents' to write, so a filter
driver planted in its configuration ran during the checkout, and the command ran outside
containment. Agents reach this with `delivery_deploy` for a command-verified integration
environment, with no person involved. The run also fetched with the installation token in that
clone, where repository-local transport settings (`http.proxy` with `http.sslVerify=false`,
`include.path`) can steer where the token goes. CD runs (SPEC-134) already avoid all of this.

## Decision

- **The commit comes from origin, into a mirror only Yulius writes.** An environment run fetches
  the stage branch's head from the repository's own origin (GitHub's URL with the installation
  token in that one command's environment, a playground's origin by its path) into
  `<data>/cd/<sandbox>/environments.git`: one commit deep, every object checked, replacement refs
  ignored (`yulius_git::Git::mirror_fetch_head`). No Git of the run reads the sandbox clone.
- **It runs as a CD run of its stage.** The commit's tree is written into the run's own directory
  `<data>/runs/env-<run>` (`export_tree`) with a fresh `HOME` there, and the command runs through
  the process host (`ci_watch`) in CD's containment: an integration environment as a dev run (the
  sandbox's containment), a production one as a production run. The run gets the CI job timeout,
  as CD runs, and no secrets. Its directory goes with it; a run a restart left running has its
  process host terminated.

## Alternatives

- **Take the commit from the sandbox clone into CD's mirror (`cd_mirror_fetch`)**, as CD does: the
  clone must be fetched first, which is the token fetch in agent-written configuration that the
  issue names.
- **Scrub the clone's transport settings before the fetch** (SPEC-176's `scrub_transport_config`):
  an agent rewrites them between the scrub and the fetch; it narrows the window, it does not close
  it.
- **Fetch the full history into CD's `source.git`**: promotion checks walk that mirror's history,
  which a shallow fetch would cut; a full fetch from the network costs more than one commit.
- **Wrap the old host runner in a hand-built `bwrap` command line**: joining a sandbox's holder is
  the process host's job; two implementations of containment would drift.

## Consequences

- An environment command sees what a CD run of its stage sees: the commit's files and the
  sandbox (dev) or the host's read-only system (production), not the host. A command that relied
  on the sandbox's home, or on host paths, must change; `HOME` is empty at every run.
- The first run of a project fetches one commit of its branch from GitHub; later runs fetch what
  changed.
- Environment runs record no Pipeline spans (no span subject exists for them).
- Containment is the server's: with `contained = false` (development) the command runs on the host,
  as CD runs do; only the Git that prepares it stays out of the sandbox clone.
- Review round 1 found two holes this path shared with CD and CI and fixed them for all three: the
  host read a run's log by path (a run could link it to a host file), and the tree export applied
  the server's global Git configuration (a git-lfs filter) to the commit's attributes.
- Yulius's other Git in the sandbox clone with a token (CI, merges, hygiene, CD's refresh) is
  still #54 part B.
