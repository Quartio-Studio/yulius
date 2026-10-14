---
yulius-task: 361857ec-0024-49b5-8de6-1ddd4f8c8ce1
status: in-progress
---

# SPEC-128: agents scan their branch with Vidoc before delivery

Status: in-progress
Date: 2026-10-14
Owner: Yulius task #109
Scope authorization: the person's request (task #109): "In keys for agents you have vidoc security
api key. Use it and bake it into testing". Earlier, in the imported build session: "later we'll
add manual tests using e2e testerarmy and Vidoc Lab security check".

## Goal

Every change to Yulius is security-scanned by [Vidoc](https://www.vidocsecurity.com/) before it is
delivered. One command, `scripts/dev security`, uploads the branch to Vidoc, scans the files the
branch changed against `dev`, waits for the scan and fails when Vidoc reports an open critical or
high finding on the branch, printing each one. Agents run it as part of verifying a change, with
the project's agent secret `VIDOC_API_KEY`, which they never see: the egress proxy puts the key in
the request for the bound host `api.vidoc.dev`.

## Context

- The agent secret `VIDOC_API_KEY` ("An API key for Vidoc https://www.vidocsecurity.com/") exists
  for this project, with no host yet. Agent sessions get `VIDOC_API_KEY=yulius-secret-VIDOC_API_KEY`
  and `HTTPS_PROXY`, `NODE_USE_ENV_PROXY=1`, `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE` for the egress
  proxy, which replaces the placeholder in any header for a bound host (ADR-0098, SPEC-121).
- Vidoc documentation (docs.vidocsecurity.com, read 2026-10-14):
  - Cloud API `https://api.vidoc.dev`; REST authentication is the header `x-vidoc-token` only.
  - CLI `@vidocsecurity/cli` (npm, Node 20+; latest 0.1.25). `vidoc ci` uploads new and changed
    files, finds the default branch from `origin` (here `dev`), scans the files the branch's
    commits changed since the merge base, waits, prints validated issues and exits 0 even when it
    found issues or the scan failed; 1 only on errors. It reads `VIDOC_TOKEN` and `VIDOC_API_URL`,
    prints `Scan started with ID: <uuid>`, and prints "No scannable files matched the selected
    scope in the current directory." when the branch changed nothing it scans. It writes
    `vidoc.log` in the current folder unless `VIDOC_LOG_FILE` is set. Its `rg` comes from
    `@vscode/ripgrep`'s per-platform optional packages; no package of the tree has an install script.
  - "Fail a pipeline on issues": check `GET /v1/scan-workflows/:id/status` is `completed` for the
    expected branch, then `GET /v1/issues?codebaseId&branch&status=open&severity=critical,high`
    and fail when `total` is not 0.
- Yulius CI jobs run with no secret in their environment and no egress proxy
  (`crates/yulius-app/src/ci/exec.rs`, `ci_job_env`); `docs/engineering/development.md`: "No
  provider login, database, Docker daemon or paid model call is part of ordinary CI". The e2e job
  already skips steps that need a key it does not get.
- `scripts/dev <task>` runs `tools/xtask`, whose gates call Python scripts under `scripts/`; the
  `check` gate runs `scripts/test_*.py` with `unittest`.

## Requirements

- SEC-001: `scripts/dev security` runs the scan from the repository root; `scripts/dev ci` does not
  include it (it needs a key, the network and 10-15 minutes).
- SEC-002: The key comes from `VIDOC_TOKEN`, else `VIDOC_API_KEY`; the API from `VIDOC_API_URL`,
  default `https://api.vidoc.dev`, which must be an `https://` URL without a path. Without a key
  the command exits 2 and says how to provide one; it never prints the key.
- SEC-003: Before uploading anything, one authenticated read (`GET /v1/codebase`) proves the key
  works; a 401 exits 2 naming the likely causes (host not bound to the agent secret, key expired).
- SEC-004: The CLI is the version pinned with integrity hashes in `tools/vidoc/package-lock.json`,
  installed with `npm ci --ignore-scripts` into the ignored `tools/vidoc/node_modules` only when the installed
  version differs from the pin.
- SEC-005: The scan runs on a named branch (a detached HEAD exits 2), with `CI=true`, the log file
  outside the working tree, and a bound of 45 minutes after which the CLI's process group is
  killed and the command fails.
- SEC-006: The command fails (exit 1) when the CLI fails, when its output has no scan ID (unless
  it reported nothing to scan, which passes and says so), when the scan is not `completed` or
  belongs to another branch, when an API answer is not the documented shape, and when the branch
  has any open critical or high finding; each finding is printed with severity, name, path, line
  and link. Otherwise it passes (exit 0) naming the scan.
- SEC-007: Agents' verification (skill `yulius-verify`, AGENTS.md commands, the development guide)
  runs `scripts/dev security` for every change before delivery and treats a failure like any
  failed gate; when the key is missing or not yet usable it is reported as unrun, not passed.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] scripts/dev security runs a pinned Vidoc CLI scan of the branch and fails on open critical or high findings
- [ ] Agents' verification runs the Vidoc scan before delivery, recorded in docs and an ADR
- [ ] A live Vidoc scan of this branch recorded as evidence
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- `scripts/dev security` with a usable key scans the branch and exits 0 only when the scan completed
  for this branch with no open critical or high finding (or there was nothing to scan).
- Each failure in SEC-002, SEC-003, SEC-005 and SEC-006 has a unit test with a fake CLI and a fake
  API that fails without the behavior.
- `scripts/dev ci` and Yulius CI are unchanged and need no key.
- The verification skill, AGENTS.md and the development guide tell agents to run it.

## Test plan

- `scripts/test_vidoc_scan.py` (run by `scripts/dev check`): the scan driver against a fake
  CLI (a Python callable) and a fake HTTP client: missing key, rejected key, bad API URL, detached
  HEAD, CLI error, no scan ID, nothing to scan, scan not completed, scan of another branch,
  malformed counts, findings present (printed, exit 1), clean scan (exit 0), and the CLI
  environment (token, API URL, `CI`, log path) and install decision.
- `tools/xtask/tests/cli.rs`: `security` is a known command listed in the help.
- Live: `scripts/dev security` on this branch once `api.vidoc.dev` is bound; output under
  Evidence.

## Decisions

- ADR-0104: the Vidoc scan is an agent-run gate, not a Yulius CI job, because CI jobs hold no
  secrets by design; making CI use agent secrets is a separate change (issue filed).
- The scan driver is Python standard library, like the other gate scripts: no new dependency, and
  `urllib` honours the proxy and CA variables of agent sessions.
- The CLI is pinned through a lockfile (integrity hashes) rather than `npx pkg@version`, which pins
  the version only, and installed without install scripts, like `e2e/` (none is needed).
- "Nothing to scan" passes: a branch that changes only files Vidoc does not scan (docs) has nothing
  to review, and failing it would make the gate useless for such changes.
