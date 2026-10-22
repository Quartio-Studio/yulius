---
yulius-task: c287f47b-b31e-4838-bc88-69cc87be3914
status: done
---

# SPEC-142: Yulius CI jobs opt in to named agent secrets through the egress proxy

Status: done. The scan it describes moved to Vidoc's vidoc-1 API in [SPEC-195](SPEC-195-vidoc-is-built-into-yulius-an-optional-i.md) (ADR-0157): the Vidoc CLI and `api.vidoc.dev` are gone.
Date: 2026-10-15
Owner: Yulius task #140 (part of task #118, SPEC-138)
Scope authorization: task #140: "A `[[ci.jobs]]` entry can opt in to named agent secrets (e.g.
`secrets = ["VIDOC_API_KEY"]`), read only from the integration branch's `yulius.toml`, never from a
pull request's own. The job then gets the egress proxy, the CA bundle and the `yulius-secret-NAME`
placeholders, never the values, as agent sessions do […]. The proxy substitutes a placeholder for a
CI job only toward that secret's bound hosts. Each CI use is in the secrets audit as `ci:<run>`.
[…] Then add a `security` job running `scripts/dev security` on pull request heads only. […] Do not
make a required check that fails every pull request until the key works." Issue #466.

## Goal

A Yulius CI job can use an agent secret the way an agent does, without ever holding its value.
`yulius.toml` on the integration branch names the secrets a job may use. The job reaches them only
through the egress proxy, which puts a value on a request only toward the hosts a person bound to
that secret, and every use is audited as `ci:<run>`. With that, this repository's Vidoc scan
(`scripts/dev security`, ADR-0104) runs as a `security` job on every pull request head.

## Context

- CI jobs run with no secret and without the proxy: `ci_job_env` in `crates/yulius-app/src/ci/exec.rs`
  builds a fixed environment, and the launch plan has `secret_env: Vec::new()`.
- Agent sessions get the proxy in `App::vault_run_setup` (`crates/yulius-app/src/vault/runtime.rs`):
  a placeholder `NAME=yulius-secret-NAME` per agent secret of the sandbox, `HTTP(S)_PROXY` =
  `http://yulius:<task token>@<internal listener>` (secret environment), `NO_PROXY`, and the CA
  bundle (host roots + egress CA) written to the run directory and named by `SSL_CERT_FILE` and
  friends. Bubblewrap binds the run's files read-only (`supervisor.rs`).
- The proxy (`crates/yulius-egress`) authenticates a client with the token in `Proxy-Authorization`
  through `EgressPolicy::egress_client`; today the only clients are live agent attempts
  (`EgressClient { attempt_id, task_id, task_number, sandbox_id }`). `egress_intercepts`,
  `egress_apply` and `egress_redactor` then use every agent secret of the client's sandbox (and the
  instance-wide ones) bound to the host; uses are audited `vault.agent.use` with actor `task:<n>`.
- Which jobs a run executes: the integration branch's `[[ci.jobs]]` (`App::ci_gate`, read at
  `origin/<integration>`), except that a pull request that changed its own jobs is tested with them
  (`own_jobs_apply`, `GateSource::PullRequest`), and a pull request into a repository whose
  integration branch has no CI uses its own. So "where do this run's jobs come from" is not "the
  integration branch" and cannot carry a secret grant.
- `scripts/vidoc_scan.py` refuses a detached HEAD and needs `git ls-remote origin` and
  `git remote show origin` to work, because the Vidoc CLI compares the branch with origin's default
  branch and falls back to `origin/main` silently. A CI checkout is a detached worktree of the
  sandbox's clone and has no GitHub credential, so the gate could never run in CI as it is.
- #470: the stored `VIDOC_API_KEY` is answered 401 by Vidoc; only a person can replace it.

## Requirements

- CSE-001: A `[[ci.jobs]]` entry may have `secrets`, agent secret names (`SecretName` rules)
  separated by spaces (`secrets = "VIDOC_API_KEY"`), at most 8, without repeats. An invalid list
  is a configuration error naming the job. Matrix values inherit the definition's list.
- CSE-002: The secrets a job may use are those that the integration branch's `[[ci.jobs]]` entry
  of the same name lists, read when the job starts from the integration head GitHub reports,
  taken into the Yulius-only CD mirror (`App::ci_verified_gate`; the sandbox clone, which CI jobs
  can write, only says whether to look). A `secrets` list in a pull
  request's own `yulius.toml` grants nothing, whether or not its own jobs run. The person's profile
  gate grants nothing. Amended by [SPEC-264](SPEC-264-ci-grants-agent-secrets-only-to-the-trus.md)
  CSG-001 (ADR-0196): the entry must also have the job's command, byte for byte, so a pull
  request's own job reusing a granted name for another command gets nothing.
- CSE-003: A job with a non-empty grant gets a placeholder `NAME=yulius-secret-NAME` for each
  granted agent secret that applies to the sandbox and is not a production-deploy (`cd_prod`)
  secret. When the egress proxy is wired it also gets `HTTP_PROXY`/`HTTPS_PROXY` (and lowercase)
  with a credential of its own (secret environment, never stored), `NO_PROXY`, `NODE_USE_ENV_PROXY`
  and the CA bundle in its run directory (read-only inside the containment). A job without a grant
  gets none of these, as before. The job's output says which granted names it got and which are
  missing.
- CSE-004: The proxy credential of a CI job is a fresh random token, valid from just before the job
  starts until its process has ended, held in memory only. The proxy client it names is
  `ci:<run>`: it may use only the job's granted secrets, only toward each one's bound hosts; a
  placeholder or `X-Yulius-Secret` of any other secret is not substituted (refused for the latter),
  and hosts bound only to other secrets are tunnelled opaquely.
- CSE-005: Each use of a secret by a CI job is in the secrets audit (`vault.agent.use`, actor
  `ci:<run>`), once per job, secret and host.
- CSE-006: In Yulius CI on a pull request head (`YULIUS_CI_REF=pr/<n>`), `scripts/dev security`
  scans a private clone of the commit on a branch `yulius-ci/pr-<n>`, whose `origin` keeps the
  real URL but resolves to a local mirror of origin's default branch, so the CLI finds the same base
  as on an agent's branch without a GitHub credential, and the sandbox's shared clone is never
  written.
- CSE-007: This repository's `yulius.toml` gains a `security` job, granted `VIDOC_API_KEY` once
  the running instance knows `secrets` (see Decisions). It runs
  `scripts/dev security` on pull request heads only and reports other heads as not scanned. It
  fails on exit 1 (findings, failed scan). Until a run can happen (#470), exit 2 (not runnable)
  does not fail the job: the output says the scan did not run and why.
- CSE-008: The development guide, ADR-0104, AGENTS.md, the architecture document and skill
  `yulius-verify` say that the scan is a CI check on pull requests, how it behaves while it cannot
  run, and how a job opts in to secrets.

## Plan

- [x] Spec SPEC-142 and the ADR on the CI secret trust boundary, validated
- [x] [[ci.jobs]] entries declare secrets, parsed and validated in the domain
- [x] CI jobs authenticate to the egress proxy as ci:<run> with only their named, host-bound secrets, audited
- [x] An opted-in CI job gets the proxy, CA and placeholders from the integration branch's grant; a PR's own grant is ignored
- [x] The Vidoc gate runs in a Yulius CI pull request checkout
- [x] The security job runs on PR heads; docs, ADR-0104 and the yulius-verify skill updated
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Review round 3 approves the final head; gates green locally
- [x] Gates green locally; ready to merge into dev on green Yulius CI (#466 stays open for the grant line)

## Acceptance criteria

- `secrets = "A B"` parses into the job and its matrix jobs and renders back; a bad name, a repeat,
  an empty list, more than 8 or a non-string is a configuration error on its line.
- A run on a real ptyhost: the job granted `API_KEY` on the integration branch sees
  `API_KEY=yulius-secret-API_KEY`, a proxy credential and a readable CA bundle, and nothing of
  another agent secret of the sandbox; a job without a grant sees no proxy, no CA and no
  placeholder; a pull request whose own `yulius.toml` grants a secret to its own job gets nothing.
- The job's proxy credential names a `ci:<run>` client while the job runs and nothing after it
  ended.
- Through `EgressPolicy`, a CI client intercepts and substitutes only its granted secret toward
  that secret's hosts; another secret's host is not intercepted, its placeholder stays literal and
  `X-Yulius-Secret` naming it is refused 403; the use is audited with actor `ci:<run>`.
- `scripts/test_vidoc_scan.py`: the private clone of a detached PR checkout has the branch
  `yulius-ci/pr-<n>`, the real `origin` URL, `origin`'s default branch read from the mirror, the
  integration branch as merge base, and leaves the shared clone's refs untouched; outside a
  pull request CI run the gate is unchanged.
- `yulius.toml` has the `security` job (its grant waits, see Decisions); a test pins its command's behavior for
  exit 0, 1, 2 and a non-PR ref.

## Test plan

- Domain unit tests (`yulius-domain` `spec_format/config.rs`, `ci/mod.rs`): parsing and validation
  of `secrets`, matrix inheritance.
- `crates/yulius/tests/secrets.rs`: a CI egress client registered on the real `App` through its
  `EgressPolicy`: interception, substitution, refusal of an ungranted `X-Yulius-Secret`, the audit
  actor, and that the client is gone once closed.
- `crates/yulius/tests/local_ci.rs` (real Git, real ptyhost): opted-in, not opted-in, and a pull
  request's own grant; the credential works during the job and not after it.
- `scripts/test_vidoc_scan.py`: CI clone preparation against real temporary Git repositories;
  `main()` in CI mode on a non-PR ref unchanged.
- `scripts/test_check_repository.py` or a new unit test: the `security` job's command semantics
  with a fake `scripts/dev`.

## Decisions

- ADR-0116 records the trust boundary: the grant comes from the integration branch by job name;
  the job holds placeholders and a per-job proxy credential, never values; the client identity is
  `ci:<run>`; production-deploy secrets are never granted; pull request code can still use a
  granted secret toward its bound hosts for anything the key allows there, which is the risk a
  person accepts by binding the host and granting the name in a reviewed `yulius.toml` change.
- The grant is read when the job starts, not frozen in the run: removing a name from `yulius.toml`
  on the integration branch takes effect for the next job, and no schema migration is needed.
- A pull request that changes its own jobs keeps the integration branch's grant for a job of the
  same name. Its code already decides what that job runs (`scripts/dev` is the pull request's), so
  inheriting the grant widens nothing, and a pull request touching `yulius.toml` keeps its scan.
- The CI credential is in memory, not in the store: it dies with the process host's job and with a
  Yulius restart, and is not cached by the proxy's 15 s client cache, so it stops at once.
- The private clone (CSE-006) uses Git alternates (no object copy) and a local URL rewrite in the
  clone's own configuration. Rejected: switching the CI checkout to a branch (it is a worktree of
  the shared clone, so the branch would appear in the sandbox), and a URL rewrite in the CI
  checkout's environment (the CLI's `git fetch origin <default>` would then rewrite the shared
  clone's `refs/remotes/origin/*` from the mirror).
- `secrets` is a space-separated string, not the TOML array of the task's example
  (`["VIDOC_API_KEY"]`): the domain's strict TOML subset has no non-empty arrays, and the same
  file already names secrets that way (`[cd.<env>] secrets`) and lists matrix values that way.
  Teaching the parser arrays for one key would make two spellings of one idea.
- Exit 2 does not fail the `security` job (CSE-007): it means only a person can make the scan run
  (#470, a missing host binding), and failing every pull request for it would stop all delivery.
  The job prints the reason, and the docs say the check enforces findings only once the scan runs.
- Review round 1 (fresh session, request changes on 6324fa8c). B1 (blocker): CI jobs can write
  the sandbox clone (the sandbox root is mounted read-write), so a `git replace` of the
  integration branch's `yulius.toml` blob, or a moved `origin/<integration>`, forged the grant.
  The grant is now read from the integration head GitHub reports in the Yulius-only CD mirror
  (`App::ci_verified_gate`), and `local_ci::a_grant_forged_in_the_shared_clone_gives_nothing`
  reproduces the forgery (it failed before the fix: the job got `OTHER_KEY`). M1: tests now pin
  that a granted production-deploy secret is never usable. m1: the credential is a guard closed
  on drop. m2: file-system errors while preparing the CI clone exit 2. m3: ADR-0116 names the
  follow-up (exit 2 fails again once a scan runs), also noted on #466. n1: wording fixed. n2
  (a `secrets` line before `name` reports job `?`, as `matrix` does) and n3 (one cached read per
  job start) stay as they are.
- Review round 2 (fresh session) approved 9c138d10 with two minors and two nits, all fixed
  test-first: the grant is verified against the clone as it is and the clone is fetched only when
  that fails, so a concurrent fetch no longer withholds the key
  (`local_ci::a_grant_verifies_without_a_fresh_fetch_when_the_clone_has_the_head` failed before);
  a granted production-deploy secret is reported as such, not as missing; ADR-0116 notes the
  mirror kept on disk; a long doc line is wrapped. Out of scope and noted on #466: the sandbox
  clone's fetch with an installation token reads repository config agents can write (#54 part B).
- Review round 3 (fresh session, the last) approved 3eb67798 with one minor and three nits. Decided
  after three rounds: the approved code ships unchanged. The minor (no test moves the integration
  head between the sync and the job, so the fetch-and-retry branch of `ci_verified_gate` and "a
  removed name takes effect for the next job" are untested) and the nits (a CD-worded message when
  a grant cannot be verified; an ambiguous report when an instance-wide and a sandbox secret share
  a name, `cd_secret_states` has the same rule) are follow-ups noted on #466, since this task
  creates no issue. The only later change is a rewrapped doc comment and this record.
- Found after round 3 (`spec_check` of the running instance): the instance, built from `main`,
  rejects `secrets` in `[[ci.jobs]]` as an unknown key, and with it the whole `yulius.toml`. Had
  `dev` carried the grant line, the running Yulius would have lost the CI of every pull request
  into `dev` until a person promotes. So the product capability ships, and this repository's
  `security` job ships without its grant line; tests pin that. A follow-up adds
  `secrets = "VIDOC_API_KEY"` once the running instance knows the key; nothing is lost meanwhile,
  as the stored key is rejected anyway (#470). #466 stays open for it, with a comment saying so.
  The round-3 reviewer reviewed this delta: text-only changes requested (the changelog headline
  said the scan runs; this spec claimed the merge and its own confirmation), made in the next
  commit.

## Evidence

- `scripts/dev ci` (doctor, check, lint, test, deps, rustdoc) at 3eb67798: exit 0; nextest 1778
  passed, 16 skipped; `python3 -m unittest discover -s scripts`: 100 tests OK; repository
  contracts passed; cargo-deny advisories, bans, licenses and sources ok. Also exit 0 at 6324fa8c
  and 9c138d10.
- New tests, each seen failing before its change:
  - `yulius-domain` `ci_jobs_name_their_agent_secrets`, `the_repositorys_own_ci_has_a_security_job_and_no_grant_yet`;
  - `yulius-app` `ci::exec::tests::only_the_integration_branch_jobs_grant_secrets`,
    `run_files_outside_the_sandbox_are_bound_read_only`;
  - `secrets::a_ci_job_uses_only_its_granted_secrets_toward_their_hosts` (host binding, ungranted
    and production-deploy secrets, 403 on `X-Yulius-Secret`, audit actor `ci:run-7`, credential
    gone once closed); it fails when `may_use` ignores the grant or `cd_prod`;
  - `local_ci::ci_jobs_get_the_secrets_the_integration_branch_grants_them` (opt-in, no opt-in, a
    pull request's own grant, the credential a `ci:<run>` client only while the job runs),
    `a_grant_forged_in_the_shared_clone_gives_nothing` (failed before B1's fix: the job got
    `OTHER_KEY`), `a_grant_verifies_without_a_fresh_fetch_when_the_clone_has_the_head`;
  - `local_ci::a_contained_granted_job_reads_its_ca_bundle` skips here: bubblewrap cannot create
    namespaces in this environment ("Operation not permitted"), as `contained_jobs_see_only_their_sandbox` does;
  - `scripts/test_vidoc_scan.py` `PullRequestCi` (5) and `SecurityJob` (4).
- `scripts/dev security` (agent-side, at 6324fa8c): exit 2, "Vidoc rejected the key (HTTP 401:
  Invalid or expired token)": unrun, not passed (#470, ADR-0104).
- Not verifiable here: a live Vidoc scan in Yulius CI (needs a valid key, #470) and the CA bundle
  inside bubblewrap (no user namespaces here). Yulius CI on the delivered head runs the new
  `security` job; without its grant line and while #470 stands it ends "the security scan did not
  run" and passes.
- `spec_check` (the running instance) after the grant line was removed: docs valid, spec valid.
- After the grant-line change: `python3 -m unittest discover -s scripts`: 100 OK; the domain test
  above passes; `cargo fmt --check`, clippy and `scripts/check_repository.py` pass.
- Reviews: three fresh read-only sessions; round 1 requested changes (B1 forged grant and four
  smaller findings, all fixed test-first), rounds 2 and 3 approved.
