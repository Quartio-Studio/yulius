---
name: yulius-spec
description: Write or refine Yulius feature specifications with traceable requirements, runtime contracts and acceptance evidence. Use for feature design or material behavior changes; routine text edits do not need a spec.
---

# Specify a Yulius change

Read `AGENTS.md`, `docs/engineering/specifications.md` and the relevant existing spec from the
repository root. Use `.ai/templates/spec.md`; preserve its evidence discipline,
not irrelevant sections. The current product brief is pending: distinguish
reference observations, user requirements and proposed defaults.

1. Inspect the affected behavior/code and scope authorized by the user. Record
   the concrete outcome and current failure or missing capability.
2. Assign requirement IDs and observable acceptance scenarios. Include invalid
   input, cancellation, retries, recovery and old-default behavior where affected.
3. Define ownership, boundary types, persistence/compatibility and the exit trigger
   for each nonterminal state. Read `docs/engineering/runtime-contracts.md` only
   for affected runtime boundaries.
4. Identify blocking decisions separately from reversible implementation choices.
   Continue independent design work; don't turn a proposal into product approval.
5. Review the spec against the actual brief, then run
   `python3 scripts/check_repository.py` for structure/link integrity. This check
   cannot prove the design is sufficient: explain acceptance evidence still needed.

Keep reference research and speculative product notes outside the repository.
Only write a product spec into the repo when the user has requested that artifact
or authorized the corresponding feature work. Deliver the spec path, state,
requirements and unresolved decisions. A spec-only
task ends with a complete reviewable spec; do not silently implement or publish it.
