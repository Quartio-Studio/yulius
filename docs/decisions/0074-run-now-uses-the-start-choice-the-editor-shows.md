# 0074. Run now uses the start choice the editor shows

## Status

Accepted. Specified in [SPEC-093](../../.ai/specs/SPEC-093-automation-editor-the-start-choice-sits.md)
(task #78). Extends [0062](0062-an-automation-starts-its-tasks-like-new-task.md).

## Context

The automation editor now shows the start choice as a chip in its action bar, beside Run now and
Save (SPEC-093). Once the choice sits next to Run now, a person who changes it and clicks Run now
expects that run to start the way the chip shows, saved or not. Until now `POST
/api/automations/{id}/run` took no body, and every run used the stored choice.

## Decision

- `POST /api/automations/{id}/run` takes an optional body `RunAutomation` = `{"supertask",
  "after_task_id", "start_rule"}`, the same three fields as `Automation`. It is the start choice
  of this run only. Nothing is stored and the run history does not show it, but the
  `automation.run_now` audit entry names a choice other than the stored one
  (`{"state":"task_created","start":"supertask"}`).
- The body is checked like a save (AUT-014: one choice at most, a task of the project, a rule
  that parses and names the project's tasks), and an invalid one answers 422 with no run
  recorded. The stored choice sent back is kept unchecked, as an update keeps it, so a run whose
  predecessor was deleted fails as a scheduled run does instead of refusing the request.
- Without a body the run uses the stored choice. Scheduled runs, catch-up runs and the
  `automation_run_now` tool behave as before.
- The editor always sends the choice its chip shows, and keeps Run now and the chip off until it
  has read the automation, so it never sends a default it has not shown. New task and the editor share one start menu
  module (`startchoice.js`).

## Alternatives

- **Save, then run.** The automation would also store every other unsaved edit (the prompt, the
  schedule) without the person asking for it.
- **Refuse Run now while the editor has unsaved changes.** The request asks for the opposite, and
  the button would turn off for any unrelated edit.
- **Ask how to start in a Run now dialog.** That adds a step, while the chip already shows the
  answer.

## Consequences

- One run can start differently from the automation's saved choice. The previous-active skip
  still applies, so a Run now with another choice cannot pile up waiting tasks.
- A Run now that crashes between recording its run and creating its task is completed at startup
  with the stored choice. Such a recovery is rare, and it creates the automation's usual task.
- Old clients and agents send no body and keep the stored choice.
