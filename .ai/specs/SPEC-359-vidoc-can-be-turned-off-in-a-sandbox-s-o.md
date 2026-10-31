---
yulius-task: 68da275a-2e0c-499b-a44a-17515c2de5b4
status: in-progress
---

# SPEC-359: A sandbox's admins switch its Vidoc scans off and on in the sandbox itself

Status: in-progress
Date: 2026-10-31
Owner: Yulius task #339
Scope authorization: task #339 (`/goal`): extend SPEC-338 (task #322) so that the people of a
sandbox turn its Vidoc scans off and on there, "not even from superadmin but per a single
sandbox". Extends SPEC-338 (the per-project switch, ADR-0229), SPEC-356 (the instance switch,
ADR-0242), SPEC-195 (the instance key, ADR-0157), SPEC-217 (the built-in CI job) and ADR-0107 (the
committed switch).

## Goal

The admins of a sandbox turn its Vidoc security scans off, and on again later, on the sandbox's
own CI/CD page, without the instance superadmin, for example while the Vidoc key is out of quota
(HTTP 429, 2026-10-22 to 2026-10-27) or refused (HTTP 401, since 2026-10-30). The page shows the
one switch SPEC-338 stores, so the Instance page's Vidoc card, the Add sandbox dialog and the
sandbox page always agree; it says who changed it and when, and says when the repository itself
(`.config/vidoc.toml`) or the instance has turned Vidoc off. When a scan stops because of the key,
the scanner's message, and so CI's `security` job log and the agents' "Needs you" cards, point to
this sandbox's switch.

## Context

- SPEC-338 stores the project switch as `vidoc: false` in the sandbox's profile JSON
  (`delivery::vidoc_switched_on`), changed by `PATCH /api/sandboxes/{sid}` `{"vidoc": bool}`
  (`Action::Administer`: sandbox admin or superadmin; a writer gets 403), audited as
  `delivery.vidoc.update` (actor, time, `on`/`off`). Its only UI is the superadmin's Instance page
  (`instancekeys.js` `vidocProjects`) and the Add sandbox dialog; a sandbox admin who is not a
  superadmin has no way to it but the API (SPEC-338 review round 1, minor 5).
- SPEC-356 adds the instance switch; `App::vidoc_scans_on(sandbox)` combines both.
- ADR-0107: `enabled = false` committed in `.config/vidoc.toml` turns the scanner off for the
  branch that carries it; Yulius never reads that file today.
- A sandbox has no single settings page: its settings live where they act (the delivery profile on
  the Git page, CI on the CI/CD page's CI tab, which lists the jobs in force and their source).
- Sandbox access mirrors GitHub: `admin` → `AccessLevel::Admin`, `maintain`/`write` → `Write`.
- The scanner (`scripts/vidoc_scan.py`) says "Vidoc refused the key (HTTP 401 …)" with
  `HOW_TO_GET_A_KEY` (the Instance page) and "Vidoc is out of quota for this key (HTTP 429 …)"
  with no pointer; in CI it then ends skipped with "it could not run (exit 2, reason above)".
  Agents read that message and raise `needs_you`.

## Requirements

- **VSB-001 One switch, one store.** `GET /api/sandboxes/{sid}/vidoc` (anyone who can view the
  sandbox) answers `SandboxVidocView`:
  - `project`: the stored switch (the same `profile.vidoc` the sandbox view and the Instance card
    show);
  - `instance`: the instance switch (SPEC-356);
  - `key`: whether a Vidoc key reaches the project when it is on (the instance key, or the
    project's own `VIDOC_API_KEY` agent secret);
  - `repository`: `on`, `off` or `unreadable`, from `.config/vidoc.toml` at the integration
    branch's head (`on` without the file), with `repository_problem` when unreadable and `branch`;
  - `scans`: whether a scan runs (all three on);
  - `can_change`: whether the caller may flip it (`Action::Administer`);
  - `changed_by`, `changed_at`: the newest `delivery.vidoc.update` audit entry, if any.
  A playground answers 422 (it has no switch). Writes stay `PATCH /api/sandboxes/{sid}`
  `{"vidoc": bool}`: no second endpoint, store or permission.
- **VSB-002 Who flips it.** The sandbox's admins (GitHub `admin` on its repository) and
  superadmins, as for the delivery profile; a member with write or maintain access sees the state
  and gets 403 from the PATCH. Each change is audited as today (`delivery.vidoc.update`, who,
  when, `on`/`off`), and the view's `changed_by`/`changed_at` show it.
- **VSB-003 The repository wins.** With `enabled = false` committed on the integration branch, the
  view says `repository: off` and `scans: false`; the card shows the switch off and disabled,
  says that the repository turned Vidoc off (`.config/vidoc.toml` on the branch) and that a
  reviewed pull request setting `enabled = true` turns it on, and offers no way to turn it on. A
  file the scanner would refuse (its keys, tables, `enabled`, `exclude` or an `[[ignore]]` entry,
  as `parse_config` checks them) is said as unreadable, never as on or off (scans then end not
  run), and the switch still works. The last change shown is this sandbox's: entries older than
  the sandbox (a removed one's, under the same slug) are not.
- **VSB-004 The CI/CD page's Security scan card.** The CI tab of `/s/<slug>/ci` shows, at its top
  (above the CI configuration card and the runs, which it decides), a "Security scan (Vidoc)" card with the switch "Scan this project with
  Vidoc", for every non-playground sandbox. While the instance is off, it says so and shows the
  project's own choice, disabled. Without a key reaching the project it says that scans end "not
  run" and that switching off makes them say "disabled". Turned off, it says that nothing is
  scanned or sent to Vidoc and nobody is asked about the key or its quota, until it is on again.
  A member who cannot change it sees it disabled with who can. A change updates the shared
  sandbox list, so the Instance card in the same browser and live updates elsewhere follow; a
  failed change goes back and says why.
- **VSB-005 Off is SPEC-338's off.** Nothing changes in what off does (VOF-002, VOF-003): nothing is
  sent, the scan reads "disabled", CI ends skipped, nobody is asked about the key or quota.
- **VSB-006 Key failures point here.** Agent runs, terminals and CI jobs of a non-playground
  sandbox get `YULIUS_VIDOC_SWITCH`, the URL of its CI/CD page (`<public_url>/s/<slug>/ci`). When
  Vidoc refuses the key (401/403) or its quota is used up (402, or a 429 that says so), the
  scanner's "not run" message adds: "To stop scanning this project until the key works, a sandbox
  admin turns Vidoc off for it in Yulius: <url> (Security scan card); a superadmin can also do it
  in Settings, Instance." Without the variable (outside Yulius) the message is as before. The
  briefing's `## Security scan` section tells agents to name that switch in any `needs_you` about
  the key or its quota, and the switched-off texts (briefing, onboarding prompt, Add sandbox hint)
  say that a sandbox admin switches it on again on the CI/CD page, or a superadmin on the Instance
  page.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A sandbox's Vidoc view says its switch, the instance's, the repository file's and who changed it
- [x] Sandbox admins switch Vidoc off and on from the CI/CD page's Security scan card
- [x] Key failures in scans and agents' texts point to the sandbox's switch
- [x] ADR, architecture, operations, API, AGENTS.md and product docs describe the sandbox switch
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- `GET /api/sandboxes/s1/vidoc` as a writer: `project: true`, `can_change: false`; the writer's
  `PATCH {"vidoc": false}` is 403. As the sandbox admin (not a superadmin): `can_change: true`;
  `PATCH {"vidoc": false}` then the view says `project: false`, `scans: false`, `changed_by:
  "alice"`; the superadmin's list (`GET /api/sandboxes`, the Instance card's source) shows
  `profile.vidoc: false`; the superadmin's PATCH on again shows in the sandbox view
  (`changed_by` the superadmin). Audit: `alice off`, then the superadmin's `on`.
- With `.config/vidoc.toml` `enabled = false` on the integration branch: `repository: "off"`,
  `scans: false`, whatever `project`; a malformed file: `repository: "unreadable"` with its
  problem.
- The scanner with `YULIUS_VIDOC_SWITCH=https://y.test/s/app/ci` and a 401 or a used-up 429 says
  the URL; without the variable it does not. Runs, terminals and CI jobs carry the variable.
- e2e on the mock: an admin flips the card's switch off and on (the Instance card follows); a
  writer sees it disabled; a repository-off sandbox shows the switch off, disabled, with the
  repository's reason.

## Test plan

- Domain unit tests: the `.config/vidoc.toml` reader (absent, `true`, `false` with a comment,
  tables and arrays after it, a non-boolean, a duplicate key).
- Rust integration tests in `crates/yulius/tests/vidoc_switch.rs`: the view and permissions (admin,
  writer, superadmin), the shared state with the sandbox list and the audit, the repository
  override and an unreadable file, a playground's 422; `pool_supervision.rs`, `shells.rs` and
  `local_ci.rs`: `YULIUS_VIDOC_SWITCH` in the run, terminal and CI job environments.
- Scanner unit tests (`scripts/test_vidoc_scan.py`): the pointer on 401 and on a used-up 429, none
  without the variable.
- Domain unit tests of the briefing's texts; onboarding's off text.
- e2e (`e2e/vidoc-switch.e2e.ts`): the card's switch, a writer's disabled card, the repository-off
  card, the Instance card following.

## Decisions

- **On the CI/CD page, not a new sandbox settings page.** A sandbox has no single settings page;
  each setting sits where it acts (the delivery profile on the Git page, CI on the CI tab). The
  switch decides CI's `security` job and the agents' scans, and the CI tab already lists the jobs
  in force and where they come from, so the card sits there, at the top (its own loader keeps
  focus while the runs below refresh), at the plain address `/s/<slug>/ci` that the scanner's message can link to. A new page for one switch
  would be a second home for sandbox settings.
- **Sandbox admins, as the delivery profile.** The PATCH already requires `Action::Administer`
  (GitHub admin, or superadmin), the permission of every other sandbox setting that can turn a
  check off (`require_checks`). Writers and maintainers can create tasks but not loosen the gate;
  turning the scan off loosens it, so they do not get it. They see the state and who can change
  it.
- **Read the existing PATCH, add only a view.** A second write endpoint would be a second path to
  the same store; the view adds what the sandbox view cannot cheaply say (the repository file at
  the integration head, the instance switch, the last audit entry) without making every sandbox
  read do Git.
- **Yulius reads the repository's switch at the integration branch, and does not refuse the
  PATCH.** The scanner honours the committed file on the branch it scans; the integration branch
  is what every new task and pull request starts from, so it is what the card reports. The
  project's own switch stays the project's choice: turning it on while the repository is off
  changes nothing visible (`scans` stays false), so the server keeps accepting it (the Instance
  card keeps working), and the sandbox card does not offer it. The Instance card does not read
  each project's repository (a Git read per project on a superadmin overview).
- **`key` counts the instance key and the project's own secret, not a personal one.** A personal
  `VIDOC_API_KEY` reaches only its owner's own tasks (ADR-0186), not CI or other people's runs, so
  the card (seen by every member) does not claim a key reaches "this project" for it.
- **The repository file's shape, not full TOML.** The domain has no TOML parser; the reader checks
  what the scanner's `parse_config` refuses line by line (review round 1), and treats a form it
  does not know (a multi-line string) as unreadable rather than guessing on or off.
- **A URL in the environment, not a guessed one.** The scanner runs in sandboxes, terminals and CI
  jobs without knowing Yulius's public address; Yulius sets `YULIUS_VIDOC_SWITCH` from
  `public_url`, as the `yulius/ci` commit status links to the same page. An older Yulius or
  another runner sets nothing, and the message stays as it was.

## Evidence

(to be filled)
