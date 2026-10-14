---
yulius-task: 94cdc9fe-a521-45b0-a7b7-b35b1e4225c8
status: done
---

# SPEC-099: playgrounds — try Yulius on your own code before connecting GitHub

Status: implemented
Date: 2026-10-14
Owner: task #84
Scope authorization: the person's request (task #84): "On onboarding, we want a mode to play
around. This should be a sandbox mode and installation mode (connection github orgs accounts etc
shouldnt be mandatory and the user can do that later but then only play around mode is available)
where the user can upload a repo or a folder but things stay in local git and arent uploaded to
remote. It's for teams who want to test the product before conneting their org. Think about how to
approach it from the user side to reduce friction completely"

## Goal

A team that has just installed Yulius can try it on its own code in a minute, without connecting
GitHub: drop a project folder (or a repository) on the home page, or start from a template, and
start tasks in it at once. The project is a **playground**: a sandbox whose Git lives only on the
server. Nothing is pushed to GitHub or anywhere else, and its tasks deliver into the playground's
own `dev` branch. Connecting GitHub stays possible at any time and is what unlocks importing and
creating the organisation's repositories. Until then the playground is the one way to use Yulius.

## Context

- Today every project path needs GitHub. Instance setup (J1) creates the superadmin with a
  password. Then the home page disables Import and Create until a superadmin connects the GitHub
  App (`home.js` `needsGithub`), and `app.js` toasts "Connect this instance to GitHub" on every
  sign-in. Import (`projects.rs` `import_permission`) and template creation (`create_repository`)
  need the App and a repository of the instance owner. So do the clone (`clone_url`: github.com
  only), delivery (`dlv_merge` merges on GitHub), CI (`ci_sync_sandbox` lists GitHub pull
  requests) and access (`access.rs`: GitHub-mirrored, except the superadmin, ACC-012).
- What already works without GitHub: the superadmin signs in with the setup password and has admin
  on every sandbox (ACC-012). A task starts from a local clone (`ensure_worktree` bases it on
  `origin/<integration>`, resolved locally) and needs only a Claude account. Every Git operation
  of Yulius goes through the clone's `origin` remote. The Git adapter accepts a path as a remote
  URL (`validate_remote_url`), runs hardened (`core.hooksPath=/dev/null`,
  `core.fsmonitor=false`, `protocol.ext.allow=never`) and has `init --bare`. Merge trains
  (`build_train`) already land a squash in a scratch worktree.
- `yulius onboard` (CLI) also needs a GitHub remote (`find_github_remote`). It stays as it is;
  onboarding a CLI checkout into a playground is a follow-up (Decisions).
- Parallel tasks touch onboarding: #81 (UI: a `pending` sandbox shows only a centred Start
  onboarding message), #82 (no resting onboarding task for a CLI import), #80 (MCP servers). A
  playground's onboarding is `not_needed`, so it never meets #81's gate. Nothing here changes
  `link_repository` or `taskview.js`'s onboarding banner.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PLY-001 | Any signed-in, enabled person can make a playground, with or without GitHub connected, in one of two ways: from a template (`POST /api/sandboxes` `{mode: "playground", name, template}`, the four PRJ-008 templates), or from a folder uploaded from the browser (`POST /api/playgrounds?name=<name>`, a gzip tar body). The name is the sandbox's name and slug (`my-shop`, `my-shop-2` when taken). The response is the sandbox, and its onboarding is `not_needed`, so tasks start at once. A person keeps at most 10 playgrounds (the superadmin is not counted; a removed one counts while its files stay); the 11th is a 409 before any upload. | `playground::a_playground_starts_from_a_template_without_github`, `a_folder_without_git_becomes_one_commit`, `a_person_makes_at_most_ten_playgrounds`, `a_removed_playground_still_counts_toward_the_ten` |
| PLY-002 | A playground's Git stays on the server. Its `origin` is a bare repository on the server outside the sandbox (`<data>/playgrounds/<id>.git`), out of reach of the sandbox's processes, whose hooks the server's Git would otherwise run. The server fetches and pushes it by that path, never by the clone's configured URL; agents do not fetch it, Yulius keeps `origin/<branch>` current in the clone. Yulius never calls GitHub for a playground: no clone, push, pull request, commit status or permission lookup. It never sends a GitHub token to its Git either, and `yulius onboard`'s import and plan refuse an owner that is not a GitHub one, so they never reach a playground. The sandbox view says `playground: true`, with an empty repository owner and URL. | `playground::a_playground_starts_from_a_template_without_github` (the fake GitHub records no call, `origin` outside the sandbox); `playground::a_playgrounds_task_merges_into_its_local_dev`, `a_rewritten_origin_url_is_never_followed`, `the_cli_import_never_reaches_a_playground` |
| PLY-003 | An uploaded folder becomes the playground's repository. A folder with a `.git` directory keeps its branches, tags and history, and the branch its `HEAD` names is the default branch. Its working files, uncommitted and untracked changes included (ignored ones left out by its own `.gitignore`), land as one commit on top when they differ; a tracked file deleted but not committed is not carried. A browser carries no file modes and follows links, so a path that differs from the committed one only by its mode or by a link turned into a file keeps the committed one. A shallow repository stays shallow with its branches. A folder without `.git` becomes one initial commit on `main`. Either way the integration branch (`dev`) exists, from the default branch when the folder has none. Of `.git` only `HEAD`, `packed-refs`, `shallow`, `objects/` (but not `objects/info/`, whose alternates would make Git read another repository on the server) and `refs/` are taken: never its `config`, hooks or anything else, so nothing in the upload configures the server's Git or reaches another repository. | `playground::a_folder_with_history_keeps_it_and_its_uncommitted_work`, `a_folder_without_git_becomes_one_commit`, `a_git_config_or_hook_in_the_upload_is_never_taken`, `an_uploaded_alternate_never_reads_another_repository`, `an_upload_keeps_executable_bits_and_links_of_the_repository`, `a_shallow_repository_keeps_its_branches`; `folder::tests::only_the_history_of_an_uploaded_git_directory_is_written` |
| PLY-004 | An upload is bounded. The body has at most 1 GiB, each chunk arrives within the upload idle bound, the archive expands to at most the import bounds (2 GiB, 200 000 entries), and at most the upload slots run at once. Paths that escape the folder, links that point outside it and files under `.git` in the working tree are skipped. A first pass reads only headers and metadata: long names and PAX records of at most 64 KiB, no PAX `size` override, and every entry's declared size counts toward the bound, so nothing is decompressed past it. An unreadable or over-limit archive is refused with the reason, and nothing of it stays on disk, also when the client goes away during the setup (it runs on its own task). The browser refuses a folder over 4 GB or a file over 2 GB before packing. | `playground::an_archive_over_the_bounds_is_refused_and_leaves_nothing`, `a_closed_tab_during_setup_leaves_no_stray_directory`; `folder::tests::an_archive_declaring_more_than_the_bound_is_refused_unread`, `oversized_names_and_size_overrides_are_refused`; `playground::a_long_path_with_size_in_its_name_is_uploaded`; `imports` extraction tests (paths, links) |
| PLY-005 | Access: the superadmin and the person who made a playground have admin on it, and nobody else sees it. There is no GitHub repository to mirror. Since its maker may be anyone signed in, a playground runs no command on the server: its profile has no deploy environment and no automatic promotion (an uploaded `yulius.toml`'s are dropped, a profile with one is a 422, an environment run and a promotion a 409). | `playground::only_its_maker_and_the_superadmin_see_a_playground`, `a_playground_runs_no_environment_command`, `a_playground_profile_has_no_environment_or_automatic_promotion` |
| PLY-006 | A playground's tasks know where they are. The definition of done in their request file and goal says to merge into the playground's `dev` with `delivery_merge` after running the project's gates and the independent review. There is no Yulius CI, pull request or deploy in a playground, and the definition says so. | `request::a_playground_done_definition_merges_locally` |
| PLY-007 | `delivery_merge` in a playground (the agent's tool and the task's merge action) squashes the task branch onto the playground's integration branch as one commit. Its title follows the branch policy, as a pull request's default title does (BPL-008), with the task number in its body. It pushes that commit to `origin`, records the stage as merged into integration, so the task can be Done, and audits it. The agent's call lands only the head its review approved. A squash that already landed (its stage lost) is recorded as merged, while a branch whose commits cancel out delivers nothing; an integration branch that moved during the merge is a 409 to merge again. It refuses, with what to do, when the spec is invalid where one is required, a step is open, the worktree has uncommitted changes or the branch has nothing to deliver. When the integration branch moved and the branch conflicts with it, it refuses naming the files and the remedy (merge `origin/dev` into the branch). | `playground::a_playgrounds_task_merges_into_its_local_dev`, `a_conflicting_branch_is_refused_with_the_files`, `an_unfinished_task_is_not_merged`, `a_merge_landed_before_its_stage_was_recorded_is_recorded` |
| PLY-008 | Nothing in Yulius reports a GitHub failure for a playground, and nothing gets a GitHub token for it. CI sync, hygiene, the automatic pull request at the end of a turn and delivery reconciliation skip it; its agents' and people's Git credential requests are refused (403), and promotion and `issue_create` answer that a playground has none (409). The task's delivery view and the Git page carry one note instead: "This playground keeps its Git on this server: no pull requests; `delivery_merge` squashes a task into `dev` here." | `playground::a_playground_never_calls_github`, `a_playgrounds_agent_gets_no_github_token_or_tool` |
| PLY-009 | The web UI offers the playground first. On the home page, an instance without sandboxes says "Try Yulius on your own code" and offers two actions: drop or choose a project folder, or start from a template. Connecting GitHub is the secondary action, and the existing Import and Create follow once it is connected. The Add sandbox dialog has a Playground tab, available to everyone. The browser packs the chosen folder itself into a gzip tar. It skips dependency and build folders (`node_modules`, `.venv`, `venv`, `__pycache__`, `target`, `.next`, `.nuxt`, `.turbo`, `.cache`) and takes only the history part of `.git`. It shows the packing and upload progress, then opens the new playground. Playground cards say "Playground · local Git on this server". The superadmin's sign-in no longer toasts about GitHub. A notification says GitHub is optional ("Playgrounds work without it"). | `e2e/tests/playground.e2e.ts` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A playground starts from a template with no GitHub, visible to its maker only
- [x] A playground starts from an uploaded folder, its history kept and its Git config never taken
- [x] A playground's tasks merge into its local dev with delivery_merge, with no GitHub call
- [x] Home and Add sandbox offer the playground first, with a folder dropped or chosen in the browser
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Review round 2: origin outside the sandbox, bounded archives, quota and promotion fixed test-first
- [x] Review round 3 (final) on the merged head, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- On an instance with no GitHub App, the superadmin starts a playground from the Empty template,
  and from a folder with a `.git` and an uncommitted change. Each opens with tasks startable at
  once, and the fake GitHub records no call.
- The uploaded folder's branches, history and uncommitted change are in the playground. Its
  `.git/config` and hooks are not.
- A playground task with a valid spec, closed steps and one commit calls `delivery_merge`.
  Playground `dev` then has one new squash commit with a conventional title, and the task's stage
  is merged.
- Another signed-in person does not see the playground.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit (`yulius-domain`): the playground's definition of done (short and full).
- Unit (`yulius-git`): `squash_land` lands one commit with the head's tree on the base, reports
  conflicts, and needs no network.
- Unit (`yulius-app` `playground`): `.git` whitelist extraction (config, hooks and `info` never
  extracted; escaping paths refused); the default branch read from `HEAD`.
- Integration (`yulius` crate, `tests/playground.rs`): a real store, real Git and the fake GitHub.
  It creates playgrounds from a template and from archives built like the browser builds them
  (with history and uncommitted work, without `.git`, with a planted `config`/hook, over the
  bounds), and checks access by person. It runs a task's merge through `delivery_merge` (clean,
  conflicting, unfinished) and asserts no GitHub call.
- e2e (`e2e/tests/playground.e2e.ts`): a fresh instance without GitHub shows the playground first.
  Choosing a folder packs, uploads and opens it, and the template path creates one.

## Decisions

- **The playground's remote is a bare repository on the server, outside the sandbox (ADR-0081).**
  Inside the sandbox root its hooks and configuration would be the maker's to write, and the
  server's own push runs them (review round 2). Every Git path of
  Yulius (clone refresh, worktree base, merges, trains) goes through `origin`. Giving a playground
  an `origin` of its own keeps those paths unchanged. Its Git stays local by construction, not by
  checks scattered through the code. Alternatives were a "local forge" adapter emulating GitHub's
  pull requests, statuses and merges (far larger, and it would pretend there are pull requests),
  or no remote at all (every `origin/…` path would need a second branch).
- **A playground is marked by an empty repository owner, not a new column.** A GitHub owner is
  never empty (`valid_owner`), so the record needs no migration. `SandboxRecord::is_playground` is
  the one test, and the repository name is the slug, so the unique `(owner, name)` index holds.
  An explicit `kind` column was the alternative. It needs a migration, which parallel tasks
  number, for no extra information.
- **Delivery in a playground is a local squash merge, gated by the same task facts.** The gates
  are a valid spec, closed steps, a clean worktree and commits ahead, with the branch policy's
  title. There is no Yulius CI in a playground: Yulius CI is driven by pull requests, and a local
  CI is a follow-up. The agent runs the project's gates itself, as the discipline already asks.
  Running CI on a local head now would double this change. The definition of done says plainly
  that there is no CI.
- **The browser packs the folder.** Folders cannot be uploaded as one file, and a zip needs a new
  server dependency. The browser builds a ustar archive (PAX headers for long names) from `File`
  blobs without reading them into memory, then gzips it with `CompressionStream`. The server reuses
  the bounded extraction of `yulius onboard` snapshots.
- **Only the history part of `.git` is taken.** The server's Git runs outside containment. An
  uploaded `config` could name filters or other commands, and hooks are scripts. Taking a
  whitelist (`HEAD`, `packed-refs`, `shallow`, `objects/`, `refs/`) and fetching from it with the
  hardened Git keeps the history and nothing that runs.
- **Onboarding is `not_needed`.** The request asks for no friction. The first task's briefing
  already lays missing foundations (DSC-014), and a person can still ask a task to onboard the
  project.
- **Access is the maker's and the superadmin's.** There is no GitHub repository to mirror. Sharing
  a playground with teammates needs them to sign in, which needs GitHub today (ACC-005). That is a
  follow-up.
- **Size.** The change is over the 400-line guide. Creation, upload, local merge and the UI are one
  mode: without the merge a playground's tasks could never finish, and without the UI nobody can
  reach the mode. Each slice is its own commit.
- **Review round 3, decided rather than fixed.** The 409 for an integration branch that moved
  between the refresh and the push has no test: the push is refused without forcing by Git
  itself, and staging that race needs a hook in the Git adapter, which this change avoids.
  Concurrent creations by one person can pass the limit of 10 by the few that run at once; a
  template playground is a few KB and an upload holds an upload slot, so the bound still holds
  within the slots, and a per-person lock is not worth its state. A removed playground keeps its
  files and counts until a purge exists (#420).
- **Follow-ups (issues):** #409 CLI onboarding into a playground, #410 moving a playground to
  GitHub, #411 Yulius CI on a playground's heads, #412 teammates without GitHub, #420 purging a
  removed playground's files. Earlier list: `yulius onboard --playground` for a CLI checkout; moving a playground to
  a GitHub repository once GitHub is connected; Yulius CI on a playground's heads; teammates in a
  playground without GitHub.

## Evidence

Run on the branch head after merging `dev` (2026-10-14):

- `scripts/dev ci`: exit 0. Repository contracts passed; nextest `1565 tests run: 1565 passed, 16
  skipped`; clippy with `-D warnings`, rustdoc, doctests, cargo-deny and cargo-shear clean.
- `cd e2e && npx e2e run`: `Test Files 43 passed | 1 skipped`, `Tests 308 passed | 1 skipped` (the
  skipped one needs a model key). `playground.e2e.ts` failed three of four tests against the
  assets before the UI change and passes with it.
- `cargo test -p yulius --test main playground::`: 23 passed, covering PLY-001 to PLY-008, among
  them the review regressions. `a_closed_tab_during_setup_leaves_no_stray_directory` failed 3 runs
  out of 3 without the spawned setup. `an_uploaded_alternate_never_reads_another_repository`
  reproduced the leak before the fix.
- `cargo test -p yulius-app --lib playground` (6), `cargo test -p yulius-git --test git_ops
  a_squash`, and the domain tests `a_playground_done_definition_merges_locally` and
  `a_playgrounds_briefing_merges_locally`: all pass.
- Independent review: three rounds in fresh sessions. Every finding was fixed test-first, except
  the two decided in Decisions (round 3).
- Not tested here: a real browser folder picker or drag-and-drop of a whole directory (the e2e
  drop uses loose files; packing is unit-tested in the page), and a real Claude Code agent
  calling `delivery_merge` in a playground (the MCP tool is exercised through the internal
  router).

