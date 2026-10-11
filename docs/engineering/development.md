# Development environment

## Bootstrap

Supported development hosts: macOS and Linux. Python 3.11+, rustup, Git and a C linker
are required. GitHub CI is configured on Ubuntu and macOS, and Yulius CI (`[[ci.jobs]]` in
`yulius.toml`) runs the same gate on Linux; GitHub Actions are removed once Yulius CI reports
commit statuses (macOS is then checked by running `scripts/dev ci` locally). Windows support for a
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
| `scripts/dev check` | Local Markdown links, skills, Cargo inheritance/layers, web UI design tokens (no colour, type, radius or spacing literals in `styles.css`; WCAG contrast of every theme in `tokens.css`); checker regression tests; locked all-target compilation |
| `scripts/dev fmt` | Rust formatting mutation |
| `scripts/dev lint` | Formatting verification and Clippy with warnings denied |
| `scripts/dev test` | nextest workspace tests with zero retries; doctests for eligible library targets |
| `scripts/dev deps` | cargo-deny advisories/licenses/bans/sources and cargo-machete |
| `scripts/dev ci` | doctor, check, lint, test, deps, then rustdoc with warnings denied |

Use `scripts/dev check`, `scripts/dev lint` or `scripts/dev test` during iteration.
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

GitHub Actions uses full-commit action pins, read-only permissions and the same
bootstrap/CI commands. `quality (ubuntu-latest)` runs on pull requests and dev/main
pushes, plus a weekly advisory refresh. `quality (macos-latest)` runs on dev/main
pushes, promotion pull requests into main, the weekly refresh and manual runs
(`workflow_dispatch`, also the way to test a branch on macOS); on pull requests into
dev it is skipped. `Branch policy` is its own workflow (`policy.yml`), so editing a
pull request's title or description re-runs only the policy. The workflow is
committed configuration; hosted execution is only proven when GitHub actually runs
it. No deployment job is present. [Git delivery](git.md) defines promotion checks and
post-merge evidence. Pinned tool binaries are cached per OS/architecture/tool pin;
Rust dependencies are cached per OS (written by dev/main pushes, read by pull
requests). Jobs run on [Blacksmith](https://docs.blacksmith.sh/) runners
(`blacksmith-4vcpu-ubuntu-2404` for Linux checks, `blacksmith-6vcpu-macos-latest` for
macOS, `blacksmith-2vcpu-ubuntu-2404` for the policy); the check names stay the ones
the rulesets require, and Blacksmith serves `actions/cache` and `Swatinem/rust-cache`
from its own cache without workflow changes. Without the Blacksmith app, switch
`runs-on` back to `ubuntu-latest` / `macos-latest`. `Web UI e2e` (`e2e.yml`) runs the
end-to-end suite when the UI, the suite or the workflow changes; it is advisory, not a
required check, and gets no model key (agent steps are skipped).

Projects that Yulius manages run their CI on Yulius itself, with no GitHub Actions
([SPEC-002 ADR-18](../../.ai/specs/SPEC-002-yulius-architecture.md)). This repository still uses
the workflows above. ADR-21 lists the steps to move it.

## Troubleshooting

- Wrong compiler: use `scripts/dev`, then `rustup run 1.99.0 rustc --version`.
- Missing tool/version drift: rerun bootstrap; do not replace the command with a no-op.
- Build cache lock: another Cargo command may be installing/building; poll the live
  process rather than starting duplicate installers.
- Missing linker on macOS: install Xcode Command Line Tools; on Ubuntu install
  `build-essential`, `pkg-config` and `libssl-dev` if a tooling dependency needs them.
- Deliberately skipped work: record the exact failed command and its prerequisite.
