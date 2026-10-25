# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-25

- A turn that starts while Yulius publishes an idle task keeps its "working …" time (SPEC-315,
  task #301, fixes #895). The publish read the task and its times, decided to end (or start) the
  idle task's working stretch, and then wrote it unconditionally: when a prompt made the task
  Working in between, its stretch was cleared and the sidebar's clock started over at the next
  change. The write now applies only while the task is still running with the activity and times
  it read.
- On 6.13+ kernels a session that dies because `yulius.service` reached its own task limit
  (`TasksMax=`) says the sandbox hit its process limit, as on 6.8 (SPEC-316, task #302, fixes #823).
  Those kernels count such a refused fork only in the service's group, which the limit counters did
  not read; they now add it, as they do for the sandboxes' common limit. Limits of slices above the
  service are not counted (ADR-0201, amended).
- Every Claude Code run Yulius starts draws full screen again, so its terminal scrolls up
  (SPEC-310, task #296, fixes #898). Tasks of a project share one Claude config dir, and each
  contained run is pid 2 in its own PID namespace. Claude Code's full-screen boot canary
  therefore took a sibling task's start for a failed one of its own, and after two it turned full
  screen off for the project. In the classic renderer, a viewer who opened the task had no
  scrollback and the wheel nothing to scroll. Task runs and the Superagent (the Cockpit) now set
  `CLAUDE_CODE_NO_FLICKER=1` (ADR-0218).
- "working 1h 20m" no longer restarts when a background shell's end wakes the agent (SPEC-309,
  task #295, fixes #894). Yulius's shell probe could see a `yulius wait` shell gone a moment
  before Claude Code woke the agent; the idle task's working stretch ended and the woken turn
  started a new one, so a task working for hours read "working 37m". A turn the task goes on with
  by itself from the state its stretch ended in (Claude Code's wake-up, work after a stop, newly
  found background work) now continues that stretch; a typed prompt still starts a new one.
- Toasts in a dialog no longer cover its Close (SPEC-305, task #291, fixes #640). The toast region
  moves into an open dialog, and below 768 px it is pinned at the top right, where the dialog's
  Close is. Toasts raised at a wider size ended up there when the window narrowed, and nothing moved
  them: the check that moves them to the other edge only protected the focused control. That check
  now also keeps the dialog's Close clear (the focused control still comes first). It runs again
  when the window is resized, and it moves the toasts only to an edge that covers less. This fixes
  the flaky axe `target-size` failure of the Team secrets test "a failed copy shows no check…",
  which the mock's sign-in request toast set off on slow runs.
- `yulius-ptyhost::supervision shutdown_escalates_past_ignored_signals_and_reaps_descendants`
  reads the descendant's pid only from a finished `CHILD <pid>` row (SPEC-307, task #293, fixes
  #647). The tty sends a line's text before the `\r\n` it makes of its `\n`; when the test typed
  `ping` in between, its echo joined the row (`CHILD 1234ping`) and the pid failed to parse.
- A file refused by the Queued messages box because its task started keeps its notice until the
  person dismisses the card (SPEC-306, task #292, fixes #559). "The task has started: the file was
  not added." went to the upload status, which the kept-card check does not see, so pasted into an
  otherwise empty box it vanished with the card on the task's next update (`starting` →
  `running`). It now goes to the card's error line, as other refusals do, and the upload status of
  a refused batch says "Nothing was added." instead of "Added …".
- A production secret's Edit dialog offers no hosts the server refuses (SPEC-308, task #294,
  fixes #622). It offered the Hosts input on every secret, so a person who added a host to a
  production secret got a 409 on Save. Now its Hosts field has no input and says the secret is
  sent to no host; an older production secret that still has hosts says agents' requests to them
  still get it and offers only "Remove these hosts". The web mock's
  `PATCH /api/agent-secrets/{id}` refuses hosts on a production secret as the server does.
- The Docker front's socket is bound under a umask private to the binding thread (SPEC-304, task
  #290, fixes #825). `Front::start` set the process umask around `bind`, so fronts started at once
  in one process (the crate's unit tests under `cargo test`) could bind a socket under another
  call's mask (not 0660) and leave the process umask at 0117. The bind now runs on a thread that
  unshares `CLONE_FS` first; the process umask never changes (ADR-0217). Production was not
  affected: each front is a process of its own.

## 2026-10-24

- Public lines of a stored key are no longer redacted (SPEC-302, task #288, fixes #599). Every
  line of a key block but its armour was a redaction needle, so an encrypted PEM's
  `Proc-Type: 4,ENCRYPTED` and `DEK-Info: …`, PGP armour headers (`Version: …`, `Hash: SHA256`)
  and OpenSSL's `subject=`/`issuer=` between a bundle's blocks showed as `[secret:NAME]` anywhere
  in the sandbox. Blank lines, named public headers (RFC 1421, PGP armour, PKCS#12) and
  `subject=`/`issuer=` lines are no needles now when their value holds no `:` or `(0x` and no
  key-length base64 run; every other line stays one, so a body joined onto a header or a key's
  colon-hex dump is still redacted (ADR-0215).
- A run lost while Yulius runs saves the screen it was lost on (SPEC-301, task #287, #686). A
  lost connection, or a stop or close whose escalation timed out, recorded the loss without saving
  the hub's screen, so the task's terminal later showed the screen of the last settled turn and
  lost any output after it. `mark_lost` now saves it to the run's `final-screen` before the
  attempt becomes lost; a hub without output (a runtime made after a restart) keeps the saved one,
  and a run lost before Yulius attached to it saves none (its hub still shows the run before).
- The startup reconciliation records an environment run left `running` as `error` before it
  ends its process host and directories (task #285, SPEC-299 TLB-005). Ending them first let a
  watcher still following the run record its end in between, so
  `delivery::a_restart_ends_a_running_environment_command` failed now and then.
- A multi-line value pasted into a team secret's Value keeps its line breaks (SPEC-299, task
  #285, #592). The field is a one-line password input, which drops them, so a PEM key or recovery
  codes were stored on one line and copied back flattened. A paste holding line breaks now carries
  each LF and CR in the field as a private-use character that the form turns back on save: the
  value is stored exactly as pasted (CRLF too) and the field stays masked. A shown value keeps its
  lines, and Copy returns the original text (ADR-0214). Agent secrets keep their literal `\n`.
- "Reload from disk" after a save conflict no longer leaves the person's text marked "Saved"
  (SPEC-298, task #284, #571). It cleared the modified mark before reading the disk, and the read
  kept the text when the disk was back at the tab's version, the file was deleted or no longer
  text. A reload now replaces the text whatever the version, as one undoable edit, and clears the
  mark only then; a file it cannot load keeps the text modified and a toast says why ("The file
  was deleted").
- CD and delivery environment runs are stopped past a disk bound for their files (SPEC-300,
  task #286, #664). A run's tree, `HOME`, states and log, and a CD run's cache, lie outside every
  sandbox directory, so the sandbox's quota never counted them: a deploy could write until its
  timeout and fill the data disk. `ci_watch` now measures them every 5 s, off its loop, and stops
  a run past `--cd-run-disk-gb` (`YULIUS_CD_RUN_DISK_GB`, default 20 GB) as an `error` that says
  what it used; a CD run's cache is then emptied, so the next run starts under the bound. Charging
  the sandbox's quota was rejected: a production deploy must not die because agents filled their
  worktrees (ADR-0216).
- An import's snapshot is checked out in a folder of the server, not in the sandbox's
  `worktrees/` (SPEC-296, task #282, #824). The import picked `worktrees/import-<job>` with an
  existence check before Git created it. A link to an empty folder planted there in between made
  the service user's Git check the head out in that folder and write the snapshot into it, and the
  files stayed there. The checkout now lives in `<data>/import-worktrees/<sandbox>/`, out of
  every sandbox. Its name is taken with a create-new `mkdir` (`import-<job>`, then `-2`, …), a
  failed import removes it, disk upkeep lists what an interrupted one leaves, and
  `extract_snapshot` refuses a linked root (ADR-0213).
- The Feature and Fix workflows no longer tell agents to promote and verify production (SPEC-292,
  task #278, #556). Their instructions ended with "deliver: merge on green, deploy and verify,
  promote, verify production", the Feature description said "from spec to verified production",
  and the AGENTS.md generated for new projects said "promotion to `main` by a merge commit, verify
  production", against ADR-0110 (only a person promotes and deploys production). They now end at a
  merge on green and cleanup, deploy dev only when the task asks (`cd_deploy`), and leave
  production, with its promotion, to a person. Unit tests reject the instruction to promote or
  verify production in every bundled workflow and in the AGENTS.md template.
- The disk-quota wait test runs in CI again (SPEC-294, task #280, fixes #485).
  `local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees` had been quarantined
  since one unexplained 2.16 s failure on 2026-10-15. Random delays at every status publish, at the
  disk check, and at a run's start, job ends and end reproduced no fast failure. Neither did clock
  jumps while the runs execute: 128 runs in all. The late disk-check publish suspected by #277
  cannot reach this test, since no run starts before the check has published. CI keeps a failing
  test's output (SPEC-145), so a next failure names its assertion.
- An open provider sign-in terminal now closes when the sign-in that opened it ends (SPEC-295,
  task #281, #759). It checked administrator access once, at connect, so signing out, a revoked API
  token, an expired session, a disabled user or a removed instance administration left it streaming
  and accepting input until the sign-in ended (up to 15 minutes). Like the task terminals and
  shells, it now re-checks every `viewer_recheck_ms` (60 s) beside the stream and closes with 4003.
- The CI stage-duration test (`local_ci::a_run_s_jobs_record_their_stages_and_test_counts`) no
  longer fails under host load (SPEC-293, task #279, #538). Yulius times a stage from the log read
  that saw its marker to the one that saw the next, and the test bounded those durations by wall
  time while a fake clock ticked along: a starved clock task or a late read gave "480 ms" or a
  zero-long stage. The job now holds each stage until the test, having seen the stage start
  recorded, moves the clock by 1000 ms and releases it, so each stage lasts exactly 1000 ms. Test
  only; Yulius's own timing is unchanged.
- An achieved goal no longer leaves its task Working for good when Yulius did not settle the
  turn's Stop hook (SPEC-290, task #276, #877). The hook client fails open, so a Stop that Yulius
  answered with an error, or could not answer, still ended the turn in Claude Code, and the task
  stayed Working after "Goal achieved". Task #272 read "working 11h 15m". Yulius now records each
  Stop it answers. The record that closes a later turn settles that turn like a clean Stop: Done
  when the goal is achieved, otherwise waiting, with its pull request, chains and queue. The same
  applies to a turn that an error ended with no Stop hook. A record read late, after a newer
  prompt, and a Stop still being answered leave the turn as it is (ADR-0212).
- The remote MCP's `secret_update` and `secret_hosts_answer` act on the secret `secrets_list`
  shows for the name (SPEC-287, task #273, #787): the project's, else the person's own personal
  one, else a global one switched on for the project. They took the oldest secret of the name
  among the project's, every global one and the person's own, so a project writer got 403 for a
  project secret an older global one shared the name of, and an administrator could rebind the
  shadowed global secret. A secret switched off for the project, or another person's, is "no
  agent secret NAME".
- An agent can no longer stop the disk upkeep with a FIFO (SPEC-288, task #274, #853). A
  worktree's `yulius.toml` was checked by path and then read by path, so a FIFO swapped in between
  blocked the read for good, and with it every later disk round of the sandbox; a worktree
  record's `gitdir` was read with no check at all (a FIFO blocked the leftovers round, a link was
  followed). Both are now opened once without following a link or waiting on a FIFO, and read
  only when the handle is a small regular file (64 KiB, 4 KiB); anything else declares nothing.
  The declared caches of an ended worktree are read off the async workers.
- A CI job's log no longer follows a link an agent planted in its sandbox's `ci/logs` (SPEC-285,
  task #271, #663). The server created it with `File::create` by path and removed it by path, so a
  linked `ci/logs`, a link planted between the removal and the create, or `ci/logs` relinked while
  the job ran made the service user create, truncate or delete a host file. The folder is now
  opened from the sandbox directory with `O_DIRECTORY | O_NOFOLLOW` (a link there is replaced by a
  folder), the log created with `O_EXCL | O_NOFOLLOW`, read through that handle and removed with
  `unlinkat` in the folder's handle (ADR-0174).
- Concurrent CI checkouts of one run no longer fail on each other's worktree registration
  (SPEC-289, task #275, #666). `git worktree list` now takes the same process-wide lock as
  worktree add, move, remove and prune; before, a checkout's list could read another checkout's
  half-written `.git/worktrees/<name>/` and die ("failed to read …/locked" or "…/commondir").
- An open event stream follows instance administration (SPEC-286, task #272, #722). It decided
  admin-only events (`account.updated`, with the subscription's account email) once, when the page
  connected, so a person whose administration was removed kept receiving them until a reconnect.
  Each re-check now reads the person again: losing the role stops those events, gaining it starts
  them, within one re-check interval and without a reconnect.

## 2026-10-23

- First-run onboarding says what each step does and what comes next (SPEC-283, task #269,
  ADR-0211). Create GitHub App on GitHub opens GitHub in a new tab (opener cut) instead of
  replacing Yulius; the Yulius tab waits, notices the App, and lists the next steps (install on
  repositories, import or create a sandbox, admit users). The setup wizard is numbered, explains
  the setup token, recovery code and keys, and ends with "Add your first Claude subscription"
  (skippable). Home's empty view lists the getting-started steps with their state. The recovery
  card of setup and of a password reset keeps its frame (only the administration dialog drops it),
  and the GitHub owner toggle keeps its width.
- A `claude` an agent runs in its worktree no longer takes over its task's session (SPEC-281,
  task #267, ADR-0209, #862). Its transcript lands beside the TUI's, and the drift check adopted
  the newest one: watchers saw the nested session ("Not logged in") while the owner's terminal
  showed the real one, and a Continue would have resumed it. Now only the run's own `SessionStart`
  hook changes the task's session (`/clear`, `/resume`), and the lineage is listed in the order
  sessions were adopted, so watchers and resumes use the session adopted last (task #266 included).
- The wheel and the trackpad scroll Claude Code's conversation as far as they scroll a page
  (SPEC-280, task #266, #863, ADR-0210). A task's terminal and the Superagent sent one wheel report
  per wheel event, small trackpad deltas damped, and Claude Code moves one row per report, so a
  notch moved one row and scrolling up a long conversation looked stuck. A plain wheel turn now
  sends one report per row of its distance (pixels over the row height, lines, pages), carrying a
  fraction on in the same direction; modified wheels, pinch-zoom and read-only viewers are as
  before.
- The README is a step-by-step onboarding guide for people who just found Yulius (task #268): who
  it is for, the path from discovery to a first task as a diagram, a checklist of what to have,
  recommended VPS providers and sizes, seven steps marked person or server, and plain explanations
  of VMs, the setup token, the recovery code, Jev and Vidoc with their keys. The developer start
  moved to its own section. The CD agent-state test no longer races the production run's start
  (#864).
- Production deploys get a Docker engine of their own (SPEC-279, task #265, ADR-0208). Each
  sandbox has a production holder (`<sid>.prod`: its own namespaces, network, cgroup, system layer
  over the base and engine storage in `sysroots/<sid>.prod`), started for production-stage runs
  and stopped when idle. Production runs join it on the host's read-only system with its Docker
  directory, so `docker buildx build --load`, `docker load` and `docker run -v "$PWD":…` work,
  with images and build cache kept between deploys and out of every agent's reach; their files
  moved to `<data>/cd/<sid>/runs`. It counts toward the sandbox's quota and is pruned, retired and
  removed with it; `yulius doctor --docker-smoke` proves it on a host, and `cd_status` says so.
- CD runs use agent states (SPEC-278, task #264, ADR-0207). A `[cd.<name>]` names the states
  its deploy needs (`states = ["QUARTIO_OPERATOR"]`): the run restores the vault's newest version
  into its own folder (`$YULIUS_STATE_<NAME>`, production too, never a task's copy) and saves what
  changed as a new version after the deploy and the verify, failed ones included, audited as
  `cd:<environment>`. One writer at a time: another deploy naming it waits, a task's save and a
  laptop send are refused meanwhile, and a task's copy made stale by a CD save is restored before
  it is written (`secrets_list` says `stale` and `held_by`). CD runs get the project's variables
  (production only those its table names, `variables = [...]`). `yulius.toml`
  accepts one-line arrays of strings. Dev runs use the sandbox's Docker and every run reaches SSH
  hosts; production runs get a Docker engine of their own in task #265.
- Demo mode is no longer part of a real instance (SPEC-277, task #263, ADR-0206). The server binary
  leaves out `assets/mock.js`, the e2e suite's fake backend with its sandboxes of Quartio's
  repositories, so `GET /assets/mock.js` is 404, and a page link with `?mock=1` is redirected to
  the same page without it. The e2e suite and local UI work still run on the mock, served from the
  source tree.
- Yulius reclaims disk by itself, and removing a sandbox asks about its files (SPEC-276, task #261,
  ADR-0205). Cargo build directories of worktrees and CI checkouts are swept hourly under Cargo's
  own lock: whole units unused for 48 hours go (compiled artifacts only where nothing runs tests),
  and a directory above 5 % of the disk loses its least recently used incremental state. From 85 % disk use the sweep runs harder every round with a warning
  pill on every page; from 95 % idle caches go too. Archived tasks lose their build output at
  once, and the Leftovers card and `yulius leftovers list` show the last automatic cleanup.
  Removing a sandbox now asks whether to delete its files permanently (as soon as nothing of it
  runs, no CLI step) or keep them; kept files are listed with their size and a Delete files
  button.

## 2026-10-22

- Projects have variables (SPEC-275, task #259, ADR-0203): non-secret values such as a region
  (`OVH_ENDPOINT=ovh-ca`), an environment name (`production`), ids, base URLs and contact
  addresses, shown in plain on the Secrets page's new Variables tab and in every task run's
  environment. Before, such a value was refused as a secret (under 8 characters) or masked every
  occurrence of the word. A laptop agent sets them with `yulius vars set|import|list|delete`
  (remote tools `variables_*`), and `yulius secrets import --to agent` now sends a `.env`'s plain
  values (short ones, and the ordinary text SPEC-274 refuses as secrets, not under a
  credential's name) to variables instead of skipping them. A name that is an agent secret is
  refused; every change is audited without its value; `secrets_list` and the onboarding prompt
  name them.
- Agent secrets no longer mask ordinary text (SPEC-274, task #258, ADR-0204). Onboarding
  Quartio-Studio/product, a company `.env` imported with `yulius secrets import --to agent --yes`
  held `PIPEDREAM_PROJECT_ENVIRONMENT=production`, and every "production" read `[secret:…]` in 221
  files, the project's config and the hand-over. The import now skips values that are ordinary
  text (configuration words, host names, e-mail addresses, URLs without credentials) and names why;
  run in the project's checkout it asks about values its tracked files hold (`--yes` skips them).
  The person's agent deletes a secret imported by mistake: `yulius secrets delete NAME` and the
  remote tool `secret_delete`, audited with the token, never the value. The onboarding hand-over
  says a masked word is a secret's value. Secrets already imported and stored hand-overs are
  unchanged.
- The Vidoc security job makes less noise (SPEC-273, task #262, ADR-0202, #850). A reworded
  repeat of an ignored finding on code the branch did not touch passes: an `[[ignore]]` may name
  its `function` and `line`, Vidoc is told each file's accepted risks and can answer "accepted",
  and every open finding shows its nearest ignore and how similar it is. An answer cut at
  `max_tokens` splits its chunk instead of failing the scan; files Vidoc's outage left unscanned
  make the scan incomplete (exit 3): in CI the job ends error "incomplete: …" naming them, which
  blocks the merge until a rerun passes, instead of skipped or failed. The security job and the
  tests no longer cancel each other, and issues for findings outside the change are filed only
  from a run whose other jobs passed.
- Yulius cleans up stale branches and worktrees itself (SPEC-269, task #255, ADR-0199). The
  6-hourly hygiene round now removes what loses nothing (branches of merged or closed pull
  requests, local copies already on GitHub, clean worktrees of finished tasks) instead of only
  listing it; unmerged work stays for a person. The CI tab's card says this and when the next
  round runs; Clean up turns the card busy at once ("Removing 3 items… 2 s so far") and then
  shows what was removed and what was kept and why, on the card, also after a reload. Before,
  only a short count toast followed a silent wait.

- Laptop onboarding moves a project's operator state into its sandbox (SPEC-271, ADR-0200, task
  #257). A deploy tool's live state (Quartio-Studio/product's `~/.quartio-operator-20260901`: a
  SQLite inventory, rollout folders, configs) could not move: `--from-file` takes 64 KiB of one
  value. `yulius secrets import --state NAME=DIR` now sends a folder (or a `.tar.gz` of one, up to
  64 MiB packed) as an agent state, sealed per version in the vault and audited as the person; a
  task restores it into the sandbox home with `secret_state_restore` (through folder handles,
  modes kept) and saves it back with `secret_state_save`; `yulius secrets state-get` downloads it.
  Once restored the sandbox's copy is the only writer: a second send is refused (409) unless
  `--replace-state`. Debug and test builds now optimize the vault's cipher, gzip and yulius-store,
  so a 50 MiB state takes a tenth of a second to seal instead of ten.
- All sandboxes together now keep the server a quarter of `yulius.service`'s tasks (SPEC-272,
  task #260, ADR-0201): `<unit>/sandboxes/pids.max` is three quarters of the service's limit
  (43 320 on the VM), written at every start and reset to `max` without one, so two busy sandboxes
  can no longer leave the server unable to fork. A session that aborts at that common limit still
  says "the sandbox hit its process limit" on kernels with `pids.events.local` (6.13+), and an
  explicit `YULIUS_SANDBOX_PIDS` above it is logged. Also from #254's review: the end banner keeps
  keyboard focus on Continue / Try again when the task's details are read again, a knob set through
  a dotted key (`RUST_TEST_THREADS.value`, `build.jobs`) keeps its value, and a paused run's group
  that met a limit is no longer removed while its counts matter.
- Parallel tasks of one sandbox no longer use up its process budget and crash Claude Code (SPEC-268,
  task #254, ADR-0198). Five quartio-v2 tasks building Rust and running tests with containers
  shared the sandbox's 8192 tasks (threads count), and a session that could not start a thread
  aborted ("Stopped (crashed)", exit 134). The default `pids.max` is now a quarter of the host's
  task table, at most three quarters of `yulius.service`'s own limit (43 320 on the VM's default
  unit; `YULIUS_SANDBOX_PIDS` still sets it); CI jobs and the Docker engine
  with its containers each stop at 3/8 of it, so the sessions keep a quarter. A session that still
  ends while the sandbox refused forks or the OOM killer ran says "the sandbox hit its process
  limit" or "ran out of memory" in its banner and overview (`limit=pids`/`limit=memory` in the
  run's exit reason). Repositories without `[resources.env]` get `CARGO_BUILD_JOBS`,
  `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` at their budget, unless they set that knob in
  `.cargo/config.toml` or `.config/nextest.toml`. Per-container `--pids-limit` joins the
  unsupported Docker flags.
- The token form on Settings → Connect your local agent lines up (SPEC-270, task #256). The Scope
  hint bottom-aligned the name field and a small Create token button to its last line, so the
  labels and controls sat on different lines. Each field now spans label, control and hint rows
  (CSS subgrid), the button takes the control row at the fields' height, the scope select has the
  app's `.select` look, and the form stacks when it is narrower than 600px (a phone, or a window
  up to about 900px beside the sidebar).
- A task reads another repository of its project's GitHub owner without asking a person (SPEC-267,
  ADR-0197, task #253, fixes #803). A task that had to port from Quartio-Studio/product asked a
  person to give the GitHub App access, because an agent's token reaches only its own repository;
  the person's Superagent then showed nothing was needed. The new `reference_repo` tool fetches the
  repository's default branch as the task's owner (after GitHub says they can read it) into a
  private folder, bundles it and attaches the bundle to the task, which clones it into
  `<sandbox>/reference/<owner>/<name>`. Another owner, no read access, a disabled owner and a
  playground are refused, and only then does the agent ask with `needs_you`. Onboarding prompts
  now fetch the repositories a project refers to and name them in AGENTS.md.
- The usage-limit step-back and the task home's creation go through handles (SPEC-266, task #252,
  fixes #661). The transcript `<home>/.claude/projects/<slug>/<session>.jsonl` is reached from
  the folder above the home without following links, read only when it is a regular file, and
  replaced by a random new file renamed within its folder, with the old permissions. A link on the
  way or at the transcript now fails the step back (the resume goes on), where Yulius used to
  read the link's target and replace the file it led to (or the link itself); a FIFO there no
  longer hangs the run's start. A task's home
  is made with `mkdirat` from its sandbox directory, so a `home` link fails the start.
- The editor's delete, folder creation and listing, and the removal of task, shell and draft
  attachments, go through folder handles (task #251, #660, SPEC-265, ADR-0174). A contained shell
  that swapped a folder on the way for a link between the server's check and its action could make
  a delete remove a host file or tree, a new folder land outside the sandbox, a listing show a host
  folder, or attachment cleanup remove host files; each now opens its folder from the sandbox
  directory without following links and fails with `403` (cleanup removes nothing). Pause-point
  tests cover each race.
- The disk upkeep no longer follows links into another sandbox (SPEC-263, task #249, fixes #649).
  A sandbox that replaced its `worktrees`, a worktree entry, `ci` or `ci/jobs` with a link to
  another sandbox's folder got that sandbox's caches pruned as its own. Worktrees, CI checkouts
  and caches now count only with no link from the sandbox directory down, each cache is checked
  again right before it is measured and removed (through `O_NOFOLLOW` handles, so on Linux only;
  macOS development hosts no longer prune caches), and an ended worktree right before Git removes
  it. Git's own worktree removal still resolves paths itself (noted on #649).
- The Vidoc findings tracked in #639 are triaged (SPEC-262, task #248, fixes #639, #688, #697,
  #714). Real ones are fixed test-first: the store refuses, in each compare-and-set, a production
  allowance on a secret agents copy into `.env` files or have hosts for, and the reverse, so two
  managers answering at once can no longer leave a secret both; audit reasons drop U+2028/U+2029
  and every invisible format character; the status line drops control characters of repository
  and branch names; document presence and the workflow count for the briefing read through a
  checked handle, so a folder swapped for a link is not followed. The rest (the `.env` target's
  Git-ignore check, unparseable stored host requests, the `.git` exclusion of dotenv targets,
  task-number allocation) are dismissed with their reasons, those whose Vidoc wording is on record
  as `[[ignore]]` entries.
- A CI job gets the agent secrets the integration branch grants only when it runs the granted
  entry's command too (task #250, #659, SPEC-264, ADR-0196). A pull request tested with its own
  `[[ci.jobs]]` that reused a granted job's name, say `deploy`, for its own command ran that command
  with the secret's placeholder and proxy; it now runs without any secret, and its output says
  `this job's command is not the one the integration branch grants them to`. A pull request that
  keeps the granted command keeps the grant.
- The documentation matches what was merged since 2026-10-17 (task #246, documentation only). The
  API reference lists the agent-secret scopes (`personal`, `enabled`, `shadowed_by`) and the
  switch route `PUT …/agent-secrets/{id}/enabled`, the leftovers and Sandbox Docker admin routes,
  the health and `yulius leftovers` internal routes, the composer's draft attachments, the
  update's Docker re-check, the stop's 409s and how a lost run closes its task. Onboarding started
  by an `operate` token, `yulius secrets import --yes`, `--from-file` and `--env-files`, the
  remote CLI and dev-stage `cd_deploy` are described wherever the old rules were (operations,
  product, architecture, remote control, ADR-0038 and ADR-0163 now link their amendments). The
  architecture's top diagram puts the sandbox browser outside the holder, on its DevTools pipe;
  the Superagent's tools, hook and `task_message` reach are corrected; the Vidoc key loses to any
  agent secret of that name that reaches the run; named CD environments' files, the
  production secrets rule and upgrade notes for migrations 0035, 0037, 0039, 0040 and 0041 are in
  operations. Four architecture sequence diagrams that did not render (a `;` in a message ends a
  Mermaid statement) render again; all 54 diagrams pass Mermaid 11's parser.

## 2026-10-21

- A CI run's and a deploy's summary keeps its content off its edges (SPEC-259, task #245, fixes
  #776). The sticky summary was a flat band with no inline padding, so its state, its facts and
  "In Pipeline" touched the band's edges; it is now a card like the CI/CD page's Now line (12 px
  inline padding, border, radius). Its loading shape has the loaded head's buttons (two for
  writers, one for readers) and no reason bar, so on a phone it wraps as the loaded head does.
- CD takes more environments than dev and production (SPEC-257, ADR-0195, task #243). A
  repository that ships more than one thing, like a self-hosted release and the hosted platform
  that serves it, or a staging machine next to dev, defines one `[cd.<name>]` table per target
  with `stage = "dev"` or `stage = "prod"`. The stage carries the rules `[cd.dev]` and `[cd.prod]`
  had: the branch, who may deploy (production stage: only an administrator who types the slug,
  never an agent), the containment and the secrets. `[cd.dev]` and `[cd.prod]` stay the defaults
  and work as before. The CD page, `cd_status`, `cd_deploy` and the API list and deploy every
  environment with its stage; runs keep it (migration 41). CI was already general
  (`[[ci.jobs]]`).
- The CI page's run list no longer shows "Run again" on every card (SPEC-258, task #244, #774).
  A run is tested again from its own view, which opens the new run; the view also has Cancel while
  the run is queued or running, and an active card keeps Cancel. Read access sees neither.
- "Onboard this into Yulius" to a person's local Claude Code is enough (SPEC-256, ADR-0194, task
  #242). The operate token made for their agent now also signs in `yulius onboard` and `yulius
  secrets import`, so no browser approval is needed. `yulius secrets import --yes` sends without
  the typed yes, only with that token. `--from-file NAME=PATH` imports a key file by its path, so
  the agent never reads the key. `--env-files` creates the secrets already allowed into `.env`
  files (`env_files` on create), where ssh and deploy scripts read them. `yulius project onboard
  --notes` (`onboarding_start {notes}`) adds the agent's hand-over, redacted, to the onboarding
  task's request before it starts. The onboarding task reads `secrets_list` and that hand-over
  before asking a person for a credential. The remote MCP's instructions and the
  [guide](remote-control.md) describe the steps. Signed in to one instance with `yulius login`
  and another with `yulius remote login`, pass `--server` to `yulius onboard`.
- A stop during a switch always ends the task (SPEC-250, task #236, fixes #527). A stop that came
  as a switch completed could leave the task `running` with every run ended, and a stop that came
  as a run ended could leave it `stopping` with no run. Stop changes and run-end status changes
  are now made again from the status read anew when they lose a race. A switch overtaken by a stop
  starts no new run. This also fixes the intermittent `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn`
  failure in Yulius CI.
- The sandbox browser has no DevTools port any more (SPEC-252, ADR-0191, task #238, #626).
  Chromium used to listen for the DevTools protocol on a loopback port without authentication, so
  any local process, including a run of another sandbox under the fallback containment, could
  drive a sandbox's browser and its logged-in sessions. It now speaks DevTools only over a pipe
  that Yulius holds (`--remote-debugging-pipe`). Agents keep using Yulius's browser tools; the
  unused raw CDP endpoint for a sandbox's MCP servers is gone. The Vidoc ignores that pointed at
  #626 are removed.
- The Claude config dir is prepared through handles (SPEC-253, ADR-0193, task #239, fixes #634).
  Before a launch, Yulius opens `<home>/.claude` from the folder above the home without following
  links. A home or config dir the agent replaced with a link now refuses the launch and names it,
  where Yulius used to read and replace `.claude.json` and `settings.json` in the link's target.
  `plugins/store` linked elsewhere is left alone. A linked worktree's main repository comes from
  its `gitdir:` path alone, read lexically: the host no longer opens or resolves paths the agent
  wrote there. The two Vidoc ignores that pointed at #634 are removed. Still by path: the
  transcript step-back (#661).
- "Work through open issues" no longer leaves out most of the core team's issues (SPEC-251,
  ADR-0192, task #237, #594). GitHub reports `CONTRIBUTOR` for a repository admin outside the
  organization, and an agent's issue filed with its person's token is not by a bot, so only 2 of
  109 open issues were in scope here. A person with `write`, `maintain` or `admin` permission
  (asked once per author) is now in scope, also for the issues agents file through the App on
  their behalf, and so is an issue this project's Yulius App's bot opened with the
  `Opened by task #<n> in Yulius.` footer; contributors and outsiders without write access stay
  out, whatever App they file through. An issue showing exactly 100 comments is read in full.
  Projects that added the automation earlier keep their copy of the prompt; this project's gets
  the new one after the merge.
- The Superagent's `secret_copy_to_env` no longer follows a sandbox repository swapped for a link
  (SPEC-254, task #240, #635): the copy walks from the canonical sandbox folder to `repo` (a task's
  worktree for a task) and on without links, and runs Git there, so a `repo` replaced by a link to
  another folder is refused and an allowed secret's value is written nowhere. Task copies are
  unchanged.

## 2026-10-20

- Connecting a Claude account is the first step before anything runs an agent (SPEC-246, ADR-0190,
  task #232, feedback from an early user whose first job failed without one). With no usable Claude
  account, Home shows Connect Claude in place of the playground and Import, the composer and the
  onboarding gate say so, and every start a person asks for (New task, Continue, Start now, Start
  onboarding, the Superagent) is refused at once with `claude_account_required` and a link to
  Subscriptions, instead of a task that waited as "no subscription has capacity left". A browser
  import then adds the sandbox and waits for Start onboarding; an onboarding start that failed for
  want of an account is offered as Retry onboarding once one is connected. Tasks started by a chain,
  a rule or an automation still wait and now say no account is connected.
- Yulius can be driven from a laptop by a local agent or the CLI (SPEC-247, ADR-0188, ADR-0189,
  task #233; an early external user's most important point: "make a task, restart it" from their
  own agent). Settings → Connect your local agent makes a personal API token, `operate` or
  `read`. It is shown once with its `claude mcp add` and `yulius remote login` commands, stored
  hashed, listed with its last use and revocable. The instance serves a remote MCP at `/mcp`. A
  person's Claude Code imports a repository with it, starts its onboarding, reads files, sets up CI
  through a change pull request merged once Yulius CI passed, and manages tasks, CI, CD (dev),
  automations and secret bindings, within the person's own access to each project. Requests are
  limited to 120 a minute per token, and changes are audited as `remote:<login>`. The new `yulius
  remote|project|task|ci` commands call the same tools. `yulius login` tokens keep working as
  before and cannot start onboarding. [Guide](remote-control.md).

- Secrets scopes, review follow-ups (SPEC-248, task #234): a hand-over rewrites the run's
  fingerprint file for the new owner, so no run keeps hashes of the previous owner's personal
  values; the 200-secret limit per project is counted in the store transaction, so concurrent
  switch-ons cannot pass it; an agent run's egress client no longer carries a frozen person
  (`EgressClient::holder()` removed); tests for the new owner's secret after a hand-over and a
  deleted task's client; SPEC-245 and the ADR index corrected.
- Claude's sign-in confirms itself (SPEC-243, ADR-0184, task #229). After a successful sign-in
  the dialog stayed open over the terminal, so it was unclear whether it had worked. Now the
  dialog closes by itself and a toast says "Claude #3 is connected as …", with the email of the
  Claude account read right after the sign-in (SPEC-244) when Claude shares it. A failed, cancelled or lost sign-in shows why, with Try again in the same
  dialog. Closing a sign-in that still runs cancels it on the server
  (`POST /api/admin/accounts/login/{lid}/cancel`) instead of leaving it running for 15 minutes.
  A token Claude already printed is kept even then. A CLI that runs on after printing its token
  is stopped after 5 seconds and the token is kept. A sign-in the server no longer has is said at
  once instead of being reconnected to for minutes.
  Failure reasons read "exit code 1" instead of "Some(1)".
- Agent secrets have three scopes: Project, Global and Personal (SPEC-245, ADR-0186, task #231),
  after an external user found the old scopes pointless. Global secrets are defined once by
  instance administrators and switched on per project; personal ones belong to one person, are
  switched on by them per project and reach only tasks they start (not other people's, not
  automations, CI or deploys), only through the egress proxy. A name resolves project, then
  personal, then global. Team-wide agent secrets became global ones switched on for every
  project, so no run lost a secret. The Secrets page opens on Project; Team secrets is the fourth
  tab (`?section=team`).
- A new GitHub repository with no commits can be onboarded (SPEC-241, ADR-0187, task #227). Starting
  its onboarding failed with "git: revision not found: origin/master": the empty clone named the
  server's own default branch, and there was no branch for the onboarding task to start from. Import
  now records GitHub's default branch, and starting onboarding pushes one commit adding `README.md`
  to it first; a repository with commits is never written to, and a sandbox already recorded with
  `master` is corrected to GitHub's branch. Creating a project now says what
  GitHub refused: the App permission it lacks, in GitHub's words, with the App's permission and
  installation pages, or, for an installation on only selected repositories, that the repository was
  created and where to add it before importing it.

- Subscriptions can be renamed, and administrators see the email of the Claude account behind each
  one (SPEC-244, ADR-0185, task #230, feedback from an early user). On the Subscriptions page, the
  pencil or a click on the name edits it in place; Enter saves and Escape cancels. The email is
  read from Claude's profile with the subscription's own credential when it signs in. The
  background usage round reads it once for subscriptions signed in before. Administrators see the
  email next to the name on the Subscriptions page, in the Status panel, on the Dashboard, in Task
  details and in the analytics. Everyone else sees the name only. Claude may refuse the profile to
  `setup-token` sign-ins; such a subscription reads "email unknown", and the name tells it apart.

- The superadmin password needs 8 characters instead of 12 (SPEC-242, ADR-0183, task #228). An
  early external user could not set up an instance with the 8-character password they wanted.
  Setup and recovery now accept 8 characters or more (NIST SP 800-63B) and refuse a 7-character
  one with "the password needs at least 8 characters". The forms recommend 12 characters: a
  shorter or repetitive password gets advice that does not block it. No composition rules or list
  of common passwords. Login rate limiting and the recovery lockout are unchanged. The mock now
  also serves "Forgot password?" while signed out.

- A finished automation run is Done instead of waiting under Needs you (SPEC-240, ADR-0182, task
  #226, fixes #719). Automation runs created their task in prompt mode, without a goal. So a run
  that opens no pull request, such as "Work through open issues", could never meet the Done rule
  (an achieved goal, or a tested pull request) and read "Your turn" until a person archived it.
  Every run's task is now a goal task, as the onboarding preparation already was: it is Done once Claude Code
  records its goal as achieved. The ready-made "Work through open issues" now says in its first
  paragraph, which its goal keeps, that the run ends with its summary without waiting for the tasks
  it starts. Tasks that are already finished in prompt mode (#217) stay as they are: archive them.

## 2026-10-19

- The CI/CD page no longer tells a person to allow a production secret that cannot be allowed
  (SPEC-237, task #223, fixes #704). A secret that agents use, through `.env` copies or proxy
  hosts, never goes to production (SPEC-134), and the Secrets page offers no Allow for it. The CD
  page still listed it as "waiting for a person to allow it". Its production state is now
  `agents_use`: the page reads "agents use it: production never gets it" and explains that
  production needs a separate secret, and a refused production run gives the same reason. The
  redaction of a secret whose value is an ordinary word ("production"), seen in the same report,
  is #705.
- A CI run and a deploy open on the CI/CD page itself instead of in a dialog over it (SPEC-238,
  ADR-0180, task #224). The run replaces its tab's list at its own address, with "All runs" to go
  back, so Back, Forward and the sidebar's CI/CD link return to it. Its summary stays at the top
  while the log scrolls and shows the time so far, ticking, the stages running now and a CI run's
  jobs. Above the tabs, on both, a Now summary lists every CI run and deploy that is running, with
  its time, and links to it; when nothing runs it says how the latest CI run and deploy ended.
- The task terminal no longer garbles when an agent prints a secret's value (SPEC-239, ADR-0181,
  task #225, part of #705). The terminal used to replace the value with `[secret:NAME]`, which is
  usually wider than the value. Claude Code redraws its screen by moving the cursor to rows and
  columns it computed from what it wrote, so the extra width sent every later redraw to the wrong
  cells. In the reported project this happened on most answers, because a secret's value was the
  word "production". The terminals (task, Superagent, Cockpit shell) now show the label fitted to
  the value's width: padded with `*`, or cut (`[secret:P]`). Transcripts and tool output keep the
  full label. Whether an ordinary word should count as a secret at all stays on #705.
- Closing the last file, switching folder or leaving the Editor while Monaco's editor worker still
  loads its script no longer reports an uncaught NetworkError on the page (SPEC-235, task #221,
  fixes #509). Monaco ends that worker as its last file goes; the cut `importScripts` threw an
  error nobody cancelled, which failed `editor.e2e.ts` under load. A worker the Editor has ended
  now cancels its errors; one in use still reports them, and `failOnPageErrors()` has no allowlist.
- The Notes tab grip's hover check in `notes.e2e.ts` hovers the tab itself, not a point read
  before (SPEC-236, task #222, fixes #503). The failure was the sandbox browser's sign-in toast
  over the Notes tabs, which pauses while pointed at. #523 had already turned that toast off for
  every notes test. The case now hovers only once nothing covers the tab, and opens the mock with
  `ambient=0` (no ambient activity).
- The supertask proposal no longer says "It replaces “After #12”" once #12 has left the task list
  (SPEC-234, task #220, fixes #587). If the task chosen in "Run after" is deleted or archived while
  the card is open, the start chip falls back to "Start now" and the card now closes, leaving the
  keyboard in the prompt. Before this fix the card stayed open, and "Start as chosen" started the
  task right away, which the card did not say. The split card cannot get into this state, because
  it never opens with a "Run after" task or a rule.
- A watcher of someone else's waiting task reads "The agent is waiting for @owner's reply." or
  "The agent needs @owner's attention." instead of being told to type in a terminal they do not
  have (SPEC-233, task #219, fixes #576). The owner's texts are unchanged.
- A leftover tree holding a directory its owner cannot read (mode `0o000` or `0o300`) is removed
  instead of failing with "Permission denied" every round (SPEC-232, ADR-0179, task #218, fixes
  #680). It mattered without an isolation and for the service user's own trees outside
  `sandboxes/`. Such a directory is now opened as an `O_PATH` handle, given `0o700` through it and
  walked, still never following a link.
- The Superagent can message tasks (SPEC-230, ADR-0178, task #216). Asked to "tell #5 it may use
  the DO budget", it calls `task_message`. A task that has not started gets the message in its
  request, as a person's queued message. A running task gets it typed into its session, labelled
  "Message from alice (through their Superagent)", and its agent reads it even mid-turn. The
  tool never types over a person who typed there in the last minute, before Claude Code's
  session started or into its folder-trust question, and a task in any other state is refused
  with what to do instead. Task agents are not given the tool.
- A reboot no longer turns finished tasks into "Stopped (server restarted)" warnings with a blank
  terminal (SPEC-229, ADR-0177, task #215, fixes #685). After the host rebooted on 2026-10-18, every task with
  a session read that way, including #213 and #214, whose goals were achieved and pull requests
  merged. A run lost while its task was Done now closes the task; every settled turn saves its
  screen, so a lost run's terminal shows how its last turn ended; and the message for a session a
  restart ended no longer says the agent was working.

## 2026-10-18

- The automatic leftovers round deletes a removed sandbox's `ci`, worktrees and browser profile,
  and a live sandbox's stale CI checkout, even when CI jobs wrote files there as the sandbox's
  sub-uids (SPEC-228, ADR-0176, task #214, fixes #675). Settings → Instance → Leftovers showed
  "Failed: …/ci: Permission denied" after every round. Every leftover below `sandboxes/` is now
  removed as root of the sandbox's mapped namespace, walking down from the data directory without
  following a link, and a removal that fails names the file that stayed.
- Settings → Instance no longer says "sandboxes may have lost Docker" twice when they have not
  (SPEC-227, task #213, fixes #674). An update helper unit from before SPEC-203 keeps `/etc`
  read-only, so the re-check failed its needrestart rule, modules list and drop-in items while
  its own "sandbox Docker" verdict held. The headline now follows that verdict ("Sandboxes get
  Docker; run `sudo yulius host setup` on the server"), the update's outcome is one line pointing
  to the Sandbox Docker card instead of repeating its list, and a failed host write names its
  file once.
- One Start onboarding shows one toast again (SPEC-224, task #210, fixes #581). A sandbox page the
  person had already left (it had opened the onboarding task) still rendered once the
  superadmin's recovery state answered, and took back the opened task's "Onboarding started:
  opened #1."; the start then said "Onboarding started." as well. A page that is gone now acts on
  no late answer. The two end-to-end tests of #581 wait on what they mean under load: the watcher
  test gives each of its three self-paced reads its own 10 s, and the onboarding tests wait for
  the start's own read of the tasks in the sidebar (the document title follows task events only).
- The sidebar's hover overlap checks in `navigation.e2e.ts` no longer fail on a loaded machine
  (SPEC-222, task #208, fixes #515). The mock's ambient activity starts task #8 about 12 s in,
  which regroups the sidebar. A `hover()` whose mouse move landed just after the regroup hovered
  the row that moved in, so the check read "Checkout API" at rest (18 px over its menu). The mock
  now takes `ambient=0` (no ambient activity at all), the test uses it, and both checks wait for
  the hovered row's control to show and poll the overlap.
- Two update checks sent together compare `main` once, and the update view never goes back to an
  older read (SPEC-226, task #212, fixes #502). A "Check now" that waited for a check in progress
  (two tabs opening the instance page, or the hourly check) reuses its result when it is younger
  than 30 s; in the browser, an answer to an older `GET /api/update` arriving after a newer one is
  dropped instead of showing the older state until the next change.
- The test of a launch `/goal` watched again after a restart runs in CI again (SPEC-223, task
  #209, fixes #514). It was quarantined as flaky under load; the cause was its own setup: it
  stopped the first server once the run was recorded, not once it was spawned, so under load the
  stop cut the spawn, the next server found no run it could reattach and ended it as lost, and no
  watch could flag it. The test now waits for the spawned run. No product change.
- Undoing past a pasted image takes it out of the composer's tray again after an IME composition
  that changed nothing (cancelled with Esc) (SPEC-225, task #211, fixes #513). Such a composition
  marked the history as typing, so the next keystroke replaced the paste's undo step and the
  upload, ending later, was attached to every step and sent after the text. A record that changes
  nothing now leaves the history alone, and typing updates its step in place.
- Yulius no longer follows links an agent plants in its own sandbox in the five places of #657
  (SPEC-220, ADR-0174, task #206). Editor reads and saves, uploads and terminal attachments,
  imported transcripts and skill copies open their folder once, without following links, and do
  everything else through that handle; files get random temporary names created exclusively.
  Before, a link swapped in between a check and the use, or planted at a fixed name, could show a
  host file in the editor, place an upload outside the sandbox, or overwrite a host file such as
  the database. The same class remains, tracked, in editor deletes and listings (#660), the
  transcript step-back (#661) and the config dir's preparation (#634).
- Delivery environment runs run contained, from a mirror only Yulius writes (SPEC-219, ADR-0175,
  task #205, fixes #656). An environment's command no longer runs on the server in the sandbox's
  clone, where a Git filter an agent planted in the clone's configuration ran during the checkout
  and the command itself ran uncontained: an agent's `delivery_deploy` could run code on the host.
  A run now fetches its branch's head from GitHub into `<data>/cd/<sandbox>/environments.git` (the
  token never passes through the clone's configuration), writes its files into the run's own
  directory with a fresh `HOME`, and runs the command through the process host in the sandbox's
  containment, as CD dev runs do (production environments as CD production runs). It times out
  with the CI job timeout, and a restart stops a running command. A deploy command that relied on
  the sandbox's home, or (for production environments) on host paths, must change: its `HOME` is
  empty at every run. CI, CD and environment runs also no longer let a job point the log Yulius
  reads at another file, and a run's files are written without the server's own Git filters.
- The MCP Test no longer runs commands from the repository's `.mcp.json` on the server (SPEC-218,
  ADR-0173, #654, task #204). Agents can write that file, and Yulius imports its servers; an
  instance administrator pressing Test, or just opening a server's page from a link ending in
  `?test=1`, ran an imported server's command on the host, outside the containment. Imported
  servers now say "From .mcp.json" on the MCP page, and their Test is off (also for an imported
  HTTP server's private or local address) until a person changes the command line and enters its
  environment again, or changes the URL. Turning a server on or off does not count. Servers
  imported before the update are marked too, so a person changes such a server once to test it.

## 2026-10-17

- The Vidoc scan blocks medium findings in a change, and every project scans in CI (SPEC-217,
  ADR-0172, task #160). Critical and high findings still fail the scan anywhere in a scanned
  file; medium ones now fail it on the lines the branch changed (3 lines of slack, all of a new
  file). Low ones there are listed for the agent to decide (fix, or say why not in the spec's
  Evidence). Medium and low findings in code a pull request did not touch no longer just scroll
  by in the log: Yulius files each as a GitHub issue once (`area: security`, `source: vidoc`),
  reopens it while Vidoc still reports it, and closes it once its line is gone from the
  integration branch. Every project whose sandbox has a Vidoc key (its own or the instance's)
  gets a built-in `security` CI job unless its CI already scans; onboarding no longer adds one.
  The first full Vidoc audit of the repository (269 chunks) fixed a GitHub sign-in takeover by a
  reused login (#653) and an editor save that wrote through an agent-planted link (#655). It filed
  three host-escape issues with fix tasks queued (#654 MCP stdio Test, #656 environment runs, #657
  link-following writes), and recorded 39 false positives with their guards in `.config/vidoc.toml`.
- Agents set up a new agent secret themselves, and every secret belongs to a sandbox unless
  team-wide (SPEC-216, ADR-0171, task #203). Adding an agent secret, "Let agents set it up" is on:
  the hosts an agent works out are bound at once and production deploys get it, with no person
  confirming; turned off (when adding it or in Edit), a person allows each as before. Secrets
  added earlier keep their confirmations. Team secrets now have a scope like agent secrets: the
  open sandbox's (the default, seen only by people who can view it) or team-wide. At the first
  start after the update, every team secret becomes one copy per sandbox (their earlier activity
  is then shown to instance administrators only); agent secrets an administrator made team-wide
  stay team-wide.
  `yulius secrets import --to team` imports into the checkout's sandbox, `--instance` team-wide.
- Leftovers are found and cleaned (SPEC-215, ADR-0170, task #202, fixes #625 and #582). A removed
  sandbox's worktrees and CI checkouts were never purged (the purge read only live sandboxes), and
  nothing ever deleted its system layer with its Docker images, its holder state, CD files or
  browser profile. Finished ("done") tasks kept 11–23 GB of `target/` each. Yulius now has an
  inventory of every path it writes, with an owner and a rule for each. It deletes the safe
  leftovers by itself at every start (so after every update) and every hour, checking each rule
  again right before deleting and auditing every deletion. A removed sandbox's disposable parts go
  10 minutes after the removal once nothing of it runs (its system layer once its holder has
  stopped), and a done task's caches go after 30 quiet minutes. Partly cleaned sandboxes are
  finished (stale Git worktree records go). Nothing is reached through a link a sandbox could plant.
  Its clone, sessions, transcripts and export stay, as one item
  only the superadmin deletes. Settings → Instance → Leftovers and `yulius leftovers list|delete`
  list and delete them by hand.
- An answered secret request ends the task's "Needs your action" (SPEC-213, task #200, fixes #633).
  An agent waiting for a person to allow a `.env` copy, a CD secret or a secret's hosts flagged its
  task, and the flag outlived the answer: the task read "Needs your action", with "The agent needs
  your attention" over its terminal, while its agent worked on for minutes, until the turn ended.
  Allowing or dismissing the last request a task waits for, or deleting the secret, now makes it
  Working at once; a task with another request waiting, or flagged by something else, keeps its
  flag.
- Agents create agent secrets from a file without seeing them (SPEC-211, ADR-0167, task #198). A
  Superagent that had to store an SSH key sitting in `~/.ssh` asked the person to `cat` it in their
  own terminal and paste it into the Secrets page. The new tool `secret_create_from_file`, for
  tasks and the Superagent (asking the person first on an uncontained server), takes a name and a
  file in the work folder or `~/`. Yulius reads the file itself (no absolute paths, no links, a
  regular file of at most 16 KiB) and stores it as an agent secret of the sandbox: a key file with
  literal `\n`, as the Secrets page does. The secret starts with no hosts and no `.env` allowance,
  an existing name is never replaced (nor `VIDOC_API_KEY`, which would hide the instance's Vidoc
  key), the person's write access is required, the audit names the agent, and the file is deleted
  only when asked. The CLI import shares the conversion: a quoted value whose only line break ends
  it is now stored without it, and spaces around a key's lines are dropped.
- A goal task's first message is the person's whole request (SPEC-212, ADR-0168, task #199). It
  read `/goal Everything requested in /var/lib/yulius/…/tasks/1.md is done. Done = …`, so the
  request hid behind a path in the sandbox home. The launch `/goal` is now the request as written,
  its lines kept and each attached file at its place as `[file N: <path>]`, followed by the
  definition of done, which now also says what the closing summary holds. Claude Code 2.1.292 has
  no other goal entry point, and its goal's judge reads the SessionStart briefing (measured
  against a mock API). So the rest of Yulius's rules stay in the briefing, which now says to open
  attached files with the Read tool. A request past Claude Code's 4000-unit cap is cut and points
  at the request file. A goal typed into a resumed session is one line of at most 700 units. It
  could reach 2000 characters before, and Claude Code takes a write of about 800 as a paste, which
  sets no goal.
- Claude Code sessions answer concisely (SPEC-210, ADR-0166, task #197). Task runs and the
  Superagent start in Claude Code's built-in Concise output style: the first sentence gives the
  result, without lead-in, narration or recap, while the work stays as thorough. Each launch adds
  `"outputStyle": "Concise"` to the sandbox's Claude Code user settings when they name no style;
  `/output-style` still switches a session (Claude Code keeps that choice for the worktree, so
  the Superagent keeps it and a new task starts concise again), and a style a repository commits
  in `.claude/settings.json` wins. Before a launch the host now opens the files of the sandbox's
  Claude config and the worktree's `.git` without following a link or blocking on a FIFO; a
  config dir that is itself a link is still open in #634.
- Yulius builds on macOS again, and CI keeps it so (SPEC-214, ADR-0169, task #201, fixes #636).
  #620 had put Linux-only calls in the sandbox and app crates (a build lock through a Linux-only
  crate, the Docker engine's `O_PATH` socket, a file mode's width), so the `yulius` CLI could not
  be installed on a Mac. They now compile there, and `scripts/dev ci` compiles every target for
  macOS (`scripts/dev macos`), with a pinned zig compiling the vendored C on Linux. Linking and
  tests on a Mac stay a manual check before a release.
- An agent's secret request shows on an open Secrets page at once (SPEC-209, task #196, fixes
  #631). When an agent or a Superagent asked for a secret's hosts or a `.env` copy, the Agent
  secrets page already open showed no request and no Allow until it was reloaded: nothing told
  the browser. The request now publishes `secrets.updated` for the secret's sandbox (every
  sandbox for a secret of every sandbox), and the page reads its list again on it and after a
  reconnect, keeping what it shows meanwhile.
- The Superagent runs in bypass-permissions mode on a contained server, as tasks do (SPEC-208,
  ADR-0165, task #195). It started as plain Claude Code, which now opens in auto mode: its safety
  check blocked commands the person asked for and its "Teach auto mode about your environment?"
  dialog interrupted the panel. It now starts with `--dangerously-skip-permissions` and
  `IS_SANDBOX=1` (it is root of its sandbox), the bypass warning already accepted; running
  Superagents restart into it once idle. Tasks already ran this way (checked on this instance's
  transcripts); an uncontained server still bypasses nothing.
- A shown secret's dialog is one box per field (SPEC-207, ADR-0164, task #194). The value's copy
  icon was hidden until hover, leaving a gap after the eye, and a detached Copy text button did the
  same thing. The value box now holds the masked value, the eye and one copy icon, always visible,
  which shows a check after a copy. The notes and the two-factor setup key sit in the same framed
  box with their own copy; every copy still clears the clipboard after 30 seconds.
- Removing a sandbox stops everything of it (SPEC-206, task #193, fixes #624). It stopped only the
  tasks: the sandbox's Superagents and terminals went on running (a person watching one saw it keep
  working), a CI run in progress ran to its end and published its status, the browser waited for
  its 30-minute idle stop, and the holder, with its Docker engine and any container or process an
  agent left, stayed up for good and was adopted again after a restart. The removal now cancels the
  sandbox's CI runs, ends its shells and browser (also one whose start was under way), and
  retires its holder: nothing new is prepared for it, and a minute later, once no run holds a
  lease, the holder stops with its engine and containers. A CD deploy already running finishes. Nothing
  armed in a removed sandbox is merged (already true, now tested).
- Secrets can be imported from the CLI (SPEC-205, ADR-0163, task #192). `yulius secrets import
  --to team|agent [FILE|-]` reads a dotenv file, stdin or hidden prompts and creates team secrets
  or a sandbox's agent secrets (`--sandbox`, `--instance`) through the existing vault API. It lists
  every name as new, existing (skipped, never overwritten) or invalid with the reason, and sends
  nothing until `yes` is typed at the terminal; no flag skips that. Values never appear in
  arguments, output, errors or files, and are wiped from memory when dropped. A comment above a key
  becomes its description or notes (a commented-out old key never does), and a PEM or OpenSSH key
  is stored as the Secrets page stores it.
- The Secrets page offers the production allowance only where it can be given (SPEC-204, task
  #191, fixes #621). A secret agents use, through its hosts or `.env` copies, showed a warning
  badge and an "Allow for production deploys" button. The server always refused that button,
  since a production secret is exclusive (ADR-0109). The card now says that production deploys
  never get a secret agents use, and that production needs a separate secret. A production
  secret whose host or `.env` request still waits offers Dismiss only. The web mock now refuses
  what the server refuses.
- Docker in sandboxes works out of the box (SPEC-203, ADR-0162, task #188). `sudo yulius host
  setup` is now the install path after the binary. It checks and fixes, in one order: a
  needrestart rule, written before any install, so installs never restart `yulius.service`; Git
  and the isolation packages; Docker Engine with buildx and compose from Docker's repository; the
  host's own Docker units stopped, unless they run containers; the kernel modules, persisted for
  every boot; the service user and its ids; the unit's delegation, by drop-in; a root-owned
  binary; and a base build that keeps live bases. It ends with the contained doctor and the smoke
  test, and a second run changes nothing. Every update then runs the new version's `yulius host
  check`, which applies only safe fixes, and a regression shows in the update's outcome and on a
  new Sandbox Docker card in Settings → Instance. The card gives the doctor's verdict, when a
  rebuilt base will be ready, the sandboxes running their own `docker.io` with the way back, and
  a Run smoke check button with each check as it ends. `yulius doctor` now names a writable
  binary and a base without `dockerd`, and without `--contained` it says that flag adds the
  check. A real fresh install in an Ubuntu 24.04 container found two bugs, both fixed: Docker's
  packages had no install candidate (apt's lists were read before the source was added), and a
  unit systemd did not report would have been rewritten.

## 2026-10-16

- The two documented Docker operator commands are safe (SPEC-202, ADR-0161, task #187, fixes #617
  and #618). A hand-run `yulius sandbox-base build` no longer deletes the base that running
  sandboxes have mounted: base collection reads the live holders' records itself, whoever builds,
  instead of trusting `--keep`, which only the server filled (on the VM it emptied every
  sandbox's `/usr/lib` until the service was restarted). `sudo yulius doctor --docker-smoke` from a
  root SSH shell now always runs as the service user in a delegated transient unit; root's own
  login slice passed as delegated, got a `sandboxes/` group and ran the checks as uid 0. The
  smoke's check lines are plain text, so compose's escapes no longer overwrite them, and the
  compose check shows the probe's result instead of the web container's expected exit 137.
- A sandbox's Docker engine starts again after Yulius restarts (SPEC-201 part 2, task #186,
  fixes #614). After the instance updated, every sandbox whose engine had stopped as idle
  answered "cannot join …/sandboxes/<sandbox>/engine: No such file or directory". systemd
  removes a stopped unit's empty cgroups, the idle engine's leaf among them. The Docker front,
  which survives the restart, prepared that leaf only when it started. The front now makes the
  leaf again before each engine start, and the server's adoption and upkeep tick do it for every
  kept engine front, and for the relay front of a SPEC-187 holder that runs its own engine. That
  also heals fronts started by an older binary, right after the update.
- The in-sandbox Docker install brings buildx and compose (SPEC-201, ADR-0160 amended, task #186,
  fixes #610). After #185, an agent that followed the hint `sudo apt-get install -y docker.io` got
  an engine, but `docker buildx build` was an unknown command: Ubuntu's `docker.io` only suggests
  `docker-buildx` and `docker-compose-v2`. A product sandbox could build no image, and its agent
  sent the person to change the host. The refusal, `yulius doctor`, the smoke test and the
  briefing now name `docker.io docker-buildx docker-compose-v2`, or, on the host, `docker-ce`
  with `docker-buildx-plugin` and `docker-compose-plugin`. The briefing tells the agent to install
  missing parts in its own sandbox, which needs no host change.
- A sandbox's Docker engine comes from its own system, not the host's (SPEC-199, ADR-0160, task
  #185). After #602 every sandbox answered "the host has no `dockerd`": Docker had been purged
  from the host on 2026-10-12, yet the engine runs the sandbox's own `dockerd`, and sandboxes on
  older bases still had one. The front now serves an engine whenever the sandbox's upper layer or
  its base has `dockerd` (links planted in the upper layer are never followed). Without one, the
  answer tells the agent to `sudo apt-get install -y docker.io` in the sandbox, and the engine
  starts at the next tick with nothing restarted. The host's Docker only gives new bases
  `dockerd`; `yulius doctor` warns without it and still checks the delegation and kernel modules.
- The automations Yulius ships are the Automations page's defaults (SPEC-197, ADR-0159, task #183).
  They sat below the list as "Ready-made automations" with Enable, and two an agent had created by
  name read "Created by an agent". The page now opens with Defaults, as Workflows does: all three
  shipped automations badged "Default" with what they do, each switched on or off in place, then
  Custom for the project's own. Deleting a default resets it: it stays listed, off, and switching it
  on again starts from the shipped version.
- A running CI run's commit status counts the jobs that already ended (SPEC-198, task #184, fixes
  #604). GitHub kept "Running on Yulius: 0 of 3 jobs passed" until a run ended, so the Superagent
  took PR #600's finished Vidoc scan for a running one and answered that no PR had been scanned.
  Each job that passes or ends skipped while other jobs still run now publishes the status again,
  and a published status resets the retry attempts, so a refused final status is still retried.
  The stale issue #470 is closed.
- Docker works in every sandbox and session, without a restart or a new session (SPEC-196,
  ADR-0158, task #182, fixes #597). After #583, sandboxes busy during the upgrade kept no engine,
  and sessions started before had no socket and no reason.
  - A Docker front beside every holder, adopted ones included, serves the socket: the engine on
    demand, a relay to a holder that already runs one, or a Docker-API error naming the missing
    prerequisite and the operator's fix.
  - Runs bind the directory `/run/yulius/docker` with `DOCKER_HOST` and a `/var/run/docker.sock`
    link, so a recreated socket reaches them. Sessions started without it get the directory
    moved into their mount namespace within one upkeep tick.
  - The briefing says `docker info` checks the engine or says why there is none; `yulius doctor`
    uses the same reasons.
  - `sudo yulius doctor --docker-smoke` proves it on a host: a throwaway sandbox goes through an
    upgrade, then hello-world, a build, compose, a recreated socket and a new run. The host test
    `isolation_docker_live` runs the same scenario. Neither can run in a sandbox or in Yulius CI.

- Vidoc is built into Yulius, and the security scan runs on vidoc-1 (SPEC-195, ADR-0157,
  task #181).
  - **The instance key.** An optional instance Vidoc key, managed like the Jev key: sealed,
    `YULIUS_VIDOC_API_KEY_FILE`, a card on the Instance page (set, test, replace, remove, adopt) and
    a skippable setup step. Agents and CI jobs granted `VIDOC_API_KEY` get its placeholder. The
    egress proxy adds the key toward `model.vidocsecurity.com` only, and a project's own secret
    wins.
  - **The new scanner.** `scripts/vidoc_scan.py` replaces the dead Vidoc CLI and `api.vidoc.dev`.
    It sends `HEAD`'s changed files, or the whole repository with `--all`, in bounded chunks to
    `/v1/messages` and fails on open critical or high findings. Exit 2 means not run.
    `.config/vidoc.toml` gains `exclude` and committed `[[ignore]]` false positives.
  - **Every project.** The binary embeds the scanner as `yulius security-scan`, which agents run
    through their shim and CI jobs find on their PATH.
  - **Infra bootstrap.** The briefing and onboarding add the `security` job only when the instance
    has a key.
  - **This project.** The `security` job runs `python3 scripts/vidoc_scan.py --ci`, so pull
    requests are really scanned again.
- A multi-line key pasted as an agent secret keeps its line breaks (SPEC-193, ADR-0156, task #179,
  fixes #591). The value field is one line, and the browser turned a pasted PEM or OpenSSH key's
  line breaks into spaces without a word.
  - The field now writes a pasted key's line breaks as literal `\n`, the vault's form for a
    multi-line key, so `secret_verify` answers `escaped_newlines` for the key file and
    `secret_copy_to_env` writes it back. Other text pastes as before. Checked with real
    `ssh-keygen` and `openssl` keys, LF and CRLF.
  - The server refuses a key flattened onto one line. One saved before the fix is not rewritten:
    its card says "Paste again" to the people who manage it.
  - Team secret values have the same one-line field (#592, not changed here).

- Task lists stay live, a task an update holds reads Paused, and a person can pause a task
  (SPEC-191, ADR-0154, task #176). The event stream had no heartbeat, so a connection that died
  without a close (a laptop asleep, a network change, a dropped proxy connection) stayed open in
  the page and the lists froze until a reload; the server now beats every 20 s and the page
  replaces a stream silent for 50 s (30 s when shown again or back online) and resyncs.
  `TaskSummary.pause` says why a live run is paused (an update, or a person) and the lists show
  Paused with why. A task's owner pauses it from the task menu (`POST /api/tasks/{tid}/pause`):
  its run is frozen as an update freezes it, or held at its turn boundary; Resume
  (`…/resume`) thaws it. Updates, restarts and their sweeps never wake it; stop, close and a
  switch thaw it first. Migration 0033.
- The ready-made automation "Work through open issues" no longer works through all of the core
  team's open issues (SPEC-194, task #180). It first drops the issues a running task owns: an
  active task whose title ends with `(#N)` or whose prompt refers to the issue, a `Taken by` task
  still running, or an open pull request that closes it. It also drops those it marked
  `Not actionable yet:` until a person answers.
  It ranks the rest (priority label, severity, reactions, age) and verifies them in that order
  until it has the 5 most important actionable ones, then starts a task for each. Issues below the
  cut are left untouched for a later run. `tasks_list` scope `active` now also lists tasks
  paused for capacity. This project's automation was updated to the new prompt.
  Automations other projects already added from the template keep their prompt.
- A task removes the files it created itself, instead of asking a person (SPEC-192, ADR-0155,
  task #177). Claude Code stops some `rm` commands for a person's approval even with approvals
  bypassed (a file at `/`, a path in a variable); the session terminal asked and the agent then
  left the removal to the person. `yulius remove <path>…` removes what is provably the task's:
  untracked or ignored paths of its worktree and paths of its session's own `/`, `/tmp`, `/run`
  and `/dev/shm`; anything shared is refused with the reason. Task runs also get `TMPDIR=/tmp`,
  so `$TMPDIR/x` no longer lands at `/x`.
- A confirmed deploy answers at once (SPEC-189, task #174, fixes #585). After "Deploy production…"
  the page showed nothing while the server checked and promoted (13 s on Yulius's own instance), so
  the click looked broken. The Deploy dialog now opens on confirmation, "Starting", with what it
  does ("Checking, then promoting quartio at 9f3c2a1 into main…") and the seconds so far, then
  shows the run in place; a refusal is said there. Closed meanwhile, the deploy goes on and a toast
  with "Open" announces it.
- The watched task's Working line moves as Claude Code's spinner (SPEC-190, ADR-0153, task #175).
  Under someone else's task, `✻ Working…` stood still: the mark never moved and the shimmer
  flashed over the whole line for a fraction of each cycle. Now the mark turns through
  `· ✢ ✳ ✶ ✻ ✽`, a light runs through the words, and the time worked follows, as in
  `✻ Working… (1m 23s)`. Under reduced motion the mark stays on `✻` and the words don't shimmer.
- Docker works inside isolated sandboxes (SPEC-187, ADR-0151, task #172, fixes #551). Each sandbox
  gets its own engine at `/run/docker.sock`, started by its namespace holder on first use, so
  agents, the Superagent, Shells and CI jobs run `docker build`, `docker run` and `docker compose`
  unchanged.
  - The engine is root of the sandbox's user namespace, never the host's, in the sandbox's network.
    It sees a run's files plus its own storage, a fresh sysfs and its own cgroup namespace.
  - A seccomp profile of its own (the run profile plus mounts, `setns` and `pivot_root`) bounds
    every container, `--privileged` included.
  - Images count toward the disk quota and are pruned under pressure. Containers count against the
    sandbox's limits.
  - The briefing now says Docker works (reverting SPEC-173 where an engine exists).
    `YULIUS_SANDBOX_DOCKER=off` turns it off, and `yulius doctor` checks `dockerd`, the cgroup
    delegation and the kernel modules.
  - Not verified on a real host yet: the opt-in host test `isolation_docker` cannot run in a
    sandbox or in Yulius CI.

- Supertask names one-shotting the product, and Jev proposes it for broad work (SPEC-188,
  ADR-0152, task #173). The start menu's Supertask now reads "Runs alone once this project’s
  running tasks end; the rest wait. For broad work: security hardening, one-shotting the
  product." (three lines, so the menu still fits a 1280 × 800 window), and the `task_create` tool
  and the product summary name the same examples. A task started now, after a task or by a rule
  is held when Jev is sure it is broad work: "Run it as a supertask?" offers Run as a supertask or
  Start as chosen. A request that splits ("fix all GitHub issues") is never proposed.
  `/intake/check` gains `supertask`, from a new fixed `breadth` question asked with the split
  question.
- The Superagent uses agent secrets as a task's agent does, and sees new tools without a restart
  (SPEC-186, ADR-0150, task #171). It runs behind the egress proxy with its own bearer, so its
  requests to a secret's bound hosts carry it (`$NAME` holds the placeholder); it copies allowed
  secrets into the repository's git-ignored `.env` and asks for hosts, each asked as "alice's
  Superagent"; production secrets stay out. Multi-line keys stored with literal `\n` (PEM,
  OpenSSH) now land in a `.env` as their files, byte for byte, for tasks too, and copying again
  replaces the whole key. A Superagent started before an update that changed its tools restarts by
  itself once idle, resuming its conversation; the panel follows it.
- A watched task follows its newest lines, with no octopus and less padding (SPEC-185, ADR-0149,
  task #170, fixes #575). The live transcript of someone else's task stopped scrolling down once
  "✻ Working…" showed, the card above changed or the window resized, and then sat behind "New
  messages". Following is now a state that only scrolling up ends, kept at the bottom through every
  change of height. The pixel octopus and its lane are gone, and the transcript is inset 16 px, as
  the card above it.
- Deploying production (and every CD deploy and promotion) works on a real instance (SPEC-184,
  task #169, fixes #574). The GitHub adapter's branch head read was a stub that always failed with
  "branch lookup via GitHub is not supported; read the fetched clone instead"; it now asks GitHub
  (`GET /repos/{owner}/{repo}/git/ref/heads/{branch}`, an unknown branch is none), as ADR-0109
  requires. The same read gates CI jobs' agent-secret grants, which it had withheld too.

## 2026-10-15

- Someone else's task is watched as its live transcript, not a broken terminal (SPEC-179,
  ADR-0145, task #166). The owner's terminal is drawn for the owner's window, so a watcher saw it
  wrapped and overlapping. Now they read the session as the Sessions view prints it, in the
  terminal's normal colours, as it writes (new messages, tool calls and prompts every 1.5 s; the
  tail of a long session first). "You are watching @owner's task" sits above it with Request
  control, the sidebar shows the owner's initials on their tasks, and an electro-blue pixel octopus
  with dot eyes and an ε mouth crawls between the top corners. New `GET /api/tasks/{tid}/transcript`.
- The step strip never overflows, the keyboard survives a hidden Archive, and the last loading
  states take their final shape (SPEC-182, ADR-0147, task #138, fixes #348, #331, #407, #449). The
  task step strip gives way whole parts by priority (chain chips, Auto, Supertask, then the bar and
  the pull request chip) so it never passes its edge and Task actions stays clickable beside a
  panel; its bar is 60 px in narrow strips as SPEC-073 meant. The keyboard on a quick Archive that
  hides moves to its row's link. The automation editor's loading start chip is a skeleton bar and
  its Supertask tooltip reads as New task's. The New sandbox lists, the delivery and CI run dialogs,
  a task's changes, commits and overview, and the CI hygiene and GitHub connection cards load in
  their final layout.
- The Editor's file in front follows the disk, and the Explorer's buttons stay in the Explorer
  (SPEC-181, ADR-0146, task #168). While the Editor is shown, the file in front is looked at every
  2 s (and at once when the Editor or the page comes back) with `known=<sha256>`; the files API
  answers 204 while the file is unchanged. What teammates, agents or shells wrote replaces unedited
  text as one undoable edit of the changed part, so the cursor stays with its text and no toast
  repeats; edited text is kept and the status bar says "Changed on disk". The Explorer's label now
  shortens at its narrowest instead of pushing the Refresh button over the first file tab.
  Co-editing of unsaved text is #566.
- The Superagent reads the agent vault (SPEC-178, ADR-0144, task #165): `secrets_list` names its
  project's agent secrets (never values) with guidance for a session outside the proxy, and the
  new `secret_verify {secret, sha256}`, the Superagent's only, tells whether a value the person holds
  is a secret's value from its SHA-256: exactly, without a final line break, or with its line
  breaks as the literal `\n` a multi-line key is stored with. Both run without asking; each
  distinct check is audited (`vault.agent.verify`).
- The original prompt reads as written (SPEC-180, fixes #565, task #167). An image inside the
  text was a thumbnail up to 160 px high, so its line grew to the picture's height. It is now the
  composer's one-line `Image N` placeholder, which opens the image. The "Attached files" list
  below the text holds every file once, each image's thumbnail carrying its number.
- A playground moves to GitHub (SPEC-176, ADR-0142, task #132, fixes #410). Once GitHub is
  connected, an admin of a playground picks "Move to GitHub…" in its sandbox menu and names an empty
  repository of the instance owner that they could import (`admin` or `maintain` on it; the
  superadmin: one the App can access). Yulius pushes every branch and tag there in one atomic push,
  points the sandbox's clone at it, records it (same slug, tasks, sessions and steps) and deletes
  the server's copy; access then mirrors GitHub. A repository with anything else in it, a live or
  queued task, or a failed push leaves the playground as it was
  (`POST /api/sandboxes/{sid}/move-to-github`).

- The session view says when the server cut a diff or a context, and every scroller is reachable
  (SPEC-177, ADR-0143, task #137, fixes #350, #380, #363). Transcript diffs and meta blocks carry
  `truncated`; a cut edit reads "Wrote path" / "Updated path" with no count and "Cut here: the
  change is longer.", and a cut context reads "Context added (at least 16 KiB)" and says it was cut
  when opened. Code, results, diffs and context are keyboard regions when they scroll as laid out
  (also on a phone), named after their call, such as `Result of Bash(npm test)`.
- Review findings in a change's own code are fixed before merge, never deferred to an issue
  (SPEC-174, ADR-0141, task #164). The delivery pipeline, `om-code-review` (skill, checklist and
  report format), the Feature and Fix workflows, the `yulius-review`, `yulius-verify` and
  `yulius-git` primitives, the engineering workflow, the briefing and the `issue_create` tool now
  say the same: an own-code finding is fixed test-first in the same pull request whatever the
  round (keep fixing, split, or ask a person); only pre-existing bugs, flaky tests the change did
  not cause and nits left at the round limit become issues, saying why; the reviewer marks each
  finding own code or pre-existing, and deferring an own-code one is "request changes". A unit
  test keeps every location stating the rule.
- A long goal no longer pushes mandatory parts out of the briefing (SPEC-174, fixes #555). A Fix
  task's workflow instructions lower the budget, and a 600-byte goal used to cut `## Architecture`
  and every optional part. The goal quote now shrinks first (down to 40 bytes; the whole request is
  in the task's prompt or request file), and no bundled workflow is larger than 1,444 bytes.
- A held redraw never eats a click or a drag, and one Start onboarding shows one toast (SPEC-175,
  task #136, fixes #477, #478, #436). A plain click on an Editor tab while a strip redraw waited
  activates the tab (the redraw runs after the click; the browser dropped the click before); a
  drag from the new-task prompt that a thumbnail's redraw interrupted is refused on drop instead
  of pasting a copy of itself; and leaving the sandbox's page and coming back while onboarding
  starts no longer shows two toasts. New tests cover a presence leave naming a link to a folder,
  a destroyed presence's waiting leave and which refusals are retried.
- Queued messages show and take files as placeholders (SPEC-168, ADR-0137, task #158). The
  "Queued messages" card no longer shows a request's files as raw `@'/path'` mentions: each is a
  placeholder (`Image N`, or the file's name) that opens the image, or downloads the file, on a
  click. The message box is the composer's prompt input: an image pasted or dropped there uploads
  to the task and sits at the caret as a placeholder that moves and deletes like a character, and
  is queued at its place. Queue waits for an upload still running.
- A playground's lifecycle at its ends (SPEC-171, ADR-0139, task #131, fixes #409 and #420).
  `yulius onboard --playground [path] [--name <name>]` brings a local checkout without GitHub into a
  new or existing playground of yours: every branch and tag, the uncommitted work (one commit on the
  checked-out branch of a new playground) and the chosen sessions (on its Sessions page), through
  the import upload; nothing is pushed anywhere and GitHub is never called. Removing a playground
  can delete its code for good ("Also delete its code from this server for good" in the dialog,
  `POST /api/sandboxes/{sid}/purge`): its `origin` and sandbox directory go once nothing of its
  sandbox runs, its place among its maker's 10 is freed, and task records and the audit stay.
- Tools and the keyboard stay where the person left them, and presence leaves arrive last
  (SPEC-170, ADR-0138, task #135, fixes #489, #439, #463). A sandbox left while its page still
  loads comes back with no tool open, not the other sandbox's; the Shell shown again for another
  task or sandbox leaves the keyboard on the page (the rail still gives it to the terminal); and
  notepad and file presence updates carry a per-tab `seq`, so an update arriving after the leave
  sent as a teammate's tab hid no longer leaves their caret drawn.
- An isolated sandbox's agent is told there is no container engine (SPEC-173, ADR-0140, task #163,
  fixes #550). An agent of another project had designed a deployer around a local Docker daemon
  before finding that `docker` exists but no daemon does: the sandbox's containment refuses what one
  needs. The briefing now has a `## Containers` section: no engine here, in Yulius CI or in CD; use
  an engine on another machine (`DOCKER_HOST=ssh://…`), keep rollback images there or in a
  registry, and install test services with `apt`. An engine per sandbox is #551.
- A folded subscription shows both its 5-hour and its weekly usage, without opening it (SPEC-172,
  task #162). The Status panel's and the Dashboard's rows showed only the 5-hour bar; each now has
  two lines, "5 h" and "Week", with a bar and the percent used. Where the list is narrower than
  360 px (the Status panel on a phone), the lines keep their percents and drop the bars; in the
  Status panel a row's state ("Limit reached") shows as its icon, so the name keeps its room.
- The sandbox tabs show that they drag (SPEC-167, task #157). Each sandbox tab in the top row has
  the six-dot grip the Notes tabs have (SPEC-136) before its name, and its tooltip adds "Click to
  open, drag to reorder" under the repository and access. The grip is faint until the tab is
  hovered or current and is hidden on devices without a mouse or pen; "All sandboxes" has none,
  since it does not move. Both strips take the hint from one constant.
- A CI run's stages are readable (SPEC-165, ADR-0136, task #155): each job is one row under
  "Jobs", its wait for a job slot (hatched) running into its run, with "waited 6 min" beside how
  long it ran and its state as an icon and a word; a job's checkout and stages fold under it (a
  failed job opens, and what is opened survives the dialog's refresh); every waterfall has a time
  axis with gridlines, a "now" mark and a legend; on a phone the bar goes under its name.
- The task strip's **PR #N** chip stays on one line (SPEC-169, task #161, fixes #542). In a strip
  crowded by a panel beside the task, it broke into "PR" over "#523" and spilled out of its pill;
  the narrow-strip e2e test now checks that no strip chip's text overflows its box.
- The Tasks page shows how tasks hang together (SPEC-164, ADR-0135, task #154). A List / Graph
  switch (`?view=graph`, also the sidebar Working group's "Show as a graph") opens Starts next (the
  queued tasks in the order they get a free session: supertasks first, then the longest queued),
  Chains and rules (chains, start rules and supertask holds drawn left to right, one arrow per link,
  each card a link to the task), Prepared and On their own. Built in the browser from the task
  summaries; no API change.
- The Editor never opens a file twice, and a moved image placeholder is never shown as selected
  text (SPEC-166, task #156). The task's screenshots, two editors stacked in one host, were #394,
  fixed by #523 before this task. Reproducing them showed the Editor still broke another way:
  - **#534:** a file opened again before its first open showed it (a double-click on its row,
    which is two clicks) got a second tab, with a second model of the same file. Every open of a
    path now waits for the one under way.
  - **#535:** a press on a placeholder in the new-task prompt, and a drop, select it so Delete,
    cut or a drag takes it. The browser painted that selection as a text highlight over the label
    "Image 1". Now the placeholder is marked as a whole and its label is never painted.
- Playgrounds get Yulius CI (SPEC-163, ADR-0134, task #130, fixes #411). The CI sync tests a
  playground's `dev` head without GitHub, and `delivery_merge` first tests the task head merged into
  `dev`, built in Yulius's clone (`Task #n` runs): it squashes on green, arms while the run goes
  (Yulius lands it when it passes, or builds and tests again when `dev` moved), and refuses on red.
  A person's merge waits for green too. `yulius wait`, `ci_status` and `ci_rerun` work for a
  playground's agents, and nothing is published to GitHub. A playground without CI merges at once,
  as before.
- The Pipeline (stages) is a tab of the Dashboard, not a sidebar entry (SPEC-161, ADR-0132, task
  #152). The sidebar lists Tasks, Git, CI/CD, Secrets and Dashboard; the Dashboard's tab strip (the
  CI/CD page's, now shared as `sectionedPage`) shows Overview and Pipeline (stages), the latter at
  `?section=pipeline` with its filters after it. `/pipeline` links, "In Pipeline" and the
  Overview's Pipeline card open the tab.
- A CI job that checked nothing reads **Skipped**, never Passed (SPEC-162, ADR-0133, task #153,
  fixes #530). Yulius gives every job `YULIUS_CI_SKIP_CODE` (78); a job that exits with it ends
  skipped, blocks no merge, and is counted in the run's commit status (`Passed: 3 jobs, 1 skipped`),
  on the CI page and in `ci_status`. The `security` job now ends skipped when the Vidoc scan cannot
  run (no key, a rejected one), and is granted `VIDOC_API_KEY` through the egress proxy (#466).
  Until `dev` is promoted to `main`, the running instance gives no skip code and the job still
  passes, with `security scan skipped` in its output.
- An update pauses every task at once (SPEC-160, ADR-0131, task #151). When the new version is
  built, each task's agent is frozen mid-turn with the cgroup v2 freezer: its output stops, nothing
  is told to it, and it goes on from the same instruction when its task is thawed after the
  restart (one by one; at once on a cancel). Updates no longer wait for long turns or get cancelled
  after 30 minutes. A run that cannot be frozen (no delegated cgroup) still pauses at its turn
  boundary. A provider response streaming at the restart is cut and retried by Claude Code.
- Reply in a task's attention bar does something visible (SPEC-156, task #147, #516). On
  "Needs you" and "The agent is waiting for your reply" it opens a reply field in the bar; Enter or
  Send types the answer into the session as the person's prompt, which also ends a `needs_you`
  request, and Escape or Cancel closes it. Before, Reply only focused the terminal, which showed
  nothing because Claude Code draws its own cursor. While the agent shows a menu in the terminal,
  Reply still goes to the terminal, so typed text never picks a menu option unseen. Phones keep
  their quick input.
- The task strip's progress bar shows the step in progress (SPEC-159, task #150): after the
  finished share comes the open step's share in the working colour, so "Step 2/4" no longer sits
  beside a bar that looks a quarter done. The API's step summary carries `active_percent`.
- The e2e suite holds under host load, and the Editor waits for all of Monaco (SPEC-148,
  ADR-0122, task #134):
  - **#354:** the suite's static server (`e2e/serve.py`) keeps connections alive and queues 1024
    of them. `python3 -m http.server` queued 5 and closed every connection, so on a loaded machine
    the kernel dropped a page's module requests and the browser waited 1–3 s to try again.
    Starved to a fifth of a CPU per thread, about 1 run in 360 still fails a 5 s wait; #354
    stays open for that.
  - **#394:** a file opened while Monaco loaded made a second editor from the half-loaded
    `window.monaco`, which lacked five services ("depends on UNKNOWN service ICodeLensCache").
    The editor now waits for all of `editor.main`, and tests touch Monaco's API only once the
    Editor shows (`editorShown`). An editor whose setup failed is disposed, so the next file
    makes one instead of a second.
  - **#356, #349:** the Editor's first opening is waited for with Monaco's cold-load budget, and the
    kept-notes test reads the text after a load that can succeed instead of waiting out the backoff.
- CI jobs and CD runs record what they used (SPEC-154, ADR-0127, task #129; fixes #488): the
  process host measures a job's reaped process tree (CPU time, the largest process's peak memory,
  bytes written on Linux) into `exit.json`, and the job's or run's span gets `cpu_ms`,
  `memory_peak_mb` and `disk_written_mb` with its end (absent when unmeasured, never 0). The
  Pipeline page's stage table shows CPU time and peak memory p50 · p90 beside the durations, and a
  waterfall row its own figures.
- Text selected in Claude Code's full-screen view is copied again (SPEC-158, ADR-0130, task #149,
  fixes #519). Claude Code copies its own plain-drag selection with OSC 52, which the web terminal
  dropped; the write now reaches the clipboard of the viewer who just used the terminal, and when
  the browser refuses it (Safari, plain HTTP) ⌘C / Ctrl+Shift+C copies it. Queries are never
  answered.
- The onboarding agent protects the production branch itself (SPEC-157, ADR-0129, task #148).
  When onboarding finds `main` writable by agents, the preparation agent calls the new
  `production_protect` tool: Yulius adds the ruleset `yulius-people-only-<branch>` (Restrict
  updates, Restrict deletions, Block force pushes, only repository admins bypass) as the person who
  owns the task, when GitHub says they are a repository admin. The agent asks a person with
  `needs_you` only when that is refused; its own token still cannot change protection.
- Automations can prepare their tasks (SPEC-155, ADR-0128, task #142, fixes #416). The automation
  editor's start chip, the automations API (`prepared`, beside `supertask`, `after_task_id` and
  `start_rule`) and the `automation_create` / `automation_update` tools offer "When I decide": each
  run, and Run now, creates the task `prepared`, and it waits for Start now. The chip, the sentence
  and the list row name it; a run while the previous prepared task still waits is
  `skipped_previous_active`. Stored in migration 0031 (schema version 30).
- The CD page's environment cards read right at every width (SPEC-152, task #146, fixes #501):
  - The deploy button stays top right, with only the title and badge wrapping beside it. Before,
    it fell onto its own row under the title.
  - A dev deploy that can't run is a plain button, not a grey primary slab.
  - A promoting production names what it promotes ("Promotes dev at e37fdd2 into main") instead of
    "First".
- Onboarding writes never undo a Skip; a task that never ran is not Closed (SPEC-153, ADR-0126,
  task #126, fixes #406, #431 and #399):
  - **#406:** every onboarding state write is a compare-and-set from the state it read, so Start
    onboarding running while an admin skips leaves the sandbox `skipped`. The mock nit was already
    done by #405.
  - **#431:** a test archives a live, Done onboarding task in a `preparing` sandbox and gets it
    closed, archived and the sandbox `ready`. The close's "changing state" 409 now reads "stop the
    task first" for archive and delete, and `archive_task` documents its 409s. SPEC-002's long
    line is reflowed.
  - **#399:** the task summary says `not_started` and `awaits_start_onboarding`. The UI reads the
    onboarding task of a `pending` sandbox "Waits for Start onboarding", and a task that never ran
    "Not started". Neither is in the Closed group, tab or count.
- Uploading files and positioning them are two steps again in the new-task composer (SPEC-150,
  ADR-0124, task #144). Pasted or dropped files show in a tray above the prompt, numbered; a click
  opens an image large, fitted to the window or at its actual size. In the text each file is a
  one-line placeholder (`Image 1`) at the caret, moved by drag, by cut and paste or by dragging its
  tray thumbnail into the text. × in the tray removes a file; a file without a placeholder is sent
  after the text.
- CI and CD are one CI/CD page (SPEC-151, task #145, ADR-0125): the sidebar's single **CI/CD**
  entry opens a page whose **CI (tests)** and **CD (deployment)** tabs work like the Secrets page's
  Team and Agent tabs; the CD tab is `?section=cd` and `/s/<slug>/cd` (with `?run=`) still opens it.
- The Superagent gets the sandbox's MCP servers, with their secrets redacted (SPEC-147, ADR-0121,
  task #141, issue #395). Its `--mcp-config` lists the servers a task run of its sandbox gets next
  to `yulius`, their values only as `${YULIUS_MCP_<SERVER>_<VAR>}` references and the values in its
  secret environment. Its Bash commands, hooks and stdio servers run through `yulius --scrub-exec`
  and its tool results pass the `PostToolUse` redaction hook, its only hook, as in task runs: an
  agent secret prints as `[secret:NAME]`. Both load their fingerprints with the person's workspace
  token, which the scrub endpoints now accept while the person may work in the sandbox, and its
  screen is redacted on the server as a task's terminal is. Literal values typed into a server's
  form are not agent secrets and stay unredacted, as in task runs; servers imported from
  `.mcp.json` with agent-secret placeholders need the egress proxy, which the Superagent does not
  have yet.
- A failed CI test's output stays readable (SPEC-145, ADR-0119, task #128; fixes #460, part of #485).
  While a job runs, Yulius keeps each failing test's block (nextest's `FAIL` line with its stdout and
  stderr, or libtest's), at most 8 KiB each and 32 KiB per job, and `ci_status` with `run_id` shows
  them before the end of the job's output. Flaky tests:
  - `cockpit::a_cockpit_manages_the_project_for_its_owner` archived the task it created before the
    task's start had failed in the background; it now waits for the task to rest (#460).
  - `local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees` advanced the clock once,
    and a disk check that ended after the jump dated the next check a minute later; it now lets time
    pass until the runs start. That race only times out, while CI saw a fast failure that stays
    unexplained, so the test is quarantined and #485 stays open.
- Opening Settings → Instance checks `main` at once (SPEC-149, ADR-0123, task #143): the page no
  longer shows a check up to an hour old until "Check now". The server reuses a check younger than
  30 s; a failed automatic check keeps the last result without an error toast.
- A shell ends only when its host is proven gone; a launch `/goal` is watched again after a restart
  (SPEC-146, ADR-0120, task #125, fixes #346 and #453). A terminal or the Superagent whose host
  cannot be terminated stays live with its run directory, listed and closable again, and its
  viewers are told; the Superagent's usage-limit move then starts no second Superagent on the
  same conversation. A goal task whose launch `/goal` was not recorded when the server restarted is
  watched again and flagged if Claude Code never takes it; a goal recorded meanwhile (also while
  Yulius was down) is never flagged.
- `pulls.e2e.ts` waits on conditions, and a failed read keeps the real chips (SPEC-144, task #133):
  - **#438, #461:** `chipsAfterRead` returns once every sidebar chip is drawn with its label. Rows
    skip rendering until drawn (`content-visibility: auto`), so a chip read before the next frame
    had its box but no text.
  - **#444:** the store plans its pull request reads on a clock the transport may give. The mock's
    `pullclock=manual` clock moves only by `advanceClock`, so "announcements within the gap make
    one read" counts reads without depending on the host's speed (ADR-0118).
  - **#415:** a new case proves that a failed later Git overview read keeps the chips the page read.
    The mock counts GETs it failed on purpose (`failedReads`).
  - **#469:** was already fixed by #490 and is closed.
- Yulius CI jobs can use agent secrets, and every pull request runs a `security` job (SPEC-142,
  ADR-0116, task #140, issue #466). A `[[ci.jobs]]` entry names them, `secrets = "VIDOC_API_KEY"`,
  and only the integration branch's entry of the job's name counts: a pull request's own grant
  gives nothing. A granted job gets placeholders, the CA bundle and a proxy credential of its own,
  never a value; the proxy client it names, `ci:<run>`, uses only those secrets toward their bound
  hosts, and every use is in the secrets audit; the grant is verified at the integration head
  GitHub reports, so nothing a job writes can forge it. This repository's new `security` job runs
  `scripts/dev security` on pull request heads, in a private clone that reaches its base without a
  GitHub credential. Findings fail it; "did not run" (exit 2) does not, while the stored key is
  rejected (#470) and until the job's grant line is added, once the running instance knows
  `secrets` (an older one rejects the whole `yulius.toml`).

- A run that ends while Yulius is down keeps its metrics (SPEC-143, ADR-0117, task #124, fixes
  #385 and #389). A run that ends before a watcher of this process read its transcript to its end
  (reconciliation finds it exited, or a reattach gets its exit at once from a lingering host) is
  counted from its transcript window (SPEC-091's recount), lines written while Yulius was down
  included, instead of saving an empty snapshot, which replaced its tokens, turns and active time
  with NULLs; when the count finds nothing the row stays. `App::recover` starts the metrics recount only after
  a reconciliation that succeeded, now proven by a test; SPEC-002 lists `metrics_recounts`.

- task_done, supervision and merge_train tests hold under host load (SPEC-140, ADR-0115, task #127;
  fixes #404, #464, #353, #443). Two product races fixed: a hook whose client gave up after its 5 s
  is still applied to its end (a slow Stop used to be cut off before setting Done), and a stop that
  comes while a run is launching (a switch, a start) asks the run once it is attached instead of
  waiting 20 s for the escalation. Tests wait for a stop within the stop escalation plus a margin and
  say what they saw; merge train tests share one deadline under nextest's 120 s.
- Open GitHub issues triaged and the first fixes delivered (SPEC-138, task #118). Of 63 open issues:
  - #423, #425 and #465 were already fixed and are closed.
  - The rest go to 20 chained tasks (#123–#142) in four lanes, or stay open with a comment naming
    what only a person can provide (#321, #412, #470 and the checkpoint issues).

  This task fixes:
  - **#424:** GitHub's stateless installation tokens (`ghs_<APPID>_<JWT>`, about 520 characters)
    are now redacted whole from transcripts, Git errors and GitHub errors. Before, transcripts
    kept them in full, and the error redactors left the JWT's payload and signature.
  - **#475:** `secret_copy_to_env` refuses a `.env` whose quoted value never closes, naming the
    line, rather than appending names inside that value.
  - **#430, #446, #455:** the `context_full` tests wait for the audit rows they check, and in the
    e2e suite opening or reloading a mock page waits for the mock, so tests no longer fail on a
    busy machine for those reasons.
- `yulius wait` and `delivery_merge` report the current push state, not a stale note (SPEC-139,
  ADR-0113, task #123, fixes #366 and #448). A head GitHub's branch already has counts as pushed
  even when `git push` reported a failure, and clears the earlier note; `yulius wait` drops a
  failed-push note once the branch is at its head and waits for the checks and the armed merge;
  `delivery_merge` answers a failed delivery with that failure, or the note its own call wrote (a
  refused push's remedy, or that the pull request did not open after its push), never with an
  older "not delivered" note. The remote is asked with `git ls-remote`, bounded to 10 s.
- Agents can no longer push to, merge into or fast-forward the production branch with a person's
  token (SPEC-141, ADR-0114, task #139, issue #486). The agent's Git and `gh` now hold an App
  installation token restricted to the repository and to contents, pull requests and issues
  (write) and checks, statuses and metadata (read): never the task owner's token, never
  Administration or Workflows. The production ruleset Yulius gives new projects also restricts
  updates, with repository admins as the only bypass actors, so GitHub refuses that token on the
  branch, `--no-verify` and `gh` included. Onboarding (`yulius onboard`, the import result and the
  preparation task's prompt) reports a production branch the agents could write and names the
  ruleset a repository admin must add. Commits stay authored by the owner; Yulius's own pushes,
  pull requests and merges for a task stay the owner's.

- Every stage of tasks, CI and CD is measured and shown (SPEC-137, ADR-0112, task #116). One `spans`
  table records, as things happen, a task's waits before it starts, its states (needs you and how
  long a person took to answer included), steps with the tokens and agent work spent in them, runs,
  usage-limit pauses, pull request open → merged, armed-merge waits and updates onto dev; a CI
  run's queue and why it waited, preparation, job-slot waits, checkouts, jobs and the stages their
  output names with `::yulius-stage` (test counts read from nextest, cargo test, pytest and
  Playwright; reused results marked, never counted); a deploy's confirmation, checks, promotion,
  deploy, verify and their stages. Existing CI, CD, run, step and pause history is backfilled and
  marked. A new Pipeline (stages) page beside CI and CD shows dev and production deployment
  frequency, lead time from merge, change failure rate and time to restore, any stage's daily p50
  and p90 and the stage table with CSV/JSON export; CI runs, deploys and task overviews show their
  stages as a waterfall, and the Dashboard the overview. `scripts/dev ci` names its gates.

- CD (deployment) runs beside CI, with the same discipline (SPEC-134, ADR-0109..0111, task #113).
  Every project defines dev and production in `yulius.toml`, as `[cd.dev]` and `[cd.prod]`. Each
  has deploy and verify scripts committed under `scripts/cd/`, or is explicitly empty while it
  deploys nothing yet. Yulius deploys exactly the commit GitHub reports for the protected branch,
  taken into a mirror only it writes, so nothing an agent can write (a task branch, a worktree,
  the sandbox clone) changes what a deploy runs. Dev runs inside the sandbox like a CI job;
  production joins only the sandbox's network, on the host's clean system. A run's files, `HOME`, log and cache are
  out of the agents' reach, and its output is stored redacted. Dev gets the agent secrets a person
  allowed outside the proxy; production only those allowed "for production deploys" on the
  Secrets page. A merge deploys nothing. Dev is deployed by a person on the new
  CD (deployment) page, by an agent with `cd_deploy` (and `yulius wait`), or by the ready-made
  "Deploy dev every night" automation, which also tests it. Production, with its promotion, is
  only an administrator's, who types the project's slug (the promotion API too). Agents lost
  `delivery_promote` and can no longer run production-stage environments. A new tool, `needs_you`, lets an agent put its task
  in Needs you ("Needs you: <action>") until a person writes to it. Onboarding finds how a project
  deploys today and writes it down this way, and "Set up CD with an agent" does the same later
  through a task. Yulius itself: dev empty (no dev machine yet), production the push to `main`
  that instances install by themselves. Task #112, the archived first attempt, left nothing behind.
- The Notes tabs show that they drag (SPEC-136, task #122). Each tab has a six-dot grip before its
  name, the one the workflow editor's steps use, faint at rest and brighter on the chosen or
  hovered tab, and its tooltip reads "Click to open, drag to reorder". A device with no mouse or
  pen, which moves tabs from the menu, does not show the grip.

## 2026-10-14

- The superadmin updates Yulius from the sidebar (SPEC-133, ADR-0108, task #121). A button beside
  the account button reads "Update" when main has moved on, and one click opens the update's
  confirmation; while an update runs it reads "Updating", and otherwise it is a quiet "Version
  and updates" icon leading to the instance page. Dismissing the tab-row pill no longer hides the
  way to update, and the account menu lists "Version and updates" too.
- The Editor gets the notepads' smooth selection and teammates' presence (SPEC-127, ADR-0103,
  task #114). A selection in the Editor is drawn in rounded shapes that glide, exactly where
  Monaco lays it out, so it stays right with word wrap on or off and when scrolled. People who
  have the same file open see each other's caret, name and selection in the colour they have in
  Notes, live; someone who switches file, closes it or leaves the Editor disappears at once.
  It is awareness only: unsaved text stays each person's own and saving works as before. The
  server relays where people are (`PUT /api/sandboxes/{sid}/files/presence`, `file.presence`)
  only to the sandbox's writers, with the notepads' rate limit. The Editor's file tabs move by
  drag, Alt+← / Alt+→ or the tab's menu. Notes and the Editor now share the presence loop, the
  drawing and the tab drag; and a tool replaced by another in the panel is hidden like a closed
  panel, so its presence leaves and the browser stream stops.
- Images keep their place in the text, from paste to the agent (SPEC-124, ADR-0101, task #105).
  A pasted or dropped image used to become a thumbnail in a row above the prompt, and the agent
  got the images after the text, apart from the words about them. Now a file lands at the caret
  (or the drop point) inside the text, as a thumbnail or a chip. It deletes like a character, with
  undo, and moves by drag or by cut and paste. The prompt is stored with each file at its place
  (`@'<path>'`). The agent gets it there: in the request file, in the CLI prompt, or pasted where
  it sits in a typed first message, still one message. The original prompt view shows it inside
  the text. Older prompts and drafts keep working; their files show after the text.
- The top bar has no Superagent button any more (SPEC-132, ADR-0106, task #120). The Superagent
  is first in the tools rail, right under the tools toggle, so the button beside the toggle was a
  second way to the same panel. The toggle now reads "Show tools (Superagent, browser, editor,
  shell, notes)", and the Superagent links (`/s/<slug>/superagent`, `/agent`, `/cockpit`) open it as
  before.
- The Vidoc security gate can be turned off and on (SPEC-131, ADR-0107, task #119). `enabled` in
  `.config/vidoc.toml` is committed as `true`; with `false`, `scripts/dev security` exits 0 at once
  and says nothing was scanned, and agents report the scan as disabled. Only a committed `false`
  counts: an uncommitted one exits 2. A missing file means on; a malformed one exits 2.
- Done means done (SPEC-130, task #117, #473, #474). The process probe could keep a task whose
  goal was achieved reading Working for good: a server the agent started as `(server &)` left
  an orphaned subshell with Claude Code's wrapper command line, which the sandbox's init adopted.
  Task #108 read Working until it was closed. Such orphans are no longer counted; only the
  agent's own shells are. The opposite case is fixed too: a task whose agent waits in
  `yulius wait` for its armed merge no longer reads Done because its plan is complete and its
  checks are green. It reads "Waiting for checks or merge" until the merge, in the sidebar and
  on the server.
- A task that has not started can change how it starts (SPEC-126, ADR-0102, task #115). A chained
  or prepared task's view has a start chip ("When I decide", "After #12", "When #12 done",
  "At …") that opens New task's start menu: it can be chained after a task or a rule, prepared,
  scheduled, started now or as a supertask. A task that started keeps its start, and a wait on
  itself is refused. API: `PUT /api/tasks/{tid}/start`. A chain evaluated before the change no
  longer starts the task, and a start rule judges the tasks that existed when it was written.
- Keys have a default place: a git-ignored `.env` beside a tracked `.env.example` (SPEC-129,
  ADR-0105, task #111). When a project's code needs an agent secret's real value, the agent calls
  `secret_copy_to_env`. Yulius writes the value into `.env` and the name into `.env.example`; the
  agent only ever sees `[secret:NAME]`, so it never writes the masked text by mistake. Git must
  ignore the file and not track it, or nothing is written. A person allows each secret once on the
  Secrets page ("Task #N asks to copy its value into the project's git-ignored .env": Allow,
  Dismiss, later Revoke). The briefing, `secrets_list` and `infra-bootstrap` name this default.
  This prepares Yulius CD (task #113).
- Agents scan every change with Vidoc before delivery (SPEC-128, ADR-0104, task #109).
  `scripts/dev security` runs the Vidoc CLI pinned in `tools/vidoc` on the current branch. Vidoc
  scans the files that the branch changed since `dev`, and the command fails when the branch has
  an open critical or high finding, printing each one. The key is the agent secret
  `VIDOC_API_KEY`: the egress proxy adds it for `api.vidoc.dev`, so agents never see it. The
  command refuses to run, with exit 2, on a tree with uncommitted files (the CLI scans the working
  tree, not the commits) and without a usable key. The verification skill and AGENTS.md make the
  scan part of every change's checks. Yulius CI does not run it yet, because CI jobs hold no secret
  (#466).
- Task numbers show in the sidebar and on the Tasks page, and pull request chips say "PR"
  (SPEC-122, ADR-0099, task #108). Each row reads `#108` (heard as "Task #108") before its
  title, and the chip reads `PR #450` everywhere, so a bare `#N` is always a task. In a narrow
  sidebar the time gives way first, then the chip, and the row's tooltip still names both.
- Loading fields take the shape of what loads (SPEC-120, ADR-0097, task #106). The Git page loaded
  as three grey bars under its header, and every list, table and editor did the same. Now each
  loading state is drawn with its page's own layout. The Git page shows its two status cards,
  Blocked, the board with the profile's columns and Activity. Lists of cards load as cards,
  tables as tables with their columns, editors as their section cards, Sessions, CI and the
  Dashboard as their own layouts, and the sidebar and Tasks lists as task rows (two lines on a
  phone) under their real toolbar and column names. What loads lands where the fields were;
  Subscriptions now reads its capacity beside the subscriptions, so the panel above them no
  longer pushes them down.
- Notes feel like a shared document (SPEC-112, ADR-0091, task #97). The Notes tabs go in any
  order: drag one along the strip, press Alt+← / Alt+→ on it, or use its context menu; the order
  is kept per browser, and until someone moves them Team comes first and opens first. A selection
  in a notepad is drawn by Yulius, one rounded shape per line gliding from one selection to the
  next. Teammates who have the Team or a sandbox notepad open show up in it, each in a colour of
  their own, with their caret, their name and their selection, moving as they move and fading
  out when they leave. The server relays where people are (`PUT …/notes/presence`,
  `note.presence`) to the notepad's readers, stores nothing and takes at most 20 updates a
  second per person and notepad.
- The name beside the logo is set like the quote above the composer (SPEC-123, ADR-0100, task
  #110). It used to read "YULIUS" in bold, widely spaced capitals of the interface face. Now it
  reads "Yulius" in Tenor Sans 400 at the quote's 22 px with no extra spacing, in the sidebar and
  on the sign-in and setup screens.
- An agent secret is a name, a value and a description (SPEC-121, ADR-0098, task #107). The add
  dialog no longer asks for hosts or for how the secret is added to requests. Instance
  administrators find the scope under More options. Agents send the `$NAME` placeholder in
  whatever header or query the service wants. They work out the service's host and ask for it
  with `secret_request_hosts`. The Secrets page shows "Task #N asks to send it to …" with Allow
  and Dismiss, and nothing is sent anywhere before a person allows it. Allow names the hosts the
  person saw, so a host an agent asks for meanwhile is never allowed unseen. API change:
  `POST /api/sandboxes/{sid}/agent-secrets` now defaults to no hosts and `injection: "none"`
  (it was `bearer`).
- A task starts with one message, before anyone can type (SPEC-117, ADR-0095, task #102). A goal
  task used to open as an empty session. Yulius then typed a first message with the attached
  images and, after its turn, a second one with `/goal`, and a person could type into the session
  the whole time. Now Claude Code is launched with `/goal <condition>` as its prompt: one message,
  nothing typed. The condition names the request file and the attached files, which the agent
  opens with its Read tool, because a `/goal` drops pasted images (measured on Claude Code 2.1.292).
  A prompt task with images still has its first message typed, since a paste is the only way to
  attach an image natively. Until it is sent, a person's keys are not written into it, and the
  first one is answered with "Yulius is typing this task's first message".
- The new-task composer squeezed keeps its controls tidy (SPEC-119, task #104, #442). In a
  narrow window or beside a tool panel, Preview, model, effort and Start used to wrap as one block
  pushed to the right, leaving the left half of the row empty. At its narrowest, Start also stuck
  out of the box. Now they take a full-width last row with Start at its right end, and no control
  leaves the box.
- Typing in a task terminal or a shell shows up as fast as the agent echoes it (SPEC-118,
  ADR-0096, task #103). A keystroke used to wait for three database reads, plus a GitHub call
  whenever the permission cache had expired. The 60 s access re-check also froze the terminal's
  output while it ran. Now each viewer holds a write grant, checked again only when something that
  decides access changes, and the re-check runs beside the stream. A hand-over still refuses the
  previous owner's next keystroke.
- Onboarding leaves the project ready to work in the discipline (SPEC-116, ADR-0094, task #101).
  Studio's onboarding left its linter, dead-code check and per-project CI as issues #57 and #58.
  Both onboarding prompts now lay the gate in the onboarding pull request itself:
  - tests, lint and format;
  - a dead-code and unused-dependency check;
  - a Yulius CI job for every project, with existing findings baselined.

  A workflow Yulius CI cannot run (Windows, macOS) is kept with a decision record instead of a
  question to a person. The agent learns that it and CI run as root.

  `delivery_merge` now pushes the head and opens the pull request when the task has none. A goal
  task whose evaluator kept every turn going never got its pull request, so studio's agent had
  pushed PR #59 by hand.

  The briefing quotes the end of a changes log whose dates run oldest first (studio's
  `knowledge/log.md`) and asks for new entries there.
- A shown secret value stays masked until its eye is pressed (SPEC-114, ADR-0093, task #99). The
  Show dialog of a team or agent secret draws a fixed run of dots and keeps the value out of the
  page; the eye button in the value box shows it and hides it again, and every dialog opens
  masked. Hovering the value (or focus in it) shows a copy button in the box, which copies even
  while masked; the Copy button beside it stays.
- Screens keep their state when you switch away and back (SPEC-113, ADR-0092, task #98, #435).
  Studio's tab used to open New task after a visit to another sandbox, whatever was open there.
  Now a sandbox tab, Home's card and the phone picker return to the screen last shown in that
  sandbox, with its filters; the sidebar's pages and the Tasks and Git back links return to the
  page's filters (the tab or link of where you are still opens it plain). Each sandbox reopens
  the tool it had (or none), and its Superagent, Browser, Editor (with unsaved text) and Shell
  survive the switch for the 3 sandboxes used last. MCP's scope and the Dashboard's period are
  now in the address too.
- Start onboarding goes straight into the onboarding session (SPEC-111, ADR-0090, task #96). The
  press turned nothing visible for seconds while the server started the task's run (the button
  stayed pressable), then stopped on "Onboarding <name>" with Follow onboarding. Now the gate turns
  into "Starting onboarding <name>" loading at once, with nothing to press, and opens the
  onboarding task when the start answers; a failed start brings the gate back. That Follow
  onboarding screen is gone: opening a sandbox while its onboarding runs opens its task.
- The Git page beside a tool panel stays readable (SPEC-115, task #100). Its summary cards and board
  columns now respond to the page's width, not the window's. Beside the Editor, the board goes from
  four columns to two or one, and task titles no longer break letter by letter.
- Token use, its API-price cost and a chart for every figure (SPEC-110, ADR-0089, task #95). The
  Capacity card (Subscriptions page and the Dashboard) adds "Tokens (7 days)" and "API cost (7
  days)": what the pool used and what it would have cost at Claude API list prices (input, output,
  cache reads and writes, per model), each with a 7-day chart; Burn rate and Paused get one too.
  Each subscription's "Observed by Yulius" rows end with "≈ $X at API prices". The Dashboard's
  Agent work per task, Agent working time, Tokens and Subscription switches figures get a chart of
  the range's days, like Tasks; every chart names its first, highest and last values for screen
  readers.
- A pull request's change reaches the sidebar when it happens (SPEC-108, ADR-0087, task #93). Task
  #56 read Done with PR #340 "checks running" 4.5 minutes after Yulius had merged it: the task's
  state came live, its pull request only from the Git overview read every minute, never while the
  tab was hidden. The server now publishes `pulls.updated` to a sandbox's viewers when a delivery
  stage moves (opened, merged, reconciled with GitHub, by any path) and when a Yulius CI run is
  queued, starts or ends; the browser reads the overview again within seconds (at most one such
  read per 3 s), or when a hidden tab is shown again. Reads of a sandbox no longer overlap, and a
  change announced during a read brings one more read after it, so an older read never has the
  last word. A pull request Yulius recorded as merged reads merged even while GitHub's list, read
  just after the merge, still says open.
- Onboarding no longer gets stuck (SPEC-106, ADR-0086, task #91, #425).
  - **Trust prompt.** A task in a repository whose `.claude/settings.json` pre-approves tools
    starts without Claude Code's folder-trust prompt: its main repository is trusted too, where
    Claude Code 2.1.292 looks a worktree's trust up. If a trust prompt still shows, Yulius waits
    for a person to answer it and then sets the goal; the answer no longer counts as a draft.
  - **Lost onboarding task.** A sandbox whose onboarding task was deleted goes back to "Onboard
    first" with Start onboarding. While onboarding is in progress, its task can't be archived.
  - **Healing.** Sandboxes stuck this way (studio) heal at the next server start.
- A session's spider web hangs exactly in its corner and keeps its colour while the transcript
  scrolls (SPEC-109, ADR-0088, task #94, issue #427). It used to start 16 px above the well, over
  the header, and to fade nearly into the background once stuck.
- "Update now" works again with the GitHub App (SPEC-107, task #92, #423). GitHub now issues
  installation tokens of about 520 characters (`ghs_<APPID>_<JWT>`). The update helper refused
  anything over 255 as "the token is malformed"; it now accepts up to 4096 of the same characters.
- A session's spider web stays in view, its spider moves and a cockroach runs by (SPEC-101,
  ADR-0082, task #86). The web sticks to the top-right corner while the transcript scrolls. Every
  8 s the spider climbs its thread in pixel steps, legs switching, and drops back. Now and then (20–60 s
  after opening, then 45–150 s apart) a pixel-art cockroach runs across the visible part of the
  session and is gone. Nothing moves under reduced motion, and the cockroach stops when the
  session is left.
- Playgrounds: a team tries Yulius on its own code before it connects GitHub (SPEC-099, ADR-0081,
  task #84). On a fresh instance Home says "Try Yulius on your own code": drop a project folder
  (a repository keeps its branches, tags and history; its uncommitted work lands as one commit) or
  start from a template, and the playground opens with tasks startable at once. Its Git stays on the
  server (a bare repository outside the sandbox, out of its processes' reach), nothing is pushed anywhere, and only the history
  part of an uploaded `.git` is taken, never its config or hooks. Its tasks merge into its own
  `dev` with `delivery_merge` (a squash after the spec, steps and clean-worktree gates; no pull
  request or Yulius CI), and their briefing and definition of done say so. It is its maker's and
  the superadmin's. Connecting GitHub is optional: the sign-in toast is gone, a notification says
  playgrounds work without it, and Import and Create follow once it is connected.
- A task can be prepared now and started later (SPEC-105, ADR-0085, task #90). New task's start
  menu offers **When I decide**: the task waits with the new status `prepared`, holding no
  session, until you press Start now in its view or menu. **At a time…** picks a date and time:
  the task starts by itself then, within a minute (after a restart, at the first minute past it;
  while onboarding blocks the project, once it is ready). Its row reads "Prepared" or
  "Scheduled · tomorrow 09:00", sits with the queued tasks, and its view offers Start now,
  Schedule… or Change time…, and Cancel. The automation editor's shared start menu does not offer
  them yet (#416). The API takes `prepared` and `start_at` on New task and
  `task_create`, shows `start_at`, and `PUT /api/tasks/{tid}/schedule` moves or clears the time
  (schema version 24, `task_schedules`).
- After a reload, the sidebar shows pull request chips at once (SPEC-104, task #89, #414). Before,
  they appeared seconds later, when the sandbox's Git overview answered, and rows could move
  between Working and Done meanwhile. The browser keeps what the last overview read said, per user
  and sandbox. It shows that until the page's own read replaces it, and drops it if that read
  fails. Sign-out or an expired session removes it.
- Notes has a third tab, **Me**, after Team (SPEC-103, ADR-0084, task #88): each person's own
  notepad, which nobody else can read (`GET/PUT /api/me/notes` always answer the caller's own) and
  which is the same in every sandbox. It saves and resolves conflicts like the other notepads, and
  an edit from the person's other tab or device shows up live: the event stream now addresses an
  event to one person (`Envelope.user`) and drops it for everyone else.
- `yulius onboard` no longer leaves a "Closed · 0/6 steps" onboarding task behind (SPEC-097,
  ADR-0077). Linking a repository creates no task: the onboarding task is created when someone
  clicks Start onboarding in the browser, or when a new task has to wait after it. Sandboxes
  already waiting keep their task. The onboarding page still lists what onboarding will do, now
  from `GET /api/sandboxes/{sid}/onboarding/plan`.
- The start menu's tasks show five at a time and say there is more (SPEC-102, ADR-0083, task #87).
  "After a task is done" lists its tasks in a box five rows tall that scrolls the rest, so Start
  now, Supertask and "When a rule is true…" stay in view however many tasks a project has. The box
  fades at the edge it continues past and a "↓ N more" line under it counts the tasks below (a
  click scrolls a page); the picked task opens in view (and focused when the menu has no filter
  focused), and the arrows and the filter work inside the box. The automation editor's start menu does the same. `openMenu` takes such a
  group as `{ header, items }`.
- The automation editor's start choice is a chip in its action bar, before Run now (SPEC-093,
  ADR-0074, task #78). It looks and works like New task's start chip and opens the same menu, now
  one shared module: Start now, Supertask, the filterable "After a task is done" list and "When a
  rule is true…". The Start row in "The task" is gone, and the chip's tooltip says what the choice
  means. Run now starts its task the way the chip shows, saved or not: `POST
  /api/automations/{id}/run` takes an optional start choice for that run only, checked like a
  save. On a phone the bar wraps below the name, and readers see the chip disabled.
- A repository's `.mcp.json` no longer stops the Superagent on Claude Code's "N new MCP servers
  found in this project" dialog (SPEC-095, ADR-0078, #391). Before each task run and Superagent
  start, Yulius adds the file's servers that the sandbox lacks to its MCP servers, so every task
  gets them. Since agents can write the file, a `${VAR}` value becomes the placeholder of the agent
  secret VAR, which the egress proxy replaces on the secret's bound hosts; the file never gets a
  clear value. The Superagent starts rejecting the file's servers through `--settings`, including
  ones a person had approved there (#395 will give it Yulius's servers). A server already
  configured is left as it is, so one turned off stays off. An MCP server's values now reach a run
  in `YULIUS_MCP_<SERVER>_<VAR>` variables. Before, one named like an agent secret, such as
  `APIFY_TOKEN`, clashed with that secret's placeholder, and every run of the sandbox failed to
  start.
- Onboarding is mandatory (SPEC-096, ADR-0076, task #81): while a sandbox awaits or runs its
  onboarding, its new-task page shows no task input. A centred message says what onboarding will
  do (its task's own plan, which differs for a repository already in the Yulius format), with
  Start onboarding, or Follow onboarding while it runs (waiting for capacity and a stopped task
  are said too). The task box shows by itself once the sandbox is ready. The page no longer offers Skip;
  the admin API keeps it as a recovery path. Tasks created through the API still wait after
  onboarding.
- The terminal's mouse is the program's when it asks for it, as in a native terminal (SPEC-098,
  ADR-0079, task #83). The "Mouse: select text · send to app" chip is gone. In Claude Code's
  full-screen view, clicks, drags, the wheel and right-clicks reach Claude Code, and Shift-drag
  (Option-drag on macOS) selects text; the same key brings the Copy and Paste menu. A read-only
  viewer, who cannot send anything, keeps the mouse for selection. A press beside another viewer's
  grid reaches no program.
- A task waiting for its checks or merge shows a still, muted branch (SPEC-100, ADR-0080, task #85,
  #393). The "Waiting for checks or merge" state took Working's tone so that it notified nobody,
  and that tone turned its git-branch icon green and spun it like the Working ring. It now takes
  the muted tone of Queued, Chained and Paused; it stays under Working and still notifies nobody.
- Loading shows shimmering fields everywhere, never a spinner with "Loading…" (SPEC-092, ADR-0073,
  task #77). `placeholder(text, 'loading')` is the skeleton, so the Git page, dialogs, Sessions,
  the editor pages and every `loader()` list draw bars where the content will appear, with what
  loads said to screen readers. The task lists, the status popover, Instance, the Jev key card,
  the Superagent's start, the editor tree, home cards, notepads, the browser panel's cover and
  usage not read yet do the same. Spinners stay for actions under way, connections and working
  tasks. The mock's `hold=<path>` and `hold=frames` keep a view loading for tests.
- Menus show an option's whole description (SPEC-094, ADR-0075, task #79). A description wraps
  onto as many lines as it needs instead of ending in an ellipsis, in every `openMenu` menu; menus
  keep their width (at most 320 px and the window less 16 px), so a short description stays one
  line. The start menu's Supertask now reads "Waits for this project’s running tasks, then runs
  alone while the project’s other tasks wait. For broad work such as security hardening."
- Runs recorded before SPEC-087 get their metrics from their transcripts (SPEC-091, ADR-0072).
  The upgrade queues every run recorded so far (migration 23, `metrics_recounts`). On the first
  start, once reconciliation is done, Yulius counts each ended run's own window of its transcript
  in the background: from its start to its end or the next run of its session, its sub-agents
  included. It uses the live watch's rules, so turns are Claude Code's turn records and active time
  is the union of turn spans. Runs whose transcript is gone keep their figures. Each run's outcome
  and the figures it replaced are recorded, and one `metrics.recounted` audit entry sums the round.
  It runs once: later starts change nothing.
- A task's squash commit names the change, not its spec (SPEC-069 BPL-008): the pull request's
  default title takes the type and scope of the branch's most significant commit (feat, then fix,
  and so on) instead of its oldest, and before Yulius merges it renews a title it gave from the
  final commits, so #381's `docs(spec)` would have read `fix(check)`; a title a person or agent
  wrote is kept. A GitHub outage only delays the merge (it stays armed); a title GitHub refuses
  stops it, and `yulius wait` says how to set one.
- A background agent that stops while its own background work still runs no longer ends in
  Yulius's eyes (SPEC-079 BGW-006, task #74). Claude Code's note says its result may be interim
  and the same task id notifies again. The task stays Working until that final notification, or
  an hour without it, and a switch waits for it; before, the task read "Your turn" in between.
- The session view folds long injected context as Claude Code's TUI hides it (SPEC-090, ADR-0071,
  #347): a meta note over 8 lines or 2,000 characters, such as a skill's body, is one faint
  `⎿ Context added (6.2 KiB)` line that opens on demand. Shorter notes people act on, such as the
  Stop hook's goal and feedback, stay as written; `session_read` is unchanged.
- `yulius doctor` warns when the host's GitHub CLI is older than 2.82.1 (SPEC-088): since GitHub
  retired Projects (classic), `gh pr edit` and `gh issue view --comments` fail on older versions,
  on the host and in every sandbox. Unrecognised version lines stay OK. The warning and `docs/operations.md`
  point to GitHub's package repository instead of `apt-get install gh`.
- The repository check fails on committed merge conflict markers (SPEC-089, #364): a line that is
  a Git conflict marker in any tracked or new file, vendored assets and binaries aside, fails
  `scripts/dev check` and Yulius CI with its file and line, where before markers in docs and web
  assets passed every gate.

## 2026-10-13

- Tests no longer time out under load, and every process plans for its share of the machine
  (SPEC-082, ADR-0065):
  - The fake agent runs Claude Code's SessionStart hook like the real CLI, so 17 goal tests no
    longer sleep the 45 s hookless fallback: about 980 s of the suite's 1 742 s of test time; the
    slowest went from 105 s (nextest kills at 120 s) to 19 s. The queued-message test waits for a
    session slot for real instead of racing the queue. Four e2e tests no longer race the page:
    they move the keyboard once the task's terminal has taken it, act once the list or the file
    has loaded, and leave the other viewer's grid a margin whatever the fonts.
  - Every CI job, agent run, terminal and Cockpit gets a CPU and memory budget in `YULIUS_CPUS`
    and `YULIUS_MEMORY_MB`, and in the variables a repository maps in `[resources.env]` of
    `yulius.toml` (`NEXTEST_TEST_THREADS = "{cpus}"`). CI jobs declare `cpus` and `memory_gb` in
    `[[ci.jobs]]` (else they share the sandbox equally) and start once their budget is free, so
    the jobs of all lanes never ask for more than the sandbox has. Agents plan for a third
    (`YULIUS_AGENT_CPUS`). Yulius no longer sets `CARGO_BUILD_JOBS` itself.
  - Sandboxes have no CPU quota by default (it throttled them half of their busy time while two
    CPUs idled); `YULIUS_SANDBOX_CPUS` still sets one.
  - This repository maps the budget to Cargo, nextest and libtest, drops the fixed 4 test threads
    and the CI job's one thread per CPU, and runs e2e with half its budget as workers.
- Armed pull requests merge in trains (SPEC-084, ADR-0067). When two or more armed pull requests
  wait and none can merge at once, Yulius builds their combined result in its clone, in arm
  order. It merges as Yulius and GitHub will, merge drivers included, and runs CI once on it.
  When that run passes, Yulius merges them one after another. Each is brought up to date with
  exactly the predicted tree and gets the train's verdict, so a strict ruleset accepts every merge
  without another run.
  - A pull request that conflicts with the ones before it is left out, told so, and waits for
    them.
  - A red run, or anything unexpected, sends every member back to the one-by-one flow.
  - A train survives restarts (`merge-trains.json`).
  - Passes deciding armed merges take turns. Two runs ending together used to merge one pull
    request while the other pass merged a branch it had read as up to date. GitHub refused that
    merge as behind, and the armed merge was dropped.
  - An armed branch still behind because its agent is busy waits instead of being refused and
    dropped. An agent's own clean merge of `dev` keeps its armed merge (SPEC-083 REV-001): it
    follows the new head instead of being dropped as "the branch moved on".
  - On 2026-10-12/13, 42% of the armed merges waited at least one extra run after their last arm
    (7.4 hours in all), and runs at updated heads took 44% of the CI time.
- `/compact` typed into a session leaves its task as it was (SPEC-085 STT-005, ADR-0068): Claude
  Code's line for the command no longer reads as a prompt, so an idle Done task no longer reads
  Working after the compaction; a prompt typed after it is work as always. While Yulius itself
  recovers a full session the task reads Working, not Your turn, until it types the prompt that
  carries on (STT-006, CTX-002 changed).
- Every time next to a task says what it measures, and the dashboard's figures are the agents' real
  work (SPEC-087, ADR-0070). The sidebar showed the time since a task's last change of anything: a
  task working for two hours, woken by its own background work a minute ago, read "1m".
  - Each group answers its own question in words, with the exact times on hover: "working 1h 20m"
    (since it last started working without needing anyone), "waiting 12m", "queued 2h", "paused
    5m", "in review 3m", "done 3h ago", "stopped 2d ago". The sidebar, the Tasks page ("Created",
    "For") and a task's overview ("Created", "Now", "Agent work … sessions open") use one rule. The
    server keeps the times: `state_since` and `working_since` (schema version 21).
  - A run's tokens, turns and active time come from its own transcript lines, so a restart no longer
    zeroes them and a resumed run no longer loses its first lines or, after a restart, everything:
    41 of 109 runs stored no active time, 17 no tokens. Active time is the union of the turns'
    spans, a turn's wait for its own background agents included. Sub-agents' tokens (34 % of
    output) count, and turns are Claude Code's turns (they were every answer, 2.3 times too many).
  - The dashboard shows "Agent work per task" (median of the tasks' active time) and "Agent working
    time" beside "sessions open, idle included", instead of the median time sessions stayed open; a
    running task whose goal is achieved counts as done in the outcomes; its days are said to be UTC.
- Parallel tasks stop colliding at merge (SPEC-083, ADR-0066). With 8 to 12 agents, numbered
  records, the shared logs and reviews of dev merges failed or delayed most merges (81 decision
  records renamed in 36 of 74 sessions; 33 of 57 merges spent a review round on dev merges or
  renumbering):
  - `record_number` gives each task the next free number of a directory's numbered files (ADRs,
    migrations, any `NNNN-…` or `PREFIX-NNN-…`), past the integration branch, every live task's
    checkout and every number given out before, and reserves it (schema version 20). Spec
    numbers come from the same reservations. The briefing points agents to it.
  - The changes log and the decision index merge by Git's union driver in every repository:
    Yulius keeps them in its block of the clone's `info/attributes` at every task start, so an
    onboarded repository needs no `.gitattributes`.
  - `delivery_merge` accepts the review of an earlier head when the branch moved past it only
    through Git's own merges of the integration branch; anything else still needs a new review.
  - Pull requests whose merge is not armed are not updated by Yulius: GitHub's conflict state
    blocks nothing, and each push would cost a CI run.
- A task's status tells the truth: Working is work, Needs you is a person's move (SPEC-085,
  ADR-0068; F 2026-10-13 "marked as running even if was done", "needs you was too often"). A
  replay of two days of transcripts found "Needs you" false for 43 of its 50 task-hours:
  - An agent waiting in `yulius wait` for its checks or armed merge reads "Waiting for checks or
    merge" under Working, not Your turn (173 periods, 37 tasks). The process probe counts such a
    shell apart; disk upkeep, DLV-203 and Done still leave it out (DSK-102).
  - Failing checks need you only once the agent stopped: while its background agent or shell
    runs, or it waits for Yulius, the task reads "Working · PR #N checks failing" (task #63).
  - Records Claude Code writes for itself start no turn: an idle compaction's summary made Done
    task #60 read Working until it was stopped; so did command echoes and their output.
- Yulius's tools answer in a few readable lines, and sessions fill the page (SPEC-086, ADR-0069):
  - A tool's result was its whole record as pretty JSON, which filled the agent's terminal (task
    #66's steps list) and its context: 6.08 MB in the transcripts of 2026-10-12/13, most of it
    `ci_status`, `step_update` and `delivery_status`. Each tool now says what changed and what is
    next, with the numbers and ids the agent passes back: the plan as a checklist, a step update
    as the changed step, the progress and the next step, CI as a line per run with the output of
    failed jobs only (`job` asks for another), delivery as the stage, the pull request and what
    blocks it. Replayed over those results, 71 % less text. A client that reads records asks
    with `_meta: {"yulius/structured": true}`; Claude Code would show the model a
    `structuredContent` instead of the text, so it is never sent by default.
  - Agents read files with `cat`, `sed -n` and `grep` because Claude Code tells them to in
    bypass mode. Measured, it costs nothing (0.3 % rereads, smaller reads than `Read`, no failed
    edit), so it stays.
  - A session's transcript fills the content area edge to edge, without side margins, border or
    rounded corners, beside the Superagent too and on phones; the header keeps its padding.

- The original prompt's thumbnails always load (SPEC-080, ADR-0064, task #63; #303). Reading a
  prompt's attached file no longer takes an upload slot: reads have their own bound of 16 and wait
  up to 30 s for a slot, so a prompt with many images, or one opened while uploads run, shows every
  thumbnail. The file is streamed in 64 KiB chunks under its slot; a stalled reader gives the
  slot back and its file is closed after 60 s. On Linux a file whose `/proc/self/fd` link cannot be read is refused. A prompt
  starting with `---` shows as text, not as properties, and the mock serves only the task's own
  attachment folders.
- An idle task's background work and its goal verdict decide its status (SPEC-079, ADR-0063, task
  #60; #305, #264, #265, #271):
  - While a goal is open, a clean Stop leaves the task Working until Claude Code's goal verdict,
    bounded by the turn record or 20 s. "Not met" no longer flashes Your turn or Done, and no chain
    starts from a Done the verdict takes back.
  - Monitor streams count as background work until their stream ends or expires. Claude Code's
    turn record does not count them, and a WebSocket monitor has no process.
  - A proactive switch at a Stop waits while a background agent the session launched is still
    pending, even one launched in the turn that just ended. The switch used to end the agent.
  - When a Done task's last background shell or agent ends, the queue drains at once instead of at
    the next tick. So does a Stop that makes a task Done. Claude Code's wake-up prompt no longer
    takes back an achieved goal, so the task woken by its finished work ends Done again.
- An automation starts its tasks like New task (SPEC-078, ADR-0062, task #70; SPEC-001 AUT-013).
  The builder's **Start** row, the API (`supertask`, `after_task_id`, `start_rule`) and the tools
  `automation_create` / `automation_update` pick one of four choices: Start now (queued, the
  default), Supertask, After a task, or When a rule is true. Each run creates its task with that
  choice. After a task that has ended, the task waits for the rule `#N done` instead of failing.
  The sentence and the list row name the choice, and a waiting task keeps the next runs skipped.
  Schema version 19.
- Home's cards and the sandbox tabs count "working" and "queued" by the sidebar's rule (SPEC-076,
  ADR-0061, task #62; #170). One domain rule (`AttentionFacts::group`) gives each task its sidebar
  group: "queued" counts queued and chained tasks, "working" the rest of the Working group
  (running, starting, continuing, paused, in review), so a card's working + queued is the
  sidebar's Working count. A Done task or one being stopped counts in neither, and a task being
  stopped no longer counts as needing you.

- Resuming after a usage limit is safe in three more cases (SPEC-072, ADR-0058, task #57; #276,
  #277, #278):
  - Claude Code sometimes continues a turn by itself at the reset. Its first agent work or turn end
    now ends the task's limit pause, so Yulius no longer terminates a turn the CLI is running. After
    a restart, a limit the CLI already continued past is not acted on again.
  - When the automatic switch after a limit is refused, the run remembers it is at its limit. A
    person's switch then resumes past the limit, without "Continue the task exactly where you
    left off.".
  - The transcript step back runs on a blocking thread, off the async runtime. The session's
    search index is rebuilt only if the step back changed the file.

  New tests cover a resume past a limit that finds no capacity, a limit pause whose process is
  gone, and goal mode after a continued turn.
- A task is owned by one person, whose GitHub credentials it uses (SPEC-070, ADR-0060, task #66).
  Every task has an owner, its creator until a hand-over. The task's Git, `gh`, pull requests,
  merges, commit identity and agent authority are the owner's. Only the owner changes the task:
  anyone else, administrators included, gets 403 naming the owner, watches the terminal
  read-only and cannot write into the task's worktree or open a shell there. They can ask for control (Request control in the task). The owner hands it over or
  declines, and the asker may withdraw. From the hand-over on, the task acts on GitHub as the new
  owner, and its terminal says so. An owner who is disabled, deleted or without write access
  passes control at once. Schema version 18 (`tasks.owner_id`, the waiting request).
- A session opened from Sessions reads as Claude Code's TUI printed it (SPEC-075, ADR-0059, task #71).
  Prompts and slash commands show as `>` lines and messages and tool calls as `⏺`, with each result
  under `⎿` folded to its first lines ("Read 2 lines", "… +2 lines"). Edits show as `Update(path)`
  with their lines. The raw `<command-name>` and `<local-command-stdout>` tags are gone, and
  reminders are hidden. The transcript is faded in both themes: fainter and less saturated than a
  live terminal (`--session-*`), yet still WCAG AA. It ends with a locked prompt box, and a
  pixel-art spider web hangs in its corner.
- The Agent panel is now called the **Superagent**, and a usage limit no longer stops it (SPEC-074,
  ADR-0055, task #69). The Superagent chose a subscription once, when it started, and kept it, so
  it could hit a weekly limit while tasks went on elsewhere. It now watches its conversation. A
  limit recorded after it started is recorded on its subscription, and the Superagent starts again
  on one with capacity. It continues the cut turn there, stepped back over the limit as a task's
  turn is, and the panel follows it without a "stopped" banner. Without capacity, it stays and
  says why. `/s/<slug>/superagent` opens it, as `/agent` and `/cockpit` still do.
- Supertasks (SPEC-071, ADR-0054, task #67; SPEC-001 TSK-018). Choose **Supertask** in the
  composer's start menu (API `supertask: true`, `task_create`) for broad work such as security
  hardening. The task waits for its project's running tasks, then runs alone. While it waits or
  runs, no other task of the project starts, also tasks queued or chained before it; they wait
  `queued` and say "Queued after supertask #N". A queued supertask says "Supertask · waits for
  #3 and #5". Running sessions are never interrupted, and other projects keep running. At the
  session limit, a supertask gets the next free slot first. Schema version 17
  (`tasks.supertask`).
- Cockpit and terminal starts survive a dropped request (SPEC-077, ADR-0053, task #64; #250). The
  part of a start from the spawn to the registration runs as a task the request awaits, so a
  client that goes away mid-start leaves one live Cockpit or terminal (or nothing on an error),
  never a second Cockpit, an unusable terminal or an orphan run directory. Idle Cockpit start
  locks are dropped. New tests pin that a signal right after a resume keeps the conversation and
  that one user's blocked start delays no other user or sandbox.
- A task's step strip no longer draws its progress bar over "Step N/M" (SPEC-073, task #68; #341).
  A long state such as "Working · 1 background task still running" or "Done · PR #N merged" kept
  its full width, so the strip's label shrank to nothing and the step count, which nothing
  clipped, ran under the bar. The same happened with a short "Done · PR #N" in a narrow strip
  beside a tool panel. The label now keeps room for the step count, the state gives its words up
  first (it takes at most 40% of the strip, and its full text stays in the tooltip), and the label
  is clipped inside its button.
- A CI run that could not run (disk, a timeout, the host) reads "CI could not run (not the code)"
  instead of "checks failing · Needs you"; an armed merge stays armed and the agent is told to run
  the checks again (SPEC-050 STA-101).
- Local test runs use 4 test threads (CI keeps one per CPU): several agents running the suite
  at once in one sandbox starved their tests into timeouts.
- Only real typing counts as typing; a typed first message is never lost (SPEC-068, ADR-0052,
  task #61, #252, #266). Focus and mouse reports and terminal answers no longer record typing or
  take control of the terminal size. A goal task whose terminal a person typed into before its
  goal leaves the draft alone and needs attention, instead of appending `/goal` to it. A first
  message with images that a server restart cut flags its task with a notice; one held for
  capacity and resumed (or manually switched) to another account is typed again there, images
  included.
- Yulius's merge gate enforces the branch policy (SPEC-069, ADR 0051, task #59; #223). Before
  Yulius merges a task's pull request (`delivery_merge`, an armed merge, the Git page) or a
  promotion pull request, it checks the title, head and base against `yulius.toml`
  `[branch_policy]` at `dev`. The standard preset is the default: typed branches, conventional
  one-line titles of at most 100 characters, and `dev` → `main` titled
  `release: promote dev to main[ - label]`; `preset = "none"` turns it off. A violation is
  refused with the rule in words, and `delivery_merge` refuses before it arms. Yulius's own
  titles comply: task titles take an allowed type and are clipped to 100 characters, promotion
  pull requests are titled `release: promote dev to main`, and a fork's pull request is never
  taken for the promotion. `scripts/check_git_policy.py` is removed.
- A stopped or closed run ends only when its processes are proven gone (SPEC-065, ADR-0050, task #54;
  #284, #285, #290). When Yulius cannot terminate a run's host after the stop or close escalation,
  the run stays live, the task's terminal says so and a running task needs attention; deleting it
  answers 409 and removes nothing. Deleting a Done task now returns only after the run's end is
  fully recorded, so a task chained after it gets one outcome. The wait re-checks every second even
  on a busy instance, a stuck host can no longer cancel the escalation, and a task that turned
  switching is refused at once. The web UI sends one delete per task, shows "Deleting #N…", and a
  stopping task's Delete says "Stopping…".
- Five flaky tests wait on what they check (SPEC-067, task #56; #243, #320, #322, #294, #269).
  Each was reproduced next to CPU-bound processes, and none got a sleep or a retry. Two were
  product bugs:
  - A task's header strip now follows its pull request read. A task whose pull request is tested
    reads "Done · PR #N" even when that read lands after the view opened. Before, it kept "Your
    turn" until an unrelated event redrew it.
  - A SessionStart hook answered before Yulius watched the new run was forgotten. The task's goal
    then waited 45 s before it was typed. The flag is now reset before the agent is spawned.

  The slash-menu, Auto, stored-draft and terminal-link tests now wait for the composer's focus,
  hold the creation, store the draft before the page opens, and wait until xterm knows the link
  and the Editor has loaded. The dropped-goal test sends Claude Code's SessionStart hook: its goal
  starts after 16 s instead of 60 s.
- The Cockpit and task agents manage the whole task and automation lifecycle (SPEC-066,
  ADR-0049, task #58, #275, #293). New MCP tools `task_stop`, `task_archive`, `task_delete`,
  `automation_delete`, `automation_run_now` and `automation_template_enable` do what the UI
  does, for the person the caller acts for, audited with the caller (`task:<n>` /
  `cockpit:<login>`); a task never stops, archives or deletes itself. `automations_list` lists
  the ready-made automations (`ready_made`). The Cockpit is served them all and asks the person
  before each. Workflow create and edit stay UI-only.
- `yulius wait` on armed merges (SPEC-064, ADR-0048, task #53; #248, #249, #255, #251). A merge
  armed while Yulius still decides an earlier one supersedes it: the wait on the new head no
  longer ends with the old head's "dropped", and the new merge stays armed and saved. The armed
  merge, its decisions in progress and its outcome are read and changed under one lock, so a wait
  never misses a decision. A wait with a merge armed and checks pending keeps waiting (#249 did
  not reproduce on `dev`; a regression test now covers it). The automatic retry of an errored
  Yulius CI run at a pull request's head is tested: once, never a third time, never when the pull
  request conflicts.
- CLI device sign-in can no longer fill the disk (SPEC-063, ADR-0047, task #55, #169).
  `POST /api/device/start` is limited per client like password sign-in (5 starts a minute); at
  most 10 device sign-ins wait per client and 1000 per instance (past either: 429, nothing
  evicted); expired and consumed grants are deleted on every start and every minute. Schema
  version 16 (`device_grants.client_key`).
- A session whose context is full recovers (SPEC-060, ADR-0046, task #51, #213). Claude Code's
  "Prompt is too long" ends the turn, so the task waits instead of staying `working`. Yulius then
  types `/compact` with a focus built from the task (goal, branch, pull request, steps left) and
  wakes the compacted session. If the compaction fails, the task continues in a fresh session
  opened with a handoff. If the fresh session is full too, the task needs attention and says why.
  Each step shows in the task's terminal and the audit log (`task.context_full`).
- Unique decision numbers and the onboarding CI proof (SPEC-058, task #49; #317, #272). The four
  decision records numbered 0034 now have their own numbers: the automations catalog keeps 0034, the
  Agent in the right panel is 0036, the personal two-factor vault 0042 and the task status from
  Claude Code's records 0044 (after #333 took 0041); the archive-all record (#332), a later second
  0040, is 0043. `scripts/check_repository.py` fails on two records with one number, a title whose
  number differs from the file name, or a record missing from `docs/decisions/README.md` (0016 was).
  The onboarding prompt and infra-bootstrap step 2 add `YULIUS_CI_MATRIX` to the proof of matrix
  jobs and say how to clean up after an interrupted proof: the proof prints its directory, the agent
  removes its own by that path and other tasks' `~/.ci-proof.*` only when over six hours old, then
  runs `git worktree prune`. They also say that checkouts are per CI lane while by default up to two
  runs share the CI home and cache, and that system packages are installed by onboarding, never by a
  CI job.
- CI waits for disk instead of erroring (SPEC-059, ADR-0045, task #50; #240, #256, #258, #245). A
  CI run in a sandbox over its disk quota stays queued, with a `pending` status. It says who holds
  the space, and starts by itself once Yulius has pruned the caches of tasks that do not work, or
  other tasks have freed space. An armed merge keeps waiting for it. Quota refusals of sessions
  name the largest consumers too. Upkeep now probes a task's background shells again before it
  prunes. A running background build or subagent, or a probe that cannot answer, keeps the
  task's caches.
- Two-factor fixes (SPEC-061, ADR-0041, task #52, #291, #295). A lost authenticator no longer
  locks someone out of the secrets for good: an instance administrator or the superadmin resets
  their two-factor from the Users page (a confirmation, then their own code), and the person sets
  it up again on their next reveal. The reset is in the secrets activity (`vault.2fa.reset`).
  Instance administrators now see the Users page (read-only, with each person's two-factor). A
  code now confirms only the seed it was checked against, so a new key set up in another tab
  while a code is checked is not confirmed by it. API: `DELETE /api/users/{id}/two-factor`;
  `AdminUser` gains `two_factor`.
- Open GitHub issues triaged, and the web UI keeps the person's keyboard, scroll and text (SPEC-056,
  task #47). Issues already fixed on `dev` were closed with the change that fixed them, and the
  other actionable ones went to chained tasks #49–#64. In the web UI:
  - a terminal that shows late leaves the keyboard in the Editor on Chrome (#325), and with a
    terminal asked for after it (#323);
  - Notes shown again for a new page does not take the keyboard (#310);
  - a menu placed again keeps its scroll (#288);
  - the task view ignores an older detail read (#316);
  - the queued messages card keeps unsent text and a refusal when the task starts (#324);
  - leaving the composer revokes every object URL it made (#259).
- Quick archive (SPEC-062, ADR-0043, task #65). A sidebar task row that can be archived shows an
  Archive button beside its `⋮` on hover or focus, and the Done group's head has **Archive all**:
  one click archives every Done task, with one Undo for the whole batch. When some fail, the
  others stay archived and one toast says how many failed and why. Phones keep the `⋮` menu as
  their only row action. No API change: each task goes through `POST /api/tasks/{tid}/archive`.
- Done tasks no longer hold their subscription (SPEC-057, ADR-0040, task #48). A subscription
  card counts only tasks still working on it, so one whose tasks are all Done (goal achieved, or
  plan complete and merged: the session limit's rule) says "No tasks right now" and can be removed. Removing it closes those Done tasks' idle sessions first, as archiving
  does. The pool's load and the Status panel count the same way as the session limit (SES-101).
- CI jobs get the CPU first (SPEC-053, ADR-0039, task #45, #283). A sandbox's cgroup is split
  into `system/`, `agents/` and `ci/`, and CI jobs run with four times the agents' CPU weight. CI
  gets up to 80 % of the sandbox while agents build, and agents keep at least 20 %. The e2e suite in CI is
  back to the default budgets and no retries, so a timeout there is a real failure. The split is checked on the VM after the next update (#321).
- Onboarding follow-up (SPEC-055, SPEC-051, task #46): `docs/api.md` no longer says an import starts the
  preparation task (a `pending` sandbox waits for Start onboarding), ADR-0038's wording and the
  decisions index order are corrected, and the import test asserts the pending onboarding summary.
- Queue messages into a task that has not started (SPEC-052, ADR-0035, task #44). A task waiting
  for a session slot, after another task or a rule, or paused before its first start shows a
  "Queued messages" card: the request as the agent will get it, and a box to queue a correction
  (Queue, ⌘/Ctrl+Enter). Each message is added to the request under "Added by … before the task
  started (… UTC)", so the request file, the goal, the first message, the scope check and
  `task_get` all carry it. Once the task has started, its session is the way in (409). API:
  `POST /api/tasks/{tid}/queued-messages`; `TaskDetail` gains `request` and `started`.
- `yulius onboard` only uploads (SPEC-051, ADR-0038, task #43): the repository, branches, snapshot
  and sessions land, and the onboarding task that adapts the repository to the Yulius standards
  waits until someone clicks **Start onboarding** on the sandbox page. The CLI says so with the
  page's URL; the start endpoint refuses CLI tokens (403). Adding a repository in the web UI still
  starts onboarding at once.
- Dead-code tools (SPEC-054, ADR-0037, task #38): a bundled skill, `dead-code-cleanup`, is how
  agents clean up: the established tool per language (vulture and skylos for Python, from the ML
  in PL workshop; cargo-shear, knip, deadcode and others), a web search when the stack is not
  listed, every candidate verified, small commits. Onboarding and `infra-bootstrap` add a
  dead-code check to a project's gate; the Chore workflow points to the skill. Yulius's own
  `scripts/dev deps` runs cargo-shear instead of cargo-machete and dropped the unused `toml`
  workspace dependency it found.
- "See the original prompt" in the task menu (SPEC-043, ADR-0033, task #42) shows the prompt a
  task started with: the text formatted as written, with Copy text, and the files attached to it
  (an image as a thumbnail that opens full size, another file to download, a file removed since
  named). `GET /api/tasks/{tid}/prompt` returns it; `GET /api/tasks/{tid}/prompt/attachments/{n}`
  serves a file only from the task's own attachment folders, never through a link or from a path a
  prompt names elsewhere.

## 2026-10-12

- Armed merges survive a server restart (a deploy no longer drops them) (SPEC-050 DLV-205).
- Secrets are a title, a value and notes, and reading one needs your own two-factor code
  (SPEC-040, ADR-0042, task #39). The first reveal sets two-factor up: an authenticator key and
  link, confirmed by a first code. After that, a code unlocks the vault in that browser for 10
  minutes. A sign-in or password alone no longer reveals anything. Older team secrets keep every
  field: the password becomes the value, and the address, user name and seed move into the
  notes. Agent secrets lose the per-secret seed and take a user name only for Basic credentials.
  The people who manage an agent secret can now read it after a code. Agents still never see it.
  Ten wrong codes in a row lock codes until the person confirms it is them again. Rolling back
  past this change needs care: the previous release reads secrets saved in the new form as
  having no password, and an edit there would drop the value.
- The Cockpit is now the **Agent**, on the right (SPEC-035, ADR-0036; task #33): a top-bar Agent
  button and the first tool of the right rail open the user's own Claude Code session in the tool
  panel beside the page, under a notice that it is for quick work (setting up automations, quick
  questions) and that anything more complex, such as writing code, belongs in a task (with a New
  task link). It is no longer in the left sidebar; `/s/<slug>/agent` and the old
  `/s/<slug>/cockpit` open the panel. The API and server keep the `cockpit` names.
- A sandbox's default memory limit is the host's memory but a reserve (an eighth, at least 4 GiB),
  not half: at half, ten sessions and two CI lanes thrashed the page cache while the host had 20 GB
  free, and CI jobs timed out (SPEC-050 MEM-101).
- The onboarding prompt tells the truth about Yulius CI (SPEC-020 ONB-001, task #34): the jobs'
  home and `$YULIUS_CI_CACHE` are their own, start empty and persist across runs (they were
  described as "an empty home"), so a job installs what it needs and never relies on an earlier
  run; the agent proves it by running each job normally and in a clean checkout with a fresh
  `HOME` and CI's environment. The `infra-bootstrap` skill says the same in its CI step. CI itself
  is unchanged.
- Menus are compact and filter themselves (SPEC-038, ADR-0030; task #36). Every drop-down stays
  inside the window and scrolls its items. Descriptions take one line, with the full text as a
  tooltip. A menu of 8 or more items opens with a focused Filter box: typed words narrow it,
  ↓/↑ and Enter pick, and Escape clears the filter first. "Run after" lists every live task on one
  line ("#12 Checkout API · Working") under "After a task is done", not just the first 20.
- Notes in the tools rail (SPEC-041, ADR-0032, task #40): a notepad for this sandbox and one for
  the team, below Shell. They save by themselves while people type, with no Save button. Everyone
  who can see the sandbox reads its notes, writers edit them, and everyone signed in shares the
  team's. A teammate's edit appears without a reload. When two people type at once, the later save
  is refused and that person picks Keep mine or Use theirs. API: `GET/PUT /api/notes` and
  `/api/sandboxes/{sid}/notes`, event `note.updated`.
- A pull request runs its own CI jobs only when it changed them itself; a branch that is merely
  behind `dev` runs `dev`'s current jobs (SPEC-050 CI-110).
- `yulius wait` says why Yulius has not pushed or opened the pull request at the end of a turn (an
  open step, a missing plan, an invalid spec, uncommitted work) instead of waiting in silence (SPEC-050 DLV-204).
- End-to-end tests in CI get longer budgets (90 s per action, 6 min per test) and two retries: they timed out while agents' builds loaded the machine, though the same tests pass locally in seconds; CI CPU priority is #283.
- Armed pull requests stay current by themselves (SPEC-050 DLV-203): when the integration branch
  moves on, Yulius merges it into the idle task's branch (the repository's merge drivers apply, so
  a shared changes log no longer conflicts) and pushes a fast-forward; the merge stays armed for
  the new head. A real conflict is left to the agent with the files named.
- Tasks run in parallel and finish without obstacles (SPEC-050):
  - a Done task no longer holds a session slot, so queued tasks start;
  - when Yulius cannot push an agent's head, `yulius wait` says why and how to fix it without a
    force push, and agents bring `dev` in by merging, never by rewriting pushed commits;
  - a shell that only runs `yulius wait` no longer counts as a build, so a waiting task reads as
    waiting and its caches can go under disk pressure;
  - agent commands run below CI and the agents in CPU priority;
  - shells and the Cockpit keep GitHub across server restarts;
  - a sandbox runs two CI runs at once, each lane with its own job checkouts, and worktree
    registrations change one at a time (#206).
- A Done task can be deleted (SPEC-039, ADR-0031, task #37). "Delete permanently…" closes its
  idle session (`closed`, as Archive does), then deletes the task. Only a task that is still at
  work or waiting for you has to be stopped first, and its menu says "Still running: stop the
  task before deleting it".
- Two ready-made automations, enabled in one click on the Automations page (SPEC-042, ADR-0034,
  task #41): "Keep the docs up to date" (the last day's changes, or all docs on the first run)
  and "Work through open issues" (the core team's open issues verified in parallel by
  subagents, a Fix task for each actionable one, never a new issue), both nightly at 03:00 CEST.
  They live in `catalog/automations/`. Tasks now report a bug they confirm with `issue_create`
  unless an issue already tracks it (briefing and the Fix workflow). Neither runs until a person
  enables it; on the team's instance both exist, disabled.
- CI runs are no longer refused over the sandbox's disk quota by the size of test builds
  (SPEC-033, ADR-0028, task #31): the `yulius` crate's 50 integration test files, each its own
  190 MB executable linking the whole product, are now modules of one test binary
  (`tests/main.rs`); a gate build's test executables drop from 9.2 GB to 1.3 GB. The repository
  check rejects a second test binary or an undeclared test file. Run one file's tests with
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^<file>::/)'`.
- A long task title in the start-rule editor stays on one line ending in an ellipsis instead of
  wrapping over the condition's border (SPEC-031, task #29).
- The Cockpit and task agents create tasks and automations (SPEC-037, ADR-0029; task #35). A new
  `task_create` MCP tool creates a task the way New task does (workflow, mode, after a task or a
  start rule), on the authority of the task's creator; tools create at most 20 tasks an hour
  per project. The Cockpit now starts with the Yulius MCP server under its own bearer. It is
  served the tools for tasks, automations, workflows and sessions, acting for its owner, so
  "create a task from that" or "every day at 4am align the docs" just works. It asks before
  creating anything. A Cockpit already running gets the tools when it starts again.
- Attached images reach Claude Code as pasted images (SPEC-030, ADR-0026; task #28). A task with
  attached images starts with no CLI prompt. Yulius types its first message into the terminal:
  each image's path is pasted (Claude Code shows `[Image #N]` and sends an image block), then the
  text with the other files mentioned. Before this, an `@` mention showed the path and reached the
  model as a Read tool call.
- The sandbox navigation is shorter (SPEC-029, ADR-0025, task #27): Cockpit, Tasks, Git,
  CI (tests), Secrets and Dashboard, then "More", which expands Sessions, Workflows, Skills,
  Automations and MCP in place. Its state is remembered per browser; arriving at one of those
  pages shows it open for that page.
- A task's status follows what its agent is doing (SPEC-032, ADR-0044; task #30): a task whose
  agent ended its turn but still waits for a background agent (a reviewer) is Working with "1
  background task still running", not Done; a goal check that says "not met", or any stop the
  session goes on from, makes the task Working again instead of leaving "Your turn" (or Done);
  informational Claude Code notices (a background agent finished, push, quota resume, …) no longer
  flag "Needs your action", and the agent's next work takes a real flag back once answered; a
  restart no longer demotes a working task whose turn a Stop hook continued. Monitor streams are
  still not counted.
- A usage limit no longer types "Continue." into a task (SPEC-034, decision 0027, task #32): the
  session moves to another subscription (or waits for the reset) and resumes as if the limit never
  happened. The transcript steps back over the limit errors and notices, and Claude Code redoes
  the lost turn by itself: it resubmits the unanswered message, or goes on after the last tool
  result.
- Tasks run Opus 5.5 at high effort by default (SPEC-023, task #21): a task without a model runs
  `claude-opus-5-5` instead of Claude Code's moving `opus` alias, and a task without an effort runs
  at `high` (none for Haiku); the Cockpit starts on them too; the composer and Settings say so.
- Messages are gone for now (SPEC-028, ADR-0023; task #26): no `message_send` or `messages_inbox`,
  no stored notices, notice digest or "unread messages" stop reason, nothing typed into an idle
  session, and no Messages panel, unread counts, message notifications or
  `GET /api/tasks/{tid}/messages`. Agents learn about their checks, delivery and armed merges from
  `yulius wait` (which now says how an armed merge ended), `ci_status` and `delivery_status`. The
  stored messages stay as history. A `yulius wait` already running when the server updates exits
  with "cannot be reached" once (its answer changed shape); start it again.
- A file attached on the new-task composer opens when its chip is clicked (an image in a preview,
  another file in a new tab), and the task's Claude Code session receives attached files natively
  as `@path` mentions in its first message, an image as an image, instead of paths to open with the
  Read tool (SPEC-025, decision 0022).
- A half-written new task is kept (SPEC-026, task #24): its prompt, attached files (with thumbnails)
  and choices come back after visiting another page or reloading, per sandbox; starting the task
  clears it, signing out removes it.
- Every user has a Cockpit, first in the sandbox navigation: their own plain Claude Code session in
  the sandbox (no Yulius hooks, MCP or briefing) on a pool subscription, for quick questions and
  work that should not become a task. `/clear` starts over; it cannot be deleted, and after an
  exit, a day unwatched or a restart it resumes its conversation (SPEC-021, ADR-0020, task #19).
- A start rule is picked, not typed: the composer's "When a rule is true…" opens an editor of
  conditions (a task of the sandbox and "is done", "is merged" or "has ended") joined by All or
  Any, shows the sentence it builds, opens again with what was picked and is not offered in a
  sandbox without tasks; the API still takes the full rule language ([SPEC-022](../.ai/specs/SPEC-022-when-rule-is-true-should-be-a-simple-edi.md), ADR-0019, #20).
- Claude Code sessions in a task terminal no longer open Claude Code's diff side panel by themselves
  ("No changes this session" on the right); it still opens by hand within a session (SPEC-024).
- A click on Claude Code's output in a task terminal reaches Claude Code, so a collapsed tool
  group expands as in a native terminal; drags and double-clicks still select text (SPEC-027).
- Ended worktrees holding only end-to-end output (`.e2e`), the APM cache (`apm_modules`) or Claude
  Code's own `.claude/` state are removed too (SPEC-019; seven were kept on the first deploy).
- Agents meet fewer avoidable obstacles (SPEC-020):
  - pinned tools are built once per home and linked into each worktree;
  - the changes log merges with Git's union driver;
  - memory pressure kills an agent's build before the agent, and agent Rust builds use a third of
    the CPUs;
  - CI reuses the result of a tree it already tested, stops a run's jobs when one fails, keeps a
    superseded pull request's queue place, retries an errored run once and says it is not the code,
    and tests a pull request with its own CI jobs;
  - proactive switches wait for background shells;
  - Yulius gives each task its spec number;
  - onboarding runs as a goal task that must run every CI job.
- Nothing refers to GitHub Actions runs any more: the delivery steps, the pull request template,
  the `yulius-git` and `yulius-verify` skills and the branch policy's promotion evidence use the
  `yulius/ci` status of the exact dev head (#184).
- Tasks finish on `dev` by themselves (SPEC-011; tasks #10, #12, #16): a goal task starts from a
  request file (the request, its attached files to open, the definition of done) and a `/goal`
  that names it, typed once Claude Code's session started and confirmed, so long requests get a
  goal and attached images reach the model; the standard profile has no manual `dev` checkpoint;
  `delivery_merge` while the checks run arms the merge and Yulius merges when they pass; agents
  wait with `yulius wait` instead of polling; the briefing and notices end at the merge into
  `dev` (never promote unless asked) and fit Claude Code's hook limit; tasks run Opus by default
  with auto-compact on and 10-minute Bash timeouts.
- Tasks can start by a logical rule: besides "run after #N", a new task can wait for a sentence such as `#12 done and #13 done` or `#12 done or #14 ended` (composer: Run after → "When a rule is true…", API `start_rule`); it starts the moment the sentence is true and goes to Needs you when it never can be ([SPEC-017](../.ai/specs/SPEC-017-task-start-rules.md), ADR-0018).
- Sandboxes stay clean and background shells stay honest (SPEC-019): an idle task's caches go after
  4 hours whatever the quota, never while its background shells still run a build; ended tasks'
  worktrees go when only rebuildable directories are ignored; orphan worktrees and removed
  sandboxes' worktrees and CI checkouts are pruned after a day; a Bash `pgrep -f`/`pkill -f` that
  would match itself is refused before it runs; an idle agent is told once when its background
  shells run 30 minutes, and the UI shows their age.
- Task-to-task messaging is removed for now: `message_send` only tells or asks people (`to="people"`), `scope_claim` shows an overlap to the claiming agent only and messages nobody, and the briefing, digest and Messages panel speak of Yulius's notices. Notices, the inbox, the digest and the idle wake-up are unchanged (SPEC-012, COM-002 withdrawn).
- A task's state reads the same everywhere and matches what it waits for (tasks #11, #13, #14):
  an idle agent whose background shells still run is Working, not "Your turn", and the task view
  no longer asks for a reply then; a task being stopped is listed under Stopped at once; any task
  shown as Done (goal achieved, or plan complete with its pull request merged or tested) can be
  archived, and its session is closed rather than stopped (SPEC-001 TSK-014, UI-050).
- Merging into `dev` no longer waits for a person: the `standard` delivery profile has no manual `dev` checkpoint, so after the merge on green checks the next action is promotion; stored profiles with the old checkpoint read as the new one (SPEC-011 DLV-009, DLV-010, #10).
- GitHub Actions are gone from this repository: Yulius CI is the only CI (`quality` and `e2e` in
  `[[ci.jobs]]`), the rulesets require only the `yulius/ci` commit status, and `.github/workflows`
  and `.github/actionlint.yaml` are deleted. macOS is checked with `scripts/dev ci` on a Mac before
  promotion; the branch policy is checked by hand until it joins Yulius's merge gate (#223). The
  `ci_setup` tool and the CI page still handle other projects' workflows (#184).
- A subscription whose only task is running but idle is read again: the usage round skipped every
  subscription with a running task, but an idle session reports nothing, so its reading went stale
  (5.6 h on production); it now skips only fresh readings (SPEC-007 USG-002, #4).
- Agents wake on new messages again: Yulius typed its notice and Enter in one write, which Claude
  Code read as a paste with a newline, so the notice never submitted and piled up in the input box;
  Enter now follows separately, and an unsent notice is only submitted.

## 2026-10-11

- A full sandbox disk no longer ends running sessions or needs a person: a switch checks that the new session can start before it ends the running one (the reason shows in the terminal and the switch log); a refusal over the quota measures again when its reading is over a minute old; Yulius removes build caches (Cargo `target` and similar, or `[disk] caches`) of tasks that are not working and the worktrees of closed and cancelled tasks, prunes idle tasks' caches from 90 % of the quota, and tells the agent what was freed; persistent CI job caches are pruned too, at once beyond a quarter of the quota ([SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md)).

- Agent primitives follow the agentic-coding workshop methodology (SPEC-005, ADR-0016): repository
  skills are APM packages (`.apm/skills` source, `apm.yml`, `apm.lock.yaml`, generated
  `.agents/skills` and `.claude/skills`), the repository check rejects deployment drift and skills
  without a "Use when/for" sentence or over 200 lines, a primitive design standard
  (`docs/engineering/primitives.md`) and the `yulius-primitives` skill apply the pattern
  vocabulary, and `infra-bootstrap` now audits a project's conventions into primitives.
- A green pull request behind its base (the repository requires up-to-date branches) is not
  mistaken for a missing permission: the task is told to merge the base, and `delivery_merge`
  says why it refuses.
- A task whose pull request conflicts with its base is told to merge the base and resolve it;
  GitHub runs no checks on a conflicting pull request, so the agent would otherwise wait for them.
- Debug and test builds keep only line tables (dependencies none): a worktree's test build had
  reached 48 GB and parallel tasks filled the sandbox disk quota, so CI runs and session starts
  were refused.
- Agents carry delivery through on their own: Yulius tells a task when its pull request opened and
  when its checks passed or failed, the Stop gate asks for a missing spec at once (and follows a
  renamed one), and the briefing says never to ask a person — an issue covers what only a person
  can do.
- Yulius CI `quality` job failed at once on a clean machine (no rustup): `scripts/bootstrap.py`
  installs it when the run is unattended or given `--install-rustup` (no default toolchain, no
  profile edits).
- Everything a task does on GitHub shows the person who created it: commits with the no-reply
  address GitHub links to their account, and the agent's Git and `gh`, Yulius's pushes and pull
  requests, the agent's merges, promotions and issues with their own token; the App only when
  they have none, said once in the task's terminal (SPEC-001 GIT-003, #188).
- The GitHub CLI is logged in as the user in sandbox shells and editors: a `gh` shim fetches the
  user's own GitHub token per call (nothing stored on disk), like Git's credential helper
  ([SPEC-004](../.ai/specs/SPEC-004-gh-cli-login.md)).
- Subscriptions show usage all the time: for a sign-in the usage endpoint refuses, an idle
  subscription is read from the rate-limit headers of a one-token request every few minutes
  instead of waiting for a task (ADR-17, #4).
- Repository prepared for Yulius: `yulius.toml`, product summary, architecture, decision
  records, changes log; GitHub Actions jobs translated into Yulius CI
  (the workflows stay until the GitHub App may post commit statuses and change workflow files).
- Tasks wait for preparation instead of being refused (#175); onboarding survives multi-byte
  text and titles are the first real prompt (#173); Secrets adds from its header and popups
  close on an outside click (#171).
