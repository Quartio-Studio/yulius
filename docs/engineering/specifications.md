# Writing specifications

Features and fixes get a spec before code; research, questions, documentation and chores do
not (SPEC-001 DSC-013). A task's spec is `.ai/specs/SPEC-NNN-<slug>.md` (Yulius names it in the
briefing) in the form `spec_check` validates: start from [the template](../../.ai/templates/spec.md)
and skill `om-spec-writing`. Record observable behavior, constraints, decisions and evidence
rather than a code narrative.

The form: a frontmatter with `yulius-task` (the task's id) and `status`, then the sections Goal,
Context, Requirements, Plan (a `- [ ]` checklist), Acceptance criteria, Test plan and Decisions.
The status moves draft → approved (optional) → in-progress → done, or abandoned; `done` needs
every Plan item ticked and an Evidence section with the commands run and their results.
`delivery_merge` refuses a head whose spec does not validate. The status is also the task's own
record of whether its work is finished: Yulius keeps it with the task, and a task whose spec
says `draft`, `approved` or `in-progress` is not Done, however complete its plan reads, so the
tasks waiting for it do not start. A task that delivers slice by slice keeps `in-progress` until
its last slice and sets `done` with it
([ADR-0332](../decisions/0332-done-needs-a-pull-request-that-would-merge-and-a-spec-that-says-done.md)).

Use stable requirement IDs and acceptance scenarios. Separate user requirements, source
observations and proposed defaults. Decide open questions with a reversible default and
record it under Decisions; a lasting choice is an ADR in `docs/decisions/`, with alternatives
and consequences. An explicit implementation request authorizes its scope, without an extra
approval ceremony for routine engineering choices.

For every nonterminal execution state, name its owner, exit trigger and failure/recovery
behavior. Define boundaries, limits, cancellation and compatibility where they affect the
change. Text-only corrections need no spec.

Mark a spec done only when every acceptance item has direct evidence from current code and
actual output. State unavailable checks explicitly; compilation alone cannot establish product
acceptance. The product specs SPEC-001 to SPEC-003 (and SPEC-084 to SPEC-087) predate this form
and keep their own headers.
