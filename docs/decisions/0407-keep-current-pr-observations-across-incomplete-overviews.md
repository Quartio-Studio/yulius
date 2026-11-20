# 0407. Keep current PR observations across incomplete Git overviews

## Status

Accepted for SPEC-558.

## Context

Home and MCP use the Git overview to identify tasks whose pull request checks failed. Two readers can finish in a different order from the one in which they started. An owner may have access to an external PR through their GitHub token even when the installation cannot read the repository.

## Decision

Number each overview at the start of its read. Publish its per-task observations together under the cache lock. A complete read can establish absence. A failed or truncated list retains known open PRs unless it supplies an open PR or confirms closure of the same PR. Apply completed responses in request-start order. A response that finishes early is visible immediately. Once earlier requests finish or are cancelled, fold completed responses into the committed cache and release their slots. Each sandbox has a bounded publication window of 30 overview requests. Home and MCP read the cache without acquiring a slot. The newest task listing determines which tasks remain in the cache.

## Alternatives

Replacing the entire cache on every response lets a partial list erase an observed failure. Dropping every response that started earlier loses evidence when the newer response omitted the task. Serializing the network requests makes all readers wait for the slowest request.

## Consequences

A partial overview can leave a task under Needs you until GitHub confirms its PR state. A complete overview can clear the failure. Updating the cache changes task classification and sends no prompt to the agent.
