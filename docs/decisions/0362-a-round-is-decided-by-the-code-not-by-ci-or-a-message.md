# 0362. A round is decided by the code, not by CI or a message

## Status

Accepted (2026-11-17). Specified in
[SPEC-506](../../.ai/specs/SPEC-506-as-fast-as-possible-from-start-to-merge.md) (FST-001 to
FST-003); task #488. Builds on [ADR-0342](0342-yulius-ci-runs-the-whole-gate-while-the-review-reads.md)
(CI runs while the review reads) and [ADR-0345](0345-an-approval-carries-across-a-follow-up-of-at-most-20-product-lines.md)
(an approval carries across 20 product lines). [ADR-0141](0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md)
stays: findings in the change's own code are fixed before the merge. Amends
[ADR-0347](0347-mutation-testing-runs-once-per-pull-request.md): beside the whole change's run,
the lines of each later round's fix are mutation-tested where the project's command can take
them (SPEC-506 pull request 2 rewords ADR-0347's own sentence). On 2026-11-21,
[ADR-0439](0439-delivery-checks-mutations-by-hand-and-builds-on-disk.md) superseded the clause requiring a campaign between review rounds. Authors still prove each fix and check every finding against the new head.

## Context

The person's measurements of 15 to 17 Nov 2026 (task #488's request): 3.2 rounds per pull
request, one more round a median 15 and a mean 23 minutes, 20 % of rounds re-approving an
approved change with ADR-0345 in force, and 81 re-reviews in which a new reviewer asked again
after a request for changes. Of the 58 of those that state a reason, 3 named only a failed or
unfinished CI run, 16 more named one beside their finding, 15 a fix with no test that can fail and
11 an earlier finding only partly fixed.

Three things in the product sent work to a new round that the round could not change:

- Six messages Yulius types into a session (a red run, two kinds of failed merge candidate, an
  armed merge dropped by a red run or by a new commit, and `yulius wait` on failed checks) told the
  author to "get the review of the new head" after the fix, though `delivery_merge` keeps the
  approval across 20 product lines. These are the moments right after an approval at which a small
  fix is made.
- `om-code-review` made "a failed job … a blocker, even if it looks unrelated" and kept the verdict
  at request changes while a command had not run, though `delivery_merge` merges only a head
  whose own Yulius CI run passed.
- Nothing asked the author to prove a fix before asking again, so the next reviewer found the fix
  untested or partial.

## Decision

- **The messages keep the approval.** Each of the six, and the four notices of a green head with
  no merge armed (pull request event, check-in, a project without CI, `yulius wait`), says one
  sentence (`yulius_domain::review::KEEP_APPROVAL`): if a review approved this head or an earlier
  one of this pull request, call `delivery_merge` with that approved head as `reviewed_head`; the
  approval carries across at most 20 product lines in all since it (tests and documents do not
  count), and the call says when the change needs a new round; with no approval yet, the review
  of the new head comes first. Both cases are said because a red run's notice also reaches an
  author with no approval, and `delivery_merge` takes `reviewed_head` on the author's word. The two messages of
  a conflict still ask for the review of the new head: a merge that resolved a conflict needs one
  (ADR-0345). A test reads the four sources and refuses the old wording outside a conflict's
  message.
- **CI alone is no request for changes.** "A reviewer does not ask for changes because of CI
  alone: `delivery_merge` merges only a head whose own Yulius CI run passed. When the review finds
  nothing to fix and the head's run is still going, or failed in a way the change does not cause,
  the verdict is approve and the report says what CI still has to do. A failure the change causes
  (its own test, format or lint, or a contract its files break) is a blocker." Where CI cannot
  run, an unexecuted gate command still keeps the verdict at request changes. Stated in
  `om-code-review` (and its checklist and output format), `delivery-pipeline`, `yulius-review`,
  `workflow.md` and, in short, the reviewer agents' prompt (that prompt now leaves it to
  `om-code-review`: [ADR-0364](0364-a-server-start-refreshes-every-sandboxs-skills-and-the-reviewers-prompt-is-a-contract.md)).
- **The author proves each fix.** "Before asking for another round, answer each finding of the last
  report with the test that fails without its fix (or why no test can), check every finding of
  that report against the new head, and run the project's mutation command on the lines of the fix
  where it can take them." Here that is `scripts/dev mutants --base <reviewed head>` (the lines
  since that head; a merge of `dev` since then brings its lines in too). Stated in `om-fix`,
  `om-code-review`, `delivery-pipeline`, `yulius-review` and `workflow.md`.
- Tests in `crates/yulius-app/src/skills.rs` hold every place to each sentence. Since
  2026-11-26 (audit L10 F2) each is stated once, in `om-code-review`, and the tests refuse a copy
  elsewhere.

## Alternatives

- **Let the messages say nothing about the review.** The author would still read "a failed run
  drops an armed merge" as needing a new approval; naming the call and the bound is what the
  count needs.
- **Count a conflict's resolution under the 20 lines.** Riskier than a nit (a semantic conflict
  passes Git's merge), and the share of re-approvals after a conflict is not known; left to the
  person's numbers.
- **Have `delivery_merge` refuse a CI-only request for changes.** It cannot read a report's
  reasons; the rule belongs where the verdict is written.
- **Require a mutation run on every fix.** A project without a command for a fix's lines would
  have to write one; "where it can take them" keeps the rule universal.

## Consequences

- A fix to make CI green after an approval, within 20 product lines, merges on its green run with
  no round; above that, `delivery_merge` refuses and names the count, as before.
- A reviewer that found nothing approves a head whose run is still going; nothing merges until it
  passes. A failure of the change found after that approval is fixed under the same 20-line bound
  (ADR-0345 measured 5 % of re-reviews after such a fix asking again, 23 of 457).
- A week later: the Review card's re-approval rounds (SPEC-506 FST-007) and `SELECT detail FROM
  audit_log WHERE action IN ('delivery.agent.merge', 'delivery.agent.merge_armed') AND detail LIKE
  '%since then%product line%'` for merges that used the bound; verdicts that approve with CI still
  to pass are found by "CI still has to" in the rounds' reports.
