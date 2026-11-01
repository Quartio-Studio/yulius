# 0248. Change notes may be a folder of one file per change

## Status

Accepted. Specified in [SPEC-363](../../.ai/specs/SPEC-363-change-notes-as-a-folder-docs-changes-ma.md);
task #346. Keeps the single changes file of SPEC-001 §7.2 as the default and the union merge of
[SPEC-083](../../.ai/specs/SPEC-083-merge-conflicts-with-many-agents.md) MRG-001 for it.

## Context

Every task adds an entry at the top of the changes log. With many tasks merging into `dev` in
parallel, each merge made every other open pull request conflict on GitHub, whose server-side
merge ignores the union driver Yulius keeps locally, and each paid a full CI rerun (quartio-v2,
2026-10-31). quartio-v2 moved to one change note per pull request in `docs/changes/`, but the
briefing still told every task to edit `docs/changes.md`.

## Decision

- **`[docs] changes` may name a folder**: a path ending with `/`, or one that is an existing
  directory of the checkout. The layout keeps it as a `String` with one trailing `/`;
  `DocsLayout::changes_folder()` answers whether it is a folder, and the app resolves an existing
  directory without the slash where the checkout is read.
- **The briefing follows it.** Rule 6 asks for one new file in the folder, named
  `YYYY-MM-DD-<task number>-<short-slug>.md` unless the repository shows another naming, and
  forbids editing other tasks' files. Recent changes quote the newest notes (by the date in the
  name, then the name), within the 4,000 bytes and budget of the single-file excerpt.
- **The other readers skip or count it**: no union line for a folder, and the documentation check
  counts an existing folder as present.

## Alternatives

- **A `ChangesLocation` enum like `DecisionsLocation`.** Typed, but every reader of the path
  changes, including those that only need "a path to show". The trailing `/` carries the same
  information with one helper.
- **A separate key (`changes_dir`).** Two keys for one document, and a project could set both.
- **Telling quartio-v2 to keep `changes.md`, relying on the union merge.** The conflicts are on
  GitHub's side, which ignores merge drivers, so they stay.
- **Ordering notes by Git commit date.** Correct for any naming, but it costs a `git log` per
  briefing and the checkout's file times say nothing; dated names are the convention asked for.

## Consequences

- Projects that switch to a folder stop conflicting on the changes log; their briefings carry the
  newest notes instead of the log's head.
- A folder whose names carry no date is ordered by name only, which may not be newest first;
  the briefing still quotes notes, and the naming rule asks for dated names.
- An empty folder passes the documentation check (a new project may hold only a README).
