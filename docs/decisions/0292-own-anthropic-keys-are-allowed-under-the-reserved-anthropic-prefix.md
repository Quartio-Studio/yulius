# 0292. Own Anthropic keys are allowed under the reserved ANTHROPIC_ prefix

## Status

Accepted. Specified in [SPEC-455](../../.ai/specs/SPEC-455-i-once-typed-something-that-s-reserved-a.md)
(task #437, #1153). Amends the reserved names of agent secrets and project variables
(`SecretName`, SPEC-211, SPEC-275).

## Context

A person adding their own Anthropic key, for an app the project builds, named it
`ANTHROPIC_API_KEY_QUARTIO` and got "ANTHROPIC_API_KEY_QUARTIO is reserved for the agent's
environment; choose another name". Every rename that kept the `ANTHROPIC_` start got the same
answer, and the message never said why, so it looked stuck.

The prefix is reserved for a reason. Agent secrets (as placeholders) and project variables (with
real values, in every task run) reach the agent CLI's own process, which holds the provider
credential. The agent CLI (Claude Code 2.1.292) holds 76 distinct `ANTHROPIC_` strings: the
credential (`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_IDENTITY_TOKEN_FILE`,
`ANTHROPIC_FEDERATION_RULE_ID`, …), where it sends requests (`ANTHROPIC_BASE_URL`,
`ANTHROPIC_AWS_*`, `ANTHROPIC_GOOGLE_CLOUD_*`, …), where it reads its config
(`ANTHROPIC_CONFIG_DIR`, `ANTHROPIC_PROFILE`), and more each release. A first fix that reserved a
hand-made list of them missed a dozen (review round 1).

## Decision

- `ANTHROPIC_` stays a reserved prefix, so a name the agent CLI starts reading later is covered.
- A person's own key extending a credential name is accepted: `ANTHROPIC_API_KEY_<NAME>` and
  `ANTHROPIC_AUTH_TOKEN_<NAME>` (at least one character after the `_`), except a name ending in
  `_FILE`, which by convention points a tool at a credential file. The agent CLI never reads
  these as settings or credentials: it only lists `ANTHROPIC_*` names (their names go into its
  startup telemetry, and its sandboxed toolset shell drops them). The unit test pins the 76
  names in its strings as refused.
- Every refusal says what is reserved and a name that works: "… as is every name starting with
  GIT_; choose another name, such as APP_GIT_TOKEN"; for `ANTHROPIC_` it names the own-key forms.
- Every other reserved name and prefix is unchanged, and so is the repository's `[resources.env]`
  list (`resources.rs`), which an agent can edit.

## Alternatives

- **Reserve only the `ANTHROPIC_` names the agent CLI reads.** Accepts more names, but the list is
  long, grows each release and was already incomplete once; a variable it misses holds a real
  value in the agent CLI's environment.
- **Drop the `ANTHROPIC_` reservation.** A variable `ANTHROPIC_BASE_URL` would send the agent
  CLI's requests and credential elsewhere.
- **Only reword the message.** The name the person wanted would still be refused.

## Consequences

- `ANTHROPIC_API_KEY_QUARTIO` works; `ANTHROPIC_KEY_PROD` is still refused, and its message
  names the forms `ANTHROPIC_API_KEY_<NAME>` (not ending in `_FILE`) and suggests
  `APP_ANTHROPIC_KEY_PROD`.
- If the agent CLI ever reads a name of the own-key form (other than `_FILE`), it must be added
  here; the pinned list in the test is the place to check on a CLI upgrade.
