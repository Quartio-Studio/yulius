# 0204. The secrets import refuses ordinary text, and an operate token deletes an agent secret

## Status

Accepted. Specified in [SPEC-274](../../.ai/specs/SPEC-274-agent-secrets-refuse-plain-words-that-ma.md);
task #258. Extends [ADR-0194](0194-an-operate-token-confirms-for-its-person-s-laptop-onboarding.md)
(unattended laptop onboarding).

## Context

Redaction masks every occurrence of an agent secret's value of 8 or more characters, everywhere
Yulius shows text from a sandbox. On 22 Oct 2026 a company `.env` imported unattended
(`yulius secrets import --to agent --yes`) held `PIPEDREAM_PROJECT_ENVIRONMENT=production`: from
then on every "production" read `[secret:…]` in 221 files of the repository, in the project's
config and in the onboarding hand-over; an imported e-mail address masked 7 more files. The
person's agent could not undo it: no remote tool or CLI command deletes an agent secret, only the
Secrets page does, which breaks "nothing else needed from the person".

## Decision

- **The import refuses ordinary text.** Before anything is sent, `yulius secrets import --to
  agent` refuses a value that cannot be told apart from ordinary text: a configuration word, a
  cloud region, time zone, locale or path, a host name, an e-mail address, a URL without
  credentials (no user information, no key-named parameter, no token-like host label or path
  segment). Each is listed as skipped with why, like the other invalid entries, with or without
  `--yes`; the message says it belongs in configuration or the hand-over unless it is a
  credential after all, which a person then adds on the Secrets page (never in the hand-over). The classifier is pure domain code
  (`vault::ordinary_text`), so the Secrets page can use it later.
- **A value found in the repository is asked about.** Run in a checkout of the project, the import
  looks for each new value in the tracked files; the person answers yes to import it anyway (a
  committed real key should be masked), `--yes` skips and lists it. The value stays in memory: no
  `git grep` argument, no temporary file.
- **The server's check is unchanged.** `check_agent_value` keeps its rules, so existing secrets,
  the Secrets page and other clients behave as before.
- **An operate token deletes a project's agent secret**: the remote tool `secret_delete` and
  `yulius secrets delete NAME` (through it), with the `Configure` right the Secrets page needs,
  audited `remote.tool` with the token and `vault.agent.delete` with the name, never the value.
  Global and personal secrets stay on the Secrets page.
- **A masked word is explained**: the hand-over added to an onboarding request and the remote
  instructions say that `[secret:NAME]` standing for an ordinary word is a secret holding plain
  text, to be read from context and deleted.

## Alternatives

- **Refuse on the server too**: would also catch older CLIs, but changes the Secrets page and
  every API client at once and cannot see the laptop's repository; left for a later change.
- **Stop redacting plain words**: would fix existing secrets, but a redaction that guesses which
  values to hide could leak a real one; the value is the person's, so the fix is to delete it.
- **Refuse repository values outright**: a real key committed by mistake is in the repository too.
- **`Administer` for delete**: a maintainer who imported a wrong value could not remove it.

## Consequences

- An unattended import of a full company `.env` creates only values that look like credentials;
  the skipped names tell the agent what to put in the hand-over instead.
- The classifier has false negatives (an unusual word with digits) and false positives (a
  password that is a lowercase word); both are visible in the plan and the person can still add
  the value on the Secrets page.
- A wrong secret no longer needs the person: their agent deletes it, and the audit shows it.
