# 0113. GitHub's branch says whether a head is pushed

## Status

Accepted. Specified in [SPEC-139](../../.ai/specs/SPEC-139-fix-366-448-yulius-wait-and-delivery-mer.md);
task #123, fixing #366 and #448. Refines SPEC-050 DLV-201 and SPEC-116 DMG-001.

## Context

Yulius keeps one note per task about why its head is not pushed or not delivered (DLV-201,
DLV-204), and `yulius wait` ends on it. The note was written from the exit code of Yulius's last
`git push` and cleared only by a later push of Yulius's own or by an end of turn that saw the pull
request at the head. In #366 GitHub reported `remote rejected … (failed)`, the agent pushed the head
itself, CI ran there, and every wait still ended at once on the old note. In #448 `delivery_merge`
answered an unrelated failure with an older "step X is in_progress" note, because it took whatever
note matched the head.

## Decision

- **A head is pushed when GitHub's branch is at it.** One helper makes every push of a task's head.
  When `git push` fails it asks the remote with `git ls-remote` (same transport and token as the
  push), and a branch at the head counts as pushed. A push that is done clears the note about that
  head.
- **The wait reconciles a failed-push note once per request.** When the note is about the wait's
  head, the wait endpoint asks the remote where the branch is; at the head, the note is cleared and
  the wait goes on. Remote unreadable: the note stands, as before. Notes that are no push failure
  (not delivered; the pull request did not open after its push) are never cleared this way.
- **The push is told apart from what follows it.** The end of a turn pushes, then opens the pull
  request; a failure to open it after the push landed is its own note, not a push failure.
- **A delivery failure carries its own note.** `try_auto_open_pr` fails with the error and the
  note it wrote (a refused push's remedy, or that the pull request did not open). `delivery_merge` answers with that note or with the error, never with a
  note it finds in the slot.

## Alternatives

- **Trust the worktree's `origin/<branch>` ref**: free, and right for an agent's own `git push`
  from the worktree, but blind to a push from elsewhere and to a push GitHub took while it reported
  a failure. Rejected as the truth; the remote's answer costs one call and only while a note exists.
- **GitHub's REST API (`branch_sha`)**: the port has it, but the production adapter does not
  implement it (only the test fake does), and CD and promotion call it too; implementing it is
  a change of its own.
- **Clear the note before each delivery and use it only if set again**: fixes #448 too, but loses a
  still-true note when the call fails before rewriting it. Rejected for a typed failure.
- **Check the remote in the CLI**: it has no GitHub token, and the note lives in the server.

## Consequences

- A stale push note costs at most one `git ls-remote` per `yulius wait` request and then disappears.
- Every push site (end of turn, `delivery_merge`, Deliver…, armed-merge upkeep, merge trains) shares
  the rule, so a new push site uses the helper.
- `delivery_merge`'s answer to a push GitHub refuses keeps its remedy (DMG-001).
