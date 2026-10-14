# 0104. Agents scan their branch with Vidoc before delivery; Yulius CI does not

## Status

Accepted. Specified in [SPEC-128](../../.ai/specs/SPEC-128-in-keys-for-agents-you-have-vidoc-securi.md).

## Context

The person asked: "In keys for agents you have vidoc security api key. Use it and bake it into
testing". The project has the agent secret `VIDOC_API_KEY`. Agent sessions can use it: the egress
proxy replaces the placeholder `yulius-secret-VIDOC_API_KEY` in requests to the hosts a person
allowed (ADR-0098). Yulius CI jobs cannot: they run with no secret in their environment and
without the proxy (`ci/exec.rs`), so code from a pull request head never reaches a credential.
Vidoc's CLI command `vidoc ci` scans the files that a branch changed since its merge base with the
default branch, in 10 to 15 minutes. It exits 0 even when it finds issues, so the documented way to
fail on findings is to read the scan status and the open issues from the REST API.

## Decision

`scripts/dev security` (`scripts/vidoc_scan.py`) is the security gate. It installs the Vidoc CLI
pinned in `tools/vidoc/package-lock.json` and runs `vidoc ci`. It fails unless this run's scan
completed for the current branch and the branch has no open critical or high finding, and it
prints each finding. A branch with no file that Vidoc scans passes, and the output says that it
had nothing to scan. Agents run the gate for every change before delivery (skill `yulius-verify`).
A failure counts like any other failed gate: fix the finding test-first. For a false positive, the
independent reviewer must agree first. Then the agent ignores the finding in Vidoc with the
reason, and lists it in the spec's Evidence and in the summary. When the gate cannot run (no usable
key, a detached HEAD, uncommitted files, an unreadable default branch: exit 2), the agent reports
it as unrun, never as passed. The gate refuses a dirty tree because the CLI uploads and scans the
working tree, not the commits. `scripts/dev ci` and
Yulius CI do not run the gate.

## Alternatives

- **A `security` job in `[[ci.jobs]]`.** This is the strongest enforcement, but it is not possible
  today: CI jobs hold no secret and have no proxy. Making them use agent secrets changes a security
  boundary (pull request code would reach a credential through the proxy) and needs its own
  design. Issue #466 tracks it.
- **Put the key into CI's environment.** This would expose the value to any code in a pull
  request. Rejected.
- **`npx @vidocsecurity/cli@<version>`.** It pins the version but not the content. A lockfile pins
  both with integrity hashes.
- **A REST-only scan (`POST /v1/scan-workflows/start`).** It scans what Vidoc already indexed
  (a repository connected through the GitHub integration). It does not scan the local branch that
  the agent is about to deliver.

## Consequences

- Each delivered change is checked by Vidoc, but enforcement depends on the agents' discipline and
  the review, not on the merge gate, until the follow-up issue lands.
- A scan takes 10 to 15 minutes of an agent's turn and uses the Vidoc plan's quota. The gate runs
  once per task, before the review, and again only when the code changed after a finding.
- The gate needs `api.vidoc.dev` bound to `VIDOC_API_KEY` and Node 20 or later. Without them it
  says why it did not run.
- The CLI uploads the branch's code to Vidoc Cloud and sends its own logs to Sentry. The gate
  gives it an allowlisted environment, which keeps GitHub and model tokens away from it, but that
  is not containment: the CLI runs as the agent.
- Vidoc never indexes `bin/`, `vendor/`, `target/` and similar directories, so
  `crates/yulius-ptyhost/src/bin/` and the vendored web assets are outside the scan.
- Vidoc keys expire (default one month). An expired key gives exit 2 with "Invalid or expired
  token", and a person has to replace the value on the Secrets page.
