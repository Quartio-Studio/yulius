# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-11

- Repository prepared for Yulius: `yulius.toml`, product summary, architecture, decision
  records, changes log; GitHub Actions jobs translated into Yulius CI
  (the workflows stay until the GitHub App may post commit statuses and change workflow files).
- Tasks wait for preparation instead of being refused (#175); onboarding survives multi-byte
  text and titles are the first real prompt (#173); Secrets adds from its header and popups
  close on an outside click (#171).
