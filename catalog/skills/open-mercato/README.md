# Open Mercato skill pack (vendored)

Curated subset of the Open Mercato agent skills shipped with Yulius (SPEC-001 SKL-003, §7.3).

- Source: <https://github.com/open-mercato/skills>, directory `skills/<name>/`
- Commit: `3fc5a1fbd84d90c6f052f3c97cb1e54d2660eb22`
- License: MIT, see [`LICENSE`](LICENSE) (copied unchanged from the upstream repository root)
- Files are byte-for-byte copies of the upstream skill directories at that commit, including
  their `references/` and `agents/` files. Do not edit them here; update by re-vendoring a new
  pinned commit and recording it in this file.

## Selection

Included, because they teach code discipline to people who do not program and fit inside a
Yulius task (which already provides orchestration, branches, delivery and step tracking):

| Skill | Purpose |
| --- | --- |
| `om-brainstorm` | Shape a vague idea into a brief before writing a spec |
| `om-spec-writing` | Write a reviewable specification / implementation plan |
| `om-code-review` | Structured code review with a checklist |
| `om-check-and-commit` | Run the configured validation gate, fix straightforward failures, commit |
| `om-root-cause` | Read-only root-cause analysis of a defect |
| `om-fix` | Fix a defect with a regression test and the full validation gate |
| `om-verify-in-repo` | Verify a claimed fix or behavior against the repository |
| `om-ux-shape` | Shape user-facing behavior with evidence-based UX guidance |

Excluded for now: the tracker/PR-autopilot chain (`om-auto-*`, `om-*-pr*`, `om-merge-buddy`,
`om-review-prs`, `om-backlog` and issue tools), product-discovery and UX-pipeline setup skills,
and meta skills (`om-create-skill`, `om-apply-upgrade-notes`, `om-setup-*`), because Yulius
itself provides task orchestration, delivery and step tracking.

## How Yulius uses the pack

The files are embedded in the `yulius` binary and extracted to `<data_dir>/skills/open-mercato/`
at startup (existing copies are overwritten, so local edits there do not survive a restart).
Each skill can be enabled or disabled per sandbox on the Skills page (default: enabled); enabled
skills are installed into both the Claude Code and Codex skill directories of a run.

Several skills read `.ai/agentic.config.json`. When the pack is enabled for a sandbox and the
repository has no such file, Yulius renders a compatible one from the sandbox profile and
`yulius.toml` (base branch = integration branch, validation commands from `[commands]`, GitHub
tracker, labels and QA gate disabled).
