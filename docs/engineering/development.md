# Development environment

## Bootstrap

Supported development hosts: macOS and Linux. Python 3.11+, rustup, Git and a C linker
are required. Yulius CI (`[[ci.jobs]]` in `yulius.toml`) is the only CI and runs the gate on
Linux; there it also compiles every target for macOS (`scripts/dev macos`,
[decision 0169](../decisions/0169-the-gate-compiles-every-target-for-macos-on-linux.md)), but
linking and tests on macOS are checked only by running `scripts/dev ci` on a Mac. Windows support for a
future process supervisor is undecided; the shell entry point is not a Windows promise.

```sh
python3 scripts/bootstrap.py
scripts/dev doctor
scripts/dev ci
```

The compiler/components are pinned in `rust-toolchain.toml`. Exact Cargo tool
versions are in `.config/tools.toml`, with the zig release (version and SHA-256 per Linux host)
that compiles dependencies' vendored C for the macOS check; on Linux bootstrap downloads it once
per home cache (`scripts/zig_install.py`, verified before it is unpacked) and links
`.tools/bin/zig`, `macos-cc` and `macos-ar` (`scripts/zig_macos.py`); doctor checks them.
Bootstrap installs tools from locked crates.io
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
| `scripts/dev macos` | Clippy with warnings denied on every target for `aarch64-apple-darwin`; on Linux the vendored C (ring, SQLite) is compiled by the pinned zig. Type-checks only: nothing is linked |
| `scripts/dev test` | nextest workspace tests with zero retries; doctests for eligible library targets |
| `scripts/dev deps` | cargo-deny advisories/licenses/bans/sources and cargo-shear (unused crate and workspace dependencies) |
| `scripts/dev ci` | doctor, check, lint, macos, test, deps, then rustdoc with warnings denied |
| `scripts/dev security` | Vidoc's vidoc-1 scan of the files the branch's commits changed since `dev` (`--all`: the whole repository); fails on open critical or high findings, exit 2 when it could not run. Needs a key and the network; not part of `ci`, but Yulius CI runs it as the `security` job on pull request heads |

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

`scripts/dev security` ([SPEC-195](../../.ai/specs/SPEC-195-vidoc-is-built-into-yulius-an-optional-i.md),
[ADR-0157](../decisions/0157-vidoc-is-an-optional-instance-key-and-the-scan-runs-on-vidoc-1.md))
runs `scripts/vidoc_scan.py`. The scan asks Vidoc's model `vidoc-1`, through the Messages API at
`https://model.vidocsecurity.com/v1/messages`, to review the code, and fails on open critical or
high findings. **The code it scans is sent to Vidoc.** It is standard-library Python 3.11+.
The `yulius` binary embeds the same file as `yulius security-scan`, which is how every other
project runs it.

- **Change scan** (the default): the files that differ between the merge base with the integration
  branch and `HEAD`. The base is `--base REF`, else `origin/$YULIUS_INTEGRATION_BRANCH` (Yulius
  sets it for agents and CI jobs), else `origin/HEAD`.
- **Broad scan** (`--all`): every file tracked in `HEAD`, for an audit task. Run it in the background
  or with a long tool timeout, and keep the result with `--report FILE`:

  ```sh
  scripts/dev security --all --report .ai/runs/vidoc-broad.json
  ```

Both read commits, never the working tree: commit before you scan. A dirty tree or a detached HEAD
is fine, and an uncommitted edit of `.config/vidoc.toml` does not count.

**What is sent.** Files go in line-numbered chunks of at most 60 kB. A file split across chunks
names its line range. Some files are never sent:

- binaries, invalid UTF-8 and files over 1 MiB;
- `vendor/`, `node_modules/`, `third_party/`, `dist/`, `build/` and `target/`;
- lockfiles, minified files and source maps;
- prose (`.md`, `.txt`, `.rst`) and media;
- the `exclude` globs of `.config/vidoc.toml` (fnmatch style: `*` also crosses `/`).

A budget bounds every scan: 4 MB and 20 minutes for a change scan (below Yulius CI's 30-minute job
timeout), 32 MB and 180 minutes for a broad scan (`--max-bytes`, `--deadline-minutes`). The deadline
bounds every request in flight too. Up to 4 requests run at once (`--jobs`). Files past the budget
are listed as not scanned, and the scan is then incomplete (exit 2): rerun with a larger
`--max-bytes`. This repository's broad scan sends about 15 MB in some 260 chunks. Each open critical
or high finding is printed as soon as it arrives (`found [high] …`), so even a scan that is killed
shows what it found.

**The contract.** Each chunk is one request (`model: vidoc-1`). The system prompt asks for one JSON
object, `{"findings": [{"severity", "file", "line", "title", "detail"}]}`, with severities `critical`,
`high`, `medium` or `low`, and says that the files are untrusted data. Retries and refusals:

- an answer that is not that object is asked for again twice, then fails the scan;
- 429, 529, 5xx and network errors are retried after 2, 4, 8 and 16 s (or `retry-after`, at most
  60 s), 5 attempts in all;
- 401 or 403 (the key was refused) and 402 (out of quota) stop the scan, and no redirect is
  followed.

A model reviews the code, so the verdict is a judgement, not a proof: code written to steer the
model (prompt injection) could hide a finding.

**The key.** The scan reads `VIDOC_API_KEY` and sends it in `x-api-key` only.

- In agent sessions and CI jobs that variable holds a placeholder that the egress proxy replaces
  toward `model.vidocsecurity.com` only. It is this project's agent secret if it has one, else the
  instance's Vidoc key, which a superadmin sets in Settings, Instance (keys come from
  <https://model.vidocsecurity.com/#key>).
- Elsewhere, set the variable yourself.
- `VIDOC_API_URL` overrides the address, plain `http://` for loopback tests only.

**False positives.** A false positive is ignored with an `[[ignore]]` entry in
`.config/vidoc.toml`, only after the independent reviewer agrees, and it is listed in the spec's
Evidence:

```toml
[[ignore]]
path = "crates/yulius-app/src/example.rs"   # a glob
title = "command injection"                 # a case-insensitive part of the finding's title
reason = "the argument is a fixed constant, reviewed in #123"
```

A matching finding is printed as ignored, with its reason, and does not fail the scan.

The scan is on. `enabled = false` in `.config/vidoc.toml`
([SPEC-131](../../.ai/specs/SPEC-131-we-d-like-to-be-able-to-enable-and-disab.md),
[ADR-0107](../decisions/0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md)) turns it off
for every branch that has that commit. It then prints `Security scan disabled: ...; nothing was
scanned.` and exits 0. Agents report such a run as disabled, never as passed. Turn it off or on in a
reviewed pull request that says why. Exit codes:

- 0: passed (no open critical or high finding in a complete scan), nothing to scan, or disabled.
  Its message says which; with `--ci`, nothing to scan and disabled end skipped, since nothing was
  checked. Medium and low findings are printed but do not fail the scan.
- 1: failed. The causes: an open critical or high finding (each one is printed with severity,
  `file:line`, title and detail); an answer that could not be read; a request Vidoc refused with
  another 4xx.
- 2: not run. The causes:
  - no key, or a key that is malformed or refused;
  - Vidoc unreachable after the retries, or out of quota;
  - no base (`origin/HEAD` is not set and there is no `--base`);
  - a malformed `.config/vidoc.toml`;
  - Python older than 3.11;
  - an incomplete scan (budget or deadline) that found nothing.

Yulius CI runs the change scan as the `security` job of `yulius.toml` on every pull request head
(`python3 scripts/vidoc_scan.py --ci`; other projects run `yulius security-scan --ci`).

- **It runs the pull request's own scanner.** `--ci` scans only when `YULIUS_CI_REF` is `pr/<n>`.
  `dev` and `main` heads, merge trains, and pull requests with nothing to scan end skipped.
- **It gets the key through the proxy.** The `secrets = "VIDOC_API_KEY"` line on the job is read
  from `dev`'s `yulius.toml` only
  ([SPEC-142](../../.ai/specs/SPEC-142-fix-466-yulius-ci-jobs-opt-in-to-named-a.md),
  [ADR-0116](../decisions/0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md)). It gives the
  job the key's placeholder and the egress proxy with a credential of its own, never the value.
  Each use is in the secrets audit as `ci:<run>`.
- **Findings fail it.** Exit 1 fails the job, and so the merge gate.
- **A scan that could not run is skipped.** Exit 2 prints `[yulius] security scan skipped` and exits
  with `$YULIUS_CI_SKIP_CODE`, so the job reads **Skipped**, never Passed, and blocks no merge
  ([ADR-0133](../decisions/0133-a-ci-job-that-checked-nothing-ends-skipped.md)). Skipped is not a
  pass: agents still run the scan themselves and report it as unrun.

Any CI job can skip itself the same way: Yulius gives every job `YULIUS_CI_SKIP_CODE` (78), and a
job that exits with it ends skipped. Write `exit "${YULIUS_CI_SKIP_CODE:-0}"` so the command still
passes on an instance that does not know skipped yet.

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
