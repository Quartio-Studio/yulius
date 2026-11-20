# 0407. Keep current PR observations across incomplete Git overviews

## Status

Accepted for SPEC-558.

## Context

Home and MCP use the Git overview to identify tasks whose pull request checks failed. Two readers can finish in a different order from the one in which they started. An owner may have access to an external PR through their GitHub token even when the installation cannot read the repository.

## Decision

Number each overview at the start of its read. Publish its per-task observations together under the cache lock. A complete read can establish absence. A failed or truncated list retains known open PRs unless it supplies an open PR or confirms closure of the same PR. For each task, keep the newest read that established a fact about it. The newest task listing determines which tasks remain in the cache.

## Alternatives

Replacing the entire cache on every response lets a partial list erase an observed failure. Dropping every response that started earlier loses evidence when the newer response omitted the task. Serializing the network requests makes all readers wait for the slowest request.

## Consequences

A partial overview can leave a task under Needs you until GitHub confirms its PR state. A complete overview can clear the failure. Updating the cache changes task classification and sends no prompt to the agent.
