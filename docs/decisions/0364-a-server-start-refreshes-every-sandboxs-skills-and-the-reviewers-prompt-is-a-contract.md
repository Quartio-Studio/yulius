# 0364. A server start refreshes every sandbox's skills, and the reviewer's prompt is a contract

## Status

Accepted (2026-11-17). Specified in
[SPEC-507](../../.ai/specs/SPEC-507-after-an-update-every-sandbox-gets-the-n.md); task #489. Builds
on [ADR-0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) (the safe
install) and [ADR-0341](0341-yulius-picks-the-model-of-each-review-round.md)
(the reviewer agents); replaces the clause of
[ADR-0362](0362-a-round-is-decided-by-the-code-not-by-ci-or-a-message.md) that put the CI rule, in
short, into the reviewer agents' prompt.

## Context

- Skills reached a sandbox's home only at a run's start there. After the update of 17 Nov 2026
  (22:18 UTC, SPEC-506's new review rules), four of five busy sandboxes kept 6 of 13 skills of the
  build before for half an hour or more; their running sessions read the replaced rules. The
  person refreshed three by starting and deleting a task in each.
- A switched skill likewise waited for the sandbox's next run.
- The reviewer agents' prompt goes to a session at its launch (`--agents`). SPEC-506 had added the
  CI rule to it, so a session launched before the update kept the old prompt beside the new skill.
- What a session asks the server (hooks, MCP tools, a review round's message) is answered by the
  running server; the briefing, the launch settings and `--agents` are fixed at the launch.

## Decision

- **At every server start**, after the catalog is extracted and before background work, queued
  runs and the update's resume start, Yulius compares each sandbox's installed skills with the
  set a run there would get and installs the set, with the run start's install, into each home
  where anything differs. Running tasks do not hold it back (a copy is swapped in whole by rename).
  A sandbox with no home waits for its first run. Copies that match are not rewritten.
- **The comparison** is a SHA-256 digest of each catalog skill (with the install's rules: links
  inside the skill to its own files are read through, others left out, the marker left out)
  against one of the installed copy read through folder handles without following a link, within
  the install's bounds. A link, a FIFO or anything else in a copy, a copy without the marker, and
  a managed copy of a skill that left the set count as different.
- **Switching a skill** on or off installs the sandbox's set at once.
- **Installs into homes are serialized** with the launch lock, which a run's install already holds.
- **Each refreshed sandbox is audited** as `skills.refreshed` (actor `system`, target its slug,
  detail the server's commit and the skills installed and removed). A failure is logged and never
  fails the start, the switch or a run.
- **The reviewer agents' prompt is a contract**: who the reviewer is, that it loads
  `om-code-review` (and the repository's review skill) and works read-only, own code or
  pre-existing, and the verdict line the code parses. Review rules live in the skill, which the
  refresh delivers and the reviewer reads at each use; the CI rule leaves the prompt. A test holds
  the prompt to the contract. Working sessions are not relaunched.

## Alternatives

- **Reinstall everything everywhere at each start.** Swaps every skill folder under every running
  session for nothing, and an audit entry at each start would say nothing.
- **A version stamp in the marker file** instead of a content digest: an agent's edit of the copy
  leaves it untrue, and a catalog change without a new build (an instance skill) would not show.
- **Refresh at the update's end only** (the helper or the resume): a plain restart, a rollback or
  a crash restart would leave homes behind, and the resume would thaw tasks onto old skills first.
- **A new reviewer prompt for each round through the PreToolUse hook** (rewriting the subagent's
  task prompt): the launch prompt would still say the old thing beside it, and it adds text to
  every round. Relaunching sessions was ruled out by the person.

## Consequences

- An update, a restart or a switched skill reaches every sandbox's skills within the server's
  start, also under running sessions; a skill a session has open keeps its old files open, and the
  next read gets the new copy.
- The start reads each home's copies once: 13 skills of a few hundred KiB per sandbox.
- A session launched before an update keeps its briefing, launch settings and reviewer prompt until
  it is launched again; none of them carries a rule an update changes as long as the review rules
  stay in the skill. A change that must reach a round at once goes into the hook's round message.
