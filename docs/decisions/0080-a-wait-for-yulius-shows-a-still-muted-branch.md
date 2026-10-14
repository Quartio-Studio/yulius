# 0080. A wait for Yulius shows a still, muted branch

## Status

Accepted. Amends [0068](0068-needs-you-means-a-person-has-to-act.md) point 2 ("with Working's
tone"). Specified in [SPEC-100](../../.ai/specs/SPEC-100-a-werid-bug-happened-git-icon-started-ro.md);
issue #393.

## Context

SPEC-085 STT-002 shows an idle agent that waits in `yulius wait` as "Waiting for checks or merge"
under Working, with the branch icon and Working's tone so that nobody is notified. Working's tone
is also a look: `styles.css` colours it green and turns every icon in it, a rule written for the
Working ring. The branch therefore spun in green, which a person reported as a bug (task #85).

## Decision

The state keeps its branch icon, its label and the Working group, and takes the `queued` tone:
muted and still, like Queued, Chained and Paused, each a wait with its own shape (#147). `queued`
is not a tone that notifies, so nobody is told when a task starts waiting, as before.

## Alternatives

- **Keep `working` and exclude the branch from the spin.** The icon would stand still but stay
  green, the colour of Working and of Done, so the row would still read as busy or finished.
- **A new tone for waits on Yulius.** More CSS for the look `queued` already gives.
- **Show the Working ring.** It hides that the agent waits for its checks or merge, which
  STT-002 shows on purpose.

## Consequences

- A task waiting for its CI or merge reads muted and still in the sidebar, on Tasks and in the
  task header. Failing checks while the agent waits (STT-003) read the same way, with the pull
  request's number in the label.
- The `tone-working` spin stays meant for the Working ring. A future state that pairs another icon
  with Working's tone turns that icon too.
