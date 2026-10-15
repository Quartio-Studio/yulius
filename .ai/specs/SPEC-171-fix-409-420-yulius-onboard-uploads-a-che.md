---
yulius-task: dd2d5a9f-be01-4be9-afcb-732278d5adf4
status: in-progress
---

# SPEC-171: a playground's lifecycle at its ends — `yulius onboard --playground` and purging a removed playground's code

Status: in progress
Date: 2026-10-15
Owner: Yulius task #131 (part of task #118, SPEC-138 lane B)
Scope authorization: issues [#409](https://github.com/Quartio-Studio/yulius/issues/409) and
[#420](https://github.com/Quartio-Studio/yulius/issues/420), follow-ups of
[SPEC-099](SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md) (playgrounds). #409: "`yulius
onboard --playground [path]` uploads the checkout (all branches as a bundle, uncommitted work as a
snapshot, the chosen sessions) into a new or existing playground of the signed-in person, reusing
the import upload pipeline and nothing GitHub-specific." #420: "Removing a playground offers to
delete its code for good, or a superadmin action purges removed playgrounds; the quota then counts
only what is on disk."

## Goal

A team can bring a local checkout that has no GitHub remote, with its Claude Code sessions, into a
playground from the command line, and a playground that is removed can have its code deleted from
the server for good, which frees its maker's quota. Both are the playground's lifecycle at its
ends: coming in from a local checkout, and going away.

## Context

- Neither issue is fixed on `dev` (2026-10-15, after #541): `yulius onboard` refuses a checkout
  without a GitHub remote (`RepoError::NoRemote`/`NotGitHub`, `crates/yulius-import/src/git.rs`),
  and the server refuses an empty owner (`refuse_non_github`, PLY-002:
  `the_cli_import_never_reaches_a_playground`). `DELETE /api/sandboxes/{sid}` keeps every file
  (PRJ-009), and `playground_quota` counts every `<data>/playgrounds/<id>.git` of the person.
- The import pipeline (SPEC-001 IMP-001..009, `crates/yulius-contracts/src/import.rs`): a plan
  (`POST /api/onboard/plan`), a job (`POST /api/import` with the manifest), resumable checksummed
  chunks, then `complete`, which imports the bundle's branches under `import/<login>/` in the
  sandbox clone, commits the snapshot on an `import/<login>/snapshot-<date>` branch and stores the
  sessions as session documents (Sessions page). Job files live under
  `<sandbox>/imports/<job>`; `import_jobs.sandbox_id` has no foreign key.
- The browser upload (PLY-003, `playground/folder.rs`): the uploaded history's branches and tags
  are copied into a new bare `origin` (`copy_refs`, hardened `git fetch`), the working files land
  as one commit on the branch `HEAD` names, and the profile comes from `yulius.toml` without
  environments or automatic promotion.
- The disk upkeep (`disk.rs`, every 5 minutes) already removes a removed sandbox's worktrees and CI
  checkouts after a day, and keeps its home and clone.
- Processes of a sandbox Yulius knows: agent runs (live attempts, a task `Stopping`), shells
  (terminals, cockpit, dev servers), the sandbox browser and CI runs (`ci_busy`/`ci_claim`). A
  playground has no CD. Removal stops tasks asynchronously: their runs end and are reaped after
  `DELETE` returns, and no new process can start in a removed sandbox (`require` treats it as gone).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PGL-001 | `yulius onboard --playground [path] [--name <name>]` works on a Git checkout with no remote or with any non-GitHub remote (a GitHub remote is not needed and not used). The playground's name is `--name`, else the checkout directory's name. It uploads a bundle of every local branch and tag, the uncommitted work as the snapshot archive of IMP-001, and the chosen sessions (the `--sessions`/`--since`/`--yes`/`--dry-run`/`--status` choices apply), through the import pipeline with its redaction, resumable chunks and idempotent re-runs. A detached `HEAD` or a checkout without commits is refused with what to do. Nothing is pushed anywhere: the CLI talks only to the Yulius server. | `yulius-import` `end_to_end::a_checkout_without_github_goes_to_a_playground`, `repo::playground_discovery_needs_no_remote`; `playground::onboard_cli_uploads_a_checkout_into_a_new_playground` |
| PGL-002 | The plan and the manifest carry `playground: true` with an empty owner and remote URL. Without that flag an empty owner is still refused (PLY-002). The plan answers the caller's existing playground of that name: one they made, not removed, named `<slug>` or `<slug>-<n>`, the oldest; else whether they may make one (the limit of 10, PLY-001). A person has one upload into a playground not made yet open at a time: the same manifest resumes it, another supersedes it (its files go; completing it is a 409). | `playground::the_cli_import_never_reaches_a_playground`, `onboard_cli_uploads_into_an_existing_playground`, `a_cli_playground_counts_toward_the_ten`, `a_new_playground_upload_supersedes_an_unfinished_one` |
| PGL-003 | A new playground from a CLI upload is made when the upload completes, as the browser upload makes one (PLY-003): every branch and tag of the bundle, the branch `HEAD` names as the default branch, the uncommitted work as one commit on it, `dev` created when missing, the profile from its `yulius.toml` without environments or automatic promotion, onboarding `not_needed`. Only the hardened fetch reads the bundle. A failed creation leaves no playground and keeps the uploaded files, so completing again retries. A re-run whose bundle the same person already applied there imports no branch again. | `playground::onboard_cli_uploads_a_checkout_into_a_new_playground`, `a_cli_upload_refused_at_completion_keeps_its_files_and_completes_later` |
| PGL-004 | Into an existing playground the upload lands as an import into a GitHub sandbox does: branches under `import/<login>/`, the snapshot on `import/<login>/snapshot-<date>`, in the sandbox clone; the playground's own branches are untouched. In both cases the sessions become session documents of the playground (its Sessions page), and Yulius calls GitHub for nothing. | `playground::onboard_cli_uploads_into_an_existing_playground`, `onboard_cli_uploads_a_checkout_into_a_new_playground` (no GitHub write, permission lookups failing) |
| PGL-005 | A playground's code can be purged: `POST /api/sandboxes/{sid}/purge` by an admin of a live playground (its maker, the superadmin) removes it as `DELETE` does and requests the purge; the superadmin may request it for an already removed playground. A GitHub sandbox is refused (409): its code is on GitHub. The request is recorded (table `playground_purges`) and audited (`sandbox.purge_requested`). | `playground::purging_a_playground_deletes_its_code_once_nothing_runs`, `a_github_sandbox_is_never_purged`, `only_an_admin_purges_and_the_superadmin_purges_a_removed_one` |
| PGL-006 | The purge deletes the playground's `origin` (`<data>/playgrounds/<id>.git`) and its sandbox directory only once every process of the sandbox ended: no live agent run or stopping task, no live shell, no running browser, no CI run (the CI checkouts are claimed while it deletes). While one runs, nothing is deleted and the request waits; it is retried at once on the request and with every disk upkeep round. Once done it is recorded (`purged_at`) and audited (`sandbox.purged`). | `playground::a_purge_waits_while_a_process_of_the_sandbox_runs`, `a_purge_waits_for_a_ci_run_and_is_done_once`; `purge::tests::a_read_only_tree_is_deleted_and_a_missing_one_is_fine` |
| PGL-007 | After the purge the maker's quota no longer counts the playground. Task records, the removal export and the audit trail stay. | `playground::purging_a_playground_deletes_its_code_once_nothing_runs` |
| PGL-008 | The web UI's removal dialog of a playground offers "Also delete its code from this server for good" and, when chosen, says before confirming that its repository, branches and files are deleted once its processes have ended, that this cannot be undone, and that task records and the audit stay. The removal of a GitHub sandbox is unchanged. | `e2e/tests/playground.e2e.ts` (purge on removal) |

## Plan

- [x] Spec and ADR-0139 written and validated
- [ ] The CLI captures a checkout without GitHub for a playground (contracts, discovery, bundle of every branch)
- [ ] A CLI upload makes a new playground with every branch, the uncommitted work and the sessions
- [ ] A CLI upload lands in the caller's existing playground, and the real CLI runs against the real server
- [ ] A removed playground's purge request is stored and waits until no process of the sandbox runs
- [ ] The purge API and the removal dialog offer to delete a playground's code for good
- [ ] Docs, changes log and architecture updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, evidence recorded

## Acceptance criteria

- On a checkout with no remote, and on one whose `origin` is a local bare repository (a
  non-GitHub remote), `yulius onboard --playground --yes` makes a playground: its `origin` has
  every local branch, the default branch has the uncommitted change as one commit, and the chosen
  session is on its Sessions page. The checkout's remote is unchanged and the fake GitHub recorded
  no write while its permission lookups fail.
- Running it again sends nothing new; with a new commit it lands in the same playground under
  `import/<login>/`.
- A purge requested while a shell of the removed playground is live deletes nothing; once the shell
  ended, the next round deletes `origin` and the sandbox directory, the maker can make a tenth
  playground again, and the task records and the audit remain.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit (`yulius-contracts`): a manifest with `playground` round-trips and serialises without the
  field when false.
- Integration (`yulius-import`, fake server): discovery without a remote, a bundle of every branch
  and tag, the plan and manifest carry `playground: true`; GitHub refusals stay for the default
  mode.
- Integration (`yulius` crate, `tests/playground.rs`): real store, real Git, fake GitHub. The real
  CLI (`run_import`) runs against the router served on a loopback port with a bearer token: new
  playground (branches, snapshot commit, session documents), re-run, existing playground. The
  purge: refused while a shell lives, then deleted; quota freed; GitHub sandbox refused; access.
- e2e (`e2e/tests/playground.e2e.ts`): the removal dialog of a playground offers the purge and its
  warning, and the purge request is sent.

## Decisions

- **An explicit `playground` flag, not an empty owner alone.** PLY-002 refuses an empty owner so a
  GitHub import never reaches a playground; an older CLI sending one must still be refused. The
  flag is skipped when false, so GitHub manifests serialise as before (resumable jobs match).
- **The playground is created when the upload completes, under an id reserved when the job is
  created.** The job's files go to `<data>/sandboxes/<id>/imports/<job>` as for any import, so
  the chunk and resume paths stay unchanged; the record appears only once its repository exists,
  so an abandoned upload leaves no half-made playground and counts toward no limit. A resumed run
  finds the caller's open job with the same manifest. Creating it at job creation (as a GitHub
  import does) would leave an empty playground for an interrupted upload. Like every import job
  today, an abandoned upload's files are not expired (a follow-up for all imports).
- **A new playground takes the branches as they are; an existing one gets them under
  `import/<login>/`.** A new one should look like the checkout, as the browser upload does
  (PLY-003); an existing one's `dev` holds what its tasks merged, which an upload must not
  overwrite.
- **A detached `HEAD` is refused.** The snapshot is a diff against `HEAD`, and PLY-003 lands the
  uncommitted work on the branch `HEAD` names; a detached head has none. Checking out a branch is
  one command.
- **The purge is a recorded request that runs once nothing runs (ADR-0139).** Removal stops tasks
  asynchronously, so a purge right after it would almost always find a run still ending; refusing
  it would make "delete on removal" fail in practice. A request in its own table survives
  restarts, needs no new column on `sandboxes` (whose record is built in many places), and the
  disk upkeep round already visits removed sandboxes. Deleting while a process runs could leave a
  process writing into a deleted tree or recreating it, and a timeout is not proof a process died.
- **Offered on removal, and to the superadmin for an already removed playground through the API.**
  Removed sandboxes are invisible in the UI; a list of removed playgrounds for the superadmin is a
  follow-up (named in the closing comment of #420).
- **Review round 1 (fresh session), fixed test-first:** a creation refused at completion (the
  limit reached meanwhile) deleted the job's files (`record_playground` removed the whole sandbox
  directory): each caller now removes only what it seeded; abandoned new-playground uploads could
  pile up without counting toward the limit: one is open per person, a newer one supersedes it
  (job state `superseded`); a re-run re-imported the same bundle under `import/<login>/`; a purge
  requested again audited `sandbox.purged` twice; the purge now deletes the sandbox directory
  before `origin`, so one that fails half-way keeps the code and its quota count and is retried;
  the quota and empty-repository messages were wrong for playgrounds.
- **Review round 1, decided rather than changed:** the shell, browser and namespace-process waits
  have no integration test: a live shell needs the process host, which the playground harness does
  not run; they share `still_running` with the agent-run and CI waits that are tested, and each
  reads an existing registry (`sandbox_shells`, `browser.running`, the sandbox manager's `/proc`
  count). A playground has no member without admin (its maker and the superadmin only), so the
  purge's refusals are a 404 for anyone else, tested. A completion reclaimed after the 15-minute
  stale bound while a first one still seeds could remove that seed; seeding a bounded (2 GiB) local
  bundle takes far less, and the second completion then makes the playground itself. The sandbox
  host's own state of a sandbox (`<data>/sysroots/<id>`, `runtime/sandboxes/<id>`) is the host's,
  and the purge leaves it, as ADR-0139 says.

## Evidence

To be recorded.
