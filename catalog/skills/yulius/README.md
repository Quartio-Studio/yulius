# Yulius skills

Yulius's own skills, written for the way agents work in Yulius (SPEC-001 SKL-003, DSC-008,
DSC-013/014, SPEC-002 ADR-19). They are universal enough to use in any repository, and the
session briefing, the workflows and the other bundled skills refer to them by name.

| Skill | Purpose |
| --- | --- |
| `delivery-pipeline` | The one autonomous path for features and fixes: ready issue, spec, test-first slices, gates, independent review in a fresh session, small pull request, merge on green, deploy and verify, promotion, cleanup |
| `git-hygiene` | One task, one branch, one worktree; no stash as a tool; atomic conventional commits; staying current without rewriting shared history; squash and merge-commit promotion; cleanup |
| `infra-bootstrap` | A project's foundations: tests, lint, one local gate run by Yulius CI on our own machines (no GitHub Actions), environments, docs, labels and templates |

License: MIT, as Yulius (see the repository's `LICENSE`). Yulius embeds these files and extracts
them to `<data_dir>/skills/yulius/` at startup; local edits there do not survive a restart.
