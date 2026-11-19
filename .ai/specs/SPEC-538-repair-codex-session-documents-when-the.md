---
yulius-task: 63807294-3d86-4019-887e-396281b484dd
status: in-progress
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

## Plan

- [ ] Reproduce late rollout discovery through Sessions and search
- [ ] Repair persisted Codex placeholder paths during synchronization
- [ ] Verify compatibility and deliver the reviewed fix

## Acceptance criteria

- **AC-1** Create a document with no rollout, read its empty transcript, write a rollout, then request the same transcript URL and see its message.
- **AC-2** Search can discover a newly written rollout before any transcript read and return a passage from that document.
- **AC-3** The replacement path persists. Later refreshes keep the index usable and existing harness readers still pass.

## Test plan

Use the Sessions HTTP integration fixture with a real store, an isolated sandbox and synthetic Codex messages. Exercise transcript-first and search-first discovery independently. Run the session integration tests and existing transcript reader tests.

## Decisions

Reuse the sandbox's bounded Codex rollout lookup and the store's existing transactional path replacement. Recognize the generated placeholder for the document's session ID so resolved documents need no additional directory walks. This is a repair of the existing document contract and requires no schema change.
