---
yulius-task: 84ea1903-0e8e-4141-8d50-e4d01d283b8e
status: in-progress
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
    deploying, or for a revision staging already serves. Nothing is carried to a head that has a
    request of a person or the schedule in any state, so a failed follow-up never loops.
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
    it off) and `serve_after_merges` (default 5, `0` turns it off).
  - Once a minute (at most every 5 minutes per project), Yulius checks each project with a
    merge-first target. When the integration head is ahead of what staging serves and no
    request is pending, it records a scheduled task-less request for the head (`yulius:schedule`,
    note naming the trigger). The triggers:
    - at least `serve_after_merges` first-parent commits since staging's revision;
    - at least `serve_every_minutes` since the environment's last staging request or run.
  - A task's own request keeps its rules ("a task's whole delivery once", one successful deploy
    per task) for its own verification bookkeeping. It never gates staging any more.
- **PST-007 Guidance names a working action.**
  - The server decides each environment's deploy control for the viewer
    (`CdEnvironment.deploy_action`: label, enabled, why). The CD page renders it as it is.
  - The production gate's "deploy dev at this commit first" names the merge-first wording
    ("ask staging for the newest green revision: Deploy dev on the CD page") only where that
    control is enabled for a person with write access.
  - A plain head deploy refused on the merge-first target (an automation's, a task's
    `cd_deploy` outside the staging path) names the working action: "Deploy dev on the CD page,
    or the Superagent's cd_deploy".
  - An agent's `needs_you` naming a CD page deploy control ("Deploy <env>") that is disabled for
    the task's owner is refused. The refusal gives the control's reason and says what works (a
    dev-stage environment: the agent's own `cd_deploy`; production: a person who may deploy it).

## Plan

- [x] Spec, ADR-0477 and the plan of three pull requests
- [x] Person staging requests in the domain: a red task-less request waits as inherited, admission of a person's repeat
- [x] The queue deploys task-less requests as their requester and carries a red one forward to the next head
- [x] Person's ask: App::staging_ask and POST /cd/{env}/staging; the plain deploy's refusal names it
- [x] The Superagent's cd_deploy of staging waits on its panel and records the person's request
- [x] CD page: Deploy dev enabled for writers, staging waits in plain words, e2e
- [ ] Docs, changes entry and the independent review of pull request 1
- [ ] Scheduled serving: serve_every_minutes and serve_after_merges in [cd.<name>]
- [ ] The staging tick records a scheduled request when staging is behind
- [ ] Docs and the independent review of pull request 2
- [ ] Server-side deploy_action for each environment, rendered by the CD page
- [ ] Refusal and production-gate texts name the working action; needs_you refuses a disabled control
- [ ] Docs and the independent review of pull request 3

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
