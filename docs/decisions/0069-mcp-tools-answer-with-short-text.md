# 0069. Yulius's MCP tools answer with short text; the record only when a client asks

## Status

Accepted. Specified in
[SPEC-086](../../.ai/specs/SPEC-086-session-noise-tool-text-and-full-width-sessions.md). Amends
SPEC-002 §7.4.

## Context

Every Yulius MCP tool answered with its whole record as pretty-printed JSON
(`serde_json::to_string_pretty`). Claude Code prints a tool's result in its terminal, so a person
watching a task saw screens of JSON: on 2026-10-13 task #66's terminal filled with the steps list,
each step with its id, times and nulls. The agent pays for the same bytes in context. Across the
341 transcripts on the instance (74 sessions using the tools, 2026-10-12/13), the tools returned
6.08 MB in 2,775 results, 13.5 % of all tool output: `ci_status` 3.13 MB in 243 calls (every job's
command repeated per run, the other tasks' runs in full, 16 KiB of output for passed jobs),
`step_update` 1.28 MB in 581 calls (the whole plan with ids and times after each change),
`delivery_status` 0.45 MB in 332 calls (a static policy paragraph and the profile each time).

MCP's `structuredContent` is meant for records next to readable text. Claude Code 2.1.292 (the
agents' version on the instance) and 2.1.296 give the model `JSON.stringify(structuredContent)`
*instead of* the text content when a result has both; the content wins only behind a remote
feature flag (`tengu_eventual_noodle`, event `mcp_content_over_structured`). Sending the record
there would put the JSON back in front of the model.

## Decision

- A tool still builds its record (the contract its tests and other clients rely on); the answer
  is text rendered from it (`yulius-app::mcp_text`): what changed, what is next, and the numbers,
  states, reasons and ids the agent passes back.
  - The step tools print the plan as a checklist (`3 [>] Title (in progress, effort 2)`, notes,
    nudges); `step_update` names the step that changed, the progress and the next step instead
    of the whole plan. Step ids and times are left out: steps are named by ordinal or title.
  - `ci_status` prints each job's command once and a line per run, with its jobs only when they
    did not all end like the run. With `run_id` it shows the output of the jobs that failed or
    errored; `job` asks for any job's output.
  - `delivery_status` prints the stage, the pull request, the spec, what is refused and why,
    notes, people's next actions and the profile on one line; the policy paragraph stays in the
    briefing and the delivery tools' descriptions.
  - `tasks_list` and `task_get` print a line per task, named by number; `project_context` and
    `session_read` print their text as written.
  - Every other tool prints an outline of its record (`key: value`, `- ` items, text on its own
    lines, nulls and empty lists left out). A record a rendering does not recognise falls back to
    that outline, so a changed shape loses nothing.
- A client that reads records asks for them: `tools/call` with `_meta: {"yulius/structured":
  true}` also gets the record as `structuredContent`, every string in it redacted like the text.
  Claude Code sends no such `_meta`, so agents and the Superagent get only the text.

## Alternatives

- Compact JSON (`to_string`): a third smaller, still unreadable in the terminal and still every
  id and null.
- `structuredContent` on every result: Claude Code would show the model the JSON instead of the
  text (above).
- A `detail` or `format` argument on every tool: ~35 more schema properties in every session's
  tool list, for detail no agent asked for in the transcripts.
- Typed reply structs per tool: the right end state if the records stop being needed, but a
  rewrite of ~35 tools and their tests for the same text.

## Consequences

- Replayed over the 1,996 successful results in the transcripts, the text is 1.76 MB instead of
  5.99 MB (−71 %): `ci_status` list −85 %, `ci_status` with `run_id` −61 %, `step_update` −86 %,
  `ci_rerun` −91 %, `steps_set` −72 %, `delivery_status` −69 %, `tasks_list` −57 %.
- Tests read the record through `_meta` and check the text separately; a renamed field is caught
  by the renderers' unit tests, which build their input from the contract types.
- A future client that wants the record must send the `_meta` key. If Claude Code starts
  preferring a result's content over its `structuredContent`, sending both by default becomes an
  option.
