# Development environment

## Bootstrap

Supported development hosts: macOS and Linux. Python 3.11+, rustup, Git and a C linker
are required. Yulius CI (`[[ci.jobs]]` in `yulius.toml`) is the only CI and runs the gate on
Linux; macOS is checked by running `scripts/dev ci` on a Mac before promotion. Windows support for a
future process supervisor is undecided; the shell entry point is not a Windows promise.

```sh
python3 scripts/bootstrap.py
scripts/dev doctor
scripts/dev ci
```

The compiler/components are pinned in `rust-toolchain.toml`. Exact Cargo tool
versions are in `.config/tools.toml`. Bootstrap installs tools from locked crates.io
packages into ignored `.tools/`, skips matching installed versions and can be rerun
after failure. It does not change `rustup default`. `--install-rustup` installs rustup
(no default toolchain, no profile edits) on a clean machine; unattended runs (`CI` set or no
terminal on stdin, as in Yulius CI) do so without the flag. First installation can take
several minutes and requires network access. Inspect the script before executing it.

`scripts/dev` invokes `rustup run <pin> cargo xtask`, so a Homebrew compiler earlier
in PATH cannot override the project. Direct `cargo xtask` is equivalent only when
Cargo is the selected rustup toolchain and `.tools/bin` is on PATH. Prefer the wrapper.
Python's standard-library `tomllib` means Python 3.10 and older are unsupported.

## Gates

| Command | Actual work |
| --- | --- |
| `scripts/dev doctor` | Compiler/tool versions and Python prerequisite |
| `scripts/dev check` | Local Markdown links, no merge conflict marker lines outside vendored assets, skills (frontmatter, "Use when" description, body size, deployments equal `.apm/skills`), Cargo inheritance/layers, one integration test binary for the composition crate, web UI design tokens (no colour, type, radius or spacing literals in `styles.css`; WCAG contrast of every theme in `tokens.css`); checker regression tests; locked all-target compilation |
| `scripts/dev fmt` | Rust formatting mutation |
| `scripts/dev lint` | Formatting verification and Clippy with warnings denied |
| `scripts/dev test` | nextest workspace tests with zero retries; doctests for eligible library targets |
| `scripts/dev deps` | cargo-deny advisories/licenses/bans/sources and cargo-shear (unused crate and workspace dependencies) |
| `scripts/dev ci` | doctor, check, lint, test, deps, then rustdoc with warnings denied |
| `scripts/dev security` | Vidoc scan of the files the branch's commits changed since `dev` (pinned CLI in `tools/vidoc`); fails unless the scan completed for this branch with no open critical or high finding. Needs a key and the network; not part of `ci`, but Yulius CI runs it as the `security` job on pull request heads |

Use `scripts/dev check`, `scripts/dev lint` or `scripts/dev test` during iteration.
The `yulius` crate's integration tests are modules of one test binary, `tests/main.rs`
([ADR-0028](../decisions/0028-one-integration-test-binary.md)): declare a new file there, and run
one file's tests with `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^<file>::/)'`
(`cargo test -p yulius --test main <file>::` filters by substring: `supervision::` also runs
`pool_supervision::`).
Use the full gate after implementation stabilizes.

The web UI has end-to-end tests in [`e2e/`](../../e2e/README.md): flows, accessibility (axe-core)
and page errors, run in a browser against the UI's mock mode. They need Node 24.8+ (test-only;
the UI has no Node at runtime) and are not part of `scripts/dev ci`. Run them after UI changes:
`cd e2e && npm ci --ignore-scripts && npx e2e run`.
No provider login, database, Docker daemon or paid model call is part of ordinary CI.
Product crates are added per the SPEC-002 delivery checkpoints.
Its binary-only workspace has no doctest targets; the gate reports this as not
applicable and automatically runs doctests when library targets are added.

`cargo-deny` fetches RustSec advisory data; fresh evidence requires network access.
If a network failure prevents that, report it as unverified rather than suppressing
advisories. The JUnit report is under `target/nextest/ci/junit.xml`. Rustdoc output
is under `target/doc`. Build artifacts and local tools are ignored by Git.

## Security scan

`scripts/dev security` ([SPEC-128](../../.ai/specs/SPEC-128-in-keys-for-agents-you-have-vidoc-securi.md),
[ADR-0104](../decisions/0104-agents-scan-their-branch-with-vidoc-before-delivery.md)) runs
[Vidoc](https://docs.vidocsecurity.com/cli/ci-cd)'s `vidoc ci` on the current branch and then
reads the verdict from the API. It needs Node 20+ and an API key in `VIDOC_TOKEN` or
`VIDOC_API_KEY` (`VIDOC_API_URL` defaults to `https://api.vidoc.dev`). In agent sessions,
`VIDOC_API_KEY` is the project's agent secret: a placeholder that the egress proxy replaces for
`api.vidoc.dev` once a person has allowed that host. The CLI uploads the working tree, untracked
files included, and scans the changed files as they are on disk, so the gate refuses a tree that is
not clean. It also checks that `git remote show origin` names the default branch: without access
to origin, the CLI would compare with `origin/main`. A scan takes 10 to 15 minutes, so run the gate
with a tool timeout of 45 minutes or more, or in the background.

The gate is on. `enabled = false` in `.config/vidoc.toml`
([SPEC-131](../../.ai/specs/SPEC-131-we-d-like-to-be-able-to-enable-and-disab.md),
[ADR-0107](../decisions/0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md)) turns it off
for every branch that has that commit: it then prints `Security scan disabled: ...; nothing was
scanned.` and exits 0 without a key, Node or the network. Agents report such a run as disabled,
never as passed. Turn it off or on in a reviewed pull request that says why; an uncommitted
`enabled = false` exits 2, and without the file the gate is on. Exit codes:

- 0: passed, the branch changed nothing that Vidoc scans, or the gate is disabled (its message says
  so).
- 1: failed. The causes: open critical or high findings (each one is printed); a failed or foreign
  scan; an unexpected API answer; a CLI error, a timeout or a signal; a failed `npm ci`.
- 2: not run. The causes: a malformed or uncommitted `.config/vidoc.toml` switch; no key, a
  malformed key, or a rejected one; Vidoc unreachable or out of quota (402, 429) before the scan;
  "nothing to scan" although the branch changed source files; no Node 20+ or npm; a detached
  HEAD; uncommitted or untracked files; a first remote other than `origin`, or origin unreachable
  the way the CLI checks it; no default branch or no merge base.

Vidoc never indexes `bin/`, `vendor/`, `target/`, `build/`, `dist/`, `out/` and similar directories,
so `crates/yulius-ptyhost/src/bin/` and vendored web assets are outside the scan. Agents run the
gate for every change before delivery. They fix findings test-first. A false
positive is ignored in Vidoc, with the reason, only after the independent reviewer agrees, and it
is listed in the spec's Evidence. To update the CLI, change the pin in
`tools/vidoc/package.json`, run `npm install --package-lock-only` there, and check that the
`Scan started with ID:` line still appears in its output.

Yulius CI also runs the gate, as the `security` job of `yulius.toml`, on every pull request head
([SPEC-142](../../.ai/specs/SPEC-142-fix-466-yulius-ci-jobs-opt-in-to-named-a.md),
[ADR-0116](../decisions/0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md)); `dev` and
`main` heads print that they are not scanned. A `secrets = "VIDOC_API_KEY"` line on the job, read
from `dev`'s `yulius.toml` only, gives it the agent secret's placeholder and the egress proxy with a
credential of its own, never the value; the proxy adds the key only toward `api.vidoc.dev`, and
each use is in the secrets audit as `ci:<run>`. That line is not in `yulius.toml` yet: it waits until the running Yulius instance knows `secrets` (after `dev` is promoted to `main`; an older instance rejects the whole `yulius.toml`), so until then the job has no key and exits 2. A CI checkout is a detached worktree without a
GitHub credential, so on a pull request the gate scans a private clone of the head on branch
`yulius-ci/pr-<n>` whose `origin` answers from a local mirror of origin's default branch (`dev`). Findings (exit 1) fail the
job and so the merge gate. Exit 2 does not fail it: while the stored key is rejected
([#470](https://github.com/Quartio-Studio/yulius/issues/470)) every pull request would otherwise be
blocked by something only a person can fix. The job then ends with `[yulius] the security scan did
not run`, which is not a pass: agents still run the gate themselves and report it as unrun.

## Dead code

Cleanup requests follow the bundled skill `dead-code-cleanup`
([catalog](../../catalog/skills/yulius/dead-code-cleanup/SKILL.md),
[ADR-0037](../decisions/0037-dead-code-tools.md)). In this repository the gate already fails on
Rust's `dead_code` and `unused_*` warnings (`scripts/dev lint`), on unused dependencies
(`scripts/dev deps`, cargo-shear) and on unused web UI imports (`scripts/dev check`). On demand,
check the Python scripts with `uvx vulture scripts/ --min-confidence 80` and
`uvx skylos scripts/ --no-upload`, and search for items a library crate exports that no other crate
uses (the compiler does not report them). Without `uvx`, use `pipx run` or a virtual environment.

## Add a crate

Add its path to workspace members, inherit `edition`, `rust-version`, `license`
and `[lints] workspace = true`, mark internal packages `publish = false`, and set
`[package.metadata.yulius] layer` to the layer in [dependency boundaries](architecture.md). Define its boundary and
acceptance tests first. The repository checker uses Cargo metadata, including
renamed and target-specific path dependencies; missing classifications fail closed.
Keep module-local AGENTS.md short and specific only when module complexity warrants it.

## Update tools or dependencies

Update the exact pins, rerun bootstrap, generate/review Cargo.lock if dependencies
changed and pass the full gate. Tool installation uses `--locked`; application
commands use `--locked`. No wildcard dependencies or unreviewed Git sources.
An advisory exception must name the advisory, affected dependency, rationale,
owner and expiry; prefer upgrading. Do not replace a broad license allowlist to
make one dependency pass. New feature combinations require an explicit test matrix.

## Editors and CI

VS Code settings recommend rust-analyzer, Clippy and format-on-save. Other editors
can use the same rust-analyzer component and toolchain. No global editor settings
or agent permission overrides are installed.

Yulius CI is the only CI ([SPEC-002 ADR-18](../../.ai/specs/SPEC-002-yulius-architecture.md),
ADR-21): there are no GitHub Actions. It runs `quality` and `e2e` (`[[ci.jobs]]`) and publishes
the `yulius/ci` commit status, which the rulesets require. The `e2e` job gets no model key (agent
steps are skipped). [Git delivery](git.md) defines promotion checks and post-merge evidence.

## Troubleshooting

- Wrong compiler: use `scripts/dev`, then `rustup run 1.99.0 rustc --version`.
- Missing tool/version drift: rerun bootstrap; do not replace the command with a no-op.
- Build cache lock: another Cargo command may be installing/building; poll the live
  process rather than starting duplicate installers.
- Missing linker on macOS: install Xcode Command Line Tools; on Ubuntu install
  `build-essential`, `pkg-config` and `libssl-dev` if a tooling dependency needs them.
- Deliberately skipped work: record the exact failed command and its prerequisite.
