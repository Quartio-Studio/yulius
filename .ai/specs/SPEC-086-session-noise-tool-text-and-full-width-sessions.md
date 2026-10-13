# SPEC-086: Yulius's tools answer in short text; agents' shell reads checked; sessions full width

Status: implemented
Date: 2026-10-13
Owner: engineer D (round of 2026-10-13)
Scope authorization: user requests of 2026-10-13: (3a) "I think there was too much text, like
meaningless json or something." (screenshot of task #66's terminal, 16:12 local: the steps list
as pretty JSON, then "Steps and Plan are in sync again"); (3b) "it looked like the agent kept
calling cat instead of just native read? Verify that."; (5) "Sessions are beautiful but should be
always 100% width and not only when minimised. Even left and right margin of the session window
and border radius should be thrown out to make it look similar to the original session."

## Problem and evidence

Measured on the instance over every agent transcript (341 JSONL files, 74 sessions that used the
Yulius tools, 2026-10-12/13; Claude Code 2.1.292).

**Tool results (3a).** `App::tool_result` printed every tool's record with
`serde_json::to_string_pretty`. The Yulius tools returned 6.08 MB in 2,775 results, 13.5 % of all
tool output (45.0 MB). Ranked by size:

| Tool | Calls | Total | Mean | p95 | What filled it |
| --- | --- | --- | --- | --- | --- |
| `ci_status` | 243 | 3,129,237 | 12,877 | 22,932 | list (132 calls, 13.7 k each): each job's command repeated per run, ten runs in full; with `run_id` (111, 11.9 k): 16 KiB of output for passed and running jobs too |
| `step_update` | 581 | 1,278,397 | 2,200 | 3,041 | the whole plan with ids, times and nulls after every change |
| `delivery_status` | 332 | 450,252 | 1,356 | 1,478 | a static policy paragraph and the profile each time |
| `tasks_list` | 43 | 412,303 | 9,588 | 19,438 | ids, timestamps and nested step objects per task |
| `steps_set` | 97 | 200,119 | 2,063 | 3,078 | as `step_update` |
| `delivery_merge` | 895 | 138,285 | 154 | 299 | small already |
| `sessions_search`, `session_read` | 34 | 180,530 | | | a transcript as one escaped JSON string |
| others (26 tools) | 650 | 291,598 | | | |

MCP's `structuredContent` cannot carry the records next to the text: Claude Code 2.1.292 and
2.1.296 give the model `JSON.stringify(structuredContent)` instead of the text content when a
result has both (the content wins only behind the remote flag `tengu_eventual_noodle`). See
ADR-0069.

**Shell reads (3b).** It is real, and Claude Code asks for it: in bypass-permissions mode it
attaches a reminder (`auto_mode` attachment, `bashFirst: true`, 262 records) saying "read files
with cat, head, or sed -n, search with grep and find … instead of the dedicated Read, Edit, or
Write tools". Agents read files with Bash 3,292 times (`sed -n` 2,225, `cat` 1,100 incl. heredoc
writes, `tail` 272, `head` 38) against 194 `Read` calls, and searched with `grep` 2,820 times
against 0 `Grep`/`Glob` calls. It does not hurt:

- Size: a `sed -n` read is 5,139 characters on average (p50 4,072, p95 12,862); `Read` averages
  8,372 (p95 38,907), 140 of its 194 calls being whole files, and its line-number prefixes are
  7.3 % of what it returns.
- Rereads: 9 of 3,292 shell reads (0.3 %, 2,986 characters) repeat a command already run in the
  session; 6 of 156 whole-file `cat`s repeat one. Same-path reads (741) are other line ranges.
- Huge outputs: 22 shell reads (0.7 %) reached 25,000 characters; 24 were saved to a file by
  Claude Code as too large.
- Line numbers: 2,381 of 2,493 `grep`/`rg` calls (95.5 %) use `-n`, and `sed -n` reads ranges
  the agent picked by number.
- Edits: 157 `Edit`/`Write` calls touched a file never read with `Read`; none failed.

So nothing changes for 3b; this section is the answer.

**Sessions page (5).** The transcript well (`.ss-tui`, SPEC-075) sat in `.ss-inner`, at most
960 px wide and centred, with a 1 px border and `--radius-lg` corners: at 1920 px it had 362 px
margins on each side (the new e2e test, red before the change).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TOL-001 | A tool's successful result is one text item rendered from its record: no JSON syntax; nulls, empty text and empty lists left out; text a tool returns (a briefing, a transcript) printed as written; `project_context` prints its briefing alone. Errors keep their message. The 256 KiB bound and the redaction of agent secrets apply to the text as before. | unit `mcp_text::tests::the_outline_is_readable_text`, `long_text_is_printed_as_written`; integration `mcp_coordination::step_tools_answer_with_short_text_not_records` |
| TOL-002 | `steps_set` and `steps_get` print the plan as a checklist (`N [x]`, `[>]` in progress, `[ ]`, `[-]` skipped, `[!]` blocked, effort above 1, notes), the progress and the nudges; `steps_set` first says "Plan set: N steps.". `step_update` says which step changed and its state, the progress, the next open step (or that every step is finished) and the nudges, not the whole plan; its record names the step (`updated`). No step id or time is printed. | unit `a_plan_reads_as_a_checklist_without_ids_or_times`, `a_step_update_says_what_changed_and_what_is_next`; integration `step_tools_answer_with_short_text_not_records` |
| TOL-003 | `ci_status` prints the jobs in force with each command once, the task's pull request and a line per run (id, label, short commit, state, duration, reason, "yours", a commit status other than published), with its jobs only when they did not all end like the run. With `run_id` it prints each job's state and the output of the jobs that failed or errored; with `run_id` and `job` (a name) that job's output whatever its state, or the run's job names when there is none of that name. `ci_rerun` says "Queued again:" and the run. | unit `a_run_shows_the_output_of_the_jobs_that_failed`, `the_ci_list_prints_each_command_once_and_a_line_per_run` |
| TOL-004 | `delivery_status` prints the stage, the pull request (number, state, base, checks, title, URL), the spec and its problems, open steps, refused actions with reasons, notes, people's next actions, checks access and the profile with its environments, without the policy paragraph. `tasks_list` and `task_get` print a line per task (number, title, status and activity, agent when not Claude Code, branch, steps, run-after, spec, created and updated times) without task ids; `task_get` adds the steps and the prompt. | unit `delivery_status_reads_as_a_few_lines`, `tasks_read_as_one_line_each`; integration `delivery::agents_merge_only_through_gated_tools` (the text), `step_tools_answer_with_short_text_not_records` |
| TOL-005 | A result has no `structuredContent` unless the `tools/call` asks with `_meta: {"yulius/structured": true}`; then it also carries the record (an object; another value as `{"value": …}`), every string in it redacted of agent secrets, and withheld when larger than 256 KiB. | integration `step_tools_answer_with_short_text_not_records`, `delivery::agents_merge_only_through_gated_tools` (no `structuredContent`), `secrets::agents_list_names_and_read_redacted_results` (text and record of `task_get` redacted in every encoding); every suite reading records through `_meta` |
| TOL-006 | A record a tool's rendering does not recognise, and every tool without its own rendering, is printed as an outline of the whole record, so nothing is lost. | unit `an_unrecognised_record_falls_back_to_its_outline` |
| TOL-007 | The descriptions of `step_update`, `steps_get` and `ci_status` say what the answer holds; `ci_status` takes `job`. | review |
| SSW-001 | A session's transcript well spans the content area edge to edge: no maximum width, no side margin, no side border, no radius, from below the header to the page's bottom; a top rule separates it from the header, which keeps the page's padding (32 px, 12 px on a narrow page), as do what stands in for the transcript (loading, errors, "Show from the start"). The same beside the Superagent, in both themes and at 390 px, with no sideways scroll and no WCAG violation. | e2e `sessions.e2e.ts` "a session fills the content area edge to edge, beside the Superagent and on a phone" |
| SSW-002 | SPEC-002 §7.4, ADR-0069, `docs/changes.md` and `e2e/README.md` describe it. | review |

## Design and contracts

- `yulius-app::mcp_text::render(tool, args, record) -> String` is pure. Each rendering returns
  `None` when the record lacks what it needs, and the outline is used instead (TOL-006).
- `App::tool_result` redacts every string of the record (rendering indents multi-line text,
  which could split what the matcher looks for), renders it, redacts and bounds the text, and
  adds the redacted record as `structuredContent` only for `_meta["yulius/structured"] == true`
  (`mcp::STRUCTURED_META`). The records the tools build are
  unchanged except `step_update`'s added `updated`.
- `browser_screenshot`'s image result is unchanged.
- The sessions page changes CSS only (`styles.css`: `.ss-inner`, `.ss-tui`, `.ss-web`); the
  markup of SPEC-075 stays.

## Failure and compatibility

- Agents running when this is deployed see the new text at their next call; nothing is
  persisted. Old transcripts keep the JSON they recorded.
- A client that parsed the text as JSON breaks: the tests did (they now ask for the record); the
  web UI, `yulius wait` and the CLI use the HTTP API, not MCP.

## Delivery and acceptance

- AC-TOL-1: through `/mcp` without `_meta`, `steps_set` of three steps answers "Plan set: 3
  steps.", the progress, three `[ ]` lines and the nudge; `step_update` answers "Step 1 "…" is in
  progress.", the progress and "Next: 2 …"; no step id appears in any of them, and the text is
  less than a third of the record's pretty JSON.
- AC-TOL-2: with `_meta`, the same call carries the record with ids and times.
- AC-TOL-3: replayed over the 1,996 successful Yulius tool results in the instance's transcripts,
  the text is 1.76 MB instead of 5.99 MB (−71 %).
- AC-SSW-1: at 1920 px the well's left and right edges are the page's, its radius and side
  borders 0; with the Superagent open the page is narrower and the same holds; in the light and
  the dark theme; at 390 px the back link is 12 px from the edge and nothing scrolls sideways.

## Open decisions

None blocking. If Claude Code starts preferring content over `structuredContent`, sending the
record by default becomes possible (ADR-0069).

## Plan

- [x] Measure the tools' results and the shell reads across the instance's transcripts
- [x] Tool results are short text rendered from their records, the record on request
- [x] Tests read records through `_meta` and check the text agents get
- [x] The session transcript fills the content area edge to edge
- [x] Spec, ADR, SPEC-002 §7.4, changes and e2e README updated; gates green

## Implementation evidence

- `cargo nextest run -p yulius-app -E 'test(/mcp_text|mcp::tests/)'`: 11 passed.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^(mcp_coordination|sessions|automations|cockpit|delivery|manage_tools|secrets|browser|live_display|automation_start|supertask|task_ownership)::/)'`:
  all passed, including `mcp_coordination::step_tools_answer_with_short_text_not_records`
  (red before: the text was pretty JSON).
- `npx e2e run tests/sessions.e2e.ts`: 13 passed; the new test failed against the previous
  stylesheet (left 362 px, right 362 px, radius 12 px, sides 1 px).
- Replay of the transcripts' results through `mcp_text::render` (a throwaway binary outside the
  repository): `ci_status` list 1,805,519 → 281,634 (−85 %), with `run_id` 1,322,647 → 517,064
  (−61 %), `step_update` 1,277,643 → 187,777 (−86 %), `delivery_status` 450,045 → 141,936
  (−69 %), `tasks_list` 412,303 → 178,765 (−57 %), `steps_set` 199,461 → 56,331 (−72 %),
  `ci_rerun` 45,762 → 4,254 (−91 %).

## Changelog

- 2026-10-13: specified and implemented.
