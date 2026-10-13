# Open Mercato skill pack (adapted)

Seven skills adapted from the Open Mercato agent skills and shipped with Yulius (SPEC-001
SKL-003, §7.3). Thank you to the Open Mercato team for writing them and publishing them under
the MIT license: their staff-engineer lens on specs, reviews and fixes is the backbone of how
Yulius agents work.

- Source: <https://github.com/open-mercato/skills>, directory `skills/<name>/`
- Upstream commit the adaptation started from: `3fc5a1fbd84d90c6f052f3c97cb1e54d2660eb22`
- License: MIT, see [`LICENSE`](LICENSE) (the upstream license, kept unchanged)

## Adapted, not vendored

These files are **no longer byte-for-byte copies** of upstream. They were rewritten so that they:

- work in any repository, with any stack: product context comes from the project's own agent
  instructions (AGENTS.md / CLAUDE.md), product summary, architecture, decision records and
  earlier specs; gates are the project's own test, lint, build and gate commands;
- run autonomously: no step waits for a person; open questions are resolved by the most
  reversible default, recorded in the spec or pull request;
- do not depend on Open Mercato's own harness and workflows: no pipeline config file, no
  tracker descriptors, label guards, claim comments or chaining markers, and no autopilot skills
  calling them.

Each `SKILL.md` carries the note "Adapted from Open Mercato's skills (MIT), rephrased to be
universal; thanks to the Open Mercato team for this contribution." Updating from upstream is a
manual re-adaptation, recorded here with the new commit.

## Selection

| Skill | Purpose |
| --- | --- |
| `om-brainstorm` | Frame a vague idea, weigh alternatives (including building nothing), challenge the conclusion, decide the next step |
| `om-spec-writing` | Write or review a spec: skeleton first, recorded defaults for open questions, a plan of similar-effort steps |
| `om-code-review` | Independent review in a fresh session: every gate, checklist, severity-ranked findings, mechanical verdict |
| `om-check-and-commit` | Run every gate, fix straightforward failures, rerun until green, commit and push following `git-hygiene` |
| `om-root-cause` | Read-only root-cause analysis with the minimal change and the regression test |
| `om-fix` | Fix test-first: a regression test that fails without the fix, the minimal change, the full gate |
| `om-ux-shape` | Shape user-facing and AI features into a concrete, evidence-tagged direction and handoff |

Removed: `om-verify-in-repo` (a QA-claim step of Open Mercato's issue autopilot; Yulius agents
reproduce defects inside `om-root-cause` and `om-fix`). Not included: the tracker and
pull-request autopilot chain, discovery and setup skills, because Yulius provides task
orchestration, delivery and step tracking itself.

## How Yulius uses the pack

The files are embedded in the `yulius` binary and extracted to `<data_dir>/skills/open-mercato/`
at startup (existing copies are overwritten, so local edits there do not survive a restart).
Each skill can be enabled or disabled per sandbox on the Skills page (default: enabled); enabled
skills are installed into every Claude Code session of the sandbox. They work together with the
other bundled skills: `test-driven-development` and `verification-before-completion` (adapted
from obra/superpowers) and Yulius's own `delivery-pipeline`, `infra-bootstrap`, `git-hygiene` and
`dead-code-cleanup`.
