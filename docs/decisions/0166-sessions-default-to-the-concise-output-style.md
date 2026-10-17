# 0166. Sessions default to the Concise output style

## Status

Accepted. Specified in [SPEC-210](../../.ai/specs/SPEC-210-https-code-claude-com-docs-en-output-sty.md).

## Context

The person asked for Claude Code's built-in Concise output style "everywhere": answers that lead
with the result and drop lead-in, narration and recaps. Yulius starts Claude Code for task runs
and for the Superagent, and prepares each launch's config dir first. Claude Code reads the case-
sensitive `outputStyle` setting from its settings layers: user (`<CLAUDE_CONFIG_DIR>/settings.json`)
< project < local < `--settings` < managed. Measured on 2.1.292: with `outputStyle` in `--settings`,
`/output-style Explanatory` reports success and writes the local file, but the session stays
Concise.

## Decision

Before every Claude Code launch, `ensure_claude_config` adds `"outputStyle": "Concise"` to the config
dir's `settings.json` when the file has no `outputStyle` key (creating the file when missing). An
existing value, any other key, and a file that is not a JSON object are left as they are. Run
settings never set `outputStyle`.

## Alternatives

- `outputStyle` in the run settings (`--settings`), like `autoCompactEnabled`: guaranteed in every
  session, but `/output-style` then claims a switch that never happens, and a repository's own
  choice is overridden.
- A custom output style file with Yulius's wording: duplicates the built-in style and drifts from
  it; without `keep-coding-instructions` it would also drop Claude Code's coding instructions.
- A `yulius.toml` switch per project: nobody asked for one, and a repository can already commit
  `outputStyle` in `.claude/settings.json`.

## Consequences

- New and existing sandboxes answer concisely from their next launch; a session already running
  keeps its style until it starts again.
- `/output-style`, `/config` and a committed project style work as Claude Code documents.
- Setting `"outputStyle": "default"` in the sandbox's user settings opts out for good; deleting the
  key brings Concise back at the next launch.
- Subagents other than forks keep their own prompts (Claude Code applies no style to them).
