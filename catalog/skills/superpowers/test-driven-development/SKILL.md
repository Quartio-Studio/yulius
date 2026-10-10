---
name: test-driven-development
description: Write the test first, watch it fail for the right reason, then write the minimal code to pass and refactor. Use when implementing any feature, bug fix or behavior change, before writing production code.
---

# Test-Driven Development

> Adapted from obra/superpowers (MIT, © Jesse Vincent), rephrased for autonomous agents; thanks to the superpowers authors for this contribution.

Write the test first. Watch it fail. Write the minimal code to pass.

**Core principle:** if you did not watch the test fail, you do not know that it tests the right
thing. A change that comes with a test failing without it is the evidence the pipeline asks for.

## When to use

Always for new features, bug fixes, refactoring with behavior risk, and behavior changes.
Exceptions (throwaway prototypes, generated code, pure configuration) are your call: record the
decision and why in the spec's Decisions or the pull request body. Never for a bug fix: a fix
without a regression test is not done.

## The iron law

```
NO PRODUCTION CODE WITHOUT A FAILING TEST FIRST
```

Wrote code before the test? Delete it and start over from the test. Do not keep it as
"reference" or adapt it while writing tests: that is testing after.

## Red, green, refactor

1. **Red: write one failing test.** One behavior, a clear name, real code (mocks only when the
   real dependency is slow or external). Name the production change that would make it fail
   before you write it (`writing-good-tests.md`).
2. **Verify red (mandatory).** Run it. It must fail, not error, with the expected message,
   because the feature is missing (not because of a typo). Passes immediately? You are testing
   existing behavior: fix the test. Errors? Fix the error and rerun until it fails correctly.
3. **Green: minimal code.** The simplest code that passes. No extra options, no refactoring of
   other code, no "improvements" beyond the test (YAGNI).
4. **Verify green (mandatory).** The test passes, the other tests still pass, the output is
   clean. "Other tests" means the project's whole suite (its test command), not your file: a
   failure you did not cause is still reported by name. Test fails? Fix the code, not the test.
5. **Refactor** only when green: remove duplication, improve names, extract helpers; keep tests
   green and add no behavior.
6. **Repeat** for the next behavior.

## Bug fixes

Reproduce the bug with a test, watch it fail with the reported symptom, fix, watch it pass. For
the record, the regression test can be its own commit before the fix (`test: ...`), so anyone can
check out that commit and see it fail.

## Tests that stay reliable

Flaky tests destroy the signal every gate depends on. Follow `condition-based-waiting.md`: wait on
conditions, never on sleeps; inject clocks; seed or log randomness; give each test its own data;
sort before comparing collections; and never retry a flaky test until green: reproduce it, fix it
or quarantine it with an issue.

## Red flags: stop and start over

Code before the test; a test added "later"; a test that passed immediately; not being able to
explain why it failed; "I already tested it manually"; "keep it as reference"; "it's about the
spirit, not the ritual"; "this case is different". Each means: delete the code, start with TDD.

| Excuse | Reality |
|---|---|
| "Too simple to test" | Simple code breaks; the test takes a minute. |
| "I'll test after" | Tests written after pass immediately and prove nothing. |
| "Manual testing is enough" | No record, no rerun, missed edge cases. |
| "Deleting hours of work is wasteful" | Sunk cost; code you cannot trust is the waste. |
| "Hard to test" | Hard to test means hard to use: simplify the design. |
| "Existing code has no tests" | You are improving it: add tests for what you touch. |

## When stuck

| Problem | Solution |
|---|---|
| Don't know how to test it | Write the wished-for API; write the assertion first. |
| Test too complicated | The design is too complicated; simplify the interface. |
| Must mock everything | Too coupled; inject dependencies. |
| Huge setup | Extract helpers; still huge, simplify the design. |

## Checklist before calling it done

- Every new function or behavior has a test, and you watched each fail for the expected reason.
- Minimal code made each pass; the whole suite passes with clean output.
- Edge cases and error paths are covered; mocks only where unavoidable.

Then apply `verification-before-completion` before claiming anything.
