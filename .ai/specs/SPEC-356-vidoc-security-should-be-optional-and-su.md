---
yulius-task: f637fdb9-46bc-4a1b-9161-5b9b24fd1f15
status: done
---

# SPEC-356: A superadmin switches Vidoc off for the whole instance, keeping its key

Status: done
Date: 2026-10-31
Owner: Yulius task #338
Scope authorization: task #338 (`/goal`): "vidoc security should be optional and superadmin should
be able to disable it". Extends SPEC-338 (the per-project switch, ADR-0229) and SPEC-195 (the
instance key, ADR-0157).

## Goal

Vidoc stays optional: a superadmin turns Vidoc security scans off for every project of the
instance in one place (Settings, Instance, the Vidoc card) and on again later, without removing
the key, without a project-by-project round, and also when the key comes from the server's
configuration file (which the UI cannot remove). While it is off nothing is sent to Vidoc for any
project, agents are not asked about the key or its quota, and every scan says "disabled", never
"passed".

## Context

- SPEC-338 (task #322, merged in #938) added a per-project switch: `vidoc: false` in a sandbox's
  profile JSON. Everything that depends on Vidoc being on for a project goes through
  `delivery::vidoc_switched_on(sandbox)`: the agent run, terminal and CI job environments
  (`YULIUS_VIDOC=off`, in `supervisor.rs`, `shells.rs`, `ci/exec.rs`), the instance key applied as
  the virtual `VIDOC_API_KEY` (`vault::sandbox_vidoc_on`), the built-in CI `security` job
  (`App::ci_scans`), the briefing (`App::vidoc_briefing`) and through it the onboarding prompt.
- The only instance-wide ways to stop Vidoc today are removing the instance key (it must be pasted
  again later; a key from `YULIUS_VIDOC_API_KEY_FILE` cannot be removed in the UI at all, only
  shadowed) or switching every project off one by one (new projects start on again).
- ADR-0229 rejected an instance-wide pause because removing the key "already stops every
  project"; that is not true for a project with its own `VIDOC_API_KEY` secret, nor for a key from
  the configuration file, and the person now asks for it explicitly.
- The Instance page's Vidoc card (`instancekeys.js`) is superadmin-only and lists the projects
  with their switches; `me.instance.vidoc` makes the Add sandbox dialog offer its Vidoc box.

## Requirements

- **VIO-001 Instance switch.** The instance has a Vidoc switch, on by default, stored in the
  `instance` row (`vidoc_off_at`, `vidoc_off_by`: off when set). `GET /api/admin/vidoc` shows it as
  `enabled` (with `disabled_at`, `disabled_by` while off). `PUT /api/admin/vidoc/enabled`
  `{"enabled": bool}` sets it (superadmin only: others get 403), takes effect at once and survives
  a restart; each change is audited as `vidoc.disabled` or `vidoc.enabled`. The key is kept, and
  can still be saved, replaced, removed and tested while the switch is off.
- **VIO-002 Off means no project scans.** While the instance switch is off, every project behaves
  as a project switched off by SPEC-338 VOF-002, whatever its own switch or own key: its agent
  runs, terminals and CI jobs get `YULIUS_VIDOC=off`, the instance key is not applied as its
  virtual `VIDOC_API_KEY`, and Yulius CI adds no built-in `security` job. The scanner's disabled
  line names Yulius without claiming which switch, since both set the same variable. A session
  that started before the switch went off sends nothing either: the egress proxy refuses its
  requests to `model.vidocsecurity.com` (403, "Vidoc is turned off in Yulius … nothing was
  sent"), whatever key they carry, and the scanner reports that reason (not run), never a refused
  key; this applies to the project switch too. A switch that cannot be read counts as off; at the
  proxy it refuses with 503 (the scan retries and ends incomplete, never "off", review round 2).
- **VIO-003 Agents are told.** The briefing's `## Security scan` section says that a superadmin
  switched Vidoc off for the whole instance, that the scan reports "disabled", never a pass, and
  that nobody is to be asked about Vidoc's key or quota; the onboarding prompt adds no scan rule.
- **VIO-004 Projects keep their own switch.** Each project's stored switch is left as it is: the
  sandbox view still shows `profile.vidoc` as the project's choice, and turning the instance back
  on restores each project's own choice.
- **VIO-005 Managed on the Vidoc card.** The card shows "Vidoc scans" with an on/off switch for
  the instance; while off it says that no project is scanned until it is switched on again, and
  the project switches are shown disabled. `me.instance.vidoc` is false while off, so the Add
  sandbox dialog does not offer its Vidoc box.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The instance switch is stored, shown and changed by a superadmin through the API
- [x] Every project follows the instance switch: environments, virtual key, CI job and briefing
- [x] The Vidoc card switches Vidoc off and on for the instance
- [x] ADR, architecture, AGENTS.md and product docs describe the instance switch
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Review round 2 approves the fixed head
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `PUT /api/admin/vidoc/enabled` `{"enabled": false}` by a superadmin answers the view with
  `enabled: false`, `disabled_by` its login; a sandbox admin gets 403; after an App restart over
  the same store it is still off; `{"enabled": true}` turns it on; both are audited.
- With the instance switch off and an instance key: a switched-on project's `ci_scans` is false,
  its agent and CI environments carry `YULIUS_VIDOC=off`, its applied agent secrets have no
  instance `VIDOC_API_KEY`, its briefing says Vidoc is off for the instance, `me.instance.vidoc`
  is false, and `profile.vidoc` stays `true`.
- The Vidoc card's instance switch calls the API and disables the project switches; covered by
  the e2e suite on the mock.

## Test plan

- Rust integration tests in `crates/yulius/tests/vidoc_switch.rs`: the API contract and
  permissions, persistence across a restart, audit, applied secrets, briefing, `me`, the project
  switch kept; `local_ci.rs`: no built-in job while the instance is off; `pool_supervision.rs`:
  the agent run environment.
- Domain unit test of the briefing's instance-off text; scanner unit test of the disabled line.
- e2e (`e2e/vidoc-switch.e2e.ts`): the card's instance switch on the mock.

## Decisions

- **Columns on the `instance` row, cached in memory.** The instance key lives there already; the
  switch is read on every run and CI job, so it is loaded at start (with the key) and kept in
  `VidocServices`, changed at once by the API. A timestamp and login rather than a boolean say who
  switched it off and when, like the key's `set_at`/`set_by`.
- **The same `YULIUS_VIDOC=off`, not a second value.** An older scanner (a repository's own
  `scripts/vidoc_scan.py` copy) honours only `off`; a new value would make it scan with a
  project's own key. The briefing tells agents which switch it is.
- **Projects keep their choice.** Folding the instance switch into `vidoc_switched_on` would make
  the sandbox view show every project off and a project toggle silently do nothing; instead
  `App::vidoc_scans_on(sandbox)` combines both and is used wherever a scan would run, while the
  project's own switch stays what the project view and its PATCH show.
- **Superadmin only.** It changes every project at once, like the key itself.
- **The proxy refuses rather than tunnels** (review round 1). Without the instance key applied, a
  running session's request to Vidoc was tunneled, code included; intercepting the host for a
  switched-off project and answering 403 sends nothing. The alternative, documenting that running
  sessions follow at their next run, would break "nothing is sent to Vidoc".

## Evidence

On 2026-10-31:

- `scripts/dev ci`: exit 0 at `36f1309e` (after the review round 2 fixes, 2840 tests; also at
  `7e97386a` and `04906128` before them; later commits edit this spec only). Repository contracts, the Python unit tests (91 scanner tests, 2 new: the disabled
  line, a refusal by Yulius's proxy), `cargo check`, `fmt --check`, `clippy -D warnings` (Linux and
  `aarch64-apple-darwin`), `cargo nextest run --profile ci` 2839 passed, 16 skipped, doctests,
  `cargo deny`, `cargo shear`, `cargo doc`. Counts below are at `7e97386a`.
- `cd e2e && npx e2e run` at `04906128`: 778 passed, 1 skipped, 1 failed. The failure,
  `superagent-requests.e2e.ts` "Create task starts it in that sandbox", happened while the gate
  loaded the host; that file passed 30/30 run alone three times right after, and this change does
  not touch its path. Filed as #978. After the round 1 UI fixes, `vidoc-switch.e2e.ts`: 8 passed.
- Test first: the instance API, persistence, briefing and onboarding tests did not compile before
  their change; the run, CI and `me` tests (`a_run_follows_the_instance_vidoc_switch`,
  `no_project_scans_in_ci_while_vidoc_is_off_for_the_instance`,
  `every_project_follows_the_instance_switch`) failed before the call sites changed; the scanner
  tests failed (4 failures) before its line changed; the card's e2e failed (no switch) before the
  card; the proxy tests (`a_switched_off_project_does_not_get_the_instance_key`,
  `every_project_follows_the_instance_switch`: "Tunneled") failed before `vidoc_refused`. Mutation
  runs: removing the store write in `set_vidoc_enabled` fails
  `a_superadmin_switches_vidoc_off_for_the_instance_and_on_again`, and reverting `shells.rs` to the
  project switch fails `a_terminal_follows_the_instance_vidoc_switch`.
- Independent review, round 1 (fresh session, at `04906128`): approve, with 5 minor findings and 4
  nits, all own code, all fixed in this change: the proxy now refuses Vidoc requests of running
  sessions while scans are off (nothing sent; the scanner says Vidoc is off), the stored switch and
  the terminal environment are tested, an unreadable switch is off, `docs/api.md` and the ADR index
  corrected, the card says the key is kept and not used while off, the view reads the switch once,
  and a failed switch goes back (e2e). The fail-closed reads (store error at start, a poisoned
  lock) have no test: the store has no fault injection, and the code was checked by reading.
- Independent review, round 2 (fresh session, at `2fd4fd67`): approve; every round 1 fix
  verified (55 targeted Rust tests, 13 `shells::`, 91 scanner tests, 8 `vidoc-switch` e2e). Its 2
  minor findings and 3 nits, all own code, are fixed in `5b91e718`: a switch the proxy cannot read
  refuses with 503 (the scan ends incomplete, which blocks a merge) instead of "off" (Skipped);
  the refusal is one constant whose marker `the_scanner_recognizes_the_proxys_refusal` checks
  against the scanner's matcher and its test (a mutation of the matcher fails it); the scanner
  drops the body's line end (its test failed with a blank line before); the text says the scan
  did not run; ADR-0242 notes that interception needs the proxy's CA. The 503 path has no test
  (no store fault injection).
- Round 3 (the reviewer of round 2, on `2fd4fd67..` the final head) checks those fixes.
- `yulius security-scan`: **not run**, exit 2. Vidoc refused the instance key (HTTP 401 "Missing or
  invalid API key"); the 25 changed code files were not scanned, and this is not a pass. No
  `[[ignore]]` entry was added.
- Not tested: a real Vidoc scan after this change (the key is refused), and a production instance
  (the change goes to `dev` only).
