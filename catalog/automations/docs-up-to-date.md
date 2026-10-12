---
name: Keep the docs up to date
description: Every night, the documentation is brought in line with what changed in the code during the last day.
cron: 0 3 * * *
utc_offset_minutes: 120
workflow: wf-docs
order: 10
---

Bring this project's documentation in line with the code, for the changes made since the last run
of this automation.

1. Find what changed. With `tasks_list` (scope `all`, `limit` 200), find the previous task titled
   like this one other than this one. If there is one, review everything merged into the
   integration branch since that task was created (`git log --since=<its created_at>
   origin/<integration branch>`, usually the last day, plus `docs/changes.md`). If there is none
   but the documentation was clearly kept up to date before (recent entries in `docs/changes.md`),
   review the last day (`git log --since="26 hours ago"`). Only when this automation was just
   enabled, review the documentation as a whole against the current code.
2. For each change, check every document the docs index lists (AGENTS.md, the product summary, the
   architecture with its Mermaid diagrams, decisions, guides, API references, READMEs, the changes
   log) and fix what is outdated, missing or contradictory. Keep each fact in one place and link
   to it. Documentation only: behaviour stays the same.
3. When the code is wrong rather than the document, confirm it (read the code, run the command or
   the test that shows it), then report it with `issue_create` unless an open issue already tracks
   it (`gh issue list --state open --search "<words>"`); leave the code alone.
4. When nothing is outdated, change nothing and end with a short summary that says so. Otherwise
   deliver one documentation pull request into the integration branch and end with a summary of
   what you corrected and why.
