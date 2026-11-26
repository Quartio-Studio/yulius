---
yulius-task: 1fbe203d-b576-4287-aa8e-b4c183b45e46
status: in-progress
---

# SPEC-667. Cleanup of delivery, the merge queue and the Git/GitHub adapters (audit L2)

## Goal

Less code that does the same thing in the delivery lane, in a shape a person can read: the
owner's November 2026 cleanup, audit report L2 (`L2-delivery.md`), plus the spec-discovery bug
that audit L10 found (F10).

## Context

The cleanup charter asks a chore for no spec. Yulius's edit gate holds code changes of this task
until it has one, so this spec stays lean: it records the plan and the evidence, and the audit
report is the design. Behaviour stays the same in every pull request, except the bug fix of
PR 0.

- `crates/yulius-app/src/delivery.rs` (8.2k lines) holds ten responsibilities (F8).
- The queue's two drivers (`agent_wait.rs`, `merge_train.rs`) and `pr_watch.rs` copy the
  readiness rule, the bring-up-to-date step and the armed-slot reads (F2, F5).
- "Merge now or arm" and "does the PR conflict with its base" are decided twice or three times
  (F3, F4).
- Queue state lives in three JSON files beside the store (F1).
- Spec discovery reads the first 500 directory entries before it sorts them; `.ai/specs` has
  more than 600 (L10 F10).

## Requirements

- **CLN-001** Spec discovery finds a task's spec in a specs directory of any size up to the
  listing bound, and prefers the file named with the task's branch slug.
- **CLN-002** Each later pull request keeps behaviour unchanged; a move is verbatim and in its
  own commit.
- **CLN-003** Nothing outside the task's claimed paths changes; `supervisor.rs` is untouched.
- **CLN-004** F7 (the parked-PR display reads) is not done: it drops a capability.

## Plan

- [x] Spec discovery: failing test for >500 specs
- [x] Spec discovery: sort before the cut, prefer the slug file
- [x] Spec discovery: independent review answered
- [x] Split delivery.rs: verbatim move into 8 files
- [x] Split delivery.rs: independent review answered
- [x] Queue: queue_readiness and armed_snapshot
- [x] Queue: bring_armed_up_to_date for both drivers
- [x] Queue: independent review answered
- [x] Merge: preview_merge vs adds_to agreement test
- [x] Merge: conflict checks compared; F3 and F4 reported
- [ ] Merge: independent review answered
- [ ] Queue leftovers: typed dissolve reason, is_merged, branch filter, typed queue
- [ ] Leftovers/finish: TaskCache kind and background probe
- [ ] F6/F9/F10: independent review answered
- [ ] Adapters: branch check, test-only fns, hand-rolled code, Git builder repeats
- [ ] Adapters: port types (F11) within paths
- [ ] Adapters: independent review answered
- [ ] Queue state: armed merges, trains, notices into the store
- [ ] Queue state: independent review answered

## Acceptance criteria

- A task whose spec is one of 700 files, listed after 500 others, is found (CLN-001).
- Of two files naming the task, the one with the branch slug is registered (CLN-001).
- Every touched test binary passes unchanged apart from the tests of deleted code (CLN-002).

## Test plan

```sh
scripts/cargo nextest run -p yulius --test agent_hooks
scripts/cargo nextest run -p yulius --test delivery
```

## Decisions

- Discovery lists every name up to a bound far above any real specs directory and reads the
  slug-named file first, instead of sorting 500 entries: a sorted cut at 500 would still drop
  `SPEC-667` among 700 numbered specs.
- The bound counts kept Markdown files and is 2,000: a discovery that finds nothing reads every
  file, so the bound limits that worst case while staying far above the 600 specs of today. The
  Plan item "sort before the cut" was done by raising the bound and ordering the slug file first.

## Evidence

PR 0 (#1774), spec discovery (CLN-001):
- `agent_hooks::a_spec_among_hundreds_is_found_and_the_slug_named_one_wins` failed on the code
  before the fix ("the spec is found", `/tmp` is tmpfs, which lists the newest entry first) and
  passes after it. After the review the task's two specs are written before and after the 700
  others, so one lies past the first 500 in either listing order.
- Hand mutations: plain `names.sort()` registers `0-old-copy.md` instead of `SPEC-900-x.md`; the
  bound back at 500 with the slug order kept, and the whole pre-fix `delivery.rs`, both fail at
  the renamed-spec assertion (`agent_hooks.rs:1510`).
- `scripts/cargo nextest run -p yulius --test main`: 2829 passed; `-p yulius-app`: 740 passed;
  `scripts/dev quick` and `clippy -p yulius-app --all-targets -D warnings`: clean.
- Independent review (Fable 5.2) approved `3b589768e`; its minors and nits are answered in the
  next commit (test order, `docs/api.md`, bound and wording, sort without clones, no audit label).

PR 1, the split of `delivery.rs` (audit L2 F8, CLN-002):
- Commit `5845fc850` moves eight regions verbatim into `delivery/{profile,text,view,open,parking,
  merge,promote,environments}.rs`, each inside its own `impl App` block; `switch_tests` moves with
  the profile code it tests. After review, `8245ce643` moves three helpers verbatim to their
  callers' files. `delivery.rs` goes from 8,236 to 1,788 lines (about 590 of them its tests).
- A line-multiset comparison of the old file against the parent and the eight files finds no line
  lost; the only lines added are the module declarations, three glob re-exports, the file
  headers, `use super::*;`, the `impl App {`/`}` wrappers and 23 `pub(super)` tokens on items
  reached from another file (one of them the test-read field `FollowupDiscovery::open`).
- Review round 1 found four `.config/vidoc.toml` `[[ignore]]` entries still naming `delivery.rs`;
  they now name `delivery/environments.rs`, where the accepted code (SPEC-219's finding in
  `dlv_environment_steps`) lives. The scanner's own `apply_ignores` ignores the four wordings
  there and no longer at `delivery.rs`.
- `scripts/cargo nextest run -p yulius-app`: 740 passed; `-p yulius --test main`: 2829 passed;
  `clippy --workspace --all-targets -D warnings`, `RUSTDOCFLAGS=-D warnings cargo doc -p
  yulius-app`, `python3 scripts/test_vidoc_scan.py` (92) and `scripts/dev quick`: clean.
- Independent review (Fable 5.2): round 1 requested changes (the Vidoc paths, three placements);
  round 2 approved `6ca422c9c`; its last notes (drop the `profile.rs` entries, this Evidence) are
  in the next commit.

PR 2, the queue's readiness rule and armed slot (audit L2 F2 + F5, CLN-002):
- `queue_readiness` (Working, Shells, NotRunning, then the worktree's MovedOn, Dirty and MovedOn
  at the head; cheapest first) replaces
  the copies in the armed-merge pass, `keep_armed_current`, `candidate_wait_note`, the train's
  member step and candidate pick (`train_ready`), and `pr_watch`'s `armed_wait_reason`;
  `agent_idle` replaces the two copies in parking. `bring_armed_up_to_date` (merge plus span)
  and `audit_branch_updated` are the update step the two drivers copied; each driver keeps its
  own push order. `armed_snapshot` reads the arm, decisions and note under one lock for
  `pr_watch`'s race guards and replaces `arm_observation` and `arm_queue_note`.
- One wording change, as the audit's F2 decided: `armed_wait_reason` read a failed
  uncommitted-changes check as clean, the rest of the queue as dirty; both now say dirty.
- Product lines +226 / −221: the copies were short inline expressions, so the gain is one rule
  instead of eight, not size.
- `scripts/cargo nextest run -p yulius-app`: 740 passed; `-p yulius --test main`: 2830 passed;
  clippy `-p yulius-app -p yulius --all-targets -D warnings`: clean. Hand mutations: without the
  background-shell check in `queue_readiness`, two `antigravity_armed_merge` tests fail; with the
  old clean default for a failed read, the new test
  `stalled_armed_green_pr_with_an_unreadable_worktree_says_uncommitted` fails (generic sentence).
- Independent review (Fable 5.2) approved `6ccb58f7a`; its notes are answered in the next commit:
  NotRunning before any Git read (no new `git status` for a task held for capacity), the
  `MovedOn` doc, no unused derives on `ArmedSnapshot`, the new test, and the stalled-PR test now
  pins its "turn as working" sentence. `crates/yulius/tests/antigravity_armed_merge.rs` is
  claimed beside task #657's `crates/yulius/tests/**`, for these two additions only.

PR 3, the conflict checks compared (audit L2 F4's precondition; F3 and F4 not done):
- `delivery::preview_merge_and_adds_to_agree_on_conflicts` (real Git, the repository's merge
  drivers): `merge_base` + `adds_to` and `preview_merge` agree in every shape with one merge base
  (apart, conflicting, union-driver, up to date, behind, the same change); an unrelated head has
  no merge base for either.
- `delivery::adds_to_and_preview_merge_differ_with_several_merge_bases`: with two merge bases
  (each side merged the other) they disagree. `preview_merge` answers as `git merge` does;
  `adds_to` from `merge_base`'s one base gives the opposite (found by the independent review).
- So switching `local_conflict` to `preview_merge` changes its answer for criss-cross pull
  requests (reachable where merge commits land both ways): a behaviour change, which the cleanup
  charter leaves to the owner. The switch was made, reviewed and reverted; only the two tests
  remain.
- Not done, outside the task's paths: F3 (`merge_or_arm`) has to fold `merges.rs`'s
  `merge_for_person` into `agent_delivery.rs`'s `delivery_merge_answer`; `merges.rs` is not
  claimed by this task. F4's `BaseRelation` cache would change `TaskRuntime.pr_conflict`'s type in
  `supervisor.rs` (task #650's), and `adds_to` stays for parking's other question.
