# 0194. An operate token confirms for its person's laptop onboarding

## Status

Accepted. Specified in [SPEC-256](../../.ai/specs/SPEC-256-user-should-be-able-to-smoothly-onboard.md);
task #242. Amends [0163](0163-the-cli-imports-secrets-after-a-typed-yes-and-shows-no-value.md)
(a typed yes, always) for `operate` tokens; extends
[0188](0188-remote-control-uses-scoped-personal-api-tokens.md).

## Context

The operator wants a person to say "onboard this into Yulius" to the agent on their laptop and get
a working project hours later, "not human action needed", keys uploaded and their use approved.
Four steps still needed the person: a second sign-in (`yulius login`'s browser approval) for
uploads and secrets, the typed `yes` of `yulius secrets import` (an agent's shell has no terminal),
the Secrets page's allowance before an agent may copy a key into `.env` (how ssh and deploy
scripts get keys), and answering the onboarding task's `needs_you` for credentials the laptop
agent had already found.

ADR-0188 already let an `operate` token start onboarding: the person makes that token on purpose,
on Settings, for their own agent, and it never exceeds their own access.

## Decision

- **One token.** `yulius onboard` and `yulius secrets import` use the `yulius login` token when
  one is stored, else the token `yulius remote login` stored for the same instance.
- **`--yes` only with an `operate` token.** `yulius secrets import --yes` skips the typed `yes`
  when the instance confirms that the remote token is `operate`; otherwise nothing is sent. Every
  other rule of 0163 stays: values come only from files or stdin, nothing prints a value, nothing
  is overwritten.
- **Key files by path.** `--from-file NAME=PATH` imports a whole file (≤ 64 KiB) as one secret,
  so the agent finds a key without reading it into its conversation.
- **Allowed into `.env` at creation.** `AgentSecretInput.env_files: true` (`--env-files`) stores a
  new project or global secret as `env_files = allowed`, audited `vault.agent.env_allow`; only
  for secrets agents set up (`auto_allow`) and never for personal ones (ADR-0186).
- **Hand-over notes.** `onboarding_start` and the approve route take `notes` (≤ 24 KiB, redacted)
  added to the onboarding task's request before it starts, as a queued message is (SPEC-052);
  refused once it started. The onboarding prompt reads `secrets_list` and the hand-over before
  asking a person for a credential.

## Alternatives

- **`--yes` for any token.** 0163 rejected it: the flag would be copied into every script. Tying
  it to a token made for an agent keeps a deliberate act behind it.
- **Values through the remote MCP** (`secret_create {name, value}`). Simple for MCP-only agents,
  but the value would sit in the agent's context and transcript. Rejected.
- **A Settings switch "trust my agent's imports".** One more step for the person before the first
  request, and the same trust as the operate token. Rejected.
- **Notes stored as a separate onboarding field.** Every reader of the request would need to learn
  it; the queued addition already reaches them all.

## Consequences

- A local agent onboards a laptop project with three commands and no person: `yulius onboard
  --yes`, `yulius secrets import --to agent --yes --env-files …`, `yulius project onboard
  --notes …`.
- Whoever holds an `operate` token can now add secrets that every agent of the project may copy
  into `.env` files. Revoking the token on Settings ends it; every create and allowance is audited
  as the person.
- People who use `yulius login` alone see no change.
