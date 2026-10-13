---
name: infra-bootstrap
description: Lay a project's engineering foundations the way a senior engineer would - strong unit and integration tests, lint and format, one local gate that our own CI runs (no GitHub Actions), dev and production environments with deploy and verification, AGENTS.md, architecture and product docs, GitHub labels and templates. Use on the first prompt of a fresh project, during onboarding, or when the briefing lists missing foundations.
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
  each, through the normal pipeline (`delivery-pipeline`).

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
     `[[ci.jobs]]` entries (`name`, `command`, optional `matrix`) when parallel jobs pay off.
2. **CI on our own machines.** Translate every `.github/workflows/*.yml` job into the Yulius CI
   configuration so nothing is lost (in Yulius, `ci_setup` lists the workflows, jobs, steps and
   matrices to translate): each job's run steps become the gate script or a `[[ci.jobs]]` entry,
   matrices become `matrix` values, actions (`uses:`) become the equivalent commands, services
   (databases, caches) become disposable local containers or processes started by the script, and
   secrets come from the project's agent secrets, never inline. Yulius CI gives the jobs a `HOME`
   and a cache (`$YULIUS_CI_CACHE`) of their own, separate from the agent's: they start empty and
   persist across runs, as does each job's checkout per CI lane with its ignored files. By default
   up to two runs at once share that `HOME` and cache, so a job's installs there must tolerate a
   concurrent run. A job installs what it needs itself and never depends on what an earlier run left
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
3. **Environments.** Define dev (after integration) and production (after promotion) in
   `yulius.toml` `[delivery]` as `[[delivery.environments]]` entries with an idempotent deploy
   `command` and `verify = "command"` for a smoke test against the deployed app (health endpoint,
   one real user path). Deploy scripts read secrets from the environment, roll forward safely,
   and print where the app runs.
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
- Dev and production deploy and verify by command.
- The docs describe the project as it is, and each choice has an ADR.
- The critical and high-cost conventions are codified as primitives or checked by the gate.
