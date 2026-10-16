---
yulius-task: 0196e6fe-dde5-4f5e-be8e-d998c6a45912
status: done
---

# SPEC-195: Vidoc is built into Yulius: an optional instance key, and a security scan on vidoc-1

Status: done
Date: 2026-10-16
Owner: Yulius task #181
Scope authorization: task #181 (requested by MrFishPL): "Integrate Vidoc into Yulius as an
optional, instance-level service, the same way the Jev key is. Rebuild the security scan on
Vidoc's current API, so that the Vidoc scan actually runs for this project (and for any project
whose instance has a key) once this task is merged." Follow-on task #160 runs the broad scan.

## Goal

A superadmin can give the instance one Vidoc key, the way they give it the Jev key. The key is
optional, and without it nothing changes and the scan says "not run". With the key, every
project's agents and granted CI jobs scan code with Vidoc's `vidoc-1` model through
`model.vidocsecurity.com`. They never hold the key: the egress proxy adds it. Two scans exist:

- **Change scan**: the files a branch changed against the integration branch.
- **Broad scan**: the whole repository, in bounded chunks.

Infra bootstrap adds the scan to a project only when the instance has a key. This repository's
`security` job really scans pull requests.

## Context

- **The old scan is dead.** `scripts/vidoc_scan.py` (SPEC-128/131/142/162, ADR-0104/0107/0116/0133)
  drives the Vidoc CLI `@vidocsecurity/cli` 0.1.25 (`tools/vidoc`) against `https://api.vidoc.dev`.
  That API answers every key with `401 token_invalid`. So no scan ever ran here, and since
  2026-10-14 every PR's `security` job ended skipped (#470).
- **The current API.** `https://model.vidocsecurity.com` is an Anthropic-compatible Messages API.
  - `GET /v1/models` lists `vidoc-1`.
  - `POST /v1/messages` with `x-api-key` and `anthropic-version: 2023-06-01` answers a normal
    Messages response with `thinking` and `text` blocks.
  - Verified 2026-10-16 through this project's agent secret: a 19k-token chunk with a planted
    `sh -c` injection answered in 9 s with a correct `critical` finding and the right line.
- **The Jev key** is the pattern to mirror: `crates/yulius-app/src/jev_key.rs`, the `instance` row
  (migration 0008), `JevKeyView` and `JevTestResult`, `/api/admin/jev*`, `assets/jevkey.js`,
  `YULIUS_JEV_API_KEY_FILE`, the audit (`jev.key_set`, `jev.key_removed`), and the optional last
  setup step (ADM-001, ADM-002).
- **Agent secrets and the proxy** (ADR-0098, ADR-0116). An agent run gets
  `NAME=yulius-secret-NAME` for each agent secret of its sandbox (`vault_run_setup`). A CI job
  gets that only for the names its integration branch's `[[ci.jobs]] secrets` grants
  (`ci_job_secrets`). The proxy replaces placeholders for bound hosts only (`egress_apply`) and
  redacts every applicable secret. Instance-wide agent secrets exist, but their names are unique
  across scopes. This sandbox already has a project-level `VIDOC_API_KEY` bound to
  `model.vidocsecurity.com`, so an instance-wide secret of the same name would be refused.
- **The running instance is built from `main`.** On this PR and on the next ones, the instance
  that runs the CI jobs does not know the instance key yet. Only the project secret reaches the
  job. The grant is read from `dev`, which already says `secrets = "VIDOC_API_KEY"`.
- **Infra bootstrap is static text.** `catalog/workflows/infra-bootstrap.md` and the bundled skill
  have no templating. The onboarding prompt (`onboarding.rs`) and the briefing (`briefing.rs`,
  `BriefingInput` flags such as `docker`) are composed per sandbox.

## Requirements

### Instance key (VDK)

- **VDK-001.** The `instance` row holds an optional Vidoc key, sealed with the master key, with
  who set it and when (migration 0034). `YULIUS_VIDOC_API_KEY_FILE`, an owner-only file, is used
  while nothing is saved ("from config"). A saved key wins, and changes take effect at once,
  without a restart.
- **VDK-002.** Only a superadmin can view the state or change the key:
  - `GET /api/admin/vidoc`: `VidocKeyView`, with the same fields as `JevKeyView`.
  - `PUT /api/admin/vidoc` with `{key}`: validated (8 to 4096 printable characters, no spaces),
    sealed, and audited `vidoc.key_set`.
  - `DELETE /api/admin/vidoc`: audited `vidoc.key_removed`; the config file applies again, if any.
  - `POST /api/admin/vidoc/adopt`: moves the config file's key into settings.
  - `POST /api/admin/vidoc/test`: one `GET /v1/models`, bounded at 12 s, that must list `vidoc-1`.
    It answers `VidocTestResult` with `ok`, `refused` (401/403), `unreachable`, `timeout` or
    `invalid`.
  No response, log or audit entry contains the key.
- **VDK-003.** The Instance page has a "Vidoc key" card that sets, replaces, tests, removes and
  adopts the key. It says what Vidoc does, that code is sent to Vidoc, and that without a key the
  scan is off.
- **VDK-004.** First-time setup offers the Vidoc key as an optional step after the Jev step. The
  person can skip it, and skipping changes nothing.
- **VDK-005.** The key reaches no agent and no browser in clear. When the instance has a key and
  the sandbox has no agent secret named `VIDOC_API_KEY`, the key acts as a virtual agent secret
  `VIDOC_API_KEY`, bound to `model.vidocsecurity.com` only, with injection by placeholder:
  - agent runs and the Superagent get `VIDOC_API_KEY=yulius-secret-VIDOC_API_KEY`;
  - a CI job whose integration-branch grant names `VIDOC_API_KEY` gets the same placeholder;
  - the proxy replaces it toward that host only, audits each use as `vault.agent.use` with secret
    `instance:vidoc`, and redacts the value from responses, terminals and transcripts;
  - `secrets_list` shows it as the instance's Vidoc key;
  - it is never resolved for MCP `${secret:…}`, `secret_copy_to_env` or `browser_fill_secret`,
    and it is not listed on the Secrets page.
  A project's own `VIDOC_API_KEY` wins over the instance key.

### Scan (VSC)

- **VSC-001.** `scripts/vidoc_scan.py` is the one scanner. It is stdlib-only Python 3.11+, and the
  `yulius` binary embeds it as `yulius security-scan [ARGS]`, which runs it with
  `python3 -I`. Agents reach that command through the sandbox `yulius` shim, and CI jobs through
  the binary bound read-only into their containment. In this repository, `scripts/dev security`
  runs the in-tree file.
- **VSC-002.** The scan always reads commits, never the working tree: file contents come from
  `HEAD`'s tree, and so does `.config/vidoc.toml`. A dirty tree or a detached HEAD is fine.
- **VSC-003. Change scan** (default): the files that differ between `merge-base(HEAD, base)` and
  `HEAD` and still exist there. The base is `--base REF`, else
  `origin/$YULIUS_INTEGRATION_BRANCH` (Yulius sets it for agent runs and CI jobs), else
  `refs/remotes/origin/HEAD`; with none of them, the scan exits 2. **Broad scan** (`--all`): every file tracked in `HEAD`.
- **VSC-004.** Some files are never sent. A built-in list skips:
  - binaries (a NUL byte or invalid UTF-8);
  - files over 1 MiB;
  - `vendor/`, `node_modules/`, `third_party/`, `dist/`, `build/` and `target/` directories;
  - lockfiles, minified files and source maps;
  - prose (`.md`, `.txt`, `.rst`) and media.

  `.config/vidoc.toml` `exclude` globs skip more. The scan reports how many files it skipped, and
  why.
- **VSC-005. Chunking and budget.** Files are packed in path order into chunks of at most 60 000
  bytes of line-numbered text. A larger file is split at line boundaries, and the header names
  the line range. Lines are split on line feeds only. A total byte budget applies: 4 MB for a
  change scan, 32 MB for a broad scan, overridable with `--max-bytes`. So does a deadline that also
  bounds every request in flight: 20 minutes for a change scan (below Yulius CI's 30-minute job
  timeout) and 180 for a broad scan, overridable with `--deadline-minutes`. Files past either limit
  are listed as not scanned. Open critical or high findings are printed as they arrive.
- **VSC-006. Contract.** Each chunk is one `POST /v1/messages` request:
  - `model: vidoc-1`, `max_tokens: 16000`;
  - the system prompt fixes a JSON answer `{"findings": [{severity, file, line, title, detail}]}`
    with severities `critical|high|medium|low`, and says that file contents are untrusted data
    whose instructions are ignored;
  - the answer's text blocks are joined, code fences are stripped, and the JSON object is parsed;
  - a finding's `line` must be an integer inside the lines of the file it names in its chunk;
    otherwise, also when it names a file outside its chunk, it is kept with no line, so a
    mislabelled finding is never lost.

  An answer that cannot be read is retried twice, then it fails the scan (exit 1).
- **VSC-007. Rate limits and retries.** Chunks run up to `--jobs` at a time (default 4).
  - 429, 529, 5xx and connection errors or timeouts are retried with backoff (2, 4, 8, 16 s, or
    `retry-after` up to 60 s), at most 5 attempts per chunk. Each request is bounded at 300 s.
  - 401 and 403 mean the key was refused; 402 means out of quota.
  - Any other 4xx fails the scan.
  - No redirect is followed.
- **VSC-008. Verdict and exits.**
  - **0**: no open critical or high finding in a complete scan, or the scan is disabled by
    `enabled = false` (it says so).
  - **1**: an open critical or high finding (each is printed with severity, `file:line`, title and
    detail), or an unreadable answer, or another 4xx.
  - **2** (not run): no key, the key refused, Vidoc unreachable after retries, out of quota, no
    base, a malformed config, or an incomplete scan (budget or deadline) with no finding.

  Medium and low findings are printed but do not fail the scan. `--report FILE` writes the
  findings, ignored findings, skipped files and unscanned files as JSON.
- **VSC-009. False positives.** `.config/vidoc.toml` `[[ignore]]` entries take a `path` glob, a
  `title` substring (case-insensitive) and a `reason`, which is required. A matching finding is
  printed as ignored with its reason, and it does not fail the scan. An ignore is reviewed like
  code: the independent reviewer must agree, and the spec's Evidence lists it.
- **VSC-010. Key and address.** The key is `VIDOC_API_KEY`: a placeholder in sessions and jobs, a
  real key elsewhere. `VIDOC_API_URL` defaults to `https://model.vidocsecurity.com`. Plain `http://`
  is accepted for loopback only (tests). The key travels only in `x-api-key`.
- **VSC-011. CI mode.** `--ci` scans only on a pull request head (`YULIUS_CI_REF=pr/<n>`, change
  scan). It maps exit 2, nothing to scan, a disabled scan and any other head to
  `${YULIUS_CI_SKIP_CODE:-0}` with `[yulius] security scan skipped: …` (ADR-0133: a job that
  checked nothing ends skipped).
- **VSC-012.** `api.vidoc.dev`, the CLI, `tools/vidoc` and `VIDOC_TOKEN` are gone from code and
  docs. Historical specs and ADRs keep their record, and a new ADR supersedes their CLI parts.

### Bootstrap (VIB)

- **VIB-001.** The briefing of a sandbox whose instance has a Vidoc key has a short
  "## Security scan" section. It names `yulius security-scan` (and `--all`), its exit codes, and
  that code goes to Vidoc. Without a key, there is no such section.
- **VIB-002.** The onboarding prompt, and the infra-bootstrap workflow and skill, add a `security`
  CI job (`secrets = "VIDOC_API_KEY"`, command `yulius security-scan --ci`) and the AGENTS.md rule
  to run `yulius security-scan` before delivery, when the instance has a Vidoc key. Without one,
  they skip the job and say so in the summary ("no Vidoc key on this instance").
- **VIB-003.** This repository's `security` job runs `python3 scripts/vidoc_scan.py --ci` with its
  grant. AGENTS.md, `docs/engineering/development.md`, skill `yulius-verify`, `docs/operations.md`
  and `docs/api.md` describe the new scan, the broad scan (`scripts/dev security --all`, for
  #160), false positives, and that code goes to Vidoc.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The security scan runs on vidoc-1: change and broad scans against a fake Vidoc, old CLI removed
- [x] The instance Vidoc key is stored, managed and tested by the superadmin API
- [x] The instance key reaches agents and granted CI jobs through the proxy as VIDOC_API_KEY
- [x] The Instance page and the setup wizard manage the Vidoc key
- [x] yulius security-scan runs the embedded scanner for agents and CI jobs
- [x] Briefing, onboarding and infra bootstrap add the scan only when the instance has a key
- [x] This project scans with vidoc-1: security job, AGENTS.md, docs, skill and ADR-0157
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- A superadmin sets, tests, replaces, removes and adopts the Vidoc key. Others get 403, and no
  response contains the key.
- Setup offers the Vidoc step and the person can skip it.
- Without any key, the scan exits 2 and the CI job ends skipped.
- With a key, against a fake Vidoc:
  - the change scan sends only changed files and passes on clean code;
  - the change scan fails (exit 1) on a planted `os.system("ls " + name)`;
  - the broad scan sends every scannable file in bounded chunks;
  - 429 is retried, 401 and 402 exit 2, and an ignored finding passes.
- One live smoke run against `model.vidocsecurity.com` is recorded in Evidence.
- An agent run in a sandbox without its own `VIDOC_API_KEY` gets the placeholder when the instance
  has a key, and the proxy injects the key toward `model.vidocsecurity.com` only. A granted CI job
  gets the same; an ungranted job gets nothing.
- The briefing and the onboarding prompt carry the security-scan text only when the instance has
  a key.
- No `api.vidoc.dev`, `@vidocsecurity/cli`, `tools/vidoc` or `VIDOC_TOKEN` remains outside history
  (old specs, ADRs and changes).
- After the merge, this project's PR `security` job scans and reports pass or fail.

## Test plan

- **Python** (`scripts/test_vidoc_scan.py`, rewritten): unit tests for the config (switch,
  exclude, ignore), file selection, chunking, answer parsing and validation, the verdict, and the
  CI mapping. Integration tests run a real git repository with a base branch and a feature branch
  against a fake Vidoc HTTP server on loopback. They cover the change scan, the broad scan, the
  planted injection, 429 then 200, 401, 402, an unreadable answer, the budget, no base, and that
  no request carries anything but the `x-api-key` header value it was given.
- **Rust integration** (`crates/yulius/tests/vidoc_key.rs`): the superadmin API with a fake
  `VidocConnector`, the config file and adopt, 403 for others, the audit, and no key in any
  response. In `secrets.rs`-style tests: the virtual secret's placeholder in a run's environment,
  injection toward the bound host only, a project secret winning, a CI grant, and redaction.
- **Rust unit**: the briefing section and the onboarding prompt with and without a key; the shim
  offers `security-scan`; `yulius security-scan` runs the embedded file.
- **e2e** (`e2e/tests/update.e2e.ts`): the Instance card, and the skippable setup step, on the mock.
- **Live**: a change scan of a scratch repository with the planted snippet, and the
  `security` job of this PR (the current instance gives it the project secret).

## Decisions

- **The scanner stays Python, embedded in the binary** (ADR-0157). Other options:
  - A Rust scanner in `yulius` would put two implementations side by side, or force this
    repository's CI job to compile the binary to test its own scanner.
  - A copy per project would drift.
  - Embedding one stdlib-only file keeps one source. This repository's pull requests test their
    own revision of the scanner, and other projects get the instance's.
- **A virtual agent secret, not a stored instance-wide one.** Names are unique across scopes, and
  this sandbox already has `VIDOC_API_KEY`. The instance key also needs the Jev key's lifecycle
  (setup step, config file, test). Injecting it only where secrets are applied keeps it out of
  MCP configuration, `.env` copies and browser filling, where it would leave in clear. The
  project's own secret wins: it is the more specific choice, for example a separate quota.
- **Same name, `VIDOC_API_KEY`.** Job grants and scripts work unchanged, whether the instance
  or the project supplies the key.
- **The scan reads `HEAD`, config included.** The old dirty-tree and detached-HEAD refusals
  existed only because the CLI uploaded the working tree. Reading the switch from `HEAD` keeps
  ADR-0107's rule that only a committed `enabled = false` turns the scan off, without a separate
  check (amends ADR-0107).
- **False positives are committed ignores**, reviewed like code. Vidoc's new API has no server-side
  issue state to ignore in.
- **An unreadable answer fails, a refusal or outage is not run.** A pull request can steer the
  model's output (prompt injection), so treating garbage as "not run" would let it skip the scan.
  Only the causes a person fixes (key, quota, service) skip. Prompt injection that makes the model
  say "no findings" is a known limit, stated in the docs.
- **Budgets are explicit and an incomplete scan never passes** (exit 2 unless it found something).

## Evidence

- **Red, then green.** Each new test was run before its change and failed for the stated reason:
  - `scripts/test_vidoc_scan.py` (43 tests) against the old CLI scanner: 3 failures, 47 errors.
    The round-1 tests (deadline, streaming, `--ci` skip of nothing checked, integration base,
    names with a line break, `\n`-only lines, error body, unwritable report) were each red
    before their fix.
  - The `vidoc_key::` tests did not compile without `VidocServices` and the routes. The proxy
    test then failed at `egress_intercepts(model.vidocsecurity.com)` until the virtual secret
    existed.
  - Each of these failed with the change reverted on its own:
    - `local_ci::a_granted_job_gets_the_instance_vidoc_key_without_a_project_secret` (the CI
      lookup through `applied_agent_secrets`);
    - `pool_supervision::an_agent_run_gets_the_instance_vidoc_key_as_a_placeholder`
      (`vault_run_setup`);
    - `local_ci::a_ci_job_runs_the_embedded_security_scan` (`ci_path`).

    The `YULIUS_INTEGRATION_BRANCH` assertions failed before the environment was set.
  - `projects::…security_scan…`, `briefing::…security_scan…` and `onboarding::…vidoc_key` failed
    before the onboarding and briefing texts.
- **`scripts/dev ci` on 9b26895a** (after merging `dev`) exited 0: nextest 1993 passed, 18
  skipped; Python 77 OK; lint, deps, rustdoc and the repository check passed.
- **`scripts/dev ci` on 6753b646** (the round-1 fixes) exited 0: nextest 1994 passed, 18
  skipped; Python 85 OK.
- **e2e:** `npx e2e run` on 9b26895a passed 585 tests, 1 skipped (56 files). `update.e2e.ts`
  covers the Vidoc setup step, saved and skipped, and the Instance card.
  `a_contained_ci_job_runs_the_embedded_security_scan` skips in this sandbox (bubblewrap cannot
  run here, like its sibling `a_contained_granted_job_reads_its_ca_bundle`); Yulius CI hosts run it.
- **Live, against `model.vidocsecurity.com` (2026-10-16).** Every run went through the egress
  proxy, with this project's agent secret `VIDOC_API_KEY` as a placeholder:
  - `GET /v1/models` answered `{"data":[{"id":"vidoc-1",…}]}`, the shape the key test expects.
  - A scratch repository, base `origin/dev` plus a branch adding `app/run.py` with
    `os.system("ls " + name)`:
    - `python3 -I scripts/vidoc_scan.py`: `[high] app/run.py:5 OS command injection`, exit 1.
    - The built `yulius security-scan`: `[critical] app/run.py:5 …`, exit 1.
    - `--all`: one chunk, the same finding, exit 1.
  - With `VIDOC_API_KEY=not-a-real-key-123`: `Security scan not run: Vidoc refused the key
    (HTTP 401 …)`, exit 2.
  - This branch's own change scan at 1486e773: 7 files in 2 chunks (76 kB), no open critical or
    high finding, exit 0. The reviewer's dry count at 9b26895a was 44 files in 24 chunks, 1.4 MB.
  - Not run: a live broad scan of this repository. It is about 15 MB and 260 chunks, which is task
    #160's audit.
- **This PR's `security` CI job** runs this branch's scanner with the project's agent secret (the
  grant is read from `dev`). Its result is in the pull request's checks and the closing summary.
- **Review.** Round 1 (9b26895a) requested changes: 2 medium findings (the deadline bound, this
  empty Evidence), 8 minor ones and nits, all in this change's own code. All were fixed test-first
  in 6753b646. Two choices were explained and accepted rather than changed: findings that name
  another file are kept (VSC-006), and CI jobs get no extra shim. Round 2 (6753b646) found every
  code finding fixed and asked only for this Evidence.
- **Not testable here.** The running instance is built from `main`, so the instance key, its
  Instance card and `yulius security-scan` for other projects reach this instance after a person
  promotes `dev`. Until then this project's PR jobs scan with its own agent secret.
- **The first real scan in Yulius CI.** This PR's `security` job, run 50ca979b on b047560e, sent
  45 files in 28 chunks through the proxy with the project's agent secret. It failed (exit 1) with
  3 high "missing authorization" findings, all in `crates/yulius-server/web/assets/mock.js`, the
  UI's in-browser fake backend (`?mock=1`), and 15 medium or low notes.
- **Exclusions and ignores.** `.config/vidoc.toml` now excludes `mock.js`, with its reason
  committed beside it: it serves fake data in the viewer's own browser, and the real checks live
  in `yulius-app`, which is scanned. The exclusion holds only while `mock.js` has no DOM sink. The
  independent reviewer agreed in round 4. No `[[ignore]]` entries are needed.
- **The change scan of 06187277**, live: 22 chunks, no open critical or high finding, exit 0. Its
  11 medium or low notes are in code this change did not write:
  - The `checkout_name` "path traversal" is a false positive: job names are validated as
    `[a-z0-9][a-z0-9_-]*`.
  - The others (the workspace-token search cost, bracketed-paste content, the egress client
    cache's TTL, TOCTOU in path checks, the `.gitconfig` login) are left to the broad audit,
    task #160.
