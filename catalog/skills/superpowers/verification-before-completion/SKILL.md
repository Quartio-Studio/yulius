---
name: verification-before-completion
description: Evidence before claims - run the command that proves a claim and read its output before saying work is complete, fixed or passing, before committing, opening or merging a pull request, or marking a step done. Use whenever you are about to report success.
---

# Verification Before Completion

> Adapted from obra/superpowers (MIT, © Jesse Vincent), rephrased for autonomous agents; thanks to the superpowers authors for this contribution.

**Core principle:** evidence before claims, always. Nobody checks your work after you: in an
autonomous pipeline your claim is what the next step, the reviewer and the merge rely on.

## The iron law

```
NO COMPLETION CLAIMS WITHOUT FRESH VERIFICATION EVIDENCE
```

If you have not run the verification command in this turn, you cannot claim it passes.

## The gate

Before claiming any status, marking a step done, committing, pushing, or reporting:

1. **Identify** the command that proves the claim.
2. **Run** it, fully and freshly.
3. **Read** the whole output: exit code, failure and skip counts.
4. **Verify** that the output confirms the claim. If not, state the actual status with the
   evidence. If so, state the claim with the evidence.
5. Only then make the claim.

Skipping a step is not verifying.

## What each claim needs

| Claim | Requires | Not sufficient |
|---|---|---|
| Tests pass | The test command's output: 0 failures | An earlier run, "should pass" |
| Lint is clean | The linter's output: 0 errors | A partial check, extrapolation |
| Build succeeds | The build command: exit 0 | Lint passing, logs that look fine |
| Bug fixed | The original symptom's test now passes | Code changed, assumed fixed |
| Regression test works | Red-green verified: fails without the fix, passes with it | Passes once |
| Subagent finished | The diff shows the changes, and you checked them | The subagent said "success" |
| Requirements met | Each acceptance criterion checked one by one | Tests passing |
| Step done | Its slice works and its tests pass now | "Mostly done" |
| Ready to merge | Every gate green on the exact head, review findings answered | Green on an older commit |

## Red flags: stop

- "Should", "probably", "seems to".
- Satisfaction before verification ("Done!", "Perfect!").
- About to commit, push, open a pull request or mark a step done without running the gates.
- Trusting a subagent's or tool's success report.
- Relying on partial verification, or "just this once".

| Excuse | Reality |
|---|---|
| "Should work now" | Run the verification. |
| "I'm confident" | Confidence is not evidence. |
| "The linter passed" | The linter is not the compiler or the tests. |
| "The agent said success" | Verify independently. |
| "A partial check is enough" | Partial proves nothing about the rest. |

## Patterns

- Tests: run the command, see "34/34 passed", then say "all tests pass".
- Regression tests: write, run (pass), revert the fix, run (must fail), restore, run (pass).
- Requirements: reread the spec, check each acceptance criterion, report gaps or completion.
- Delegation: a subagent reports success, you read the diff and rerun the checks, then report
  the actual state.

## When to apply

Always before any claim of success or completion, any positive statement about the state of the
work, committing, pushing, opening or merging a pull request, marking a step done, moving to the
next step, and delegating. The rule covers paraphrases and implications, not just exact words.
