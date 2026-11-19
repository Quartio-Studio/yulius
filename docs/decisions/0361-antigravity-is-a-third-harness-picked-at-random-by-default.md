# 0361. Antigravity is a third harness, on a pinned agy, picked at random by default

## Status

Accepted on 2026-11-17 (`F 2026-11-17 Antigravity`: the person's request in task #487, "Implement
antigravity cli next to claude cli … it should be randomised by default what we are using but also
we should be able to choose. Use Gemini 4 Carbon high by default."). Specified in
[SPEC-505](../../.ai/specs/SPEC-505-implement-antigravity-cli-next-to-claude.md). Its **Draw**
was superseded by [ADR-0368](0368-a-tasks-agent-is-set-never-drawn.md) and then by
[ADR-0377](0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md)
(2026-11-19): Random draws uniformly across all harnesses (`claude`, `codex`, `antigravity`) that
have non-exhausted subscriptions; the rest stands. Follows
[ADR-0355](0355-codex-returns-as-a-second-harness-with-a-pinned-cli.md) (Codex) in every seam;
provider switching and session translation stay removed (SPEC-002 ADR-06).
Refined by [0369](0369-a-google-sign-in-runs-until-the-person-is-done.md): a Google sign-in runs
on for the CLI's setup until the person is done. Its **Model** and **CLI** are amended by
[0373](0373-gemini-4-neon-is-antigravitys-default-model-on-agy-1-5-12.md): Antigravity tasks
default to Gemini 4 Neon on `agy` 1.5.12, with Gemini 4 Carbon kept selectable.

## Context

- Yulius runs Claude Code and, since ADR-0355, Codex. The person wants to compare Claude Code with
  Google's Antigravity CLI on real work and later keep one, using the transcripts and outcomes.
- A comparison where people pick the agent per task measures their choice as much as the agent.
- `agy` 1.5.10 (checked against a local Gemini API stub, SPEC-505 Context) keeps everything under
  `$HOME/.gemini`, takes `--model`, `--effort`, `--conversation`, `--prompt-interactive`, writes a
  JSONL transcript per conversation, runs Claude-like hooks from `config/hooks.json` (an empty
  `PreToolUse` answer denies the tool), keeps a Google sign-in in a file when no D-Bus session bus
  exists, and updates itself unless `AGY_CLI_DISABLE_AUTO_UPDATE` is set. It is one Go binary
  published as a tarball with a sha512 on Google's release manifests; there is no npm package.

## Decision

- **Harness.** `Harness::Antigravity` (`antigravity`) with `Provider::Google` (`google`). A task
  keeps its harness for life; rotation, moves and removals stay within the provider.
- **Draw.** A task created without a harness (or with `auto`) runs Claude Code or Antigravity,
  drawn uniformly among those two with a usable subscription (Claude Code when neither has one);
  a harness given is kept, and so is the one a model given without one implies (Antigravity for
  a Gemini model, else Claude Code). An automation without an agent draws for each run. The
  task records `harness_choice` (`random` or `chosen`). Codex runs only when chosen.
- **Model.** An Antigravity task that names no model runs `gemini-4-carbon` at effort `high`,
  passed on every launch.
- **Homes.** Each run has its own home in its run directory, `<run>/antigravity-home`, whose
  `.gemini/antigravity-cli/{brain,conversations}` link to the sandbox's shared ones
  (`<sandbox>/antigravity/shared`). A contained run binds that `.gemini` over the sandbox home's
  (a path only the server wrote, never one an agent of the sandbox could swap for a link); an
  uncontained one sets `HOME` to it.
- **Hooks.** `yulius hook antigravity <event>` translates Antigravity's payloads and answers to
  the Claude-shaped contract the server already serves; its fail-open answer to `PreToolUse` is
  `allow`.
- **Credentials.** A Gemini API key (secret environment, `modelProvider: gemini`) or the token
  file the CLI's own Google sign-in writes in a temporary home, sealed in the vault, written into
  the run's home and written back when the run changed it; the file is opaque, so the run's own
  agent can replace the account's sign-in with a file of its making (a sign-in a person made
  meanwhile is never replaced).
- **CLI.** Yulius installs `agy` 1.5.10 from the URL and sha512 in its source into
  `<data>/antigravity-cli/1.5.10-<platform>` (Linux x86-64 and arm64), with auto-update off, and
  starts it by its absolute path.

## Alternatives

- **Let people choose only**: simplest, but the comparison the person wants would be biased by
  who picks which agent for which work.
- **Alternate between agents, or weight by capacity**: predictable or biased; a uniform draw
  among usable agents is neither.
- **Draw among all three harnesses**: Codex is not part of the comparison the person asked for.
- **Run `agy` with `HOME` set to its home in every run**: the agent's shell would lose the sandbox
  home's Git, GitHub and SSH configuration.
- **`agy acp` or headless `-p` runs**: no native terminal to watch and type into.
- **Follow `agy`'s self-update**: an unchecked release would reach every run.
- **Parse the Google token file**: its format is undocumented and was not seen live; Yulius keeps
  it opaque.

## Consequences

- Each task says whether its agent was drawn, and the analytics export lists it, so drawn tasks
  compare the two agents on the same mix of work.
- Not verified without a live Google account: a real Google sign-in and the token file it writes,
  its refresh and write-back, a plan's real quota texts and reset times. A Google subscription's
  usage windows are not shown; its limits are learnt from the transcript with an unknown reset.
- About 270 MB in the data directory per installed release and a 100 MB download on first use.
- Claude-only features stay Claude-only for Antigravity tasks, as for Codex: typed openings with
  pasted images, the transcript step-back, the status line, sub-agent tails, session import and
  search, workflows, model limits and their question. A goal task gets its whole request as its
  first message and is settled by its turns.
- An uncontained run's shell sees the Antigravity home as `HOME`.
- A sandbox MCP server that needs a bearer token is not offered to Antigravity runs.
- A Google sign-in is the whole `agy` TUI on the host (it has no sign-in-only command), stopped
  5 s after its token file appears. Once signed in, it could run a shell command an
  administrator types into it, which an instance administrator can already do with an MCP
  server's Test (SPEC-218); unlike ADR-0256's sign-ins it is not a login-only command. A sign-in
  that asks more after writing the file (a billing or project choice, not seen without a live
  account) is cut then; the run's own TUI asks it again, and the run's write-back keeps what it
  stores.
- A resumed conversation gets no new briefing (the CLI runs no `SessionStart` then); its first
  start's briefing stays in its history.
