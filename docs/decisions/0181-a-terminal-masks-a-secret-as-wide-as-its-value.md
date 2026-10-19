# 0181. A terminal masks a secret with a label as wide as its value

## Status

Accepted. Specified in [SPEC-239](../../.ai/specs/SPEC-239-termial-gets-bugged-often-fix.md);
task #225, part of #705. Refines exact-value redaction (SPEC-002 ADR-20, VLT-007) for the
terminals people watch.

## Context

- The terminals people watch (a task's, a Superagent's, a Cockpit shell's) pass PTY output through
  a streaming redactor before the hub's `vt100` screen and every viewer. It replaced each form of a
  secret's value with `[secret:NAME]`, whatever the width.
- Claude Code and other full-screen programs redraw with cursor motions computed from what they
  wrote: "up two rows", "column 30", "erase this line". When the label is wider than the value, the
  line on the viewer's screen is longer and wraps, so every later motion lands in the wrong cells.
  The screen fills with fragments such as `IRONMENT]owances`. With a secret whose value is an
  everyday word (`production`), this happened on most answers.
- Text channels (transcripts, MCP results, tool output, proxy responses) address no cells.

## Decision

- In the terminals only, a value is replaced by `fitted_label(name, width)`. The width is counted
  as the hub's `vt100` screen counts it: each character's `unicode-width` 0.2 width added up (no
  grapheme rules, so an emoji with a skin-tone modifier takes 4 columns), and no column for a
  control character below 256. When the value is at least as wide as the label, the result is the
  full label followed by `*` up to that width. When the value is narrower, the label is cut and closed with
  `]` (`[secret:P]`). Below two columns it is `*`.
- The domain redactor takes the replacement as a function (`Replacement`, through
  `StreamRedactor::with_replacement` and `redact_all_with`). The function gets the whole form that
  was found and how many of its bytes the stream had already shown. Every other channel keeps
  `label_replacement`, the full `[secret:NAME]`.
- Holding and releasing tails is unchanged. When a beginning of a value has already been shown
  (under the 4-byte hold minimum, or released after 40 ms of quiet), the mask covers the value's
  width minus the columns of the whole characters already shown. The screen draws nothing for a
  character cut in the middle.

## Alternatives

- **A fixed mask of `*` as wide as the value.** It keeps the layout but no longer says which secret
  was hidden. The fitted label keeps the name whenever there is room.
- **Not redacting the terminal**, or redacting only lines that are not redrawn. That shows the
  value to everyone who watches, which VLT-007 forbids.
- **Rendering the screen on the server and redacting the cells.** Viewers would get a re-encoded
  screen instead of the program's own stream. That is a large change to the hub and the viewers,
  for the same visible result.
- **Refusing ordinary words as values.** That is #705's policy question. It would help in this one
  case, but every real credential shown inside a full-screen program would still break the layout.

## Consequences

- A redraw over a redacted value lands in the cells the program meant. The test is
  `vault::terminal::tests::redraws_over_a_redacted_value_keep_their_cells`, which uses a `vt100`
  screen and splits the stream at every point.
- A short value shows a shortened label (`[secret:P]`), not the full name. Its full name still shows
  in the transcript.
- Anyone watching a terminal can now read each value's display width, which is roughly its length,
  from the label and its padding. The old fixed label did not show it. This is accepted: the agent
  can already read the length (`${#VAR}`), and the length is not the value.
- Some characters are drawn differently by xterm.js and `vt100`: ambiguous-width characters, and
  grapheme clusters, which xterm.js may join. A tab moves to a tab stop that no printable mask can
  reach. A value holding any of these can still shift a viewer by a column. Agent secrets are ASCII
  in practice.
- Tests that search a screen for `[secret:NAME]` still find it whenever the value is at least as
  wide as the label. Text right after it is now preceded by the `*` padding.
