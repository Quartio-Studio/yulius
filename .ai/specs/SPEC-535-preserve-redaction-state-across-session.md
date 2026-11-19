---
yulius-task: 1391bf1d-5afc-41a0-8c92-86c13b3c102d
status: in-progress
---

# SPEC-535 Preserve dotenv redaction across session index batches

## Goal

People and agents searching sessions see redacted values when a dotenv read and its result arrive in different index refreshes. Fixes #1453.

## Context

The session store commits passages with the byte and line offsets. On resume, `read_for_index` seeks to those offsets and creates an empty `EnvCalls`. The previous offset test splits before a bookkeeping line. It exercises resumption but keeps the call and its result together. Claude call IDs, Codex continuations and Antigravity step context all depend on earlier records. A stored passage can outlive the server that indexed it.

## Requirements

- **IDX-001** Resume from saved offsets using the same redaction context as an uninterrupted read, including after restart. Emit each newly completed passage at its original line number.
- **IDX-002** Reconstruct tracking from the indexed prefix only. Keep the existing line and session bounds, and account for replay work during refresh.
- **IDX-003** Remove potentially affected historical passages and their full-text matches on upgrade. Rebuild from the original transcripts through normal refresh.

## Plan

- [x] Specify redaction at the saved index offset
- [x] Prove the split-batch search regression
- [ ] Resume indexing with the transcript’s redaction context
- [x] Clear passages indexed before the fix
- [ ] Verify and review the delivery head

## Acceptance criteria

- **AC-1 (IDX-001)** Reopen a synthetic transcript at a saved split between a dotenv read and its result. The indexed passage shows the names and redaction markers. An ordinary tool result keeps its value.
- **AC-2 (IDX-001, IDX-002)** A resumed index produces the same passages as one uninterrupted batch across harness-specific call and continuation boundaries. An unfinished suffix can complete on a later refresh. If the saved offsets disagree with the file, a refresh accounts for the failed read and clears that index only while its offset is still current.
- **AC-3 (IDX-003)** On upgrade, affected documents lose old chunks and full-text matches, their offsets reset, and their transcript files remain available for indexing.

## Test plan

Use synthetic JSONL with a neutral assignment name whose value only dotenv-aware redaction removes. Reopen the file with just byte and line offsets, compare passages with an uninterrupted index, and exercise SQLite migration on synthetic stored passages. Run transcript and migration tests, quick checks, independent review and Yulius CI.

## Decisions

Replay the prefix with the existing transcript parser so restart needs only the existing offsets and future changes to the parser’s tracking apply to indexing as well. The session byte limit bounds prefix work. Keep historical cleanup in a one-time migration ([ADR-0382](../../docs/decisions/0382-session-index-resumption-replays-redaction-context.md)).

## Evidence

- On the original reader, `cargo test -p yulius-app --lib resumed_index_redacts_dotenv_results_across_a_restart` failed at the missing redaction marker for `SHOP_REGION`.
- In a synthetic SQLite FTS5 database, migration 0071 removed the sole match for the fixture value and reset the document’s byte offset, line number and full-index flag.
- The baseline SQLite restart integration test also failed at its missing snippet redaction marker. All 23 transcript unit tests passed with replay, including Claude, Codex and Antigravity fixtures.
- The SQLite schema-upgrade, migration-position and two session-store tests passed. A synthetic deletion with the FTS trigger disabled showed why the upgrade assertion queries FTS directly: the joined query returned zero while the direct FTS query found one stale match.
