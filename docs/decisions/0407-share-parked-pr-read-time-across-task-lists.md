# 0407. Share parked PR read time across task lists

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

Home and MCP can list tasks before anyone opens the sandbox Git overview. A recorded merge can precede a replacement PR on the same branch. Checking the head and its CI can take longer than rendering the rest of the list.

## Decision

Use the same recorded and branch PR verification as parked dependencies. Give a list 750 ms for PR reads. Check up to eight rows concurrently, in order, and reserve a share of the remaining time for each later batch. Limit local Git operations from status reads to eight workers per application. A dispatched worker owns its permit until Git returns. Keep incomplete evidence under Needs you. Status reads do not send prompts or release a dependency.

An owner can see an external PR through their GitHub token even when the instance lacks a working installation. A failed or truncated overview retains that known open PR. A complete list can establish absence, and a current branch or recorded-PR read can confirm its status.

## Alternatives

Reading rows sequentially lets a single stalled PR use the entire deadline. Starting all rows together lets concurrent list requests exhaust the host. An overview-only lookup can miss an externally opened replacement and loses its observations on restart.

## Consequences

A completed row can show Done even when an earlier PR cannot be read. A timeout leaves its row awaiting verification on the next read. A slow local Git process can retain a worker after the requesting client disconnects.
