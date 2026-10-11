# 0007. GitHub App as the authority for repositories and permissions

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-08); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Access must be narrow and revocable, and GitHub stays the source of permissions.

## Decision

A GitHub App created by manifest flow provides installation tokens and user-to-server OAuth; permissions are read per user per repository with a 5-minute cache.

## Alternatives

An OAuth App with `repo` scope; personal access tokens.

## Consequences

Needs a public callback URL. Reads the App lacks permission for degrade instead of failing. See [operations](../operations.md).
