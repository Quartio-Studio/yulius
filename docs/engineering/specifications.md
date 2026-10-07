# Writing future specifications

Write a specification before a new feature or material behavior change. Record
observable behavior, constraints, open decisions and evidence rather than a code
narrative. Use [the template](../../.ai/templates/spec.md); omit irrelevant sections.

Keep product research and speculative notes local unless the user requests them
in the repository. A future accepted feature spec can live in `.ai/specs` when
that feature work is authorized. The accepted product specs are SPEC-001 to SPEC-003.

Use stable requirement IDs and acceptance scenarios. Separate user requirements,
source observations and proposed defaults. Mark decisions blocking or nonblocking.
The lifecycle is discovery → proposed → accepted → implemented → superseded.
Record scope authorization; an explicit implementation request is sufficient for
that scope, without an extra approval ceremony for routine engineering choices.

A bug fix may reference an existing contract plus its regression case. Text-only
corrections do not need a new spec. For every nonterminal execution state, name
its owner, exit trigger and failure/recovery behavior. Define boundaries, limits,
cancellation and compatibility where they affect the change.

Only mark implemented when every acceptance item has direct evidence from current
code and actual output. State unavailable checks explicitly; compilation alone
cannot establish product acceptance. Use a short ADR for substantial architecture
choices, with alternatives and consequences, when such a decision is needed.
