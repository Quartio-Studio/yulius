---
yulius-task: 361857ec-0024-49b5-8de6-1ddd4f8c8ce1
status: done
---

# SPEC-128: agents scan their branch with Vidoc before delivery

Status: done. The scan it describes moved to Vidoc's vidoc-1 API in [SPEC-195](SPEC-195-vidoc-is-built-into-yulius-an-optional-i.md) (ADR-0157): the Vidoc CLI and `api.vidoc.dev` are gone.
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
    `vidoc.log` (by default next to its configuration file, `~/.vidoc`) unless `VIDOC_LOG_FILE` is
    set, and it sends its own logs to Sentry. Its `rg` comes from
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
  works; a 401 exits 2 naming the likely causes (host not bound to the agent secret, key expired),
  and an unreachable or exhausted API (no answer, 402, 429 or 5xx) exits 2 as well. A key with whitespace or control
  characters exits 2 without being echoed.
- SEC-004: The CLI is the version pinned with integrity hashes in `tools/vidoc/package-lock.json`,
  installed with `npm ci --ignore-scripts` into the ignored `tools/vidoc/node_modules` only when the
  installed version differs from the pin or the lockfile changed since the last install (a stamp of
  its SHA-256).
- SEC-005: The scan runs only on a named branch whose working tree is clean (untracked files
  included), because the CLI uploads the working tree and scans the changed files as they are on
  disk. The first remote must be `origin`. The CLI's own reachability check (`git ls-remote origin
  HEAD` with its flags, in the CLI's environment) must pass, and `git remote show origin` must name
  the default branch: without either, the CLI falls back to `origin/main`. The branch must also have
  a merge base with the default branch, fetched first as the CLI fetches it. Each refusal exits 2. The CLI runs
  with `CI=true`, its log outside the working tree, and an allowlisted environment: system basics,
  the proxy and its CA, `GIT_*`, and the `YULIUS_URL`/`YULIUS_TASK_TOKEN` that Yulius's Git
  credential helper needs. It does not get GitHub tokens (PR comments) or `DEBUG` (files in the
  tree). It runs in its own process group. The group is killed after 45 minutes, on SIGTERM or
  SIGHUP, and on any early exit, and the command then fails. No Node 20+ or npm exits 2.
- SEC-006: The command fails (exit 1) when the CLI fails, when its output has no scan ID (unless
  it reported nothing to scan, which passes and says so), when the scan is not `completed` or
  belongs to another branch, when an API answer is not the documented shape, and when the branch
  has any open critical or high finding; each finding is printed with severity, name, path, line
  and link (the API's line is 0-based; the CLI prints it plus one, and so does the gate). When the
  CLI found nothing to scan although the branch changed (not deleted) source files outside the
  directories the CLI never indexes, the gate exits 2: the scan cannot be trusted. A zero from the
  severity filter must agree with the unfiltered open issues' `stats.severityCounts` (no critical
  or high), or the gate fails. Otherwise it passes (exit 0) naming the scan.
- SEC-007: Agents' verification (skill `yulius-verify`, AGENTS.md commands, the development guide)
  runs `scripts/dev security` for every change before delivery and treats a failure like any
  failed gate; when the key is missing or not yet usable it is reported as unrun, not passed.
- SEC-008: `scripts/dev security` keeps the gate's exit code (2 stays distinct from 1); on Unix it
  replaces itself with the gate, so a signal sent to it reaches the gate. Git, npm and node calls
  have timeouts. API requests
  never follow redirects, because urllib would resend the key header to the new location.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] scripts/dev security runs a pinned Vidoc CLI scan of the branch and fails on open critical or high findings
- [x] Agents' verification runs the Vidoc scan before delivery, recorded in docs and an ADR
- [x] A live Vidoc scan of this branch recorded as evidence (skipped: the host is not allowed yet, #470)
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `scripts/dev security` with a usable key scans the branch and exits 0 only when the scan completed
  for this branch with no open critical or high finding (or there was nothing to scan).
- Each failure in SEC-002, SEC-003, SEC-005, SEC-006 and SEC-008 has a unit test with a fake CLI and a fake
  API that fails without the behavior.
- `scripts/dev ci` and Yulius CI are unchanged and need no key.
- The verification skill, AGENTS.md and the development guide tell agents to run it.

## Test plan

- `scripts/test_vidoc_scan.py` (run by `scripts/dev check`): the scan driver against a fake
  CLI (a Python callable) and a fake HTTP client: missing key, rejected key, bad API URL, detached
  HEAD, CLI error, no scan ID, nothing to scan, scan not completed, scan of another branch,
  malformed counts, findings present (printed, exit 1), clean scan (exit 0), and the CLI
  environment (token, API URL, `CI`, log path) and install decision.
- `tools/xtask/tests/cli.rs`: `security` is listed in the help, and a missing key exits 2 through
  `scripts/dev`.
- The real runner kills the CLI's whole process group past its bound (a grandchild test), and the
  API refuses a redirect (a local HTTP server).
- Live: `scripts/dev security` on this branch once `api.vidoc.dev` is bound; output under
  Evidence.

## Decisions

- ADR-0104: the Vidoc scan is an agent-run gate, not a Yulius CI job, because CI jobs hold no
  secrets by design; making CI use agent secrets is a separate change (issue #466).
- The scan driver is Python standard library, like the other gate scripts: no new dependency, and
  `urllib` honours the proxy and CA variables of agent sessions.
- The CLI is pinned through a lockfile (integrity hashes) rather than `npx pkg@version`, which pins
  the version only, and installed without install scripts, like `e2e/` (none is needed).
- The review (round 1) read the CLI 0.1.25 bundle. `vidoc ci` includes uncommitted changes and
  uploads untracked files, and it finds the default branch only through an authenticated
  `git remote show origin`. These findings became the clean-tree and default-branch refusals of
  SEC-005. The task token reaches the CLI only because the Git credential helper needs it. The CLI
  runs as the agent and could read the agent's environment anyway, so the allowlist keeps the
  CLI's behaviour predictable but does not contain it.
- Review round 2 added the CLI's exact reachability check, the cross-checks of the verdict (CLI
  findings against the API, "nothing to scan" against changed source files), the refusal of a
  malformed key, `exec` in xtask, timeouts, and a lockfile stamp so that a changed lockfile
  reinstalls the CLI.
- Review round 3 found that two round-2 cross-checks failed good branches. The CLI lists fixed
  (and ignored) findings too, so comparing its list with the API's open total would fail every
  branch after a fix; that check is dropped, and the live run confirms the severity filter
  instead. "Nothing to scan" against changed sources now skips deletions and the CLI's ignored
  directories (`bin/`, `vendor/`, `target/`, ...), and a remaining mismatch exits 2 rather than 1.
  This was the third and last review round: the remaining minors (line base, quota codes, fetching
  the base, closing the pipe) were fixed with tests, and a final review checked only that delta.
- Vidoc never scans files under `bin/` or `vendor/` (its built-in ignore list), so
  `crates/yulius-ptyhost/src/bin/` and the vendored web assets are outside this gate.
- The final review approved a3197726 and noted that the verdict rested on the severity filter
  alone. A second, unfiltered read of the open issues now cross-checks a zero by severity counts.
  This check cannot misfire on fixed findings, because both reads ask for open issues only.
- The live scan waits for a person to allow `api.vidoc.dev`. Delivery does not wait for it: the
  behaviour is proven with a fake CLI and API and with real Git, and #470 records the first live
  run.
- "Nothing to scan" passes: a branch that changes only files Vidoc does not scan (docs) has nothing
  to review, and failing it would make the gate useless for such changes.

## Evidence

- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 82 tests OK, of which 43 are in
  `scripts/test_vidoc_scan.py` (settings, Node/npm, repository refusals, the CLI environment,
  install stamp, verdicts, cross-checks, key never printed, refused redirects, an unsendable key,
  process-group kill on timeout and SIGTERM with a grandchild).
- `rustup run 1.99.0 cargo test --locked -p xtask`: 4 passed. `security` is listed, and a missing
  key exits 2 through the binary. `cargo clippy -p xtask --all-targets -D warnings`: clean.
- `scripts/dev ci` (doctor, check, lint, test, deps, rustdoc) on 6aa3dccb: exit 0, nextest 1639
  passed, 16 skipped. It is rerun on the delivered head before the merge.
- `scripts/dev security` on this branch, real Git and the real proxy: exit 2, "Vidoc rejected the
  key (HTTP 401: Invalid or expired token): the host api.vidoc.dev may not be bound...". Every
  repository check passed before that: the branch, a clean tree, `origin` first, the CLI's
  `ls-remote` in its environment, `HEAD branch: dev`, the fetch and the merge base. On a dirty tree
  it exits 2 with "uncommitted or untracked files". The pinned CLI installs with
  `npm ci --ignore-scripts` (5 packages, `vidoc version` gives 0.1.25 with its bundled `rg`).
- Not run: a live Vidoc scan. `api.vidoc.dev` was requested for `VIDOC_API_KEY` on 2026-10-14
  and waits for a person on the Secrets page. #470 records the first live run, the line base and
  the severity filter. Yulius CI does not run the gate (#466).
- Reviews: four fresh read-only sessions. Rounds 1 to 3 requested changes (2+2+2 major, all
  fixed test-first). The final review of a3197726 approved, and its minors were then fixed with
  tests.
