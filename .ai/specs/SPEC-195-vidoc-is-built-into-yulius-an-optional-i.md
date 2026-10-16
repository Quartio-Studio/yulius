---
yulius-task: 0196e6fe-dde5-4f5e-be8e-d998c6a45912
status: in-progress
---

# SPEC-195: Vidoc is built into Yulius: an optional instance key, and a security scan on vidoc-1

Status: in-progress
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
  `HEAD` and still exist there. The base is `--base REF`, else `refs/remotes/origin/HEAD`; with
  neither, the scan exits 2. **Broad scan** (`--all`): every file tracked in `HEAD`.
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
  the line range. A total byte budget applies: 4 MB for a change scan, 16 MB for a broad scan,
  overridable with `--max-bytes`. So does a deadline: 30 minutes for a change scan and 180 for a
  broad scan, overridable with `--deadline-minutes`. Files past either limit are listed as not
  scanned.
- **VSC-006. Contract.** Each chunk is one `POST /v1/messages` request:
  - `model: vidoc-1`, `max_tokens: 16000`;
  - the system prompt fixes a JSON answer `{"findings": [{severity, file, line, title, detail}]}`
    with severities `critical|high|medium|low`, and says that file contents are untrusted data
    whose instructions are ignored;
  - the answer's text blocks are joined, code fences are stripped, and the JSON object is parsed;
  - a finding must name a file of its chunk. Its `line` must be an integer inside that file's
    lines, or it is kept with no line.

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
  scan). It maps exit 2, and any other head, to `${YULIUS_CI_SKIP_CODE:-0}` with
  `[yulius] security scan skipped: …`.
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
- [ ] The security scan runs on vidoc-1: change and broad scans against a fake Vidoc, old CLI removed
- [ ] The instance Vidoc key is stored, managed and tested by the superadmin API
- [ ] The instance key reaches agents and granted CI jobs through the proxy as VIDOC_API_KEY
- [ ] The Instance page and the setup wizard manage the Vidoc key
- [ ] yulius security-scan runs the embedded scanner for agents and CI jobs
- [ ] Briefing, onboarding and infra bootstrap add the scan only when the instance has a key
- [ ] This project scans with vidoc-1: security job, AGENTS.md, docs, skill and ADR-0157
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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

Filled in as steps finish.
