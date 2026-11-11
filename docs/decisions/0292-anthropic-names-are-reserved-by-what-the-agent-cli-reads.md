# 0292. ANTHROPIC_ names are reserved by what the agent CLI reads

## Status

Accepted. Specified in [SPEC-455](../../.ai/specs/SPEC-455-i-once-typed-something-that-s-reserved-a.md)
(task #437, #1153). Narrows the reserved prefixes of agent-secret and project-variable names
(`SecretName`, SPEC-211, SPEC-275).

## Context

A person adding their own Anthropic key, for an app the project builds, named it
`ANTHROPIC_API_KEY_QUARTIO` and got "ANTHROPIC_API_KEY_QUARTIO is reserved for the agent's
environment; choose another name". Every rename that kept the `ANTHROPIC_` start got the same
answer, because `SecretName::parse` reserved the whole `ANTHROPIC_` prefix.

The reservation exists because secrets (as placeholders) and project variables (as real values)
reach the agent CLI's own process, which holds the provider credential: a name it reads, such as
`ANTHROPIC_BASE_URL`, would redirect it or break it. `CLAUDE_`, `GIT_` or `LD_` are namespaces of
tools; `ANTHROPIC_` is also the natural start of a person's own Anthropic key.

## Decision

- `ANTHROPIC_` names are reserved by what the agent CLI and the Anthropic SDKs read:
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`,
  `ANTHROPIC_BETAS`, `ANTHROPIC_LOG`, `ANTHROPIC_UNIX_SOCKET`, and the families
  `ANTHROPIC_CUSTOM_*`, `ANTHROPIC_DEFAULT_*`, `ANTHROPIC_SMALL_FAST_MODEL*`,
  `ANTHROPIC_BEDROCK_*`, `ANTHROPIC_VERTEX_*`, `ANTHROPIC_FOUNDRY_*`.
- Any other `ANTHROPIC_…` name is accepted, such as `ANTHROPIC_API_KEY_QUARTIO`.
- Every other reserved name and prefix is unchanged, and so is the repository's `[resources.env]`
  list (`resources.rs`), which an agent can edit and keeps the whole `ANTHROPIC_` prefix.

## Alternatives

- **Allow only names extending a reserved one (`ANTHROPIC_API_KEY_*`).** Fits the reported name,
  but `ANTHROPIC_KEY_PROD` would still be refused for no reason.
- **Drop the `ANTHROPIC_` reservation.** A variable `ANTHROPIC_BASE_URL` would send the agent
  CLI's requests and credential elsewhere.
- **Keep the prefix and reword the message.** The person still cannot use the name they want.

## Consequences

- A person names their own Anthropic keys and variables freely; the agent CLI's settings stay
  out of reach.
- A variable the agent CLI starts reading later under a new `ANTHROPIC_` name outside these
  families is not reserved until it is added to the list. Secrets reach it only as placeholders.
