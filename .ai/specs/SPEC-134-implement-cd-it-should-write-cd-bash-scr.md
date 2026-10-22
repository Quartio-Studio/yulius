---
yulius-task: 7caf70b1-25f1-4986-a274-7ce2babebc65
status: done
---

# SPEC-134: CD (deployment): dev and production deploys that agents build but cannot cheat

Status: done
Date: 2026-10-14
Owner: Yulius task #113
Scope authorization: the person's request (task #113, which replaces the archived task #112):
"Implement CD. It should write CD bash scripts that handles deployment. It should have forced
discipline and be protected and work at the same level as CI. […] dev deployment = installing on
some machine (it's not specified yet - i'll later just add vps credentials to the vault and the
agent should be able to set up everything), prod deployment = pushing to main and installing on our
machine (VMs self-updates themselves). Dev deployment can be done by agent but main deployment
should require strong confirmation and be always done by humans. CD can evolve in time […] we dont
have dev deployment at the moment so it'll be nearly empty (but should be!!!) but we have main
deployment. […] On onboarding it should be detected whether CD already exists. On configuring CD
user can be prompted. To make something be in "needs you" the agent should explicitly call a tool
and prompt the user to do something, for example to put the credential in secrets etc. CD dev +
prod should be explicitly defined but both can be empty […] possible for non-tech but techy users
to configure using our harness. […] make sure there are no leftovers. […] in CD section add
deployment in brackets: "CD (deployment)". […] make sure we have integration tests. And make sure
what you build generalises to all other projects (like studio, product etc). Decide how much of the
infra scripts should be commited to github", and the two additions: "it should not be done with
github but be a separate mechanism like CI. Make sure agents can build and improve the infra but
can't cheat it" and "there is no need for dev deployment after every dev merge. It should be
something separate. For example we can set an automation to deploy to dev every night and test it".

## Goal

Every Yulius project has a CD (deployment) next to its CI (tests): two explicitly defined
environments, **dev** and **production**, each with an optional deploy script and an optional
verify script committed to the repository. Yulius runs them itself, like CI: from a clean checkout
of a protected branch's exact commit, inside the sandbox's containment, with only the secrets the
environment names, recorded with their output. Agents write and improve the scripts through the
normal pull-request pipeline and may deploy dev; only a person deploys production, after typing a
confirmation. A dev deploy is its own action (a person, an agent, or a nightly automation), not a
step of every merge.

## Context

- **CI already is the model** (DLV-006, ADR-0015, `crates/yulius-app/src/ci/`): configuration in
  `yulius.toml` read from the integration branch head (a pull request cannot weaken its own CI);
  jobs run through the process host inside the sandbox's containment (`ci/exec.rs`), with a
  persistent home and cache, a timeout, an output tail and a log; results in `ci_runs`/`ci_jobs`;
  the CI page (`pages/ci.js`, nav "CI (tests)") and the tools `ci_status`/`ci_rerun`/`ci_setup`.
  `layout.js` already says "a CD page would join CI here".
- **Deploy environments exist but are not CD.** `[[delivery.environments]]` (DLV-001/003/008) are
  per-task checkpoints of the delivery path. Their commands live in the database profile that only
  an administrator edits, run **unsandboxed with the server's privileges** in the shared clone,
  get **no secrets**, and are tied to each merged task (`delivery_deploy`). The `product` preset's
  `dev-vm` and `rollout` are manual checkpoints with no command. Nothing reads `[delivery]` from
  `yulius.toml` after the import.
- **Promotion is open to agents.** `delivery_promote` acts with `Initiator::User` under the task
  owner's authority; only the tool's description asks agents not to use it. For Yulius, pushing to
  `main` is the production deployment: instances update themselves from `main` when a superadmin
  chooses Update now (ADR-0013, `docs/operations.md` "Updates from main").
- **Secrets** (ADR-0014/0098/0105): agents never see values; the egress proxy adds them for bound
  hosts; `secret_copy_to_env` writes a value into a git-ignored `.env` once a person allowed that
  secret (`env_files = allowed`: "its value may be used outside the proxy"). CI jobs get no secret.
  ADR-0105 was written to prepare this task.
- **"Needs you"** (ADR-0068) is per task: a held prompt, a browser request, or an agent that stopped
  and waits. `browser_request_user_action`, `secret_request_hosts` and `secret_copy_to_env` flag the
  task, but there is no general tool for "a person has to do this".
- **Automations** (ADR-0034) start tasks from a prompt on a cron; the ready-made catalog lives in
  `catalog/automations/`.
- **Onboarding** (`crates/yulius-app/src/onboarding.rs`, ADR-0094) lays the gate and says missing
  deploy environments "need a person's machines and credentials: file an issue". The
  `infra-bootstrap` skill's step 3 describes `[[delivery.environments]]`.
- **Leftovers of task #112.** The earlier, archived attempt left no branch (local or remote), no
  worktree, no spec file and no commit; its reserved spec number SPEC-125 was never written.

## Requirements

Configuration (CDC):

- CDC-001. `yulius.toml` defines CD with two tables, `[cd.dev]` and `[cd.prod]`, each with optional
  keys `deploy` (one-line command), `verify` (one-line command, a smoke test of what is deployed),
  `secrets` (space-separated agent-secret names, at most 16) and `about` (one line for people,
  ≤ 300 characters); `[cd.prod]` also takes `promote = true|false` (default false). Unknown keys,
  `promote` under dev, multi-line or overlong commands and invalid secret names are errors with a
  line number, like every other table. [SPEC-257](SPEC-257-current-ci-cd-is-too-strict-dev-prod-is.md) extends this
  to any `[cd.<name>]` with a `stage`: the two tables stay the defaults, and "dev" and
  "production" in this spec are the stages.
- CDC-002. Each environment is in one of three states: **not defined** (its table is missing),
  **empty** (the table is there, with nothing to run: no deploy, no verify, no promotion) or
  **ready**. Both may be empty; "not defined" is a gap the briefing and onboarding report.
- CDC-003. Yulius reads dev's table from the integration branch head and production's from the
  commit it deploys: the integration head decides whether production promotes (its `[cd.prod]`
  says `promote = true`), after a promotion the new production head is read and authorized again,
  and without one the production head is read (its own `promote` is ignored). Each commit is the
  one GitHub reports for the branch, taken from the sandbox clone into a mirror only Yulius writes
  with every object checked and replacement refs ignored. A task branch, a worktree, the sandbox
  clone or the database never changes what a deploy runs.
- CDC-004. Scripts live in the repository (`scripts/cd/` by convention) and are reviewed and tested
  like code; machines, addresses and credentials are agent secrets, never committed; run records
  and logs live in Yulius, not in Git.

Runs (CDR):

- CDR-001. A deploy of an environment is one **CD run**: Yulius verifies the commit (CDC-003),
  reads its table, writes the commit's files into the run's private directory and runs `deploy`
  and then `verify` (each skipped when absent, `verify` only after a successful `deploy`) through
  the process host: dev inside the sandbox's containment, production in the sandbox's network
  only (no host loopback) on the host's read-only system with empty `/var`, `/root` and `/home`. The run's files, `HOME` and log are bound
  only into its own mount namespace and removed with it; its cache is `<data>/cd/<sandbox>/cache/
  <env>`. Its result is `passed` (every step exited 0), `failed`, `error` (could not run, timed out,
  lost) or `cancelled`.
- CDR-002. A run's environment has no task token, provider credential or GitHub token; it has
  `YULIUS_CD=1`, `YULIUS_CD_ENV` (`dev` or `prod`), `YULIUS_CD_SHA`, `YULIUS_CD_BRANCH`,
  `YULIUS_CD_RUN`, a `HOME` made for the run and removed after it, a persistent `YULIUS_CD_CACHE`,
  and each named secret's real value. A secret is given only when it is an agent secret of the
  project allowed for that environment: for dev, allowed outside the proxy (`env_files = allowed`,
  ADR-0105); for production, allowed for production deploys (`cd_prod`, its own allowance, set by
  someone who manages the secret, `POST /api/agent-secrets/{id}/cd-prod`; exclusive: not while
  agents can use it through `.env` copies or proxy hosts, and dev never gets it). A missing or not-allowed
  secret refuses the run before anything starts (before a promotion) and says what a person must
  do. For production, a secret agents use is `agents_use`, not `not_allowed`: nobody can allow it,
  so the refusal says production names a separate secret (SPEC-237).
- CDR-003. Stored output is the run's tail with every agent secret's value replaced by
  `[secret:NAME]`.
- CDR-004. One active run per environment and project; a second request is refused while one runs.
  A run is bounded by the CI job timeout; cancelling kills its process group. A run that was
  running when Yulius restarted (or that is cancelled without an executor) has its process host
  terminated and its files removed, is recorded as `error` ("outcome unknown") and is never
  started again by itself. Runs are kept 14 days, 50 per environment, and each environment's last
  passed run always.
- CDR-005. Every run and refusal of a production deploy is audited (`cd.deploy`, `cd.finished`,
  `cd.refused`) with the actor (`login`, `task:#N`).

Who deploys (CDA):

- CDA-001. **Dev**: a person with write access (the CD page, `POST /api/sandboxes/{sid}/cd/dev/deploy`)
  or an agent (`cd_deploy`), including an automation's task. No step of a task's delivery waits
  for it.
- CDA-002. **Production**: only a person with administrator access, through the CD page or the API
  with `{"confirm": "<project slug>"}` typed exactly. Agents are refused, whatever they pass.
  With `promote = true` the deploy first promotes the integration branch with the profile's method
  (it opens the promotion pull request, or merges it when its checks passed; a promotion that cannot
  complete now refuses the deploy and says why); then it deploys the new production head.
- CDA-003. Agents never promote: the `delivery_promote` tool is removed, and `delivery_deploy`
  refuses production-stage environments. The promotion API (`POST …/delivery/promote`) needs an
  administrator and `{"confirm": "<slug>"}`, like a production deploy. A profile's `auto_promote`,
  set by an administrator, keeps working.

Agents and people (CDT):

- CDT-001. MCP `cd_status`: each environment's state, commands, secrets (ready, missing, waiting for
  a person's allowance, or for production one agents use, which it never gets, SPEC-237), the
  branch head, the last run and the page link.
- CDT-002. MCP `cd_deploy {environment: "dev"}`: starts a dev run and returns its id; refuses
  production (CDA-002), an empty or undefined environment, missing secrets (asking a person to
  allow the ones that exist, like `secret_copy_to_env`) and a running run.
- CDT-003. `yulius wait` also returns when a CD run the task started ends, with its result, also
  when it ended before the wait began (reported once).
- CDT-004. MCP `needs_you {action, why?, link?}`: the agent asks a person to do something only a
  person can (add a credential, allow a secret, deploy production). The task shows "Needs you:
  <action>" (sidebar group Needs you, on every page that groups tasks) until a person writes to the
  task (a prompt they typed, or continuing it; prompts Yulius types do not count) or the agent calls
  `needs_you {resolved: true}`; an ended task shows none. The request is audited (`task.needs_you`).
  Its link must be a page of this Yulius.
- CDT-005. The **CD (deployment)** page (nav entry next to "CI (tests)") shows dev and production:
  state, what runs, secrets and their state with a link to Secrets, the deployed commit against the
  branch head, the last run, Deploy buttons (dev: confirm; production: a dialog that says what will
  happen and asks for the project's slug), recent runs with their output and Cancel, and "Set up CD
  with an agent", which starts an Infra bootstrap task with a CD prompt, so a person who does not
  write scripts configures CD through the harness.

Teaching (CDK):

- CDK-001. Onboarding detects existing CD (deploy scripts, container and platform files such as
  `Dockerfile`, `compose.yaml`, `fly.toml`, `vercel.json`, `netlify.toml`, `Procfile`, `k8s/`,
  Ansible or Terraform, deploy jobs in `.github/workflows`) and maps it into `[cd.dev]`/`[cd.prod]`
  with scripts under `scripts/cd/`; when there is none, both tables are written empty with an
  `about` saying so. Credentials it needs are asked for with `needs_you`.
- CDK-002. The `infra-bootstrap` skill, its workflow and the `delivery-pipeline` skill describe CD as
  above; the briefing reports "CD (deployment) not defined" as a missing foundation and ends the
  delivery path at the merge (deploys are CD runs).
- CDK-003. A ready-made automation "Deploy dev every night" starts a task that runs `cd_deploy dev`,
  waits for it, and files an issue when it fails.
- CDK-004. Yulius's own `yulius.toml` defines CD: dev empty (no dev machine yet: an agent writes
  `scripts/cd/` once a person adds the machine's credentials), production `promote = true` (instances
  update themselves from `main`).

## Plan

- [x] CD configuration parsed from yulius.toml with the rule of who may deploy each environment
- [x] CD runs recorded and executed in the sandbox from the protected branch, with allowed secrets
- [x] Production is deployed only by a person who types the confirmation, promoting first
- [x] Agents deploy dev, wait for it and ask a person through needs_you
- [x] The CD (deployment) page shows both environments, deploys them and starts a setup task
- [x] Onboarding, skills, briefing and a nightly automation teach CD
- [x] Yulius's own CD defined, docs and decisions updated
- [x] Review round 1: deploys run verified commits out of the agents' reach, fixed test-first
- [x] Review round 2: production off the host loopback, exclusive secrets, commit-bound confirmation
- [x] Review round 3 approves the exact head
- [x] Every gate green and the merge into dev armed on Yulius CI

## Acceptance criteria

- AC1 (CDC-001..003): a configuration with both tables, with empty ones, with one missing, and each
  invalid form parses to the documented states or errors; a task branch that changes `[cd.dev]`
  changes nothing a deploy runs until it is merged.
- AC2 (CDR-001..003, CDA-001): an agent's `cd_deploy dev` runs the integration head's deploy and
  verify scripts in a fresh checkout; the run passes, its output holds `[secret:NAME]` but never
  the value, and the scripts saw the value, `YULIUS_CD_*` and no task token.
- AC3 (CDR-001): a failing deploy skips verify and the run is `failed`; a failing verify is
  `failed`; an empty or undefined environment and a missing or not-allowed secret are refused
  before anything runs.
- AC4 (CDA-002/003): an agent's production deploy, `delivery_promote` (no such tool) and a
  production-stage `delivery_deploy` are refused; a person without the typed slug, or below
  administrator, is refused; with it, production is promoted and the production head is deployed.
- AC9 (review C1, H1, H2): a replacement planted in the sandbox clone and files planted in the
  sandbox change nothing a deploy runs; a production secret allowed only for `.env` is refused
  until it is allowed for production deploys; a lost run leaves no files.
- AC5 (CDR-004): a second request while a run is active is refused; a run left running at a restart
  is recorded as `error`.
- AC6 (CDT-003/004): `yulius wait` returns when the task's CD run ends; `needs_you` puts the task
  in Needs you until a person writes to it or the agent resolves it.
- AC7 (CDT-005): the CD (deployment) page shows the two environments and deploys dev; production asks
  for the slug (e2e).
- AC8 (CDK-*): the briefing, onboarding prompt, skills and the catalog automation say the above
  (unit tests on their text and the catalog parser).

## Test plan

- Domain unit tests: `cd` configuration parsing and states, the deploy rule (who, which
  environment, the typed confirmation), `config.rs` errors with line numbers.
- Store tests: CD runs (insert, one active per environment, finish, restart reconcile, prune).
- Integration tests (`crates/yulius/tests/cd.rs`, real Git and the real process host as in
  `local_ci.rs`): AC1–AC6 through MCP and the HTTP API, including the anti-cheat case (a worktree
  and a task branch with another `[cd.dev]`), secrets and redaction, refusals, promotion with the
  fake GitHub, restart reconcile, `yulius wait`.
- e2e: the CD page (`e2e/tests/cd.e2e.ts`), with the axe check every page test has.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **CD is its own mechanism beside CI, not `[[delivery.environments]]`** (ADR-0109). It reuses CI's
  pieces (configuration in `yulius.toml` read from protected branches, the process host, the
  sandbox's containment, logs and output tails) but has its own runs table: deploy runs must never
  count as test results of a commit (the CI planner tests a commit once whatever reference carries
  it) and must never be re-run by themselves after a restart. Alternatives: extend the delivery
  environments (commands in the database, run unsandboxed, tied to every merge: the opposite of
  three of the person's points); reuse `ci_runs` with a new reference kind (deploy results would
  leak into the merge gate and the planner).
- **Configuration and scripts in Git, read only from protected branches** (ADR-0109). Agents improve
  CD by pull requests that pass CI and review; nothing in a task's branch or worktree runs as a
  deploy. Production reads what it deploys, so a script change reaches production only through a
  person's production deploy. Alternatives: the database profile (agents cannot build CD at all),
  the task worktree (an agent could run anything as a deploy).
- **Not on the server; production not in the agents' sandbox** (ADR-0109). Deploy scripts are
  code agents write; running them with the server's privileges (as delivery environments do)
  would hand agents the instance. Dev runs in the sandbox (agents' ground, their tools); production
  runs on the host's read-only system with its own root, since the sandbox's system layer, cache
  and files are writable by its agents (review C1).
- **Secrets per environment** (ADR-0109). Dev takes ADR-0105's allowance (the same exposure as
  `.env`, which agents already use); production has its own allowance, which no agent can use
  otherwise (review H1). Alternatives: one allowance for both (a production credential usable by
  any agent), all of the project's secrets (more than a deploy needs).
- **Production is a person's, always** (ADR-0110). An agent never deploys production or promotes;
  a person with administrator access types the project's slug. Alternatives: a checkbox confirm (a
  click is not strong), two-factor codes (not every account has a second factor), letting agents
  promote when asked (the person said "always done by humans").
- **Dev deploys are separate from merges.** Merges do not trigger dev; an agent deploys dev when its
  task asks, and a nightly automation can deploy and test it. Alternatives: deploy on every merge
  (the person said there is no need), a Yulius-native schedule (automations already schedule work
  and a task can test what it deployed).
- **`needs_you` is one general tool** (ADR-0111). The person asked that something reaches "needs you"
  only when an agent explicitly asks; the existing requests are each bound to one surface (the
  browser, a secret). Alternatives: steps marked blocked (a plan state, not a request to a person),
  issues (they never reach "needs you").
- **Independent review, round 1 (request changes; all fixed test-first).** C1: production runs and
  their sources moved out of the agents' reach (verified mirror, private run files, production
  outside the sandbox). H1: production secrets got their own allowance. H2: lost runs stop their
  host and lose their files. M1/M2: a run executes exactly the commit whose table it read, and
  production is re-read after a promotion; the dialog names the commit it promotes. M3: the
  promotion API follows the production rule. M4: `yulius wait` reports a run that ended before
  it. M5: only a person's prompt ends a `needs_you` request. L1, L2, L4, L5, L6, L7 fixed; L3
  (cancel racing the end) cannot happen: the executor leaves the map only after writing the end.
- **Independent review, round 2 (request changes; all fixed test-first).** H-A: production on the
  host's network reached its loopback; it now joins the sandbox's network only, on the host's
  read-only system. M-A: reported run ends are stored with the run and reported whenever unseen.
  M-B: production secrets are exclusive of agents' uses. M-C: the dialog sends the commit it
  showed and promotion is decided and verified on the mirror. M-D (agents' GitHub token can write
  `main` outside Yulius, predating this task): issue #486, ADR-0110 states the limit. L-a, L-b
  fixed; L-d recorded in ADR-0111; L-e fixed; the ADR-0109 file renamed.
- **Independent review, round 3 (request changes; all fixed test-first).** H1: the process host
  refused a joined run whose system root is `/`, so production would never have launched under
  isolation; the launch spec gained an explicit `host_system` join mode that the process host
  builds itself (host `/usr`, `/etc`, `/opt` read-only, `/var` and the homes empty), tested through
  the wiring, the launch validation and the `bwrap` argv. M1: the run's resolver is bound where the
  host's `/etc/resolv.conf` really points (Ubuntu's link into `/run`). M2: `${secret:NAME}` MCP
  references never resolve a production secret. M3: a promotion takes exactly the confirmed
  commit (`dlv_promote_exactly`), refused before anything moves otherwise. L1: a CD end is
  reported before a merge outcome; L3: agents cannot ask for production secrets and dev shows
  them as `production`; L4 documented; L2 (`expect` stays optional for API callers, the page
  always sends it) and L5 (two administrators racing on one secret's allowances) accepted as
  low. After three rounds, a confirmation review checks only these fixes on the final head.
- **One pull request.** The parts depend on each other (the page needs the runs, the tools need the
  rule, onboarding teaches the tools); split, each part would ship unusable. The diff is larger
  than the usual 400 lines; the review gets the whole.

## Evidence

- Domain (`cargo nextest run -p yulius-domain`): `cd::tests` (states, who may deploy, the typed
  slug), `spec_format::config::tests::cd_tables_*` (tables, errors with line numbers) and
  `the_repositorys_own_cd_is_defined` (Yulius: dev empty, production a promotion) pass.
- Git adapter (`cargo nextest run -p yulius-git`):
  `the_cd_mirror_takes_only_the_expected_commit_and_ignores_replacements` passes (a planted
  `git replace` never reaches the mirror; a moved tracking ref is refused).
- Store: `cd::tests` (one running run per environment, ends once, retention keeps the deployed run).
- Process host and wiring: `containment::tests::host_system_runs_take_the_hosts_system_read_only`
  and `wiring::session::tests::a_production_cd_run_launches_on_the_hosts_system` (the production
  containment passes the launch validation and becomes a joined bwrap run on the host's read-only
  system; it failed before round 3's fix); `ci::cd::tests::production_runs_on_the_sandbox_network_with_the_hosts_system`.
- Integration (`cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^cd::/)'`): 11
  passed. They cover dev from the integration head with the allowed secret redacted (AC2), a
  task branch changing nothing (AC1), a failing deploy, one run at a time, cancel (AC3), empty
  and undefined environments (AC3), a lost run (AC5, files removed), production by an
  administrator with the typed slug, a stale confirmation refused, promotion, nothing to promote
  (AC4), agents deploying dev, asking with `needs_you`, `yulius wait`, refused production,
  `delivery_promote` gone and production-stage `delivery_deploy` refused (AC2/AC4/AC6), a person's
  prompt ending `needs_you` and a Yulius-typed one not (AC6), and tampering, exclusive production
  secrets (no `.env`, hosts, MCP reference or agent request) and lost-run cleanup (AC9).
- `wait_cli` unit tests: a running deploy keeps the wait; an end before the wait, between two polls,
  or with a merge outcome in the same poll is reported once.
- Onboarding, briefing and automation catalog tests (CDK-001..003, AC8) pass; the delivery,
  automations, import, playground and secrets suites were updated and pass.
- e2e (`npx e2e run`): `cd.e2e.ts` 5 passed (the page, the slug dialog naming the promoted commit,
  members, setup task, `needs_you`), `secrets.e2e.ts` 11 passed (production allowance),
  `navigation.e2e.ts` 8 passed, `menus.e2e.ts` and `workflows.e2e.ts` pass. Whole suite: 433 of 435.
  The one remaining failure, `places.e2e.ts` "a sandbox left without a tool comes back without one",
  fails the same way on `origin/dev` in this sandbox and passes on Yulius CI (dev 3052c54).
- `scripts/dev ci` (format, Clippy, repository contracts, 1713 tests, docs, dependencies): green
  on c6e2fb11.
- `scripts/dev security`: not run (exit 2). Vidoc rejected the agent secret `VIDOC_API_KEY`
  (HTTP 401, invalid or expired token), so the scan is unrun, not passed.
- Independent review in fresh sessions: three rounds, each requesting changes, all findings fixed
  test-first (Decisions), and a confirmation review of the round-3 fixes.
- Not tested here: a real production deploy under sandbox isolation (this sandbox has no user
  namespaces for the holder; the argv and launch validation are tested instead), and a real VPS
  dev deploy (Yulius has no dev machine yet; its `[cd.dev]` is empty).
