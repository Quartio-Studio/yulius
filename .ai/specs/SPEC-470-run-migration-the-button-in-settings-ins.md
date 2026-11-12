---
yulius-task: 719087ff-c914-4241-bf1d-1d0a477a2041
status: in-progress
---

# SPEC-470: Run migration: the button in Settings → Instance

Status: in progress
Date: 2026-11-11
Owner: Yulius task #447 (slice D of #442)
Scope authorization: task #447, asked by the project's owner: "A superadmin starts the move from
Settings → Instance, follows the same checklist there and is asked the same things first." It
delivers requirement MIG-060 of
[SPEC-460](SPEC-460-run-migration-one-command-moves-the-whol.md)
([ADR-0297](../../docs/decisions/0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md));
slice C ([SPEC-466](SPEC-466-run-migration-yulius-checks-its-github-a.md)) put the Address card
and the "moved" note on the same page.

## Goal

A superadmin moves the instance to another host from Settings → Instance: the form, the plan with
everything a person has to do, a typed confirmation, the live checklist, an abort that says what
it does. Root runs the same command as on the host, asked through a request file as updates are.
While the instance is frozen the page says so and keeps trying; once the new instance runs, its
page shows the finished checklist.

## Context

- `yulius instance migrate` (SPEC-460) is a root command on the old host. It keeps
  `/var/lib/yulius-migrate/state.json` (0600) and `status.json` (0644: the phase and the
  checklist so far, no secret values), ends after its plan when there is neither a terminal nor
  `--yes`, and has `--abort`. Without a terminal the one-time key is not installed by a sign-in:
  the plan ends with a `manual` line that holds the public key line to add on the new host.
- The server runs as the unprivileged service user and is frozen during the copy; after a move it
  stays frozen for good on the old host. So it cannot run the move, cannot be asked during the
  copy, and whatever it keeps in its database travels to the new host with the copy.
- The pattern for a root action asked from the web is the update helper (ADR-0013, SPEC-002
  ADR-18): the server writes `<data>/update/request.json`, a root path unit starts a oneshot
  helper that trusts nothing in the file, and the server reads the helper's status file.
- Facts checked for this spec (2026-11-11):
  - A oneshot unit whose command runs is `activating`, not `active`. The export's freeze knows
    (`freeze.rs::helper_idle`); the move's own plan check compares with `active` only and so
    never sees a running update (reported as an issue; the freeze still refuses).
  - The command's signal handling: before the freeze a SIGTERM ends it (nothing is frozen), from
    the freeze until the new instance's start is ordered it thaws the old instance and exits,
    after that it is ignored (`migrate.rs::RealWorld::arm`, `freeze::on_signal`).
  - A `fetch` with `redirect: 'manual'` answers an opaque redirect (`type` `opaqueredirect`) for
    the old host's 307, whatever origin it leads to; a followed cross-origin redirect is a
    network error for the page.
  - The new public URL made from the new host's address is built from four numeric octets only
    (`profile::address_url`), so what a new host says about itself cannot put text into the old
    host's proxy configuration.
- Parallel tasks change the command (#445 the warm copy, #450 review leftovers): this change
  keeps its edits there small and additive. Both merged while it was in work; the warm copy
  (SPEC-465) changed what a move is to the page: a phase `Copy` during which the instance
  works, a freeze only for the changes, and a bound on it (`--max-downtime`).

## Requirements

The request and the root helper

- MVB-001 **Request file.** The server writes `<data>/migrate/request.json` (0600, in a 0700
  directory, atomically): `version` 1, `id` (8 to 64 of `[A-Za-z0-9-]`), `action` (`plan`,
  `move`, `abort`), `target`, `ssh_port`, `public_url`, `data_disks`, `requested_at` (ms). There
  is no other field, so nothing secret can be in it.
- MVB-002 **Field rules**, the same for the server (before it writes) and the helper (before it
  acts). For `plan` and `move`: `target` as `remote::destination_ok` takes it, at most 255
  bytes, and `root` when it names a user; `ssh_port` 1 to 65535; `public_url` absent or
  `https://host` with nothing after it, at most 255 bytes; `data_disks` none, one or two
  different ones, each `/dev/` followed by `[A-Za-z0-9._/-]`, without `..`, at most 128 bytes.
  For `abort`: none of the four.
- MVB-003 **The helper trusts nothing.** `yulius migrate-helper` (root, a oneshot started by the
  path unit) takes the request only as a regular file (not a link) owned by the data directory's
  owner, which is not root, of at most 16 KiB, with the exact schema (an unknown field is
  refused), the version, a timestamp at most 15 minutes old and 60 s ahead, and an id it has not
  acted on (`<state>/last-request`; a request it cannot record there is not run, and the helper
  ends with an error). It refuses while the update helper's unit runs (`active`,
  `activating` or `reloading`), and a target whose host is not in `YULIUS_MIGRATE_TARGETS` when
  the root-owned unit sets that list. A refusal changes nothing and is reported with its reason,
  which never repeats the request's content.
- MVB-004 **It runs the command.** An accepted request runs this binary's `instance migrate`
  with the unit's data directory, service, update service and state directory, and the request's
  values each as one argument (`--ssh-port=`, `--public-url=`, `--data-disk=`, then `--` and the
  target): `plan` without `--yes` (no terminal, so it ends after its plan), `move` with `--yes`,
  `abort` as `--abort` and never with `--yes` (an abort that needs a person's word that the new
  instance is stopped stays a command on the host). No shell; standard input is closed; the
  command's output goes to the journal. The checklist of an earlier run is removed first, so
  `status.json` is always of the run in flight.
- MVB-005 **It reports.** `<state>/helper.json` (0644): the request's id, action and values,
  `running` with a heartbeat every 5 s, then `ended` with the command's exit code, or `refused`
  with the reason and none of the request's values. When the command ended refused before its checklist (exit 2) or could not be
  started, its last words (the command prints no secret value, MIG-025) are the error.
- MVB-006 **Abort stops a run.** While a command runs the helper looks at the request file twice
  a second. A new valid `abort` sends the command SIGTERM (the command's own rule decides: the
  old instance is thawed and runs on, or, once the new instance's start is ordered, the signal
  is ignored), waits for its end and then handles the abort. A SIGTERM to the helper itself is
  passed on the same way, and the helper ends only after the command, with its report written.
  At most three requests are handled in one run.
- MVB-007 **Units.** `deploy/systemd/yulius-migrate.path` (`PathChanged=` the request file) and
  `yulius-migrate.service` (oneshot, root, no start timeout, 10 minutes to stop, no start
  limit: requests that come quickly must not fail the path unit), named after the server's
  unit as its sessions unit is (`<stem>-migrate`). `host setup` does not install them: root
  opts in, and the operations guide says what that allows.
- MVB-008 **They travel.** A move carries the helper's unit files and drop-ins when the old host
  has them. Once the new instance runs, the path unit is enabled there when it was enabled on
  the old host; the plan and the checklist say which.

The command, for readers

- MVB-010 **Status for the page.** `status.json` also holds `public_key` (the one-time key's
  public line, while a person has to add it) and `new_url` (once the plan decided it).
- MVB-011 **The checklist travels.** The move's Finish leaves the finished checklist on the new
  host as `/var/lib/yulius-migrate/arrived.json` (0644, root's file in the flow's own
  directory): the lines so far, the new host's own Finish lines, and the lines the old host
  adds after it. Nothing is written into the data directory for it: that belongs to the user of
  an instance that already runs. A later Finish replaces it, or removes it when it brings none.

Application and API

- MVB-020 **View.** `GET /api/admin/instance/migration` (superadmin) answers: `helper_installed`
  (systemd says the path unit is active; kept for 10 s); `update_running`; `request` (written
  and not taken by the helper yet: `requested`, or `unanswered` after 20 s; from 5 s on the same
  request is written again whenever it is read, since a change of the file that comes while the
  helper is ending another run starts nothing); `run` (the helper's
  report: action, values, `running`, `ended` with the exit code, `refused` with the reason, or
  `lost` when a running helper's heartbeat is more than 60 s old); `move` (the command's status:
  id, target, phase, lines, `public_key`, `new_url`, `updated_at`, and `by_command` when it was
  written after the helper's last run ended, or without any, that is by a run started on the
  host); `arrived`
  (the checklist a move to this host left, MVB-011); and `can` (`plan`, `move`,
  `abort`: what would be accepted now). A request the helper took is removed by the server
  itself, read or not (it looks every 250 ms for 20 s after writing it), so a move does not
  copy it to the new host as one that waits there; one never taken is removed when it is read
  after 15 minutes. A removal is of the request that was looked at, never of a newer one.
- MVB-021 **Ask.** `POST /api/admin/instance/migration` with `action`, `target`, `ssh_port`,
  `public_url`, `data_disks` and `confirm` answers 202 with the view. Refused: 403 for anyone
  but the superadmin and without the CSRF token; 422 for a field outside MVB-002, and for `move`
  when `confirm` is not the target's host; 409 when the helper is not installed, when an update
  or restart runs, when a request waits or a run is in flight (`abort` may arrive during a run:
  it stops it), for `move` unless the last run was a plan of the same values that ended without
  a failed line and without a key to add at most an hour ago (or a `move` of the same values:
  it continues), and for `abort` when nothing is recorded. Asks are taken one at a time: of two
  at once, one is written and the other is told that a request waits.
- MVB-022 **Audit.** `migration.planned`, `migration.started` and `migration.aborted`: the
  actor, the target, the request's id and values.
- MVB-023 **Updates wait.** "Update now" and "Restart…" are refused (409) while a move's request
  waits or its run is in flight.

Settings → Instance

- MVB-030 **The card "Move to another host"** (superadmin only, after Address): the form (new
  host, SSH port, new public URL, data disks) and "Show the plan". Without the helper: what it
  is, where the operations guide has its units, and the command to run on the host instead.
- MVB-031 **The plan.** The checklist as the command wrote it, a line per step with its outcome;
  the lines a person has to act on (`manual`) and the failed ones are repeated first. When the
  key has to be added: the key line with a copy button, what to do with it, and "Show the plan"
  again. A plan with a failed line offers no move.
- MVB-032 **Run migration.** Behind a dialog that says root on this host will give this
  instance's data (every repository, secret and key) to the named host and start it there, that
  the data is copied while the instance keeps working and what the freeze is for (what changed
  since, the proof, the start), how long the plan estimated it, and that this host keeps a
  frozen copy; the host's name is typed to confirm.
- MVB-033 **Live.** While a request waits or a run is in flight the card reads the view every
  3 s and shows the phase (Plan, Prepare, Copy, Cutover, Finish) and the lines as they come; a read
  that answers late never replaces a newer one. A run that ended says how: the plan is shown,
  moved, did not complete (with "Continue the move…"), refused with the reason, the helper did
  not answer (with the command that enables it), or lost.
- MVB-034 **Abort.** Offered while a move is recorded or runs, behind a dialog that says what it
  does: a run in flight is stopped, the old instance runs on, the key is removed from the new
  host and deleted here, what was installed on the new host stays, and so does data that was
  already copied, with its keys and secrets, which the checklist then says to remove; after a
  move that had started the new instance, the abort stops it there first; an abort that needs a
  person's word says so and names the command.
- MVB-035 **Frozen.** When reads fail while a move runs, the card says that the instance is
  frozen while the last changes are sent and the copy is proven (with the last estimate the
  move gave: the plan's, or the one the copy measured), keeps the last checklist and keeps trying
  every 3 s with a short timeout. When this address answers again it shows what answers: the
  old instance with why the move did not complete, or the new one (MVB-037).
- MVB-036 **Moved to another URL.** While frozen the card also asks this address without
  following redirects: an answer that is a redirect means the old URL now leads to the new
  instance, and the card says so with a link to this page at the new URL (`new_url`).
- MVB-037 **On the new instance** the card shows the move that brought the instance there: when,
  from where, the lines a person still has to act on, and the whole finished checklist.

## Design

- **Contracts** (`yulius-contracts::migrate`): `MoveRequest` (`deny_unknown_fields`),
  `MoveAction`, `MoveHelperStatus`, `MoveHelperState`, `MoveChecklist`, the limits and the files'
  names. The API views live in `yulius-contracts::api::migration`.
- **Rules** (`yulius-app::migration::rules`): `check_request` (MVB-002) with
  `destination_ok`, moved there from `instance/migrate/remote.rs`, which keeps the name by
  re-export, and `public_url_ok`. Used by the application before it writes and by the helper
  after it read.
- **Helper** (`yulius::migrate_helper`): `MoveHelperArgs` from the root-owned unit (data
  directory, state directory, service, update service, allowed targets; hidden: the program to
  run and `systemctl`, for tests); `parse_request` (bytes and now → request or reason);
  `command_args` (request → argv); `Helper::run`, which reuses the update helper's
  `read_owned_file` and `write_atomic`. The command is a child process in its own process
  group: the helper can stop it, wait for it and report its end, which a call in the same
  process could not.
- **Port and adapter.** `yulius_app::ports::migration::MoveHelper` (installed, write and read
  back and clear the request, the helper's status, the command's status, the arrived checklist);
  `wiring::migrate_files::FileMoveHelper` reads the files bounded (16 KiB, 256 KiB, 1 MiB) and
  asks systemd about the path unit named after the server's own unit.
- **Application** (`yulius-app::migration`): the view is computed from the three files on every
  read and nothing is stored: the server is frozen during the copy and its database travels, so
  a row "running" would arrive on the new host as a move of that host. States of a request:
  `requested` (owner: the path unit; exit: the helper's report with its id, or `unanswered`
  after 20 s, when the person asks again), `running` (owner: the helper; exit: `ended`, or
  `lost` when its heartbeat stops, after which the person continues or aborts), `ended`,
  `refused`, `lost` (terminal; the next request replaces them).
- **Command** (`instance/migrate.rs`, `target.rs`, `profile.rs`): `Status` gets the two fields;
  a new step `Request::Carry` brings the checklist (`Carried`: the lines so far and the lines
  that follow the Finish) and stages it in the flow's directory, as `Expect` is staged for the
  copy; the new host's Finish step adds its own lines and leaves the whole as `arrived.json`
  after it removed the flow's other files. `Request::Finish` keeps its shape. A new
  `Request::Enable` enables the helper's path unit after the start; `Profile` reads the helper's
  units (`units::migrate_unit`) and whether its path unit is enabled.
- **Page** (`web/assets/instancemove.js`): one card module; the view decides what is shown, the
  module only remembers the move it follows (its id, `new_url`, the estimate) across the freeze.

## Plan

- [x] Spec SPEC-470 and ADR-0305
- [x] The request's contract and field rules, shared by the server and the helper
- [x] `yulius migrate-helper`: owner checks, one request once, never during an update, runs the command, reports, stops a run on an abort
- [x] Units in `deploy/systemd`, carried by a move and enabled on the new host as on the old one
- [x] The command tells readers the key line and the new URL, and the finished checklist travels to the new host
- [x] Application and API: the view, plan, move and abort for the superadmin, audited, updates and moves refuse each other
- [x] The card "Move to another host": form, plan with the key line, typed confirmation, live checklist, abort, on the mock with e2e
- [x] Frozen and moved: the card keeps trying, follows the old URL's redirect, and the new instance shows the finished checklist
- [x] Operations guide §7 and §8, API, architecture, product summary, changes
- [ ] Gates green and the independent review answered

## Acceptance criteria

- A request file that is a link, owned by another user, larger than 16 KiB, with an unknown
  field, another version, a malformed id, an old timestamp, a target `-oProxyCommand=x` or
  `user@host`, a port 0, a URL with a path, a disk outside `/dev/` or three disks, or an abort
  that names a target is refused; the helper's report says why without the request's text, the
  command is not run and the id is not acted on again.
- With the update helper's unit `activating` every action is refused; with
  `YULIUS_MIGRATE_TARGETS=a.example` a plan for `b.example` is refused and one for
  `root@a.example` runs.
- A plan request runs the program with exactly `instance migrate --data-dir … --service …
  --update-service … --state-dir … --ssh-port=2222 --public-url=https://new.example
  --data-disk=/dev/vdc --data-disk=/dev/vdd -- root@new.example`; a move adds `--yes`; an abort
  runs `instance migrate --abort` with the four unit options and nothing of the request. The
  same request file read again runs nothing.
- While the command runs, `helper.json` says `running` and its `updated_at` advances; an abort
  request written meanwhile ends the command by SIGTERM, then runs `--abort`; the helper's last
  report is the abort's. A SIGTERM to the helper ends the command first and leaves `ended`.
- A move between two fake hosts whose old host has the helper's units enabled carries both unit
  files and enables `yulius-migrate.path` on the new host after the start; with the units
  present and the path unit disabled they are carried and not enabled; without them the
  checklist says there is none.
- A plan without a way to add the key ends with `public_key` in `status.json`, equal to the
  key's public line; a run that signs in has none. After the plan's URL step `new_url` is there.
- After a finished move the new host's `/var/lib/yulius-migrate/arrived.json` is 0644 and holds
  the same lines, in the same order, as the old host's final `status.json`; nothing else of the
  flow is left there and nothing was written into the data directory; a Finish that brings no
  checklist removes an earlier one; a new host that cannot keep it says so (`manual`) and the
  move ends all the same.
- `GET` and `POST /api/admin/instance/migration`: a member gets 403; a POST without the CSRF
  header 403; a superadmin's plan writes one request with the given values and audits
  `migration.planned`; a second plan while the first waits is 409; after the helper's report the
  request file is gone; `move` is 409 before a plan, after a plan with a failed line, with a key
  to add, with other values, or with a plan more than an hour old, 422 without the typed host,
  and 202 after a good plan (audit `migration.started`); `abort` during a run is accepted and
  audited; all three are 409 while an update runs, and "Update now" and "Restart…" are 409
  while a move runs. A request nobody took is `unanswered` after 20 s and can be asked again; a
  `running` report whose heartbeat is 61 s old is `lost`.
- The view on a host that received a move holds `arrived` with the carried lines; one whose
  `status.json` is newer than the helper's last run says `by_command`.
- e2e on the mock: the form and "Show the plan" show the key line with its copy button, then
  the plan with its manual lines first; "Run migration…" needs the host typed and names the host
  that gets the data; the checklist grows while it runs; "Abort…" says what it does and ends the
  run; while the mock does not answer the card says the instance is frozen and keeps the
  checklist; when it answers as the new instance the card shows the finished checklist; when
  the old address redirects the card links to the new URL; a failed move offers "Continue the
  move…"; without the helper the card names the unit and the command; a member sees no card.

## Test plan

- `yulius-app` unit tests (`migration::rules`): MVB-002, each rule and its boundary.
- `yulius` unit tests: `migrate_helper::tests` (MVB-003 to MVB-006 against a fake program, a
  fake `systemctl` and real files and signals), `wiring::migrate_files` (MVB-001, bounded and
  versioned reads), `instance::migrate` tests (`profile`, `target`, `flow`: MVB-008, MVB-010,
  MVB-011), a CLI test (`migrate-helper --help`, units parse).
- `crates/yulius/tests/migration.rs` (the app harness with a fake helper port): MVB-020 to
  MVB-023, with the failure cases above.
- e2e (`e2e/tests/instance-move.e2e.ts`) on the mock: MVB-030 to MVB-037, with accessibility
  checks.
- Gates: `scripts/dev ci`, the e2e suite, `yulius security-scan` (Vidoc is off for this project:
  disabled, not passed).
- Not testable here: systemd starting the helper from the path unit, and a move asked from the
  page between two real hosts (the sandbox has no systemd); recorded in Evidence, with what a
  QEMU guest could show if one is run.

## Decisions

- [ADR-0305](../../docs/decisions/0305-a-root-helper-runs-the-move-asked-from-the-page-and-the-checklist-travels-with-it.md)
  records the lasting ones: a request file and a root path unit as for updates, with the command
  as a child process; root opts in and may pin the targets; the view computed from files and
  never stored; the checklist carried by the Finish step; an abort from the page never speaks
  for a person; a move needs a plan shown for the same values.
- **Carry the status, do not read it from the old host.** The old instance is frozen after a
  move, so only its proxy could serve the status, to anyone who asks; the new instance shows
  what the Finish step brought. The lines the old host adds after that step are known before it
  (the key deleted here, GitHub's answer, the notes for people), so they travel with it.
- **The checklist is left in root's directory, not in the data directory.** At the Finish the
  new instance already runs, and its user owns the data directory: a file root wrote and gave
  away there could be redirected by that user between the two steps. The server reads the
  flow's directory anyway (the helper's report, the command's status), so the checklist is
  left there, readable and root's. It also does not travel on with the next move, which brings
  its own.
- **The page's abort never passes `--yes`.** That flag answers "is the new instance stopped?"
  for the person. It is needed only when the new host cannot be reached after its instance was
  started; the checklist then names the command to run on the host.
- **No banner for everyone during the freeze.** An update announces its restart to every user
  because the server knows it beforehand; a freeze is decided by root's command at a moment the
  server cannot announce. The card says it for the superadmin who started the move; the plan
  says how long.
- **One pull request.** The helper without the page cannot be reached, and the page without the
  helper does nothing; each part has its own tests.
- **Not in this change.** The plan's own "an update is running" check (`active` only) is
  reported as issue #1196: the helper refuses first when a move is asked from the page, and the
  freeze refuses in any case. The update helper's units have the start limit this change
  switches off for its own (below): issue #1201.
- **The warm copy, as the page shows it.** SPEC-465 merged into `dev` while this change was
  ready for review. The card names the phase `Copy` (unknown, it read as "Plan"), says in the
  confirmation that the data is copied while the instance works and what the freeze is for,
  shows the last estimate the move gave while frozen, and says in the abort's dialog that data
  already copied stays on the new host with its secrets. The request has no field for
  `--max-downtime`: MIG-060 names the target, the URL and the disks, and the move then decides
  as the command does without the option (it copies again while that shortens the freeze). A
  field for it on the page is issue #1202.
- **No start limit on the helper's unit.** On a real host six requests within 10 s (the helper
  ends at once when it refuses) hit systemd's default start limit, and systemd then failed the
  path unit too: nothing was taken until root reset it, and the page said the helper was not
  enabled. `StartLimitIntervalSec=0` on the service; the path unit's own trigger limit (200 in
  2 s) still bounds a loop. With it 14 requests in 7 s left the path unit waiting.
- **A defect of the command fixed here.** The real moves asked from the page (Evidence) showed
  that `--abort` after a plan that ended with the key to add, once the key was added, failed on
  the new host (exit 127: the step that removes the key is run by the build, which such a plan
  never brings) and left the key there. That is the page's most natural abort (plan, key, think
  again), so the abort now brings the build for that step, with a flow test.
- **A taken request is removed without a reader, and by its id.** The request went when the
  view was read, which the page does every 3 s; with the page closed right after "Run
  migration" it stayed, and the move copied it (the new host would have said "unanswered" for a
  few minutes). The server now looks for the helper's answer itself. Root does not remove it:
  that would be root deleting in a directory the service user controls. Found in my own review,
  with two more of its kind: two asks at once both found nothing waiting (a lock now), and a
  removal could hit a request written a moment later (it now names the id).
- **An untaken request is written again, not re-triggered by the helper.** The path unit starts
  the helper on a change of the file; a change that comes while the helper is ending another
  run starts nothing (seen on a real host). Updates write their request again after 20 s for
  the same reason; a move's request is written again from 5 s on while the page reads it, and
  said `unanswered` after 20 s. A level-triggered unit (`PathExists=`) would need the helper to
  delete every file it finds there, also those it must not read.
