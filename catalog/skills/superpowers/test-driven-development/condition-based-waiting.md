# Reliable tests: condition-based waiting and the flaky-test rules

Adapted from obra/superpowers (`systematic-debugging/condition-based-waiting.md`, MIT, © Jesse
Vincent), extended with test-reliability rules for autonomous pipelines.

## Wait for the condition, not for time

Flaky tests often guess at timing with arbitrary delays. They pass on a fast machine and fail
under load or in CI. Wait for the condition you care about instead:

```typescript
// Before: guessing at timing
await new Promise(r => setTimeout(r, 50));
expect(getResult()).toBeDefined();

// After: waiting for the condition
await waitFor(() => getResult() !== undefined, 'the result');
expect(getResult()).toBeDefined();
```

| Scenario | Pattern |
|---|---|
| Event | `waitFor(() => events.find(e => e.type === 'DONE'))` |
| State | `waitFor(() => machine.state === 'ready')` |
| Count | `waitFor(() => items.length >= 5)` |
| File | `waitFor(() => fs.existsSync(path))` |

A polling helper polls every ~10 ms, calls the getter inside the loop (no stale data), and always
has a timeout with a clear error naming what it waited for. Give it a generous budget (seconds,
not milliseconds): a deadline is a failure detector, not a speed test.

An arbitrary delay is correct only when testing timing itself (debounce, throttle): first wait
for the triggering condition, base the delay on known timing, and comment why.

## Rules for tests that stay reliable

1. Never assert after a bare sleep; wait on a signal or condition with a bounded timeout.
2. Use an injected or fake clock instead of the system clock for time-dependent logic.
3. Seed randomness, or log the seed, so a failure can be replayed.
4. Each test owns its data (unique keys, its own temporary directory or database rows); no test
   depends on another's leftovers or on execution order.
5. Sort before comparing collections whose order is not part of the contract.
6. Locks, transactions and migrations are tested against the real database engine, not a
   stand-in with different semantics.
7. **No retries.** A flaky test is reproduced (run it repeatedly, under load, in parallel),
   then fixed, or quarantined with an issue that carries the evidence. Rerunning until green
   hides the defect; it needs a recorded decision with the cause, never a habit.
8. Show that the test fails without the change (`test-driven-development`).
9. A skipped test is not a passing test; report skips by name.
10. A changed snapshot or golden file is reviewed line by line, not regenerated blindly.
11. A change to a wire format updates the golden fixture and both sides of the contract.
