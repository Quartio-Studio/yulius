# 0385. Recover an interrupted initial mock load once

## Status

Accepted for the e2e harness. Extends [ADR-0262](0262-an-e2e-tests-first-look-at-a-page-has-a-first-load-budget.md). [SPEC-545](../../.ai/specs/SPEC-545-recover-the-browser-test-s-initial-load.md)

## Context

The browser and static server use loopback. When another process changes an interface, Chromium can abort the page's in-flight module requests with `net::ERR_NETWORK_CHANGED`. Waiting longer cannot install a module that never arrived.

## Decision

Watch the first mock `app.open` for that exact failed-request code on the main frame's app document or same-origin scripts under `/assets/`. If the document failed, open the requested URL again. If a script failed and the mock is missing, reload the document. Report the recovery in the test output and wait for mock readiness as usual. Allow one recovery per test.

## Alternatives

A blanket test retry would repeat the test's actions and could hide product failures. Isolating the entire suite in a separate network namespace would add a platform-specific dependency to the normal browser test command. The pinned runner exposes the request failure directly.

## Consequences

An interrupted initial mock load costs one more navigation. A second interruption fails the test. The existing budgets apply to mock installation and to later actions.
