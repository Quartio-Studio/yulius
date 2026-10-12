## Outcome

Describe the concrete trigger and changed behavior. Link the spec and requirement IDs.

## Evidence

Commands actually run, observed results, and relevant UI evidence. Name unrun checks.

## Review notes

Changed contracts, migrations, failure/recovery behavior and material limitations.
Delete sections that do not apply; do not claim acceptance based only on a green build.

## Delivery

- Change PR: typed branch → dev, squash merge by its author after checks pass.
- Promotion PR: dev → main, merge commit; link the successful `yulius/ci` status of the exact dev head.
- Record the `yulius/ci` status of the resulting dev commit before claiming completion.
- Delete merged short-lived branches; keep dev/main. No reviewer approval required.

Include the applicable evidence and remove instructions that do not apply.
