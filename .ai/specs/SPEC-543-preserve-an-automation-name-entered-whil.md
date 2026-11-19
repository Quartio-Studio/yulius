---
yulius-task: bb4cd7dc-afb4-4a1f-8e89-556a66a9be72
status: in-progress
---

# SPEC-543 Preserve an automation name entered while recipes load

## Goal

A person can name a new automation as soon as its editor opens and save it with that name.

## Context

Issue #1416 records a saved name of “Daily feedbackNightly Codex review”. The editor mounts an editable name field before its workflow request settles. The completion handler applies the first recipe, which writes to the field if the current name is empty or matches any recipe. The existing Codex test leaves the order of that write and its input to browser timing.

## Requirements

- **ANM-001** Completing the initial workflow read preserves the current name and any edit in progress, including an empty field or text matching another recipe.
- **ANM-002** An untouched new automation gets the first recipe's defaults. Choosing a recipe still suggests its name under the existing rules.
- **ANM-003** The Create button and save shortcut wait for the form to load. A pending workflow read leaves the name editable.

## Plan

- [x] Specify the delayed workflow regression
- [x] Demonstrate the name race in the Codex browser test
- [x] Preserve the entered name and verify the editor
- [ ] Independently review the fix

## Acceptance criteria

- **AC-1** With the workflow read held, enter another recipe's name. Once the read completes, the field retains that name, the first recipe is selected and creating the automation stores the entered name and the selected Codex agent. While the read is held the button is disabled and pressing the save shortcut leaves the form open without an error.
- **AC-2** Opening a fresh editor gives its first recipe the usual name, prompt and schedule. Changing recipes still updates a suggested name and preserves a custom one.

- **AC-3** Select the name while workflows are held, release the read, then type “Nightly Codex review”. The field contains exactly the replacement.

## Test plan

Use the browser mock's held GET and explicit release in the existing Codex automation scenario. Assert the name after the recipe controls appear and check the saved automation. Run the Codex and automation builder coverage and the browser suite.

## Decisions

Initialize the suggested name before the field accepts input. The workflow response supplies choices for the form and has no information about the name. Moving its initial write removes the timing dependency even when a person is between selecting text and inserting a replacement.

## Evidence

- Before the fix, the held-workflow Codex test expected “Weekly summary” and observed “Daily feedback”. Releasing the same read between Select All and typing on dev `8d05c25bf` produced exactly “Daily feedbackNightly Codex review” from #1416.
- With the suggested name available at mount, the held-workflow test also exposed an enabled Create button before its required prompt control existed. The editor now gates the button and keyboard shortcut on the existing loaded state.
- `npx e2e run automations --grep 'between selecting the name|suggests recipe names'` passed both browser scenarios, including selection across the workflow response and explicit recipe picks.
- `npx e2e run codex --grep 'an automation chooses the agent' --repeat-each 6 --workers 3` passed all six runs with workflows held and retries disabled. The test submitted the exact entered name and Codex harness after release.
