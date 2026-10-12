# 0019. The composer builds a start rule from picked conditions

## Status

Accepted. Refines [0018](0018-task-start-rules.md); specified in
[SPEC-022](../../.ai/specs/SPEC-022-when-rule-is-true-should-be-a-simple-edi.md).

## Context

Start rules (ADR-0018) were entered in the composer as text, such as `#12 done and #13 done`.
People had to know the grammar and the task numbers, and a mistake showed only when the task was
created. The request: "when rule is true should be a simple editor, not a string to be written".

## Decision

The composer's "When a rule is true…" opens an editor of **conditions**: each row is a task of the
sandbox (a native select of `#N title`, newest first, archived ones left out) and a state ("is
done", "is merged", "has ended"). With two or more rows a segmented choice says whether **all**
or **any** must hold. The editor shows the sentence it builds and sends it as `start_rule` in the
canonical form the server stores, so the server, the store and the rule language are unchanged.
The pure part (`ruleText`, `ruleChoices`) lives in `web/assets/ruleeditor.js` and is unit-tested
in the page.

## Alternatives

- **A nested tree editor with `not` and groups**: expresses every rule, but turns a two-click
  choice into a small programming tool; no request so far needs it, and the API still takes it.
- **The text field with autocomplete and inline validation**: still a string to write, which is
  what the request rules out.
- **Both modes, with "Edit as text"**: doubles what has to be built, tested and explained for a
  rare need the API already covers.

## Consequences

Picking a rule needs no typing and cannot be malformed: at most 16 conditions, the server's
limit. A picked task that is archived or deleted before the task is created stays in the rule as
"#N (archived or removed)" (the server then refuses a deleted one with a clear message) rather
than being dropped unseen. The composer can express only flat
`and`/`or` rules over `done`, `merged` and `ended`; rules with `not` or mixed operators come
through the API, and a task made that way still shows its rule everywhere. A new state in the
language needs a row in `RULE_STATES`.
