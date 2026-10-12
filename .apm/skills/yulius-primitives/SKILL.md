---
name: yulius-primitives
description: Design, package and review agent primitives (skills, personas, instructions, hooks) with APM and the pattern vocabulary. Use when adding or changing anything under .apm/, apm.yml or a catalog skill, or when auditing a repository's conventions. Not for ordinary code changes.
---

# Author an agent primitive

Read `docs/engineering/primitives.md` first; it holds the tables this skill applies.

1. Decide the primitive type for the knowledge (instruction, persona, skill, prompt, memory, spec,
   hook). If a deterministic check can enforce it, write the check instead.
2. Edit only `.apm/skills/<name>/`. One concern per skill; split a skill that routes and does.
3. Write the description as `Use when ...` with a negative case if skills overlap. Changing an
   existing description is a breaking change: say so in the commit.
4. Keep `SKILL.md` lean: templates in `assets/`, long rules in `references/`, noisy work in a
   child that returns a JSON receipt, state read only from a command's exit and output.
5. Restrict personas to the tools they need; reviewers are read-only.
6. Run `apm install`, then `python3 scripts/check_repository.py` and `apm audit` if installed.
   Commit source, `.agents/skills`, `.claude/skills` and `apm.lock.yaml` together.
7. For a repository with unwritten conventions, run the instrumentation audit in the standard and
   start with the highest failure cost.

Name the pattern each choice uses and the anti-pattern it avoids in the pull request.
