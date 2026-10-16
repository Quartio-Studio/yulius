# 0161. A base a live holder mounts is never collected

## Status

Accepted. Specified in [SPEC-202](../../.ai/specs/SPEC-202-fix-617-and-618-the-root-smoke-test-runs.md);
task #187, fixes #618. Amends the base collection of SPEC-002 ADR-22 §3.

## Context

A sandbox's system layer is an overlay whose lower layer is a base, `<data>/system/base-<n>/`.
After a build, `base::collect` removed every base that was neither the new current one nor named
by `yulius sandbox-base build --keep`. Only the server filled `--keep`, from the holders it held
in memory at its upkeep tick. `docs/operations.md` tells operators to run the build by hand,
without `--keep`. On the reference VM (2026-10-16) such a build removed `base-3` while every
running holder had it mounted: `/usr/lib` went empty in the sandboxes, `python3` failed, and only
killing and restarting `yulius.service` recovered them, ending every session.

## Decision

`base::collect` reads the holder records itself (`<data>/runtime/sandboxes/*/host.json`, at most
10,000) and keeps the base of every holder that `holder::alive` judges live (same boot, same
user, network and mount namespaces), as the server's adoption does. `--keep` still adds bases;
the current base is always kept. A dead holder's record keeps nothing, and every other base is
still removed.

## Alternatives

- **The CLI reads the records, `collect` stays as it was**: fixes the hand build only. The
  server's build would keep its own window, a holder started between its tick and the build's
  end, and any later caller would have to remember the rule.
- **A hand-run build never collects; only the server does**: leaves old bases (about 6 GB each)
  on disk until the server's next build, and the server's `--keep` still misses late holders.
- **Refuse a hand build while the service runs**: the documented first install builds before
  the service starts, and an operator rebuilding after a host change would need a restart, which
  ends every session.

## Consequences

- A hand build is safe while sandboxes run; a sandbox moves to the new base when its holder
  restarts, and the next build after that removes the old one.
- Judging a holder live needs its `/proc/<pid>/ns/*`, readable by the service user (as
  documented) and root. Another user cannot judge them, but cannot remove the service user's
  bases either.
- A holder that starts between `collect`'s read and its removal is still not seen. The server
  starts holders on the current base, which `collect` never removes, so only a holder that read
  the previous `current.json` just before the build switched it is exposed.
