---
name: infra-bootstrap
description: Lay a project's engineering foundations the way a senior engineer would - strong unit and integration tests, lint and format, one local gate that our own CI runs (no GitHub Actions), CD (deployment) with dev and production deploy and verify scripts, AGENTS.md, architecture and product docs, GitHub labels and templates. Use on the first prompt of a fresh project, during onboarding, when the briefing lists missing foundations, or to set up or change CD.
---

# Infra bootstrap

Foundations are what make every later task fast and safe. Lay them once, properly, for the
project's own stack, then leave them stable: change them only when the product needs it (for
example when it starts deploying for real), and record every change as a decision.

## Principles

- **Respect what exists.** Read the repository first: language, framework, package manager,
  existing scripts, tests, CI files, deploy scripts, docs and conventions. Map, don't move: keep
  working conventions and wire them into the gate instead of replacing them. Every project has
  its own foundations (a Rust workspace, a TypeScript monorepo, a Python service): pick what is
  state of the art for *that* stack.
- **One gate.** A single command runs everything a merge depends on, the same way on a laptop,
  in the sandbox and on CI.
- **Our own machines only.** CI runs on our own infrastructure through Yulius CI, which runs the
  gate for every pull request head and for the integration and production branches. Never add
  GitHub Actions or anything that needs a paid GitHub plan (rulesets, hosted runner minutes).
- **Decide, don't ask.** Compare options, choose, and write an ADR per lasting choice.
- **Small pull requests.** Deliver the bootstrap as a short series of pull requests, one concern
  each, through the normal pipeline (`delivery-pipeline`). During onboarding, steps 1 and 2 land
  in the onboarding pull request itself, so the project starts work with its gate; never leave
  them as follow-up issues.
- **Ratchet, don't rewrite.** On an existing code base, adopt each check without changing
  application code for findings that already exist: commit the tool's baseline or allowlist, or
  scope the check to what passes today, so the gate passes now and fails on new findings, and
  file one issue to burn the baseline down.

## Steps (each one a pull request of similar effort)

1. **Gate and tests.** Add or complete:
   - formatting check and linter with warnings as errors, type checking where the language has it;
   - a dead-code and unused-dependency check, cheap and with few false positives, pinned and with
     its allowlist committed (skill `dead-code-cleanup` names each language's tools, for example
     vulture or skylos for Python, cargo-shear for Rust, knip for TypeScript);
   - unit tests for the core logic and integration tests at the real boundaries (HTTP, database
     on the real engine in a disposable instance, queues, external APIs behind fakes), fast,
     deterministic and isolated (`test-driven-development`);
   - one entry point that runs them all in order (for example `scripts/ci` or `make ci`), exiting
     non-zero on any failure, plus the dependency and license audit the ecosystem offers;
   - `yulius.toml` `[commands]`: `test`, `lint`, `build`, and `gate` (the entry point), or
     `[[ci.jobs]]` entries (`name`, `command`, optional `matrix`) when parallel jobs pay off;
   - in a monorepo, a CI job for every project (each app, client, package or service with its own
     build) running its own checks, so no project is left without CI;
   - **the security scan, only when the instance has a Vidoc key** (the briefing then has a
     `## Security scan` section): Yulius CI already runs it on every pull request as a built-in
     `security` job (`yulius security-scan --ci`, granted `VIDOC_API_KEY`), so add no job of your
     own. Vidoc's vidoc-1 scans each pull request's changed files: critical or high findings fail
     it, medium findings fail it in the changed lines, findings in untouched code become GitHub
     issues, and a scan that cannot run ends skipped, never passed. Add the AGENTS.md rule that
     every change runs `yulius security-scan` before delivery, fixes each failing finding
     test-first and decides on each low finding in the change (fix it, or say why not in the
     spec's Evidence). Exit 2 is not run, never passed; a false positive
     goes into `.config/vidoc.toml` as an `[[ignore]]` with its reason once the reviewer agrees;
     AGENTS.md says the scan sends the changed code to Vidoc. `yulius security-scan --all` scans
     the whole repository, an audit for a task of its own. Without that section add neither, and
     say in your summary that the scan was skipped: no Vidoc key on this instance.
2. **CI on our own machines.** Translate every `.github/workflows/*.yml` job into the Yulius CI
   configuration so nothing is lost (in Yulius, `ci_setup` lists the workflows, jobs, steps and
   matrices to translate): each job's run steps become the gate script or a `[[ci.jobs]]` entry,
   matrices become `matrix` values, actions (`uses:`) become the equivalent commands, services
   (databases, caches) become disposable local containers or processes started by the script, and
   secrets come from the project's agent secrets, never inline. Yulius CI gives the jobs a `HOME`
   and a cache (`$YULIUS_CI_CACHE`) of their own, separate from the agent's: they start empty and
   persist across runs, as does each job's checkout per CI lane with its ignored files. By default
   up to two runs at once share that `HOME`, so a job's installs there must tolerate a concurrent
   run; the cache is each job checkout's own, never shared with a concurrent job, so build outputs
   (a Cargo target directory) go there or in the checkout, never in `HOME`. A job installs what it needs itself and never depends on what an earlier run left
   behind, or it passes on the cache and fails on a fresh machine. System packages (`sudo apt-get`,
   a browser's system libraries) belong to the sandbox: install them during onboarding, never in a
   CI job, where they race the other CI run for the dpkg lock; jobs see what the sandbox installed.
   Run the gate locally and show it passes, from the repository root both normally and, once
   committed, in a clean checkout with a fresh home and CI's environment (this prints its directory
   and the job's exit status and removes everything it made; for a matrix job add
   `YULIUS_CI_MATRIX=<value>` after `env -i`, once per matrix value):

   ```sh
   d="$(mktemp -d "$HOME/.ci-proof.XXXXXX")"; echo "proof $d"; mkdir "$d/home" "$d/cache"
   git worktree add --detach "$d/repo" HEAD && (cd "$d/repo" && env -i HOME="$d/home" \
     YULIUS_CI_CACHE="$d/cache" PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 TERM=dumb \
     NO_COLOR=1 CI=true GIT_TERMINAL_PROMPT=0 YULIUS_CI=1 sh -c '<command>' </dev/null); s=$?
   git worktree remove --force "$d/repo"; chmod -R u+w "$d"; rm -rf "$d"; echo "exit $s"
   ```

   Run it with a long timeout or in the background: if it is killed, its cleanup never runs, so
   remove what it left at the path it printed. The home is shared with other tasks' running
   proofs, so remove theirs only when they started over six hours ago, then prune:

   ```sh
   d=<path>; git worktree remove --force "$d/repo"; chmod -R u+w "$d"; rm -rf "$d"
   find "$HOME" -maxdepth 1 -name '.ci-proof.*' -mmin +360 \
     -exec chmod -R u+w {} \; -exec rm -rf {} \;
   git worktree prune
   ```

   Then delete `.github/workflows/` in the same pull request, so gates and merges rely only on
   Yulius CI results (`ci_status`). Merge gating needs no rulesets: Yulius enforces the gate at
   merge.
3. **CD (deployment).** Yulius runs CD like CI, never GitHub: `yulius.toml` defines the
   environments, `[cd.dev]` and `[cd.prod]` by default, both always explicit, even when one
   deploys nothing yet.

   ```toml
   [cd.dev]                              # read from the integration branch head
   about = "the team's dev VM"
   deploy = "scripts/cd/deploy.sh dev"
   verify = "scripts/cd/verify.sh dev"   # a smoke test of what is deployed
   secrets = "DEV_SSH_KEY DEV_HOST"      # agent secrets; a person allows each outside the proxy
   states = ["DEV_OPERATOR"]             # agent states: restored before, saved after each run

   [cd.prod]                             # read from what production deploys
   promote = true                        # first promote the integration branch (a push to main)
   deploy = "scripts/cd/deploy.sh prod"
   verify = "scripts/cd/verify.sh prod"

   [cd.platform]                         # one more target, deployed and verified on its own
   stage = "prod"                        # its rules: "dev" (agents, integration branch) or "prod"
   deploy = "scripts/cd/deploy.sh platform"
   verify = "scripts/cd/verify.sh platform"
   ```

   - **One environment per target.** A repository that ships more than one thing (a self-hosted
     release and the hosted platform that serves it, a staging machine next to dev) gets one
     `[cd.<name>]` table per target (lowercase letters, digits and `-`), each with
     `stage = "dev"` or `stage = "prod"`. The stage carries the rules of dev or production: the
     branch, who deploys it (production stage: only a person), where it runs and which secrets
     it may get. Keep `[cd.dev]` and `[cd.prod]` for the main targets; one script deploying
     everything at once hides which target failed.

   - **Find what exists first**: deploy or release scripts, a Dockerfile or compose file,
     platform files (`fly.toml`, `vercel.json`, `netlify.toml`, `Procfile`, `k8s/`, Helm,
     Terraform, Ansible), deploy jobs in `.github/workflows`, the README. Move them into
     `scripts/cd/` instead of inventing a second way. When nothing deploys yet, write the table
     empty with an `about` line ("no dev machine yet"): empty is a decision, a missing table a gap.
   - **Scripts, not commands.** Each environment runs `deploy` and then, if it succeeded, `verify`
     (`/bin/sh -c`, one line each, usually a committed bash script). Scripts are idempotent (a
     second run changes nothing), roll forward safely, print where the app runs, and exit non-zero
     on any failure (`set -euo pipefail`). `verify` checks the deployed app from outside (a health
     endpoint, one real user path), not the build.
   - **What is committed.** Scripts and the tables are reviewed and tested like code (add
     `bash -n` or `shellcheck` of `scripts/cd/` to the gate). Machines, addresses, keys and tokens
     are agent secrets named in `secrets`, never in the repository; the run gets their real values
     as environment variables, only once a person allowed each one for that environment on the
     Secrets page (dev: outside the proxy; production: "for production deploys"). Do not write
     secrets to disk: the run's `HOME` is removed after it, `$YULIUS_CD_CACHE` persists for caches
     only.
   - **How it runs.** Yulius runs the exact commit GitHub reports for the protected branch (never
     your task branch, worktree or the sandbox clone), in a directory of its own, with
     `YULIUS_CD_ENV`, `YULIUS_CD_SHA` and `YULIUS_CD_BRANCH`, keeps a redacted output tail, and
     allows one run per environment at a time. Dev runs in the sandbox with its tools; production
     runs outside it on a clean system, so production scripts install what they need (a CLI, a
     package) into `$YULIUS_CD_CACHE` themselves. You improve CD only through a pull request, and
     nothing you merge reaches production until a person deploys it.
   - **State a deploy tool keeps** (an inventory, rollout receipts, locks, pinned host keys) is an
     agent state, never a file in the repository or on the laptop: name it in `states`. Before
     the deploy the run restores the vault's newest version into a folder of its own and exports
     its absolute path as `$YULIUS_STATE_<NAME>` (scripts refuse to run without it); after the
     deploy and the verify, failed ones included, Yulius saves what changed as a new version.
     A second run naming it waits; a task's save of it is refused meanwhile, and a task restores
     it again (`secret_state_restore`) before changing it. Runs also get the project's variables
     (production only those its table names in `variables = ["NAME"]`). The state is data:
     never run code from it, since dev runs and tasks write it too.
   - **Docker and SSH.** Dev runs have the sandbox's Docker (`docker buildx`, `docker load`);
     production runs have an engine of their own, which no agent reaches (`docker buildx`,
     `docker load`; the run's own files mount with `docker run -v`, nothing of the sandbox does).
     Runs reach any host over SSH (port 22): pin the hosts' keys in
     the state or the repository (`StrictHostKeyChecking=yes` with its own known-hosts file), since
     each run's `HOME` is fresh.
   - **Who deploys.** A merge deploys nothing. Dev: a person on the CD page, or an agent with
     `cd_deploy` when its task asks (then `yulius wait`), or a nightly automation that deploys and
     tests it. Production: only a person with administrator access, typing the project's slug;
     agents never promote or deploy it. What only a person can do (add a machine's credentials,
     allow a secret, deploy production) you ask with `needs_you`, then carry on.
   - Keys the app itself reads have one default place: a `.env` file Git ignores (`.gitignore`
     lists `.env`, `.env.*` and `!.env.example`), beside a committed `.env.example` that names
     every key with no values. Code loads `.env` when its variables are unset. When code that runs
     outside Yulius' proxy needs an agent secret's real value, copy it with `secret_copy_to_env`
     (a person allows each secret once). Never write or edit those lines yourself: you only see
     `[secret:NAME]`.
4. **Docs.** `AGENTS.md` (rules, commands, layout and the docs index), a `CLAUDE.md` pointer to
   it, `docs/product.md` (purpose, users, scope, current state), `docs/architecture.md` with a
   Mermaid diagram, ADRs for the choices above, and the first entry of `docs/changes.md`. When
   the repository already has equivalents, point `yulius.toml` `[docs]` at them.
5. **GitHub hygiene (free features only).** Labels: `type: bug|enhancement|docs|chore|performance`,
   `priority: high|medium|low`, `area: <part>` per main area, `risk: high`. Issue templates with
   the Definition of Ready (summary, current and expected behavior, out of scope, acceptance
   criteria), a pull request template (summary, spec, evidence, review verdict, `Fixes #N`),
   `.gitignore` and `.gitattributes` for the stack, and automatic deletion of merged branches.
6. **Conventions as primitives (instrumentation audit).** Rules that live only in people's heads
   are invisible to agents. List the project's conventions (setup, logging, structure, release,
   review, decision log), note where each lives (heads, code, guidelines, tooling, docs), rank them
   by failure cost (critical: blocks progress or corrupts results; high: hurts reproducibility;
   medium: causes rework; low: style) and map each to a primitive: a path-scoped instruction, a
   restricted agent, a skill, a prompt, a memory or ADR, a spec, or a hook. Prefer a deterministic
   check in the gate over any instruction a model must remember. Write the starter set for the
   critical and high items only, small: skills with a "Use when ..." description, templates in
   `assets/`, noisy work behind a script that prints a short result. Where the project can use an
   agent package manager (`apm.yml`, lockfile, one source directory deployed to each harness, a drift check in the gate),
   set it up as its own pull request and record the choice as an ADR.

## Done when

- The gate passes from a clean checkout and fails on a deliberately broken test.
- No GitHub Actions workflow is left; `[commands] gate` (or `[[ci.jobs]]`) is set and Yulius CI
  reports on it.
- With a Vidoc key on the instance, the `security` job runs `yulius security-scan --ci` and
  AGENTS.md asks for the scan before delivery; without one, the summary says it was skipped.
- `[cd.dev]` and `[cd.prod]` are defined (empty with an `about` when nothing deploys yet), with a
  `[cd.<name>]` and its `stage` for each further target, and each one that deploys has a deploy
  and a verify script that passed on a real run.
- The docs describe the project as it is, and each choice has an ADR.
- The critical and high-cost conventions are codified as primitives or checked by the gate.
