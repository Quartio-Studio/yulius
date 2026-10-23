---
yulius-task: 6ddce5ee-4c8d-4818-966c-41e764a1d0ec
status: done
---

# SPEC-278: CD runs use agent states, project variables, Docker and SSH

Status: done
Date: 2026-10-23
Owner: task #264
Scope authorization: task #264. First user: Quartio-Studio/product on 23 Oct 2026; its founder
asked for "dev: rollout to Quartio's VM, prod: rollout to all others" (its decision 0017).

## Goal

A deploy tool whose live state moved into Yulius (SPEC-271) deploys from CD: a `[cd.<name>]`
table names the agent states its run needs, Yulius restores each from the vault into the run
before the deploy, exports its path as `YULIUS_STATE_<NAME>`, and saves it back as a new version
after the deploy and the verify, failed ones included, while no other writer can touch it. CD
runs also get the project's variables, Docker where it exists and outbound SSH, all documented.

## Context

Confirmed against `dev` at 470f1029.

- Agent states (SPEC-271, ADR-0200, `crates/yulius-app/src/vault/states.rs`): a sealed gzip tar
  per sandbox and name, versioned; `secret_state_restore` writes it into the sandbox home and
  records `restored` (path, version, task); `secret_state_save` packs the restored folder and
  stores version + 1 with a compare-and-set on the version and the restored copy
  (`agent_state_put`); a laptop import over a restored copy is refused (409) unless `replace`.
  `restore` of a stale copy (vault newer) replaces the folder; `save` of a stale copy is refused.
- CD (SPEC-134, SPEC-257, `crates/yulius-app/src/ci/cd.rs`): a run gets a fresh `HOME` in
  `<data>/runs/cd-<run>/work/home`, the commit's tree in `work/src`, the secrets its table names,
  `YULIUS_CD_*`. Dev runs in the sandbox's containment (the sandbox's Docker directory is bound
  in, `ci_join` keeps `docker_dir`); production joins only the sandbox's network on the host's
  read-only system, with `docker_dir = None` and empty `/home`. Paths are bound at their host
  paths, so `work/…` is the same path inside and out.
- Project variables (SPEC-275, ADR-0203) reach task runs only ("CD runs take their environment
  from the verified `yulius.toml`; letting a laptop token change it is a separate decision").
  They are set by people with write access (`Action::Configure`), never by task agents; names
  follow `SecretName` (reserved: `PATH`, `ENV`, `BASH_ENV`, `NODE_*`, `LD_*`, `GIT_*`, proxies …).
- The sandbox network is `slirp4netns` with host loopback off and no port filter: any Internet
  host is reachable over TCP, port 22 included (probed from a sandbox on 23 Oct 2026).
- The sandbox's Docker engine (ADR-0151, ADR-0158) runs the sandbox's own `dockerd` (from the
  sandbox's system layer, which agents can change with `sudo apt`) in the sandbox's namespaces;
  production has no engine.
- The yulius.toml parser (`spec_format/config.rs`) knows strings, booleans, integers and `[]`.

## Requirements

- **CDS-001** `[cd.<name>]` takes `states`, the agent states its run uses: an array of names
  (`states = ["QUARTIO_OPERATOR"]`) or a space-separated string, each an agent secret name, at
  most 8, duplicates dropped. The parser reads one-line arrays of quoted strings (nothing nests). The CD page, the API
  (`CdEnvironment.states`) and `cd_status` name them.
- **CDS-002** A deploy is refused before anything runs (409, "nothing ran") when a named state
  is not in the project's vault.
- **CDS-003** Before the deploy command, a run takes the hold of each named state (all at once,
  in one step, so two runs never deadlock), waits while another run holds one (the output says
  which run; bounded by the job timeout and ended by a cancel), then restores the vault's
  newest version of each into `work/states/<NAME>` of its own directory (never a task's copy, in
  both stages; never inside `HOME`, so a state cannot shadow the run's dotfiles), and exports
  `YULIUS_STATE_<NAME>` with that absolute path. Audited `vault.state.restore` with the run.
- **CDS-004** After the deploy and the verify, whatever their outcome (passed, failed, timed
  out, cancelled), the run packs each state's folder and stores it as the next version with a
  compare-and-set on the version it restored; a folder unchanged since the restore is not saved
  (the version stays). Audited `vault.state.save`. A save that fails ends the run as `error`
  naming the state, and the output says what was lost; then the holds are released.
- **CDS-005** One writer at a time: a task's `secret_state_save` and a person's import take the
  same hold without waiting and are refused (409) while a CD run holds the state, naming the run.
  A task may restore while a run holds it (its copy becomes stale when the run saves).
- **CDS-006** After a CD save a task's restored copy is stale: `secret_state_save` refuses it
  and says to restore first (which replaces the folder with the newest version);
  `secrets_list` marks each state `stale` (with the newest version and who wrote it) and `held_by`
  a running CD run. A laptop import over a state whose newest version a CD run wrote is refused
  like one over a restored copy, unless `replace`.
- **CDS-007** CD runs get the project's variables in their environment, first (Yulius's own
  variables and the secrets win): a dev-stage run all of them; a production-stage run only those
  its verified table names (`variables = ["NAME"]`, at most 32), since a person with write access
  sets variables and only an administrator deploys production. A named variable the project lacks
  refuses the deploy (409, nothing ran); the others are listed as left out in the output.
- **CDS-008** Docker: dev runs keep the sandbox's engine (`DOCKER_HOST`, buildx and `docker
  load` as tasks have them). Production runs get no engine in this change: a production engine
  agents cannot reach needs its own holder and storage, built by the chained task (see
  Decisions); until then the docs and `cd_status` (for each production-stage environment) say
  so.
- **CDS-009** SSH: CD runs reach every Internet host over TCP, port 22 included (the sandbox's
  network, host loopback off); Yulius keeps no host list. A project names its hosts and pins
  their keys in its own state or repository (`UserKnownHostsFile`/`StrictHostKeyChecking=yes`);
  the run's `HOME` is fresh, so nothing is trusted on first use from an earlier run.
- **CDS-010** Documented: ADR-0207, `docs/architecture.md`, `docs/remote-control.md`, the CD
  section of `docs/operations.md`, the infra-bootstrap skill's CD part, `docs/changes.md`.

## Plan

- [x] Spec and ADR-0207: CD runs restore and save agent states under one hold
- [x] A CD table names its states (parser arrays, domain, API, cd_status)
- [x] One writer: a hold per state that CD runs wait for and task saves and imports respect
- [x] A CD run restores its states before the deploy and exports their paths
- [x] A CD run saves its states after deploy and verify, failed ones included, audited
- [x] Tasks and the laptop see stale and held states (secrets_list, save and import refusals)
- [x] CD runs get the project's variables, production only those its table names
- [x] Docker and SSH in CD runs are documented and production's engine is a chained task
- [x] A served instance deploys dev and production with a state and refuses a concurrent save
- [x] Review findings fixed test-first
- [x] Gates are green and the spec has its evidence (the Vidoc scan is unrun, left to the CI security job)
- [x] The pull request is merged into dev on green

## Acceptance criteria

- **AC1** A served instance with the state `QUARTIO_OPERATOR` version 1 (sent over HTTP as the
  laptop does) and `[cd.dev]` naming it: a dev deploy's script finds `YULIUS_STATE_QUARTIO_OPERATOR`
  as an absolute path holding the state's files, appends to a file and adds one; after the run
  the vault holds version 2 with both changes, the audit has `vault.state.restore` and
  `vault.state.save` for the run.
- **AC2** A failing deploy still saves what it wrote (version 3).
- **AC3** While a deploy holds the state, a task's `secret_state_save` is refused naming the run,
  and a second deploy naming it waits and then restores the version the first saved.
- **AC4** A production deploy restores the vault's newest version, not a task's restored copy
  that has unsaved changes, and saves its own.
- **AC5** After a CD save, the task's restored copy is reported stale by `secrets_list`, its save
  is refused, and `secret_state_restore` replaces it with the newest version.
- **AC6** A deploy naming a state that does not exist is refused, nothing ran.
- **AC7** A dev run has the project's variables; a production run has only those its table names
  (`OVH_ENDPOINT`, not `PYTHONPATH`); a table naming a variable the project lacks runs nothing.
- **AC8** A deploy that leaves its state alone keeps the version; one that replaces the folder
  with a link ends as `error` naming the state, the link not followed and the version kept; a
  deploy cancelled while it waits for the hold ends cancelled while the holder keeps it.

## Test plan

- Unit (`yulius-domain`): `states` parsing (array, string, bounds, bad names, unknown arrays
  elsewhere still refused, deep nesting refused), `variables` parsing.
- Unit (`yulius-app`): the hold (all-or-nothing, waiting woken on release, try refusals),
  the unchanged-folder digest.
- Integration (`crates/yulius/tests/cd.rs`, the real Git adapter and detached ptyhost, the
  router served on a loopback port for the deploys and the state upload): AC1–AC8.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **ADR-0207**: a CD run is a writer of the agent states its table names: it restores the vault's
  newest version into its own directory and saves a new version after the run, under an in-memory
  hold per state that tasks and imports respect. Tasks keep writing their restored copy, but only
  while it is current: a CD save makes it stale, and a stale copy is restored (replaced) before it
  is written again. Alternatives: CD runs reuse the task's restored copy in the sandbox home (dev
  only; production must not run on files an agent wrote, and two writers share one folder);
  CD-only writers once a table names a state (forbids the operator's manual fixes through a task);
  a persisted lock row (outlives a restart that already ends the run as error; the in-memory hold
  goes with the process that held it).
- **States go to `work/states/<NAME>`, not the task's restore path**: the restore path is chosen
  by an agent; inside a production run's `HOME` it could shadow `~/.ssh` and its `config`.
- **Unchanged folders are not saved**: a verify-only or early-failing run would otherwise make
  every task copy stale for nothing.
- **Production names its variables** (review of SPEC-278, M3): a first version gave production
  every variable but a deny list of code-loading names; every tool has its own hooks
  (`SHELLOPTS`/`PS4`, `OPENSSL_CONF`, `CLASSPATH`, `AWS_CONFIG_FILE`), so the list could never be
  complete. The reviewed table now names them. Dev runs are agent-reachable anyway and get all.
- **The vault's state is data production restores whoever wrote it** (a dev run, a task): the one
  exception to "nothing an agent wrote runs with production's secrets"; deploy tools must not run
  code from it (ADR-0207 consequences).
- **Production's Docker engine is a chained task**: an engine agents cannot reach needs its own
  holder (its own user and network namespace, the base's `dockerd`, not the sandbox's), storage
  outside the sandbox and the host-level smoke test; it is a separate concern of several hundred
  lines that cannot be verified inside a sandbox. This change keeps production without an engine
  and says so.
- **SSH needs no change**: the sandbox network already reaches port 22; restricting hosts would
  need a per-run network namespace and filter, which the founder did not ask for; the deploy tool
  pins host keys.
- **One pull request**: the hold, the restore, the save and the rules for tasks only work
  together; the line budget is exceeded on purpose, as SPEC-271 did.

## Evidence

On `feat/264-…` after merging `origin/dev` at 55a3129c (task #263's merge; no conflict; ADR-0206
and ADR-0207 kept in order, `docs/changes.md` checked).

- `scripts/dev ci` passed (exit 0) at 25afc700: 2554 tests passed, 17 skipped (repository checks,
  script tests, `cargo check`, `fmt`, Clippy for Linux and macOS, nextest, doctests, `cargo deny`,
  `cargo shear`, `cargo doc`). Earlier runs failed and were fixed:
  - `agent_states::a_state_is_restored_once_saved_back_and_never_forked` asserted the old wording
    of the stale-copy refusal ("newer"); it now asserts the new one.
  - The same test then failed once on its own check that no gzip magic bytes (`1f 8b`) appear in
    the first 64 bytes of random ciphertext, which happens about once in 1 000 runs (a
    pre-existing flaky assertion of SPEC-271). It now checks that no offset of the stored bytes
    decodes as an archive.
  - `cargo deny` failed on RUSTSEC-2026-0385 (`ctutils` < 0.4.4) until `dev`, which already had the
    bump, was merged.
  After the review fixes, `scripts/dev ci` passed again (exit 0) at 2a347759: 2555 passed, 17
  skipped. One run before it failed Clippy's `type_complexity` on a tuple parameter; the run's
  inputs are now a named struct (`RunInputs`).
- End-to-end suite (`cd e2e && npm ci --ignore-scripts && npx e2e run`): 686 passed, 1 skipped,
  1 failed: `cd.e2e.ts` "named environments show their stage and deploy by its rules" read the
  mock's recorded deploy as soon as the deploy view opened. The view opens before the request is
  sent (SPEC-189), so under the full suite's load the read came first (it passed alone). It now
  waits for the condition (`expect.poll`); `cd.e2e.ts` passes 18/18. The CD page's new States row
  is not in the mock's data, so no e2e test shows it (the API field is tested in Rust).
- `scripts/dev security`: **not run** (exit 2). Vidoc answered HTTP 429, "This API key has used
  its 50,000,000 tokens", so the 13 changed code files were not scanned. That is unrun, not
  passed; Yulius CI's `security` job tries again on the pull request.
- Behaviour tests, each seen failing first (or by a mutation):
  - Domain: `cd_tables_name_their_states` covers arrays, strings, bounds, bad names, `variables`,
    and a nested and a 65 000-bracket line refused. The bracket line aborted the test process with
    a stack overflow before the fix (review B1).
  - App unit: `vault::holds` covers all-or-nothing holds, a waiting run woken by the release, and
    another project's state of the same name. `an_unchanged_folder_has_the_restored_archives_digest`
    covers the digest.
  - Over a served instance (`crates/yulius/tests/cd.rs`, real Git adapter and ptyhost, HTTP on a
    loopback port), `deploys_restore_write_and_save_an_agent_state_with_one_writer`:
    - **AC1:** the state is sent as raw gzip like the laptop does. A dev deploy restores it outside
      `HOME`, appends to it and adds a receipt. The vault then holds version 2 by `cd:dev`, with
      both changes in the download, and audits `vault.state.restore` and `vault.state.save` with
      the run.
    - **AC2:** a failing deploy saves version 3.
    - **AC3:** while a run holds the state, a task's save, `secrets_list` (`held_by`) and a send
      are refused or report the hold, and a production deploy waits.
    - **AC4:** production restores version 3, never the task's unsaved line, and saves version 4.
    - **AC5:** the task's copy is then `stale`, its save is refused, and a restore replaces it.
    - **AC6** and **AC7:** a missing state or variable refuses the deploy. Dev gets
      `PYTHONPATH`; production gets only `OVH_ENDPOINT`.
    - Mutations, each of which fails the test: a task save ignoring the hold, a run that drops its
      hold, an import that ignores a CD writer, and production receiving every variable.
  - `a_state_left_alone_broken_or_waited_for_keeps_its_version` (**AC8**, review M2):
    - an unchanged folder keeps version 1;
    - a folder replaced by a link to `/` ends the run as `error` ("the deploy passed, but …"),
      naming the state, with the link not followed and version 1 kept;
    - a run cancelled while it waits for the hold ends `cancelled` while the holder keeps and saves
      it. Without the cancel branch the test would time out.
- Independent review in a fresh session (skill `yulius-review`, read-only):
  - Round 1 found 1 blocker, 2 majors, 6 minors and 4 nits, all in this change's own code:
    - B1, unbounded recursion in arrays: fixed test-first.
    - M2, failure paths untested: the tests above.
    - M3, production's deny list of variables could be bypassed (`SHELLOPTS`/`PS4`,
      `OPENSSL_CONF`): replaced by a `variables` allow list in the table.
    - Minors and nits fixed or documented in ADR-0207.
  - Round 2: **APPROVE** at 66d71abe. Its remaining nits were addressed: the ADR wording on PID
    namespaces is fixed, and the e2e race above is fixed.
- Not tested:
  - The save's compare-and-set retry (a task restore landing between the save's read and write;
    review round 2, minor 1). Forcing that window needs a store hook the harness does not have. The
    retry reuses one envelope, whose associated data (row id, version) the loop requires unchanged,
    and gives up after three attempts with the run in `error`.
  - The hold-wait timeout, which shares its `select!` with the tested cancel.
  - The pinning of the waiting path in the cancel test: the "waiting for" line is stored only once
    the process starts.
  - A real contained host: the tests run without sandbox isolation (`contained = false`). That the
    run directory is visible at the same path inside both containments is read from the ptyhost's
    bind rules (`--bind` at host paths) and the existing production containment test.
  - Quartio-Studio/product's real scripts and Deployer, which live on its branch. The test script
    exercises the same contract (`YULIUS_STATE_<NAME>` absolute, refuse without it).
  - Docker in production runs: delegated to task #265. Dev runs use the sandbox's engine as before.
  - SSH: outbound TCP 22 from a sandbox was probed (`/dev/tcp/github.com/22` open on 23 Oct 2026).
    No CD run made an SSH connection in a test.

