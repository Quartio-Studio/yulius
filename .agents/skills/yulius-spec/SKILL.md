---
name: yulius-spec
description: Write or refine Yulius specifications with traceable requirements, runtime contracts and acceptance evidence. Use for features and fixes; research, questions, documentation and text edits need no spec.
---

# Specify a Yulius change

Read `AGENTS.md`, `docs/engineering/specifications.md` and the relevant existing spec from the
repository root. Start from `.ai/templates/spec.md`, the form `spec_check` validates (every
section stays; one that does not apply says why). Distinguish reference observations, user
requirements and proposed defaults.

1. Inspect the affected behavior and code and the scope the user authorized. Record the concrete
   outcome and the current failure or missing capability; read the neighbouring tasks first
   (`delivery-pipeline`, Ready).
2. Assign requirement IDs and observable acceptance scenarios. Include invalid input,
   cancellation, retries, recovery and old-default behavior where affected.
3. Define ownership, boundary types, persistence/compatibility and the exit trigger for each
   nonterminal state. Read `docs/engineering/runtime-contracts.md` only for affected runtime
   boundaries.
4. Separate blocking decisions from reversible implementation choices; a proposal is not
   product approval.
5. Review the spec against the actual brief, then run `spec_check` (its form) and `python3
   scripts/check_repository.py` (links); say what acceptance evidence is still needed.

Keep reference research and speculative product notes outside the repository. Deliver the spec
path, state, requirements and unresolved decisions; a spec-only task ends with a complete
reviewable spec, neither implemented nor published.
