# 0337. A missing ability is reported to the Yulius repository in one call

## Status

Accepted. Specified in
[SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md) (ABL-001 to
ABL-004), task #469, slice 11. Follows
[0330](0330-the-superagent-does-what-its-person-may-do.md) (an ability is withheld only for a
reason the ability table names).

## Context

The Superagent offered three times to report a defect it met and never did: no tool of its own
reached the Yulius project, and `issue_create` opens issues in the task's own repository (wall 12
of task #469). A wall that nobody hears of stays.

## Decision

- **`ability_report {ability, case, needed_for?}`**, served to the Superagent and to every task's
  agent: an ability named in a few words, the case without project details, what it would do
  with it. Refused: empty or over-long texts, invisible characters, unknown arguments; at most 5
  an hour per person.
- **Always kept on the instance**: audited `ability.reported` with the ability, the case and who
  reported it, where the instance's administrators read it.
- **Opened in the Yulius repository** (`Quartio-Studio/yulius`, where its updates come from) as an
  issue labelled to be triaged, under the person's GitHub name, when their GitHub access reaches
  that repository; otherwise not, and the call says so. The issue holds the agent's words only,
  never the project's name; the tool's description tells the agent to write in general terms.
- **The instructions send a missing ability there**, after the tool list was read.

## Alternatives

- **`issue_create` in the task's own repository.** The Yulius project would never hear of it.
- **An issue opened as the GitHub App.** It would speak for every instance and every person, and
  need the App installed on the Yulius repository.
- **A task in the Yulius project.** Only an instance that hosts the Yulius repository has one.

## Consequences

- A wall becomes one call; the Yulius project triages it as any issue.
- A person outside the Yulius team reports to their own instance's administrators only.
