# 0019. Tasks default to Opus 5.5 at high effort

## Status

Accepted. Supersedes the `opus` alias of SPEC-011 HRN-012; specified in
[SPEC-023](../../.ai/specs/SPEC-023-make-sure-the-default-model-is-always-op.md).

## Context

A task that named no model ran Claude Code's `opus` alias, which moves to each new Opus, and a task
that named no effort sent none, so Claude Code chose. People asked for the default to be "always
opus 5.5" at high effort (task #21).

## Decision

`yulius_domain::harness` holds `DEFAULT_CLAUDE_MODEL = "claude-opus-5-5"` and
`DEFAULT_CLAUDE_EFFORT = "high"`. Every launch plan (new run, resume, account switch) passes
`--model` and, unless the model is a Haiku model (no effort levels), `--effort`. The defaults are
applied at launch; a task record keeps `None` for "Default".

## Alternatives

- Keep the `opus` alias: always the newest Opus, but not the model people chose, and it changes
  under running projects without a release of Yulius.
- Store the default on the task when it is created: tasks would show the model they run, but
  existing tasks and every API client would need it filled in, and changing the default later
  would mean a migration.
- Set `model` and `effortLevel` in the run's Claude settings file: a `--model`/`--effort` flag is
  already the one place the task's choice goes, and the flag outranks the settings.

## Consequences

Moving to a newer Opus is a one-line change of the constant and its test. The weekly Opus cap
keeps binding for default tasks. A model added later that takes no effort level and is not a Haiku
needs a rule here.
