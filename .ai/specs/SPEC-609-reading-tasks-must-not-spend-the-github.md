---
yulius-task: 70138e35-5fd4-4a6e-992f-34354142a209
status: in-progress
---

# SPEC-609 Reading tasks within the GitHub request budget

## Goal

Keep task polling affordable and show operators which work consumed GitHub requests during a shortage.

## Context

On 21 November the instance reported exhaustion at 14:18:07 and 22:13:06 UTC, with 579 and 904 seconds until retry. The staging warning identifies a refused caller. The incident report names 15 parked tasks with merged pull requests across two projects and a script that read two lists and approximately 20 deliveries every two minutes. The existing transport retains neither a successful-request journal nor hourly counters. The task sandbox has no host journal. The historical distribution must therefore distinguish measured polling costs, extrapolation and unattributed traffic. Task #592 owns task presentation on refusal.

### Baseline before product changes

At revision 47e0d70e2 the two consecutive task-list reads of a parked merged fixture made 0 and 1 branch-list calls within their display deadline. Both delivery reads made one numbered PR call and zero checks calls. Three concurrent authenticated open-list readers made three HTTP GETs on the fake server. The repeated delivery and concurrent-reader assertions failed on those counts.

At 30 polls/hour, 15 merged parked rows at the warmed fixture rate of one branch-list request per row give 450 branch lists. Twenty merged delivery reads per cycle give 600 numbered GETs. This models 1,050 PR requests (21% of 5,000) in a full hour, assuming one page per listing. A delivery using the caller's user token spends that user's quota. The installation share depends on the actual credential. Open PRs add checks, UI poll counts are unknown, and the observed 15 tasks are a snapshot at 22:13. The warnings and this replay cannot attribute the remaining traffic to staging, CI, merges, imports, other readers or external clients. The first attempt to read organization API Insights for the earlier window returned HTTP 403 because the installation itself was rate limited. After reset, the same installation received HTTP 403 “Resource not accessible by integration” for the 13:27:46–14:27:46 UTC window. The organization owner reports HTTP 404 for both route-stat windows and no outside counts are available. They report approximately 13 merges, an open web UI, and three extra bulk passes of roughly 55, 18 and 60 delivery reads before 22:13. If all those deliveries were merged, the bulk passes add 133 GETs, bringing the estimated script total to 1,183 (23.7% of 5,000). Even if every modeled request used the installation, that scenario leaves about 3,817 calls unexplained. Each merge involves head and check reads, the merge and confirmation; CI status publication, staging reconciliation, general server traffic, other clients and UI polls also use GitHub. The log gives neither their successful-request counts nor credentials. The earlier hour has only the exhaustion and retry evidence. The script has since stopped.

These figures depend on the paths that actually completed. The old parked verifier can re-list a branch after checking it. Two completed one-page branch lists per parked row would raise the modeled script total from 1,183 to 1,633. The web sidebar's visible project watch refreshes its parked-task list and Git overview once per minute, plus event-driven reads paced at three seconds per source. For a single tab showing eight parked merged tasks for a full hour, one branch list per row implies another 480 requests; two imply 960. An overview adds an open-PR listing and, for readable checks, a check-runs and combined-status read per open head. The staging tick is once per minute per eligible queue and reads the integration ref even to evaluate its target. Open deliveries are reconciled every three minutes. Hours spent open, active queue counts, open-head lifetimes, retries and actual UI events are unknown. These rates explain how the recorded operations could add up without identifying a dominant caller. [Confirmed regression, issue #1650](https://github.com/Quartio-Studio/yulius/issues/1650) [GitHub organization API Insights](https://docs.github.com/en/rest/orgs/api-insights#get-route-stats-by-actor)

## Requirements

- GB-001 Measure list and delivery requests on the existing implementation before changing them. Report the workload assumptions and what can be attributed.
- GB-002 Serve observed terminal pull requests locally. Coalesce open display refreshes and use the response ETag on expiry. Scope cached evidence to its repository and access rights. Keep fresh action checks.
- GB-003 Attribute actual HTTP attempts to display, CI statuses, merges, staging, imports and other. Serve counters and GitHub's remaining quota with reset time in the Status panel and instance tool.
- GB-004 Suspend display refreshes when the remaining quota reaches its reserve. Report the pause, allow operational requests, and resume display refreshes when the window resets.

## Plan

- [x] Measure the existing polling paths and document the incident evidence
- [ ] Share conditional display reads and retain terminal pull requests
- [x] Attribute hourly requests and reserve capacity for delivery
- [ ] Serve budget evidence in instance status and the Status panel
- [ ] Verify the behavior, obtain independent review and deliver to dev

## Acceptance criteria

Repeated list and delivery reads of a known merged or closed-unmerged PR make zero GitHub requests about that number. A replacement PR is tracked independently. Concurrent readers share the open refresh and revalidate with its ETag. The Status panel and instance tool report observed hourly counts and remaining capacity. At the reserve the pause is visible, merges, CI status writes and staging can proceed, and the next window admits display reads. Count retries and 304 responses as HTTP attempts. GitHub supplies the actual remaining quota.

## Test plan

Count calls at list and delivery entry points on a synthetic parked task. Exercise concurrent readers, conditional responses, distinct credentials, low remaining capacity and reset at the transport. Verify delivery actions and status serialization through existing interfaces.

## Decisions

[Decision 0447](../../docs/decisions/0447-task-display-shares-github-reads-and-yields-to-delivery.md) describes the reserve and cache lifetime. Use the same transport to observe response headers and count attempts, including retries. Reading the counters itself must cost zero GitHub requests.


## Evidence

The fake GitHub transport suite exercised concurrent readers of an open PR, credential isolation, ETag revalidation, physical retries, all six purposes, quota-window rollover and the reserve. The two parked-task regressions saw zero branch-list and zero numbered-PR calls on consecutive list and delivery reads. The counter readings are process-local, with a separate lower bound for GitHub-charged requests outside the observed attempts.
