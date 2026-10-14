---
name: Deploy dev every night
description: Every night, dev is deployed from the integration branch with the project's CD and tested; a failure becomes an issue.
cron: 0 3 * * *
utc_offset_minutes: 120
order: 30
---

Deploy this project's dev environment from the integration branch and test it (CD, SPEC-134).
This run changes no repository file itself, so it opens no pull request. Never deploy production
and never promote: production is always a person's.

1. Read CD with `cd_status`. When dev is not defined, or empty (nothing to deploy yet), or its
   last run already deployed the current integration head and passed, deploy nothing: end with a
   one-line summary saying which.
2. When a secret dev needs is missing or waits for a person's allowance, ask with `needs_you`
   (what to add or allow on the Secrets page, and the link), say so in the summary and stop.
3. Deploy: call `cd_deploy` with `{"environment": "dev"}`, start `yulius wait` with
   run_in_background and end your turn; when it returns, read the run with `cd_status` and its
   `run_id`. The run already ran the verify script.
4. Test what is deployed beyond the verify script where the project gives a way: its smoke or
   end-to-end tests against the dev address (documented in the README, AGENTS.md or `scripts/cd/`),
   or one real user path in the browser tools. Never change data that is not a test's own.
5. A failed deploy, verification or test: find the likely cause from the output and the commits
   since the last passed run (`git log <deployed>..origin/<integration>`), then file one issue with
   `issue_create` (type bug, priority high, area deploy) with the run id, the failing step and the
   evidence, unless an open issue already tracks it (comment on it instead). Do not fix it in this
   task.
6. End with a summary: the commit deployed, the run's result, what was tested, and the issue
   filed or updated.
