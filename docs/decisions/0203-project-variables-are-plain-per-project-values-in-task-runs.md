# 0203. Project variables are plain per-project values in task runs

## Status

Accepted. Specified in [SPEC-275](../../.ai/specs/SPEC-275-project-variables-non-secret-values-regi.md);
task #259. Extends [0194](0194-an-operate-token-confirms-for-its-person-s-laptop-onboarding.md)
(laptop onboarding) and [0188](0188-remote-control-uses-scoped-personal-api-tokens.md) (remote
tools).

## Context

A company `.env` mixes keys with plain values: a region (`ovh-ca`), an environment name
(`production`), base URLs, account ids, a contact address. A project only had agent secrets, which
need 8 characters (so a short region is refused) and are redacted everywhere they occur (so
`production` as a secret masks every occurrence of the word in the repository and the
transcripts). The other places were hand-over notes, read once, or a commit, which is wrong for a
value of one deployment rather than of the code.

## Decision

- **A project variable** is a row of `project_variables` (migration 0043): the project's sandbox
  id, a name (agent secret name rules), a value in plain (≤ 4 KiB, one line, may be empty), a
  description (≤ 1000), who changed it and when; unique per project, at most 200, deleted with the
  sandbox. It is not encrypted, not redacted and not proxied.
- **A name that is an agent secret** of the project (its own, or a global one switched on for it)
  is refused. In a run an agent secret of the same name (a personal one, unseen by the person
  setting the variable) wins, because its placeholder is added after the variables.
- **Every task run** of the project gets the variables in its environment, added first, so
  Yulius's own environment and the agent secret placeholders win over a variable of the same name. CI jobs and CD runs do not: their
  environment comes from the repository's `yulius.toml` at a verified commit (ADR-0109).
- **Who changes them**: anyone who may configure the project (write access), on the Secrets page's
  Variables tab, over `PUT/DELETE /api/sandboxes/{sid}/variables`, or with the remote tools
  `variables_set` and `variable_delete` (operate token; `variables_list` for read tokens too) that
  `yulius vars set|import|delete|list` call. Each create, change and delete is audited
  `variable.*` with the name and changed fields, without the value.
- **One rule tells a plain value from a secret** (`yulius_domain::vault::plain`): shorter than 8
  characters and plain-shaped, or ordinary text by
  [0204](0204-secrets-import-refuses-ordinary-text-and-an-operate-token-deletes.md)'s rule
  (`ordinary_text`: a configuration word, a host, an e-mail address, a URL without credentials, a
  region, time zone, locale or path), unless the name says it is a credential (a word ending in
  `KEY`, `TOKEN`, `PASS`, `PW`, `CODE`, …, or containing `SECRET`, `PASSWORD`, …) or the value is
  a key.
  `yulius secrets import --to agent` offers such entries as variables and, with `--yes`, sends them
  as variables instead of skipping them or masking a word.

## Alternatives

- **Agent secrets with a "plain" flag**: every secret path (redaction, placeholders, proxy, `.env`
  copies, the minimum) would need a branch, and the flag would weaken the one guarantee secrets
  give. Rejected.
- **A committed config file**: the value belongs to one deployment, not to the code; a laptop agent
  would need a pull request per value. Rejected.
- **Hand-over notes only**: read once by the onboarding task, invisible to later tasks. Rejected.
- **Injecting into CI and CD too**: a laptop token could then change what a production deploy
  runs with, outside the verified commit. Left for a separate decision.
- **Refusing credential-like names on the server**: a person may keep a public "key" (a
  publishable id) as a variable on purpose; only the bulk paths from a `.env` refuse them.

## Consequences

- A value put into a variable by mistake is visible to everyone who sees the project and to every
  task; the CLI's name guard and the secrets import's rule make that unlikely from a `.env`, and a
  person can delete it.
- The import's ordinary-text rule (0204) decides both what is refused as a secret and what is sent
  as a variable; this decision adds only the credential-name guard and short values.
