## Outcome

Describe the concrete trigger and changed behavior. Link the spec and requirement IDs.

## Evidence

Commands actually run, observed results, and relevant UI evidence. Name unrun checks.

## Review notes

Changed contracts, migrations, failure/recovery behavior and material limitations.
Delete sections that do not apply; do not claim acceptance based only on a green build.

## Delivery

- Change PR: typed branch → dev, squashed by Yulius (`delivery_merge`) on green Yulius CI after
  an independent review in a fresh session approved the exact head.
- Promotion PR: dev → main, merge commit, by a person on the CD page; the exact dev head has a
  successful `yulius/ci` status.
- Record the `yulius/ci` status of the resulting dev commit before claiming completion.
- Delete merged short-lived branches; keep dev/main.

Include the applicable evidence and remove instructions that do not apply.
