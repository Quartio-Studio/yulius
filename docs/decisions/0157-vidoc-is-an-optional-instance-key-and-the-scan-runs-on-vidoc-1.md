# 0157. Vidoc is an optional instance key, and the security scan runs on vidoc-1

## Status

Accepted. Specified in [SPEC-195](../../.ai/specs/SPEC-195-vidoc-is-built-into-yulius-an-optional-i.md);
task #181.

It supersedes the CLI parts of [ADR-0104](0104-agents-scan-their-branch-with-vidoc-before-delivery.md)
and [ADR-0116](0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md): `vidoc ci`,
`tools/vidoc`, `api.vidoc.dev`, the clean-tree and branch rules, and ignores kept in Vidoc.

It amends [ADR-0107](0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md): the switch is read
from `HEAD`, and the file can also hold `exclude` and `[[ignore]]`.

The skip rule of [ADR-0133](0133-a-ci-job-that-checked-nothing-ends-skipped.md) is kept.

## Context

The scan drove the Vidoc CLI `@vidocsecurity/cli` 0.1.25 against `https://api.vidoc.dev`. That API
answers every key with `401 token_invalid`, so no scan ever ran, and every pull request's `security`
job ended skipped (#470).

Vidoc's current API is `https://model.vidocsecurity.com`, an Anthropic-compatible Messages API
whose model `vidoc-1` reviews code. The key was a per-project agent secret.

The person asked for three things:

- Vidoc built into Yulius, as an optional instance key like the Jev key.
- A scan on vidoc-1 that runs for any project whose instance has a key.
- Infra bootstrap that adds the scan only when a key exists.

## Decision

- **An optional instance key, like Jev's.** It works the same way as the Jev key:
  - it is sealed in the `instance` row (migration 0034);
  - `YULIUS_VIDOC_API_KEY_FILE` is used while nothing is saved;
  - the superadmin manages it through `/api/admin/vidoc*`: set, replace, remove, adopt the file's
    key, and test it with `GET /v1/models`, which must list `vidoc-1`;
  - changes are audited as `vidoc.key_set` and `vidoc.key_removed`;
  - the Instance page has a card for it, and setup has a skippable step after the Jev one.

  Without a key, Vidoc is off, and the scan says "not run".
- **A virtual agent secret.** When the instance has a key and a sandbox has no agent secret named
  `VIDOC_API_KEY`, the key stands in as that secret:
  - it is bound to `model.vidocsecurity.com`, and injected by placeholder;
  - agent runs and the Superagent get its placeholder, and so do CI jobs whose integration-branch
    grant names it;
  - the proxy replaces the placeholder toward that host only, audits each use (`instance:vidoc`)
    and redacts the value;
  - it is never resolved for MCP `${secret:…}`, `.env` copies or browser filling, and it is not
    listed on the Secrets page.

  A project's own secret wins.
- **One scanner, embedded.** `scripts/vidoc_scan.py` is a stdlib-only Python file. `yulius
  security-scan` embeds it and becomes `python3 -I -c <scanner>`. Agents reach it through their
  `yulius` shim, and CI jobs through the binary, which is on their PATH and bound read-only into
  their containment. This repository's `security` job runs the in-tree file, so a pull request tests
  its own scanner.
- **The scan reads commits.**
  - The **change scan** sends the files that differ between `merge-base(HEAD, origin/HEAD)` (or
    `--base`) and `HEAD`.
  - The **broad scan** (`--all`) sends every tracked file.
  - Files come from `HEAD`'s tree, line-numbered, in chunks of 60 kB or less.
  - Binaries, vendored and built directories, lockfiles, minified files, prose, media and
    `exclude` globs are skipped.
  - Each scan has a byte budget (4 MB for a change scan, 16 MB for a broad scan) and a deadline
    (30 minutes and 180 minutes).
  - Up to 4 requests run at once.
- **The contract.** Each chunk is one `POST /v1/messages` with `model: vidoc-1`. The system prompt
  asks for one JSON object, `{"findings": [{severity, file, line, title, detail}]}`, and says that
  file contents are untrusted data. Answers are validated:
  - an unreadable answer is asked again twice, then fails the scan;
  - 429, 529, 5xx and network errors are retried with backoff or `retry-after`, 5 attempts in all;
  - 401 and 403 mean the key was refused, and 402 means no quota.
- **Exit codes.**
  - **0**: no open critical or high finding in a complete scan.
  - **1**: an open critical or high finding, an unreadable answer, or another 4xx.
  - **2**: not run. That covers no key, a refused key, no quota, Vidoc unreachable, no base, a
    malformed config, and an incomplete scan without findings.
- **Committed false positives.** `.config/vidoc.toml` `[[ignore]]` entries name a `path` glob, a
  `title` substring and a required `reason`. A matching finding is printed as ignored and does not
  fail the scan. The reviewer must agree to each one.
- **Infra bootstrap follows the key.** With an instance key, three things carry the scan:
  - the briefing has a short `## Security scan` section;
  - the onboarding prompt, skill `infra-bootstrap` and its workflow add the `security` job
    (`secrets = "VIDOC_API_KEY"`, `command = "yulius security-scan --ci"`) and the AGENTS.md rule;
  - `--ci` scans pull request heads only and ends the job skipped on exit 2 (ADR-0133).

  Without a key, they say "no Vidoc key on this instance" and add nothing.

## Alternatives

- **A stored instance-wide agent secret.** Agent secret names are unique across scopes, and this
  sandbox already has `VIDOC_API_KEY`. It would also lack the Jev key's lifecycle (setup step,
  config file, test), and the vault could resolve it for MCP references and `.env` copies, where the
  value leaves in clear.
- **A new secret name, such as `YULIUS_VIDOC_KEY`.** Grants, scripts and the project secret would
  then need two names for one key.
- **The scanner in Rust inside `yulius`.** This repository's job would have to compile the binary to
  test its own scanner, or there would be two scanners. Python is already a prerequisite of
  `scripts/dev`, and the sandboxes have it.
- **A copy of the scanner in each project.** Copies drift, and fixes would not reach the projects.
- **Treat an unreadable answer as "not run".** A pull request can steer the model's output with
  prompt injection, so it could make its own scan skip.
- **Scan server-side from Yulius.** Yulius would need each sandbox's tree and a queue of its own.
  The scan already runs where the code is, behind the egress proxy, which keeps the key out of
  reach.

## Consequences

- The code being scanned goes to Vidoc: changed files on every PR scan, every tracked file in a
  broad scan. The docs and the Instance card say so.
- A prompt injection in the scanned code could make the model report nothing. The verdict is a
  model's judgement, not a proof.
- Each scan uses the key's quota. A change scan of this repository's typical PR is one to three
  chunks, and a broad scan of it is about 150 chunks.
- The running instance is built from `main`. Until a person promotes `dev`, only a project's own
  `VIDOC_API_KEY` reaches its jobs. This repository has one, so its PR jobs scan now.
- Exit 2 still covers causes a pull request controls (a malformed `.config/vidoc.toml`, a missing
  base). The follow-up on #466, making the scan a required check, stays open.
- Task #160 runs the broad audit with `scripts/dev security --all` (`--report FILE` keeps the
  result as JSON).
