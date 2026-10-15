---
yulius-task: f794fb5e-c905-47da-89ac-411b6f9224ab
status: done
---

# SPEC-176: moving a playground into a GitHub repository

Status: done
Date: 2026-10-15
Owner: Yulius task #132 (part of task #118, SPEC-138)
Scope authorization: issue [#410](https://github.com/Quartio-Studio/yulius/issues/410), a
follow-up of [SPEC-099](SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md) (playgrounds): "An
admin of the playground picks "Move to GitHub": Yulius creates (or uses an empty) repository of the
instance owner, pushes the playground's branches, switches the sandbox's `origin` to it, sets
`repo_owner`, and from then on access mirrors GitHub. Tasks and their history stay." The task adds:
decide in an ADR whether Yulius creates the repository or only takes an existing empty one, use the
fake GitHub for tests, and update the docs and SPEC-001 PRJ-012.

## Goal

A team that tried Yulius in a playground and then connected GitHub keeps what it did there: an
admin of the playground moves it into an empty GitHub repository of the instance owner, and it
becomes an ordinary GitHub sandbox with the same slug, tasks, sessions, steps and history. Its
branches and tags are on GitHub, its clone's `origin` is that repository, and its access mirrors
GitHub from then on.

## Context

- Not fixed on `dev` (2026-10-15, after #560): a playground stays one forever. Nothing changes a
  sandbox's repository once recorded.
- A playground is a sandbox with an empty `repo_owner` (`SandboxRecord::is_playground`, ADR-0081).
  Every playground branch in the code asks that one test at the time it runs. Access
  (`access.rs`), the agents' Git credentials (`identity.rs`), the clone's refresh
  (`dlv_refresh_clone`), delivery, CI (SPEC-163), hygiene and the briefing (`supervisor.rs`, built
  per attempt) all do. So writing a GitHub owner and name into the record turns every one of them
  into its GitHub path.
- Its `origin` is a bare repository on the server, `<data>/playgrounds/<id>.git`, which holds every
  branch and tag (SPEC-099 PLY-003). Task branches live in the sandbox clone, and agents never
  reach `origin`. A playground's merges hold the sandbox's armed-merge pass (`armed_pass`).
- Importing a repository (`import_permission`, IMP-002) already decides who may link a repository
  of the instance owner. A person needs `admin` or `maintain` on GitHub. The superadmin needs a
  repository the App installation can access (ACC-012).
- The GitHub App's manifest asks for Administration: write (`crates/yulius-github/src/app.rs`), and
  creating a project (`create_repository`, PRJ-004) is for instance administrators. For a personal
  account owner it acts with the owner's own user token.
- The real Git adapter reaches `https://github.com/…` only, so integration tests cannot reach the
  fake GitHub's repositories through it today.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MPG-001 | `POST /api/sandboxes/{sid}/move-to-github` `{name}` moves a live playground into the repository `<instance owner>/<name>`. Only an admin of the playground (its maker or the superadmin) may move it, and only one IMP-002 would let import that repository: a person needs `admin` or `maintain` on it on GitHub, the superadmin one the App installation can access. Without the App it is a 409, and so is a repository that is already a sandbox. A GitHub sandbox is a 409. The response is the sandbox's new view. | `playground_move::a_playground_moves_into_an_empty_github_repository`, `only_who_could_import_the_repository_moves_a_playground` |
| MPG-002 | The repository must be empty. It may also hold only this playground's branches and tags, at their commits or behind them, as a move that failed after its push leaves it (merges may have landed since); the push then fast-forwards them. A repository with any other branch, tag or commit is refused (409) and nothing is pushed. | `playground_move::a_repository_that_is_not_empty_is_refused`, `a_move_retried_after_its_push_completes`, `a_retry_after_the_playground_moved_on_fast_forwards` |
| MPG-003 | The move pushes every branch and tag of the playground's `origin` to the repository in one atomic push that never forces, with the installation token. It points the sandbox clone's `origin` at the repository and fetches it. It then records the GitHub owner, name and repository id on the sandbox in one compare-and-set that holds only while the sandbox is still a playground. Last, it deletes the bare `origin` on the server and audits `sandbox.moved_to_github`. The slug, tasks, attempts, sessions, steps, task branches and worktrees stay. | `playground_move::a_playground_moves_into_an_empty_github_repository` |
| MPG-004 | From then on the sandbox is a GitHub sandbox. Access mirrors GitHub (ACC-007): its maker keeps access only through their GitHub permission on the repository, and nobody else's access comes from having made the playground. Yulius fetches and pushes it on GitHub, and its tasks deliver by pull request. The playground note and limits are gone. | `playground_move::a_playground_moves_into_an_empty_github_repository` (maker without GitHub permission sees nothing; with it, their GitHub level) |
| MPG-005 | A refused or failed move leaves the playground as it was: its record, its clone's `origin`, the bare repository and its branches. It refuses before any push while a task of the sandbox is live or queued, or a CI run of it is, and says so; it checks again just before the record update. A push GitHub refuses changes nothing. A failure after the push (the clone's fetch, the record update) puts the clone's `origin` back. The move runs detached from its request: a client that goes away never leaves it half done. Once the record changed, a failed audit or deletion of the bare `origin` is logged and the move still answers its new view. | `playground_move::a_failed_push_leaves_the_playground_as_it_was`, `a_move_failing_after_its_push_points_the_clone_back_and_is_retried`, `a_live_task_holds_the_move`, `an_active_ci_run_holds_the_move`, `a_repository_that_is_not_empty_is_refused`, `a_repository_that_is_already_a_sandbox_is_refused`, `a_move_ends_when_its_client_goes_away` |
| MPG-006 | The web UI offers it. The sandbox menu of a playground has "Move to GitHub…" for its admins once GitHub is connected. Its dialog names the owner, asks for the name of an empty repository, and says what happens: branches and tags are pushed, access then follows GitHub, tasks stay. On success the page shows the sandbox as a GitHub sandbox. | `e2e/tests/playground.e2e.ts` "a playground moves to GitHub from the sandbox menu" |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Git pushes every branch and tag of a repository atomically and repoints origin
- [x] A playground admin moves it into an empty GitHub repository, its tasks kept
- [x] A refused or failed move leaves the playground as it was
- [x] The sandbox menu offers Move to GitHub once GitHub is connected
- [x] Docs, SPEC-001 PRJ-012, the ADR and the changes log say what a move does
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green and the change delivered into dev

## Acceptance criteria

- The maker of a playground with two branches, a tag and a resting task, who has `admin` on the
  empty `Quartio-Studio/alices-lab` on GitHub, moves it. The GitHub repository then holds both
  branches and the tag at the playground's commits. The sandbox view has owner `Quartio-Studio`
  and `playground: false`, with the same slug. The task, its steps and its attempt are unchanged.
  The clone's `origin` URL is the GitHub one, and `<data>/playgrounds/<id>.git` is gone.
- Once GitHub says the maker has no permission on the repository, the sandbox is a 404 for them.
- A non-empty repository, a person with only `write` on it, and a live task are each refused.
  The playground stays as it was in each case.
- A push that fails leaves the record, the clone's `origin` and the bare repository unchanged. A
  retry once the repository holds what the push sent completes the move.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit (`yulius-git`): `push_all_refs` sends every branch and tag atomically and never forces,
  `remote_refs` lists a remote's branches and tags, and `set_remote_url` repoints a remote
  (`crates/yulius-git/tests/git_ops.rs`).
- Unit (`yulius-store`): the compare-and-set writes only while the sandbox is a playground.
- Integration (`crates/yulius/tests/playground_move.rs`): the real Git adapter maps
  `https://github.com/` to a directory of bare repositories (a test seam), with the fake GitHub
  for permissions and installation repositories. It covers the happy path, access after the move,
  each refusal, a failed push (the GitHub repository unwritable) and a retry.
- e2e (`e2e/tests/playground.e2e.ts`, mock server): the menu item and dialog, and the view after
  the move.

## Decisions

- **Yulius takes an existing empty repository; it does not create one (ADR-0142).** The App
  already has Administration: write, so creating would work. But it would let anyone who can make
  a playground (anyone signed in) create repositories in the organisation, which PRJ-004 keeps for
  instance administrators. And a repository the App creates in an organisation gives its maker no
  permission on it, so under ACC-007 the maker would lose their own project at the moment of the
  move. Taking a repository the mover can already import (IMP-002) proves through GitHub that they
  may host code there. It also keeps them admin through GitHub afterwards, and it works alike for
  an organisation and a personal account. The cost is one step on GitHub first: create an empty
  repository, and on an installation limited to selected repositories, add it to the App.
- **Atomic push, then the clone, then the record: the record is the commit point.** `git push
  --atomic` updates all refs or none, so a refused push leaves GitHub as it was. After the push,
  the clone's `origin` is repointed, and the record's compare-and-set flips the sandbox to GitHub.
  When that write fails, the clone is pointed back. The bare `origin` is deleted only after the
  record changed, so a failure at any point leaves a working playground. A failure after the push
  leaves the GitHub repository holding exactly the playground's refs, and MPG-002 accepts that so a
  retry completes the move.
- **Review round 1 (fresh session), fixed test-first:** the move now runs detached from its
  request (it was cancelled with a client that went away, leaving the clone or the bare `origin`
  behind), re-reads the record under the merge pass, holds on an active CI run and re-checks both
  before the record update, accepts a repository behind the playground on retry, and logs (never
  fails on) the audit and the deletion after the commit point. A playground merge that waited for
  the pass now refuses when the sandbox moved meanwhile; that guard has no test, as staging the race
  needs the pass held from outside the app crate. The record update's own failure path has no
  integration test either: its compare-and-set is tested in the store, and it shares
  `restore_origin` with the fetch failure, which is tested. Finished CI runs of the playground stay;
  after the move, publishing their status to GitHub may record a "not published" note for a commit
  that only existed on the server, which changes no gate.
- **Review round 2 (fresh session, approved with minors), fixed:** the repository is checked
  against other sandboxes again under GitHub's own name for it (a renamed repository), before any
  push; the merge pass is taken only after GitHub's permission and token answers; the delivery loop
  skips a sandbox whose pass is held instead of waiting minutes on a move; the playground's default
  branch is pushed alone first, so GitHub makes it the repository's default (the local test remote
  cannot show GitHub's choice, so only the push is tested); the happy-path test checks the audit
  row, steps and session; the mock names a moved sandbox by its repository, as the server does.
  The second busy check's rollback has no test: staging a task start during the push needs a hook
  on the receiving repository, and the server's Git runs with `core.hooksPath=/dev/null`, which
  reaches a local remote's `receive-pack` too. It shares `restore_origin` with the tested fetch
  failure.
- **Live or queued tasks hold the move.** Their agents were started with a playground's briefing,
  tools and Git credentials, and a running playground merge or armed CI merge would land on the
  bare `origin` after the push. Requiring them to be stopped first (they continue after the move
  with a GitHub briefing, which is built per attempt) is simpler to reason about than migrating a
  live run. The move also holds the sandbox's armed-merge pass against a person's merge in the
  same moment.
- **The record keeps its id, slug and history; a compare-and-set changes owner, name and
  repository id.** It needs no migration: SPEC-099 marks a playground by its empty owner. The
  unique `(owner, name)` index refuses a repository that another sandbox holds.
- **Rulesets are not created.** An imported repository gets none from Yulius either; its
  onboarding or `production_protect` adds them (ADR-0129). The moved sandbox keeps onboarding
  `not_needed`, as the project was already prepared in the playground.
- **A test seam maps GitHub to local repositories.** `GitAdapter::with_github_base(dir)` rewrites
  `https://github.com/` to `dir/` through `url.<base>.insteadOf` in the command's environment. It
  is off unless a test sets it, so the integration tests drive the real Git adapter end to end
  without reaching the network.

## Evidence

Run on the branch head after merging `dev` (`152d7ec7`, 2026-10-15):

- `scripts/dev ci`: exit 0. Repository contracts passed; nextest `1882 tests run: 1882 passed, 18
  skipped`; clippy with `-D warnings`, rustdoc, doctests, cargo-deny and cargo-shear clean.
- `cd e2e && npx e2e run`: exit 0, `Test Files 55 passed | 1 skipped`, `Tests 526 passed | 1
  skipped`. The two new tests in `playground.e2e.ts` failed before the UI change (no "Move to
  GitHub…" menu item) and pass with it.
- `playground_move::` (12 integration tests, real Git and store, fake GitHub): all pass, covering
  MPG-001 to MPG-006. They failed before the change (no route). Mutations proved the regressions:
  without the live-task check `a_live_task_holds_the_move` fails; without the emptiness check
  `a_repository_that_is_not_empty_is_refused` fails; without the detached run, the CI hold and the
  fast-forward acceptance, `a_move_ends_when_its_client_goes_away`, `an_active_ci_run_holds_the_move`
  and `a_retry_after_the_playground_moved_on_fast_forwards` fail.
- `yulius-git` `every_branch_and_tag_is_pushed_at_once_and_read_back` and
  `a_push_of_every_ref_is_all_or_nothing`, and `yulius-store`
  `a_playground_record_moves_to_github_once`: pass (they did not compile before the change).
- `scripts/dev security`: exit 2, **not run**. Vidoc rejected the key (HTTP 401), as since
  2026-10-14 (#470, ADR-0104). This is unrun, not passed.
- Independent review: two rounds in fresh sessions. Round 1 asked for changes (1 major, 6 minor),
  all fixed test-first. Round 2 approved with minors, which were fixed as well (Decisions). A final
  review covers the head that is delivered.
- Not tested here: GitHub itself. The fake GitHub answers permissions, and local bare repositories
  stand in for its Git (`GitAdapter::with_github_base`), so GitHub's choice of default branch, its
  push limits and a real installation token are not exercised. A real browser on a real server
  was not driven either: the e2e suite runs against the mock server.
