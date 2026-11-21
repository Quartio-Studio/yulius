# 0448. Repository file readers run before merge

## Status

Accepted.

## Context

A merge train accepted a composer edit whose old text a Rust test still required. The first failing result arrived on dev.

## Decision

Run repository text contracts in path-filtered CI on the pull request and train. Derive coverage from the files the tests read. Run shared readers together. The task briefing asks for the entire touched test binary during local verification.

## Alternatives

Running both shards on each train repeats the full suite. Executing tests in delivery_merge duplicates CI checkout, cancellation and result handling.

## Consequences

A stale file assertion blocks the candidate. Focused jobs still pay for compiling and linking the selected targets on a cold checkout. Dev runs the complete suite before staging.
