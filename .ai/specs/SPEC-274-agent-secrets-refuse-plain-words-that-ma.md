---
yulius-task: 813310f0-3768-4693-8f35-a6f94801d35e
status: done
---

# SPEC-274: Agent secrets refuse plain words that mask the repository; an operate token deletes a secret

Status: done
Date: 2026-10-22
Owner: task #258
Scope authorization: task #258. Found on 22 Oct 2026 while onboarding Quartio-Studio/product from
a laptop (SPEC-256's first user).

## Goal

`yulius secrets import --to agent` no longer turns ordinary text into agent secrets: a value that
cannot be told apart from ordinary text (a configuration word, a host name, a plain URL or e-mail
address) is refused and named, and a value that occurs in the project's repository is asked about
(skipped and listed with `--yes`). A secret imported by mistake can be deleted by the person's
agent with its operate token (`yulius secrets delete NAME`, remote tool `secret_delete`), audited,
never showing the value. An onboarding task told a masked word is a secret's value can ask.

## Context

Confirmed against `dev` at 9080cf3e.

- A company `.env` imported with `yulius secrets import --to agent --yes` held
  `PIPEDREAM_PROJECT_ENVIRONMENT=production` and `PIPEDREAM_CUSTOMER_PROJECT_ENVIRONMENT=production`.
  `check_agent_value` (domain `vault/mod.rs`) only requires 8 characters, so both were created.
  Redaction (`vault/redact.rs`, every surface that shows sandbox text) then replaced every
  `production` with `[secret:…]`: 221 files of the repository as tools return them, the project's
  config (`[secret:PIPEDREAM_CUSTOMER_PROJECT_ENVIRONMENT]_branch: main`) and the hand-over notes
  in the onboarding request. An e-mail address imported the same way masked 7 files.
- The import classifies entries in `yulius-import/src/secrets_import.rs` (`agent_value`):
  `Class::Invalid(why)` entries are listed `! NAME  skipped: why`, counted in `invalid`, never sent,
  and make the run exit 1. The run asks `Type yes` at the terminal, or with `--yes` sends unattended
  under the operate token (SPEC-256 LOB-002, ADR-0194).
- `DELETE /api/agent-secrets/{id}` exists (`App::delete_agent_secret`: needs `Action::Configure`
  on the project, audited `vault.agent.delete` with the name). The remote MCP (SPEC-247) has
  `secrets_list`, `secret_update` and `secret_hosts_answer` (`Action::Configure`), no delete;
  `yulius secrets list` calls `secrets_list` through `remote_cli::run`. Every remote call that
  changes something is audited `remote.tool` with the token id, never its arguments.
- The onboarding hand-over is added to the task's request by `request::handover_addition`
  (SPEC-256 LOB-005).

## Requirements

- **PLW-001** A value is *ordinary text* when, trimmed, it is (a) a configuration word: only
  letters with `-`, `_`, `.` or space separators, every part a known configuration word
  (production, development, staging, main, true, false, …, any case); generic names of the
  project are caught by PLW-003 instead, so random lowercase passwords are not refused; (b) a host
  name (dot-separated lowercase or all-capital labels, an alphabetic last one, no label that is a
  token) or an IPv4 address, with an optional port; (c) an e-mail address; (d) a URL
  (`scheme://`) without credentials: no user information, no query parameter named like a key
  (`key`, `token`, `secret`, `password`, `sig`, `auth`, …), and no host label, path segment or
  query value that is a token (6 or more letters and digits mixing both, 16 or more characters
  mixing letters and digits, or 24 or more mixing cases), so webhook URLs stay secrets; (e) a
  setting: a cloud region (`us-east-1`, `europe-west4`: a known area and directions), a time zone
  (`Europe/Warsaw`), a locale (`en_US.UTF-8`) or a path of names (`/var/lib/app`). The domain
  names which of them.
- **PLW-002** `yulius secrets import --to agent` refuses ordinary-text values before anything is
  sent: each is listed `! NAME  skipped: <which>: redaction would mask it everywhere it appears;
  it belongs in configuration or the hand-over; if it is a credential after all, a person adds it
  on the Secrets page` and counted invalid, with or without `--yes` (a credential the classifier
  took for text must never be pointed at the hand-over). The value itself is never printed.
- **PLW-003** When the import runs in a checkout of the project it imports into, each new value is
  looked for in the checkout's tracked files (`git ls-files`, files up to 1 MiB, bounded total).
  A value found there is asked about at the terminal (`yes` imports it, anything else skips it),
  naming how many files and one path; a no is counted as declined (the run still succeeds); with
  `--yes` it is skipped and listed as invalid. The search is one pass (Aho-Corasick) over regular
  text files, skipping links, binary files and files over 1 MiB, and stops at 256 MiB, saying so.
  The value never reaches a process argument or the output. A checkout on a detached `HEAD`
  counts. Outside a checkout of the project nothing is scanned and the run says so.
- **PLW-004** The remote tool `secret_delete {project, secret}` deletes the project's own agent
  secret `secret` (not a global or personal one shown in its list): it needs an operate token and
  `Action::Configure` on the project (as the Secrets page and `secret_update`), is audited
  `remote.tool` with the token id and `vault.agent.delete` with the name, and answers the name,
  never the value.
- **PLW-005** `yulius secrets delete NAME [--project owner/name]` calls `secret_delete` with the
  token `yulius remote login` stored; exit codes as the other remote commands.
- **PLW-006** The onboarding hand-over added to a task's request says that `[secret:NAME]` is the
  value of the agent secret NAME, and that where it stands for an ordinary word that secret holds
  plain text by mistake: read the word from context and ask the person to delete the secret. The
  remote instructions tell the person's agent the same and name `yulius secrets delete`. Notes
  already stored are not rewritten.

## Plan

- [x] Spec, ADR and plan
- [x] Domain tells ordinary text from secrets
- [x] Import refuses ordinary-text values
- [x] Import asks about values found in the repository
- [x] Remote tool secret_delete
- [x] CLI yulius secrets delete
- [x] Hand-over and instructions explain masked words; docs
- [x] Independent review, gates and delivery

## Acceptance criteria

- An import of `ENV=production`, `HOST=api.example.com`, `MAIL=ops@example.com`,
  `URL=https://api.example.com/v1` sends none of them and lists each as skipped with why; a real
  key, `postgres://u:pass@h/db` and a Slack-style webhook URL are imported.
- With `--yes`, a value that occurs in a tracked file of the project's checkout is skipped and
  listed; without `--yes` the run asks, and `yes` imports it.
- An operate token deletes a project's agent secret by name through `secret_delete`; a read token
  cannot; a global secret is refused; the audit holds the tool and the name, never the value.
- `yulius secrets delete NAME` deletes it and exits 0; an unknown name exits 1.

## Test plan

- Domain unit tests for every PLW-001 class and the false positives that must stay secrets
  (random keys, mixed-case tokens, credential URLs, webhook URLs).
- `yulius-import` integration tests (`tests/import/secrets.rs`, the mock vault server): refusal
  with and without `--yes`; repository scan asking and skipping; nothing sent for them.
- `yulius` integration tests (`tests/remote_mcp.rs`): `secret_delete` with operate and read
  tokens, global secret refused, audit rows; the CLI test for `yulius secrets delete`.
- Domain test of `handover_addition`; remote instructions test.

## Decisions

- **Refuse on the laptop, not on the server** (ADR-0204): the server keeps `check_agent_value`
  as it is, so existing secrets, the Secrets page and e2e fixtures are unchanged; the import is
  where unattended bulk values come from. A person typing a value on the Secrets page sees it.
- **Ask about repository values, never refuse them outright**: a real key committed by mistake
  occurs in the repository too, and masking it there is right; only the person can tell.
- **Delete needs `Configure`, as on the Secrets page**: whoever could import the secret can remove
  it; `Administer` would leave a maintainer who imported a wrong value stuck again.
- **The CLI deletes through the remote MCP**: one audited path (`remote.tool` with the token),
  the same token as `yulius secrets list`.

## Evidence

- `scripts/dev ci` (fmt, lint, macOS compile, deps, docs, nextest) at 69718ecb: exit 0, 2495
  tests passed; after review round 1 at 19877a75: exit 0, 2499 passed, 17 skipped. Round-2 nits
  (437be996, three tests added, docs) verified with `cargo nextest run -p yulius-domain -p
  yulius-import` (484 passed) and `cargo clippy -p yulius-import -p yulius-domain --all-targets
  -D warnings` (clean); Yulius CI reruns the whole gate on the pull request head.
- New tests: `vault::ordinary::tests::*` (words, hosts, e-mails, plain URLs, settings, and real
  credentials and webhook URLs that must stay secrets); `repo_values::tests::*` (one pass, links,
  large and binary files, the byte budget); `import secrets::ordinary_text_values_are_refused_and_named`,
  `an_unattended_run_skips_values_found_in_the_repository`, `a_value_found_in_the_repository_is_asked_about`
  (yes imports, no is declined and the run stays ok), `outside_the_project_checkout_the_repository_is_not_searched`,
  `a_detached_checkout_of_the_project_is_searched`, `a_checkout_without_commits_is_searched`;
  `remote_mcp::an_operate_token_deletes_a_projects_agent_secret` (read token, reader, global and
  personal secrets refused; `remote.tool` and `vault.agent.delete` audited without the value),
  `remote_mcp::the_cli_deletes_an_agent_secret_by_name` (exit 0, then 1 for the gone name);
  `request::tests::a_hand_over_follows_the_request_under_whose_agent_wrote_it` and
  `remote::tests::the_instructions_teach_onboarding_a_local_folder` (PLW-006). Each new test
  failed before its change, except `a_detached_checkout_of_the_project_is_searched`, a guard:
  the old code already handled a detached `HEAD`.
- Independent review in a fresh session: round 1 requested changes (webhook URLs taken for plain
  URLs and pointed at the hand-over; regions, time zones, locales and paths missed; scan limits
  untested and unbounded in time; personal-secret refusal untested; ADR wording; a declined value
  failing the run), all fixed test-first in 19877a75; round 2 approved 19877a75 with nits (all-digit
  ids, a silent matcher failure, a no-commit checkout, the exit-code doc), fixed in 437be996.
- `scripts/dev security` at 437be996: exit 2, not run (Vidoc HTTP 429, the key's 50,000,000
  tokens used); continued without Vidoc as the task's owner asked on 2026-10-22. Not a pass.
- Not tested: a real laptop onboarding against the live instance; existing secrets that already
  hold plain words are not changed (delete them with `yulius secrets delete NAME`), and stored
  hand-overs keep their old text.
