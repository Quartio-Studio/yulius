---
yulius-task: 84ea1903-0e8e-4141-8d50-e4d01d283b8e
status: done
---

# SPEC-650: Merge-first staging: a person and their Superagent deploy staging at the gated head, and staging never stalls

## Goal

A person, and their Superagent on their request with the panel's confirmation, can always ask
the merge-first staging target to deploy the newest integration revision whose full gate passed.
A head still under test or red queues the request for the next green revision, and the CD page
says so in plain words ("waiting for green tests on `<sha>`") instead of showing a disabled
button. Staging also moves on by itself, on a per-project timer or after a number of merges.
Guidance never sends a person to a control that is disabled for them.

## Context

Seen in a project on 2026-11-24/25 (task #634's request):

- The CD page disables the merge-first environment's "Deploy dev" button for everyone
  (`cd.js` `envCard`: `env.staging ? 'Staging deploys a task’s merged revision…'`). The only
  visible clue is "the branch has moved on". Task notes and needs-you cards told the person to
  press it.
- The Superagent's `cd_deploy` of a dev-stage environment runs the person's `App::cd_deploy`,
  which `cd_prepare` refuses for a merge-first target without a pinned revision
  (`crates/yulius-app/src/ci/cd.rs`: "[cd.dev] is the merge-first staging target; it changes only
  to a merged revision whose full gate passed, when a task asks with cd_deploy").
- Only a task can ask (`App::staging_request`). It is refused while any step is open and after
  the task's one successful staging deploy (`stg::admit`). So when no task finished its whole
  plan, merged green revisions never reached staging: one night, about 8 hours and a dozen merges.
  A production promotion must promote the revision staging verified (SPEC-575 MFS-009), so
  production was blocked too.
- The store already has task-less requests (`staging_requests.task_id` NULL, "a person's request
  for the integration head", migration 0078). Nothing creates them, and the queue cannot deploy
  them: `staging_start_deploy` needs a task's owner, so they would fail as `deploy_failed`.
- The production gate's refusal says "deploy dev at this commit first"
  (`yulius_domain::rollback::production_gate`). On a merge-first project that pointed to the
  same disabled button.
- The Superagent's production deploy waits on its panel for the person's click and the typed
  project slug (SPEC-487 REL-001 to REL-005, `release_requests.rs`).

Related: SPEC-575 (merge-first staging), ADR-0411 (the queue), ADR-0425 (a production deploy
pinned to verified dev), SPEC-487 (Superagent release requests), SPEC-631 (finished tasks hear
their outcome). Parallel task #633 (an administrator could not deploy production) changes
production authorization, not staging.

## Requirements

- **PST-001 A person asks for staging.** A person with write access asks the merge-first
  environment to deploy on the CD page ("Deploy dev") or through `POST
  /api/sandboxes/{sid}/cd/{env}/staging`.
  - Yulius records a task-less staging request for the verified integration head. The request is
    recorded even while another deploy of the environment runs; the queue takes it in order.
  - A green head (its full gate passed) is deployed, then verified, in revision order.
  - A head whose gate is queued or running waits. A head with no run gets one candidate run
    (SPEC-575 MFS-003).
  - A person's request for a revision that already has a pending request of a person or the
    schedule answers with that request. Nothing is recorded twice.
  - A succeeded one answers only while staging still contains the revision. After a rollback, a
    failed deploy or a cancel, a new request is recorded for the one asking now. A failed request
    is never made ready again by the queue, so a person's cancel stands.
  - "A person's or the schedule's" is read from `requested_by` (`user:<id>`, `yulius:schedule`).
    A deleted task's request also has no task id, and stays a task's.
  - The plain deploy (`POST …/deploy`) of the merge-first target stays refused (it answers a
    CD run, which a request is not). Its refusal names what works: "Deploy <env> on the CD page
    or a Superagent's cd_deploy".
- **PST-002 A red head queues for the next green revision.**
  - A task-less request whose revision's full gate failed becomes `inherited`, not
    `gate_failed`. Its note names the failed run and says it waits for green tests on a newer
    revision.
  - Once the integration head moves past it, Yulius records one task-less request for the new
    head (the same requester). Once a revision containing it is verified, the waiting request
    is `included`.
  - One carried request at a time: nothing is carried while a newer request is testing, ready or
    deploying, or for a revision staging already serves. Nothing is carried to a head that has any
    request (a task's too) in a state but superseded or inherited, so a failed follow-up never
    loops, and a cancel, rollback or failed deploy there stands.
  - A waiting request moves on once a rerun of its revision passes the full gate.
- **PST-003 Who a task-less deploy acts for.**
  - A person's request deploys as that person (`requested_by` = `user:<id>`), while they are
    active with write access, checked again when the deploy starts.
  - A scheduled request (`yulius:schedule`) deploys as the project's creator. If the creator
    cannot (disabled, no write access), it deploys as the newest requester in the environment's
    queue (a person, or a task's owner) who still has write access. With neither, it fails with
    why.
  - A failed deploy of a task-less request restores staging as a task's does (SPEC-575 MFS-007).
- **PST-004 The Superagent asks on its panel.**
  - The Superagent's `cd_deploy` of the merge-first environment becomes a request on its panel,
    with the same card and typed project slug as a production deploy. Write access is enough;
    production still needs an administrator.
  - Approved, it records the person's staging request (PST-001) as that person and answers
    with the request.
  - The tool's description says so.
- **PST-005 The CD page says what staging waits for.**
  - On the merge-first card, "Deploy dev" is enabled for a person with write access whenever the
    environment is defined. A running deploy does not disable it.
  - The staging section names each task-less request as "Asked by <login>" or
    "Yulius's schedule".
  - It shows the newest pending task-less request's wait in plain words:
    - "Waiting for green tests on `<sha>`" while testing;
    - "Waiting for green tests: `<sha>` failed; the next merged revision is tested" when
      inherited;
    - "Deploying `<sha>`" while deploying.
  - A disabled deploy button always shows its reason as visible text, not only in a tooltip.
- **PST-006 Staging never stalls on one task.**
  - `[cd.<name>]` with `merge_first = true` takes `serve_every_minutes` (default 60, `0` turns
    it off) and `serve_after_merges` (default 5, `0` turns it off), refused on any other table.
  - The staging tick checks each project at most every 5 minutes (a project whose last check
    found no merge-first target, once an hour), after the repair notices.
  - When the integration head is ahead of what staging serves and no request is on its way
    (testing, ready or deploying; an inherited one waits for others), Yulius records a scheduled
    request for the head (`yulius:schedule`, note naming the trigger). The triggers:
    - staging serves no revision of the branch yet;
    - at least `serve_after_merges` first-parent commits since staging's revision;
    - at least `serve_every_minutes` since the environment's last staging request or run
      (deploy, rollback or drill).
  - It never asks for a head any request already tried, a task's included: a failed deploy, a
    person's cancel or rollback stands until a newer head lands.
  - A verified scheduled deploy is promoted where the delivery profile promotes automatically, as
    any verified staging revision is.
  - A task's own request keeps its rules ("a task's whole delivery once", one successful deploy
    per task) for its own verification bookkeeping. It never gates staging any more.
- **PST-007 Guidance names a working action.**
  - The server decides each environment's deploy control for the viewer
    (`CdEnvironment.deploy_action`: label, enabled, why). The CD page renders it as it is.
  - The production gate's "deploy dev at this commit first" names the control instead: "get it
    deployed on dev first with Deploy dev on the CD page (a merge-first dev asks for the
    integration head)". Since PST-001 that button works on a merge-first dev for a person with
    write access, and production's deploy needs an administrator, who has it.
  - A `needs_you` card names a button by its label with its capital ("Deploy dev",
    "Deploy production"), as the page shows it; prose such as "ask an administrator to deploy
    production" names no button and is asked.
  - A plain head deploy refused on the merge-first target (an automation's, a task's
    `cd_deploy` outside the staging path) names the working action: "Deploy dev on the CD page,
    or the Superagent's cd_deploy".
  - An agent's `needs_you` whose to-do (`action`) names a CD page deploy control ("Deploy <env>")
    that is disabled for the task's owner, and for every person the card names in `who`, is
    refused. The refusal gives the control's reason (a named person's when theirs is more than
    access, else the owner's) and what works for that reason (`deploy_action.reason`): an
    administrator deploys production; an environment with nothing to
    deploy needs its table; a running deploy is waited for; a drill's wait ends by itself.
    `happened` and `problem` may name a button: they say what happened.
  - The texts that send agents to `needs_you` for production name who deploys it: an
    administrator of this project, named in `who`.

## Plan

- [x] Spec, ADR-0477 and the plan of three pull requests
- [x] Person staging requests in the domain: a red task-less request waits as inherited, admission of a person's repeat
- [x] The queue deploys task-less requests as their requester and carries a red one forward to the next head
- [x] Person's ask: App::staging_ask and POST /cd/{env}/staging; the plain deploy's refusal names it
- [x] The Superagent's cd_deploy of staging waits on its panel and records the person's request
- [x] CD page: Deploy dev enabled for writers, staging waits in plain words, e2e
- [x] Docs and changes entry for pull request 1
- [x] Review round 1 of pull request 1: findings fixed test-first
- [x] Independent review of pull request 1 approved
- [x] Scheduled serving: serve_every_minutes and serve_after_merges in [cd.<name>]
- [x] The staging tick records a scheduled request when staging is behind
- [x] Docs and the independent review of pull request 2
- [x] Server-side deploy_action for each environment, rendered by the CD page
- [x] Refusal and production-gate texts name the working action; needs_you refuses a disabled control
- [x] Docs and changes entry for pull request 3
- [x] Review rounds 1 and 2 of pull request 3: findings fixed test-first
- [x] Independent review of pull request 3 approved

## Acceptance criteria

- With a green head, a person's "Deploy dev" records a request that becomes `ready` and deploys
  that head. With a red head, the request is `inherited` and the page says "Waiting for green
  tests: `<sha>` failed". When a newer green head lands, it deploys that one and includes the
  waiting request.
- With the head's gate running, the request stays `testing` and the page says "Waiting for green
  tests on `<sha>`".
- The Superagent's `cd_deploy` of staging opens a panel request. Approved with the slug, it
  records the person's staging request. Declined, nothing is recorded.
- A project whose tasks never finish their plans still gets staging moved to the green head
  within `serve_every_minutes` or `serve_after_merges`.
- No server text sends a person to a deploy control the page disables for them. `needs_you`
  refuses a card that does.

## Test plan

- **Domain unit tests** (`yulius_domain::staging`): a task-less red verdict maps to inherited;
  `pick` includes an inherited task-less request once a later revision deploys. **Config
  tests**: the `serve_*` keys parse and default, invalid values are refused.
- **Integration tests** (`crates/yulius/tests`, fake GitHub, fake process host):
  - a person's request with the head green deploys and verifies it;
  - with the head red, the request waits, the next head is requested, deploys, and the first is
    included;
  - with the head's run still running, the request stays testing;
  - the Superagent's request waits on the panel, then records the request (approve) or nothing
    (decline);
  - the timer and the merge count each record one scheduled request, never a second while one
    is pending;
  - `needs_you` refuses "press Deploy prod" for an owner who is not an administrator.
- **e2e** (`e2e/tests/cd.e2e.ts` on the mock): the merge-first card's button is enabled. It
  shows "Waiting for green tests on `<sha>`" after a press. A disabled button shows its reason
  as text.

## Decisions

- **A task-less request, not a new kind of deploy.** The queue already orders, gates, retries
  and restores requests, and the store already allows `task_id` NULL. A person's request is one
  more request, so every SPEC-575 guarantee (revision order, gate on exactly that commit, one
  deploy at a time) holds. Rejected: letting a person deploy the head directly (it would put an
  untested revision on staging and break MFS-009's promotion pin).
- **The head, not an older green revision.** The request names the verified head. If the head
  is red, it waits for the next green one (`inherited`), as the request asks. Deploying an older
  green revision at once was rejected: the person asked for "what is merged now". The schedule
  (PST-006) still moves staging forward on its own.
- **Inherited for a red task-less request.** `pick` already lets an inherited request wait
  without blocking newer ready ones and includes it once a descendant verifies. A new state
  would need a table rebuild and touch every view.
- **Defaults on.** The defaults are 60 minutes and 5 merges, so projects that never configured
  anything stop stalling. Each scheduled request costs at most one full gate run on a revision
  that has none.
- **The project's creator deploys a scheduled request.** A deploy needs a person whose write
  access is checked. The creator is fixed and known. The fallback is the newest requester in the
  queue: a person who works with staging now.
- **The server decides the deploy control.** One function says whether "Deploy <env>" is
  enabled for a given access level, and both the page and `needs_you` use it. So guidance and
  the UI cannot disagree.
- ADR: [0477](../../docs/decisions/0477-people-and-a-schedule-ask-merge-first-staging-for-the-gated-head.md).

## Evidence

Three pull requests, each merged into `dev` by `delivery_merge` on its own green Yulius CI run
after an independent review in a fresh session (Fable 5.2) approved it:

- **#1752** (PST-001 to PST-005): a person's and the Superagent's staging request. Review round 1
  asked for changes (a retry loop, deleted tasks taken for a person's, who a retry deploys as, a
  repeat after a rollback); round 2 approved `46a2d36e6`. Merged at `04ad705b0` from `693fd7bc9`;
  `dev`'s full suite on `04ad705` passed (run `4865b775`: both test shards and e2e).
- **#1754** (PST-006): the schedule. Round 1 asked for changes (an inherited request stalled it,
  a tried head was deployed again, tests did not hold `pending`); round 2 approved `d8227316d`.
  Merged at `c2090ba8b` from `5152ea3de`.
- **#1755** (PST-007): the server-side Deploy button and the `needs_you` guard. Rounds 1 and 2
  asked for changes (advice by stage instead of reason, untested wording and owners, a named
  person's reason); round 3 approved `ac7943c98`.

Tests (each new one written before its fix and failing without it; the main fixes were undone by
hand to watch their test fail, then restored):

- `scripts/cargo nextest run -p yulius-domain -p yulius-app -p yulius-contracts`: 1440 passed.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^staging::/) | test(/^cd::/) | test(/^cd_rollback::/) | test(/needs_you/) | test(/^cockpit::/) | test(/^local_ci::/) | test(/^merge_train::/) | test(/^bootstrap_upgrade::/))'`:
  all passed (380 in the widest run). The new ones in `crates/yulius/tests/staging.rs`:
  - a person's request with the head green deploys it; red, it waits as inherited, the next
    green head is asked for by itself, deploys and includes it; while the gate runs it says
    "Waiting for green tests on `<sha>`";
  - the Superagent's request waits on the panel (declined: nothing; approved with the slug: the
    person's request), and `superagent_on_panel` routes it;
  - a failed follow-up is never deployed again, a person's ask after a rollback deploys the head
    again, a deleted task's request stays a task's, a passing rerun moves a red request on, who
    may ask and who a request deploys as;
  - the schedule: after its merges, on its timer, at most every 5 minutes (hourly without a
    target), never past a request on its way but past an inherited one, never again for a tried
    head, who a scheduled deploy acts for, and its timer counted from the last request or run;
  - the `needs_you` guard: refused for a disabled button with the reason's advice (a writer and an
    administrator owner, the label with and without "…"), asked for a working one, for prose,
    for a button in `happened`, and when a named person can press it; a named person who cannot
    gives their reason.
- Hand mutations, each failing its test: the red-head override in the judge; the follow-up's
  "already tried" check; `is_asked`; the rerun re-read; `superagent_on_panel`'s staging clause;
  the schedule's throttle, `pending`, the tried-head check and the last-run operand; the hourly
  recheck; the button label's "…" trim; the named-person lookup.
- e2e: `cd.e2e.ts` and `cd-releases.e2e.ts` (30 passed), `superagent-requests.e2e.ts` (21),
  `quality`, `responsive` and `text-fits` (72).
- `scripts/dev quick` passed; `cargo clippy -D warnings` on the touched crates is clean.
- Security scan: Vidoc is switched off for this project in Yulius, so it was not run (disabled,
  not passed).

Not tested:
- A `needs_you` card when the CD page cannot be read leaves the card as before this change. Access
  cannot reach it (a person without permission still reads the page as a reader), and the store,
  Git or clone failures that would reach it break the task's own call first.
- The staging tick checking the next project after one project's GitHub read failed. The loop
  logs the error and goes on, but the fixture has one project.
- Real staging on a project's VM: the queue's deploys ran against the fixture's scripts. The
  staging request of this task's own merged delivery is its live run.
