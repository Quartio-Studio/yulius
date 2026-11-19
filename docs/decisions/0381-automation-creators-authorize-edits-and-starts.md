# 0381. Automation creators authorize edits and starts

## Status

Accepted for [SPEC-534](../../.ai/specs/SPEC-534-enforce-automation-ownership-across-edit.md).

## Context

[ADR-0060](0060-a-task-is-owned-by-one-person.md) assigns GitHub actions to the task owner. [ADR-0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md) distinguishes steering work from tidying it. Automation configuration and Run now control future tasks before those tasks have owners.

## Decision

An automation's creator owns it for its lifetime. Saving any configuration, including the schedule switch, and starting a run require both project configure access and this identity. Tools use the person they represent. Enabling an existing paused template follows the same rule. An already-enabled template returns its current state. Each task starts under the creator's identity.

Anyone with configure access can delete the automation. A teammate can create a replacement in their own name. This also handles a creator who leaves the project.

## Alternatives

Transferring ownership on each edit or enable would let teammates maintain the same schedule, but a partial edit would also adopt the entire stored prompt. Transferring a single manual run would need to persist the starter through crash recovery. Letting a teammate pause through the current full-record update would require a separate pause operation to avoid writing stale configuration.

## Consequences

Editors see whose GitHub identity a run uses. Existing automation rows have no trustworthy record of the last person who approved the prompt. The upgrade pauses those schedules once. Creators review their saved configuration before enabling it or choosing Run now. Old pending runs with an existing task link to it, and pending slots without a task end as skipped.
