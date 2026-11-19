---
yulius-task: 4aaa1fd6-4570-4583-93ef-a22a88cfff28
status: in-progress
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
- [ ] Verify and independently review the fix for delivery

## Acceptance criteria

- **AC-1 (CBR-001)** A synthetic Bash launch reading .env followed by either output tool shows the assignment names and redaction markers. An interleaved ordinary shell keeps its public value.
- **AC-2 (CBR-002)** The same output is protected on a full page, a page starting at the result, a read resumed from immediately after the launch result, and a tail containing only the later results.

## Test plan

Use the transcript reader's JSONL fixtures and public page and tail entry points. Exercise the legacy Bash launch notice and modern task output, including a repeated read. Run the regression before and after the fix, the transcript tests, repository quick checks and the configured security command. Yulius CI verifies the pull request.

## Decisions

Extend the transcript-local tracking already kept in ReadCursor. Associate a shell with the result of its protected Bash launch, then associate each output call with that shell by its exact ID. This uses the existing reader lifecycle and requires no persistent or client-visible cursor change.

## Evidence

- On the original reader, `cargo test --locked -p yulius-app --lib claude_background_shell_output_is_redacted_on_every_read` failed. The BashOutput result retained both synthetic assignment values.
- The manual-background notice and a structured launch result accompanied by a text block each failed before their respective fixes.
- `cargo test --locked -p yulius-app --lib transcripts::` passed all 22 transcript tests. The regression exercised both tools with five launch variants across four read paths. The unrelated shell retained its public assignment on each read.
- Review round 1 found that the prefix scan skipped the accepted `server_tool_use` block type. Switching the synthetic TaskOutput to that type reproduced the cold-page failure. The prefix scan now recognizes both Claude tool-use block types.
- Incremental search indexing resets tracking at a batch boundary on dev. That separate surface is tracked in #1453 and noted on #1457.
