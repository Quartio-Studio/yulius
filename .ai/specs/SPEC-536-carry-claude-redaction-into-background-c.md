---
yulius-task: 4aaa1fd6-4570-4583-93ef-a22a88cfff28
status: done
---

# SPEC-536 Claude background shell output redaction

## Goal

Hide dotenv values when Claude retrieves the output of a background shell that read a dotenv file. Fixes #1457.

## Context

SPEC-525 carries dotenv tracking through Codex and Antigravity continuations. Claude's transcript reader recognizes a Bash command on a dotenv file and remembers its tool-use ID. The launch result returns a shell ID. A subsequent BashOutput or TaskOutput names that shell but has a fresh tool-use ID. The reader currently loses the association. Earlier Claude cursor tests cover only the original tool result.

## Requirements

- **CBR-001** Apply dotenv redaction to every result of BashOutput or TaskOutput for a protected shell. Keep ordinary output from unrelated shells readable.
- **CBR-002** Preserve the shell association in the server's read cursor and reconstruct it when scanning earlier lines for a page or tail.

## Plan

- [x] Document the missing Claude shell association
- [x] Reproduce the read paths with synthetic Claude transcripts
- [x] Carry dotenv protection through Claude shell continuations
- [x] Verify and independently review the fix for delivery

## Acceptance criteria

- **AC-1 (CBR-001)** A synthetic Bash launch reading .env followed by either output tool shows the assignment names and redaction markers. An interleaved ordinary shell keeps its public value.
- **AC-2 (CBR-002)** The same output is protected on a full page, a page starting at the result, a read resumed from immediately after the launch result, and a tail containing only the later results.

## Test plan

Use the transcript reader's JSONL fixtures and public page and tail entry points. Exercise the legacy Bash launch notice and modern task output, including a repeated read. Run the regression before and after the fix, the transcript tests, repository quick checks and the configured security command. Yulius CI verifies the pull request.

## Decisions

Extend the transcript-local tracking already kept in ReadCursor. Associate a shell with the result of its protected Bash launch, then associate each output call with that shell by its exact ID. This uses the existing reader lifecycle and requires no persistent or client-visible cursor change.

## Evidence

- On the original reader, `cargo test --locked -p yulius-app --lib claude_background_shell_output_is_redacted_on_every_read` failed. BashOutput retained both synthetic assignment values. The manual-background notice and a structured launch result accompanied by text blocks also failed before their respective fixes.
- The first independent review found that the prefix scan skipped the accepted `server_tool_use` block type. The synthetic TaskOutput reproduced that failure on a cold page. Round 2 approved `2fa18978024a05e2bc7121615bb07364c05b99bc` after the scanner recognized both Claude tool-use block types.
- `cargo test --locked -p yulius-app --lib transcripts::` passed all 23 transcript tests after merging current dev into the feature branch. The regression checked five launch variants, two retrieval tools and an unrelated public shell across four read paths. The additional fast-path assertion checks that the prefix scan skips ordinary transcript text.
- `scripts/dev quick` passed repository contracts and formatting. Scoped Clippy passed with `cargo clippy --locked -p yulius-app --lib --tests -- -D warnings`. `scripts/dev security` reported that Vidoc is disabled. Verification used static source inspection and synthetic JSONL.
- The original workspace runner reported `29 listed, 21 caught, 0 missed, 0 timed out, 4 unviable, 4 not reached (bound 30 min, took 1805 s)`. Its post-review run against `b1c1ede59` reported `2 listed, 2 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached (bound 30 min, took 1266 s)`. All of those reported catches came from the unrelated xtask failure tracked by #1364. The mutation evidence below comes from matching baseline and mutant test selections.
- A focused run exposed a missing fast-path assertion when replacing the prefix scan with a constant true. After adding that assertion, two disjoint `cargo mutants --test-package yulius-app --test-tool nextest` runs selected all 31 mutations. They reported 11 caught and 2 unviable in the first group, and 16 caught and 2 unviable in the second. One log showed only an unrelated hub timing failure. A separate run selecting just the synthetic Claude regression caught that mutation in 2 minutes. The transcript tests detected all 27 compilable mutations.
- Incremental search indexing resets tracking at a batch boundary on dev. That separate surface is tracked in #1453 and noted on #1457.
