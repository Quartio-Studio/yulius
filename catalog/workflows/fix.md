---
name: Fix
description: A bug fixed from a reproduction, with a regression test.
when_to_use: Something that used to work, or should work, behaves wrongly.
spec: optional
tests: required
order: 20
---

## Steps

1. Reproduce with a failing test
2. Fix the cause
3. Review the change
4. Deliver

## Instructions

Reproduce the bug first, with a test that fails because of it. Fix the cause rather than the
symptom, keep the change small, and make sure the new test passes with the rest of the suite.
