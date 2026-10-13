---
yulius-task: 8c9bb5a5-cd5a-4d26-b87c-f8b5a081afb2
status: done
---

# SPEC-058: unique decision numbers with a repository check; onboarding CI-proof wording

Status: implemented
Date: 2026-10-13
Owner: Yulius task #49 (part of task #47)
Scope authorization: user request: "Fix GitHub issues #317 and #272 of Quartio-Studio/yulius,
following each issue's acceptance criteria".

## Goal

Every decision record has its own number, and the repository check stops two records from sharing
one again (#317). The onboarding prompt and the infra-bootstrap skill tell the agent everything it
needs to prove a CI job the way Yulius CI runs it: matrix values, recovering from an interrupted
proof, per-lane checkouts that share the CI home and cache, and system packages installed by
onboarding rather than by a job (#272).

## Context

- #317: four parallel tasks each took the next free number and merged within the hour, so
  `docs/decisions` has four `0034-*.md` records, and "ADR-0034" in the docs and specs names four
  different decisions. Nothing checks the numbers: `scripts/check_repository.py` checks links,
  skills, crates and UI contracts.
- #272: task #34 added the fresh-home proof to `ENVIRONMENT_SETUP`
  (`crates/yulius-app/src/onboarding.rs`) and infra-bootstrap step 2
  (`catalog/skills/yulius/infra-bootstrap/SKILL.md`). Its `env -i` drops `YULIUS_CI_MATRIX`, which
  `ci/exec.rs::ci_env` sets for matrix jobs, and a killed proof leaves `~/.ci-proof.*` and a stale
  worktree entry. Since #238 (CI lanes) a job's checkout is per lane (`<job>@<lane>`,
  `ci/exec.rs::checkout_name`) while up to `CiSettings::lanes` (2) runs of a sandbox share its CI
  `HOME` and cache; the `ci/exec.rs` module doc still says "each job keeps its own persistent
  checkout". Since #299 (#300: an `apt-get` in a job raced the other lane for the dpkg lock)
  system packages belong to the sandbox: CI jobs run inside its holder and see what it installed.
- SPEC-020 ONB-001 states what the prompt says, traced to
  `onboarding.rs::prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof`.

## Requirements

- ADR-001: each decision record has a unique number: the earliest merged of the four 0034 records
  (ready-made automations, #297) keeps 0034; the Agent in the right panel becomes 0036 (a free gap),
  the personal two-factor vault 0042 and the task status from Claude Code's records 0041. Each
  record's H1, the index in `docs/decisions/README.md` (in numeric order) and every reference in
  the docs, specs and code comments follow.
- ADR-002: `scripts/check_repository.py` fails when two files in `docs/decisions` share a number,
  naming them, when a record's H1 number differs from its file name's, and when the index does not
  list a record (0016 was missing).
- ADR-003: the decisions README says the check refuses a duplicate number, so the second task to
  merge takes the next free number after merging `dev`.
- ONB-101: `ENVIRONMENT_SETUP` and infra-bootstrap step 2 say to add `YULIUS_CI_MATRIX=<value>` to
  the proof once per matrix value.
- ONB-102: both say to run the proof with a long timeout or in the background, and how to recover
  from an interrupted one: the proof prints its directory (`echo "proof $d"`), the agent removes
  its own by that path, removes other tasks' `~/.ci-proof.*` only when started over six hours ago
  (`find "$HOME" -maxdepth 1 -name '.ci-proof.*' -mmin +360 ...`; the agent home is shared with
  their running proofs), and runs `git worktree prune`.
- ONB-103: both say a job's checkout is per lane while by default up to two runs at once share the
  CI `HOME` and cache, so a job's installs there must tolerate a concurrent run.
  `docs/operations.md` says the same of `ci/jobs/<job>@<lane>`, `home` and `cache`.
- ONB-104: both say system packages (`sudo apt-get`, a browser's system libraries) belong to the
  sandbox and are installed during onboarding, never in a CI job.
- ONB-105: the `ci/exec.rs` module doc describes per-lane checkouts; SPEC-020 ONB-001 states the
  new wording.

## Plan

- [x] Decision records have unique numbers and every reference follows them
- [x] The repository check refuses duplicate decision numbers
- [x] The onboarding prompt covers matrix values, interrupted proofs, shared CI home and system packages
- [x] The infra-bootstrap skill, the CI module doc and SPEC-020 match the prompt
- [x] Gates green and an independent review approves the head
- [x] Spec records its evidence and the pull request names both issues

## Acceptance criteria

- `ls docs/decisions` shows no number twice; `git grep 'ADR-0034'` names only the automations
  catalog; every moved record's links resolve (the link check passes).
- `test_check_repository.py` has a test that fails without the duplicate-number check, and
  `scripts/dev ci` passes with it.
- `prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof` asserts ONB-101..104 for both
  onboarding prompts and fails without the new wording.
- SPEC-020 ONB-001 and the infra-bootstrap skill say the same as the prompt.

## Test plan

- `python3 -m unittest scripts/test_check_repository.py` (via `scripts/dev ci`): a temporary
  decisions folder with two `0007-*.md` records and one H1 mismatch is refused; a clean one passes.
  Run before the check exists (must fail), then after.
- `cargo nextest run -p yulius-app -E 'test(prompts_describe_the_persistent_ci_home)'` with the new
  assertions before the prompt changes (must fail), then after.
- `python3 scripts/check_repository.py` on the renumbered tree (must pass), and on the tree before
  the renumbering (must fail with the four 0034 records).
- `scripts/dev ci` for every gate.

## Decisions

- **Which record keeps 0034:** the first merged (#297 at 04:47, ready-made automations), so the
  number keeps meaning what it meant first. The others take free numbers in merge order: 0036 (the
  gap left when SPEC-054 took 0037), then 0040 and 0041 after `dev`'s 0039; while this branch was
  open, #328 (task #48) merged its own 0040 first, the new check failed on the merge of `dev`, and
  the two-factor record took 0042; then #332 (task #65) merged a second 0040 onto `dev`, which this
  branch renumbers to 0043 (#328's, merged first, keeps 0040). The issue's "0035 to 0037" was
  written before 0035 and 0037 were taken.
- **Historical notes** in SPEC-035, SPEC-040 and SPEC-052 that tell how a number was chosen at the
  time stay as written; the ones that name the record get "now 00NN (#317)" so they are not
  ambiguous.
- **A check rather than a numbering service:** a duplicate cannot be prevented across parallel
  branches without coordination, but every task merges `dev` and runs the repository check before
  its merge, so the second one fails and renumbers. The H1 check catches a renumber that forgot its
  title. No ADR: the check is a repository contract, documented in the decisions README.
- **#272 wording only:** no CI behavior changes; the prompt grows by under 1 KB, far below
  `MAX_PROMPT_BYTES`.
- **Review findings** (round 1, APPROVE at 205fe9c with minors): the leftover cleanup could remove
  another task's running proof in the shared agent home, so it removes only proofs started over
  six hours ago (checked on a fake home: a 7-hour-old read-only proof is removed, a fresh one
  kept); `docs/operations.md` described one checkout per job; "two runs" is the default lane count
  (`YULIUS_CI_LANES`); the duplicate message named the first record in file order as the one to
  renumber. All fixed, test-first where tested. Not changed: the manual `git worktree prune` can in
  theory race a Yulius `worktree add` outside Yulius's in-process lock; the window is tiny, the
  issue asks for the prune, and Yulius prunes before its own checkouts anyway.
- **Parallel branches** that add a decision 0041 to 0043 or cite "ADR-0034" for the two-factor vault
  must renumber or retarget after merging `dev`; the check catches the file and link, the pull
  request says so for the prose.
- **Review round 2** (REQUEST CHANGES at 2172770): with only the six-hour `find`, the agent's own
  killed proof stayed behind, which is the case #272 asks to recover from. The proof now prints
  its directory and the recovery removes it by that path first; the six-hour `find` is kept for
  other tasks' leftovers. Also the changelog's lane wording and long lines. Fixed test-first.
- **Review round 3** (REQUEST CHANGES at 41e2038): every round-2 fix verified, including the
  rendered shell; but `dev` had gained #332's second 0040 (renumbered here to 0043), the review
  item was ticked before an approval, and SPEC-040 had long lines. All fixed; the index check
  (every record listed) answers its nit that 0016 was missing.
- **Not changed after round 4:** the Plan line of the prompt step is 104 columns because it must
  equal the Yulius step title.
- **The plan ends before the merge:** `delivery_merge` refuses while a step is open, so merging and
  closing #317 and #272 are the delivery after the plan, reported in the task summary. The review
  item is ticked only after a round approves, in a spec-only commit that a short confirmation
  review covers, since `delivery_merge` needs an approval of the exact head.

## Evidence

- Repository check, red then green: with the test and before `decision_errors` existed,
  `python3 -m unittest test_check_repository` failed (ImportError); with the check and the old tree
  it failed listing the three extra 0034 records; after the renumbering, 31 tests OK and
  `python3 scripts/check_repository.py` printed "... unique decision numbers ...".
- Prompt test, red then green: `cargo nextest run -p yulius-app -E
  'test(prompts_describe_the_persistent_ci_home)'` failed with `missing "YULIUS_CI_MATRIX=<value>"`
  before the prompt change, with `infra-bootstrap misses ...` before the skill change, and with
  `missing "find \"$HOME\" -maxdepth 1 -name '.ci-proof.*' -mmin +360"` before the review fix;
  passed after each.
- `scripts/dev ci`: exit 0 at 13f6f8c (1266 passed), at aa59cad (1268), at e634281 (1268) and at
  5da77ef, the head after round 4's nits and the merge of `dev` #330 (later commits change only
  this spec and `docs/changes.md`): nextest 1268 run, 1268 passed, 16 skipped; repository contracts and 39 Python tests
  OK; cargo-deny advisories, bans, licenses and sources ok.
- Merging `dev` after #328: `python3 scripts/check_repository.py` failed with
  "0040-personal-two-factor-guards-the-vault.md: shares decision number 0040 with
  0040-done-tasks-release-their-subscription.md"; after renumbering to 0042 it passed.
- Merging `dev` after #332: `python3 scripts/check_repository.py` failed on #328's and #332's two
  0040 records; after #332's became 0043 it passed. The index check, red then green: the new test
  failed (`0 != 1`) before the check, then the real tree failed on the unlisted 0016, then passed.
- `git grep 0034` in docs, specs and code: only the automations catalog record, its references
  and historical notes.
- Independent review (fresh read-only sessions, skill `om-code-review`): round 1 of
  81bf3aa..205fe9c APPROVE with minors; round 2 of 2172770 REQUEST CHANGES (own-proof recovery);
  round 3 of 41e2038 REQUEST CHANGES (#332's second 0040 on `dev`); round 4 of e634281 APPROVE with
  one minor and nits (the changelog and README name all three rules; the index test split out; a
  missing index reported, not raised), fixed in f6ca9d4. A confirmation review covers the final
  head's delta from e634281: APPROVE at 7006140, with one minor: the union merge driver of
  `docs/changes.md` had cut #332's "Quick archive" entry in two, keeping the old ADR-0040 in the
  copy. The cut line is removed and the entry says 0043; a last confirmation covers that fix.
- The rendered prompt's commands, run in a scratch repository with a scratch `HOME`: the proof
  killed by `timeout 3` printed `proof <dir>` and left the directory and a worktree entry; the
  own-path recovery plus `git worktree prune` removed both (exit 0); the six-hour `find` removed a
  7-hour-old read-only proof and kept a fresh one.
- Prompt test red for round 2: `missing "echo \"proof $d\";"` before the change; green after.
