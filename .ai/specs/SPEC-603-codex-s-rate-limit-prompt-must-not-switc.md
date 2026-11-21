---
yulius-task: adab5cc1-e96b-4b19-b932-9362085d4855
status: in-progress
---

# SPEC-603 — Codex keeps its task model at the rate-limit reminder

## Goal

Keep a Codex task on its requested model and effort when Yulius delivers a message near a subscription's usage limit.

Fixes #1641.

## Context

On 21 November 2026 around 21:30–21:40 UTC, Codex 0.170.0 showed an “Approaching rate limits” picker with “Switch to gpt-6-luna” selected in quartio-v2 #350 and #355 and Yulius #581, #584 and #585. The next message from Yulius confirmed the first item. The live status line then showed GPT-6-Luna at medium effort. Starting the task again applied its requested model and effort. The screen in task #587 supplies the regression fixture.

[SPEC-503](SPEC-503-accept-outside-pr-1146-codex-returns-as.md) defines Codex launches, and [SPEC-581](SPEC-581-i-keep-getting-this-even-is-nothing-is-t.md) defines the existing protection for typed messages and possible drafts.

## Requirements

- **CMN-001** Every new or resumed Codex launch writes `notice.hide_rate_limit_model_nudge = true` into its generated configuration. The pinned native CLI accepts the table and boolean key.
- **CMN-002** If an attached Codex terminal visibly shows the rate-limit model picker, including with a wrapped heading, Yulius holds automated input. Check the rendered screen immediately before writing each paste and again before writing its separate Enter, with the writer acquired. Viewer keys can dismiss the picker or choose “Keep current model”.
- **CMN-003** While the picker remains visible, an automatic PR notice remains pending in its existing watch. The same watch may deliver it when the conversation composer is back. If a paste preceded the picker, the run holds further automated input until the composer is confirmed empty. The hold belongs to the attempt and survives server reattachment to its PTY. A person sending a message through a tool receives a conflict with the reason. Existing draft and attempt checks still apply.

## Plan

- [x] Specify how Codex keeps the task’s model
- [x] Suppress the model switch reminder in new and resumed Codex sessions
- [x] Protect automated messages while the Codex reminder is visible
- [ ] Verify the behavior and obtain independent approval

## Acceptance criteria

- **AC-1 (CMN-001)** New and resumed plans contain the boolean under `[notice]`. Codex 0.170.0 reads it in strict mode.
- **AC-2 (CMN-002, CMN-003)** Replay the incident screen with the switch option focused. A tool message is refused and a pending PR notice stays pending. No paste or Enter goes to the picker. Repeat with the title wrapped in a 20-column terminal. After dismissal, the watch delivers its notice once.
- **AC-3 (CMN-002, CMN-003)** Show the picker after the paste. The pending Enter is withheld. Dismissing the picker onto retained text still holds new input, including after a server restart. A repaint showing an empty composer allows the next pass to send it. Ordinary conversation text mentioning rate limits does not cause a hold.

## Test plan

Extend the launch assertions for both launch modes. Run the pinned native CLI against an isolated home in strict mode and read the setting back over its local app-server connection. Replay the reported screen on the fake Codex PTY and observe writes, conflicts and delivery after dismissal. Run the affected suites, repository quick checks, independent review and Yulius CI. Request staging once after the merge.

## Decisions

- Suppress the reminder at launch and protect sessions already running with a hold at the terminal writer. A menu selection synthesized from a rendered screen depends on the selection state and can land in the composer if the picker has gone. [ADR-0443](../../docs/decisions/0443-hold-codex-input-at-the-rate-limit-model-picker.md)
- A task-page live model warning is deferred. Attempt metrics report the model in the latest observed turn. An idle switch can happen before the next turn and requires new authoritative live observation to distinguish the current model from the last one that answered.

## Evidence

- Both launch tests failed on the missing notice setting. After adding it, `scripts/cargo test -p yulius-harness --test launch` passed all 27 cases.
- The native 0.170.0 Linux CLI from the pinned package read `notice.hide_rate_limit_model_nudge = true` through `app-server --strict-config` and `config/read` in an isolated home. A string at the same key failed type validation. This used no subscription or model call. [Native reminder and preference](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/tui/src/chatwidget/rate_limits.rs)
- Without the terminal guard, `scripts/cargo test -p yulius --test main task_message::the_codex_rate_limit_picker` failed both regressions. The tool reported `sent` while the switch option was focused. The PR watch also reported delivery when the picker appeared between paste and Enter.
- With the guard, both regressions passed. The PR notice arrived once after dismissal. When the picker appeared after paste, a screen showing possible retained text kept the notice pending, and an empty composer released it.
- `scripts/dev quick` passed the repository contracts and Rust formatting.
- Review of `1f4d5eae7` found two own-code gaps. On that implementation the 20-column regression reported a message sent through a wrapped picker. The reattachment regression also reported a message sent behind the retained paste after a server restart. Both new regressions failed before their fixes. The pinned picker renderer wraps header text. [Native picker header](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/tui/src/bottom_pane/list_selection_view.rs#L473-L482)
- All four picker regressions passed with wrapped-heading detection and the attempt's on-disk hold. The restarted server refused even a request to send past the retained draft and accepted a later message after an empty repaint.
