# 0007. GitHub App as the authority for repositories and permissions

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-08); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Access must be narrow and revocable, and GitHub stays the source of permissions.

## Decision

A GitHub App created by manifest flow provides installation tokens and user-to-server OAuth; permissions are read per user per repository with a 5-minute cache. Since 2026-10-11 a task acts on GitHub with its creator's user-to-server token (pushes, pull requests, merges, promotions, issues, the agent's Git and `gh`); the installation token is the fallback and serves instance housekeeping (CI statuses, branch cleanup, background reads). Since 2026-10-15 the agent's Git and `gh` hold a narrowed installation token instead ([ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md)).

## Alternatives

An OAuth App with `repo` scope; personal access tokens.

## Consequences

Needs a public callback URL. Reads the App lacks permission for degrade instead of failing. GitHub shows the person who created a task as the author of its work; an agent holds its creator's reach within the installation until the token expires (SPEC-002 §8, R-08). See [operations](../operations.md).
