---
yulius-task: 63807294-3d86-4019-887e-396281b484dd
status: done
---

# SPEC-538 Repair Codex session documents when the rollout appears

## Goal

People can read and search a Codex session even when they opened Sessions before its first turn.

## Context

Issue #1454 follows the transcript reader work in SPEC-525. When Codex has reported its session ID but has yet to write its dated rollout, document creation saves a placeholder path. Later synchronizations skip existing documents, and the existing path repair only follows resumed transcript copies. Both the transcript endpoint and the full-text index read the persisted path. Existing reader tests use a rollout already on disk.

## Requirements

- **RLT-001** A Codex task document created before its rollout exists reads the matching rollout as soon as it appears, including through a direct transcript request. Until then the document can be read again.
- **RLT-002** A search refresh can discover the rollout independently and index its messages under the original document ID.
- **RLT-003** Apply the repair to existing Codex task placeholders. Preserve established native paths, stored-copy handling, and Claude Code and Antigravity document behavior.
- **RLT-004** A second refresh acting on an old placeholder preserves the index built by the winning refresh, including if the second request is cancelled immediately afterward.

## Plan

- [x] Reproduce late rollout discovery through Sessions and search
- [x] Repair persisted Codex placeholder paths during synchronization
- [x] Preserve indexed passages across concurrent path repairs
- [x] Verify compatibility and deliver the reviewed fix

## Acceptance criteria

- **AC-1** Create a document with no rollout, read its empty transcript, write a rollout, then request the same transcript URL and see its message.
- **AC-2** Search can discover a newly written rollout before any transcript read and return a passage from that document.
- **AC-3** The replacement path persists. Later refreshes keep the index usable and existing harness readers still pass.

## Test plan

Use the Sessions HTTP integration fixture with a real store, an isolated sandbox and synthetic Codex messages. Exercise transcript-first and search-first discovery independently. Run the session integration tests and existing transcript reader tests.

## Decisions

Reuse the sandbox's bounded Codex rollout lookup and the store's existing transactional path replacement. Recognize the generated placeholder for the document's session ID so resolved documents need no additional directory walks. Direct reads resolve the pending path without writing. The index refresh persists it and resets the index cursor in its existing transaction. Existing rows are eligible on their next read or refresh. The transaction checks the previously observed path before resetting the index. A refresh that loses this race reloads the winning row.

## Evidence

- On the original implementation, `cargo test --locked -p yulius --test main late_codex_rollout` failed both tests. The transcript lacked the message and search returned no document after the fixture wrote the rollout.
- With the fix, `cargo test --locked -p yulius --test main sessions::` passed all five tests. The two request orders returned the rollout text and its search hit under the original document ID. The stored path changed from the legacy placeholder to the dated rollout. The Claude Code and Antigravity rows with the same session ID retained their original paths and index state.
- `cargo test --locked -p yulius-app --lib transcripts::tests::` passed all 21 reader tests, including the Claude Code, Codex and Antigravity formats.
- The first independent review found that a second refresh could erase the winning refresh's passages. `cargo test --locked -p yulius-store --lib a_repeated_path_repair` reproduced the loss of the 42-byte, two-line index cursor. With conditional path replacement, `cargo test --locked -p yulius-store --lib session_docs::tests::` passed all three tests, including preservation of the cursor and the search hit after a stale repair.
- A fresh independent reviewer approved `434ac35a3`, verified the conditional transaction and reran the three store tests. `scripts/dev quick` and `git diff --check` passed. `scripts/dev security` reported Vidoc disabled for this project.
- On the approved head, the five Sessions HTTP tests passed again. The review-fix mutation run against `856db384a` reported 12 listed, 1 caught, 0 missed, 0 timed out, 11 unviable and 0 not reached. The workspace baseline passed.
- The initial campaign was stopped when parallel workers copied different revisions across the review fix. The isolated full-diff run at `434ac35a3` passed its 3,128-test baseline and reported 33 listed, 6 caught, 0 missed, 0 timed out, 27 unviable and 0 not reached.
