# 0016. Agent primitives are managed with APM

## Status

Accepted. Details in [SPEC-005](../../.ai/specs/SPEC-005-agentic-primitives.md).

## Context

Repository skills were hand-edited in `.agents/skills` with no manifest, lock or drift check, and
only one harness layout. The ML in PL agentic-coding workshop (microsoft/apm, *Agentic SDLC*)
treats primitives as code: sources in `.apm/`, `apm.yml` + `apm.lock.yaml`, deployment per harness,
`apm audit`, pinned external packages, and design patterns that keep primitives small and testable.

## Decision

`.apm/skills/<name>/` is the only edited source. `apm.yml` (targets `claude`, `codex`) and
`apm.lock.yaml` are committed; `apm install` generates `.agents/skills` and `.claude/skills`, which
are committed too. `scripts/check_repository.py` verifies deployed files equal their sources
without needing `apm`. New primitives follow `docs/engineering/primitives.md`; onboarding audits a
project's conventions into primitives.

## Alternatives

Hand-edited `.agents/skills` (status quo, drifts); symlinks; a custom sync script; ignoring the
deployed directories (breaks clones that do not run `apm`).

## Consequences

Edits go to `.apm/skills` and need `apm install` (pip `apm-cli`). Deployed copies add duplicate
files to diffs. Moving vendored `catalog/skills` to pinned APM dependencies is a follow-up.
