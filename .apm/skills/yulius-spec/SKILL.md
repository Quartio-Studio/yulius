---
name: yulius-spec
description: Write or refine Yulius specifications with traceable requirements, runtime contracts and acceptance evidence. Use for features and fixes; research, questions, documentation and text edits need no spec.
---

# Specify a Yulius change

Read `AGENTS.md`, `docs/engineering/specifications.md` and the relevant existing spec from the
repository root. Start from `.ai/templates/spec.md`, the form `spec_check` validates (every
section stays; one that does not apply says why). The product brief is SPEC-001 in `.ai/specs`:
distinguish reference observations, user requirements and proposed defaults.

1. Inspect the affected behavior/code and scope authorized by the user. Record
   the concrete outcome and current failure or missing capability.
2. Assign requirement IDs and observable acceptance scenarios. Include invalid
   input, cancellation, retries, recovery and old-default behavior where affected.
3. Define ownership, boundary types, persistence/compatibility and the exit trigger
   for each nonterminal state. Read `docs/engineering/runtime-contracts.md` only
   for affected runtime boundaries.
4. Identify blocking decisions separately from reversible implementation choices.
   Continue independent design work; don't turn a proposal into product approval.
5. Review the spec against the actual brief, then run `spec_check` (its form) and
   `python3 scripts/check_repository.py` (links). Neither proves the design is sufficient:
   explain acceptance evidence still needed.

Keep reference research and speculative product notes outside the repository.
Only write a product spec into the repo when the user has requested that artifact
or authorized the corresponding feature work. Deliver the spec path, state,
requirements and unresolved decisions. A spec-only
task ends with a complete reviewable spec; do not silently implement or publish it.
