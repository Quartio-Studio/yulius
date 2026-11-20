---
yulius-task: 389d1a3c-436b-4cb5-97fd-d753c19c6d06
status: in-progress
---

# SPEC-568. Preserve ADR index order across parallel delivery

## Goal

A reader of Yulius's decision index gets one intact navigable entry per ADR in numeric order after independent PRs integrate in either order. This is repository infrastructure outside the effort comparison.

## Context

Issue #1303 and PR #1488 established the order check after 0332 landed before 0331. Commit 8fd11ce29 failed it for 0401 before 0400. Both corrected inputs b95902970 and 0bd899936 pass. The three-member train in run d02da883-f50f-46d6-92dd-ddbff619cdf8 passed that check at its penultimate tree and failed at fa226992837b with 0402 before 0401.

Both the committed attributes and Yulius's higher-precedence managed info/attributes select Git's union driver. A manual integration update and Git::merge_into merge dev into the checked-out PR. Git::build_train checks out each PR and uses the same merge_as operation with the accumulated integration commit. Its squash has the resulting tree and the accumulated commit as parent. A union at one append point orders the incoming PR's lines first.

## Requirements

- **IDX-001** Distinct reserved numbers have independent edit locations in the index before integration. Markdown retains the authors' exact links, numbers and descriptions, including descriptions that differ from record titles.
- **IDX-002** Wrong order, duplicate numbers, missing entries and conflicting descriptions remain observable. A check must reject an author edit that would reintroduce the shared append point.
- **IDX-003** Both orientations of plain Git and the protected candidate builder preserve the expected index and the tree used by delivery. Other Markdown and other projects keep their merge behavior.

- **IDX-004** The protected decision verdict depends only on checked-out tree content. Authoring checks that consult branch ancestry run separately from every tree-reusable CI gate.

## Plan

- [x] Trace both Git paths and reproduce the failures on the recorded trees
- [x] Make distinct decision entries merge in numeric order and keep conflicts visible
- [x] Prove both orders on the author and protected candidate paths
- [x] Review and deliver the verified repository change
- [x] Prove tree-reusable CI verdicts and validate shared slots on the author path
- [ ] Verify and reconcile pre-migration ADRs on the landed dev prefix

## Acceptance criteria

- AC-1 (IDX-001, IDX-003). Replay the exact decision additions from #1511 and #1513 onto a common index with reserved locations. In each order compare the checked-out author merge, merge preview, train step and landed candidate with the expected entry bytes and tree.
- AC-2 (IDX-002). Two descriptions for one decision, two records for one number, an omitted decision and an out-of-order pair cannot pass. Unfilled locations stay invisible in rendered Markdown.
- AC-3 (IDX-003). Existing union behavior for the changes log remains in force. Protected dev delivery tests the actual candidate. Tasks #540, #548 and #556 retain ownership of their PRs.

- AC-4 (IDX-004). Two commits with the same complete tree, different parents and a fixed origin/dev receive the same protected result. The author command detects when a branch first used a slot absent from its common base.

## Test plan

Run each historical tree's decision_errors on its own records and repeat plain merge-tree in both directions. Use disposable clones for the new author layout and exercise the production Git::merge_into and Git::build_train on both queue orders. Test same-number conflicts and slot replenishment. Run focused repository contracts, quick checks and independent review. Cite protected CI through the task delivery evidence.

## Decisions

Use shared, hidden Markdown slots allocated ahead of authors in bounded blocks. Each marker stays in the file when its entry is inserted immediately before it, giving Git unchanged context even for consecutive decisions. The existing Git implementation works with these locations in both the contained author checkout and the confined server checkout. A custom executable merge driver would require separate registration and a change to host Git confinement. Sorting in a CI checkout would leave a different tree at integration.

## Evidence

- Each tree's own decision_errors reported zero errors on b95902970, 0bd899936 and the penultimate train tree 7b6e6658c7a7d4375b83d76608be0883df39851b. It reported exactly 0401 before 0400 on 8fd11ce29 and exactly 0402 before 0401 on fa226992837b. Plain git merge-tree --write-tree exited zero in both directions. The trees were 9a746c88ae564eba3d3593f9462235c2efc9a2c8 (sorted) and 944d315d11f01bc2d436787f23cb2a84cd345d7e (reversed).
- Linked a disposable Rust replay to the production yulius-git crate. Git::build_train with squash, starting at the recorded #1510 landing 1b651312a, produced exactly the historical failing tree aef31034982c1bd6c5e0f7c5592df541a42b7a5b for the unchanged #1511 and #1513 inputs.
- For the fixed replay, added the common slot layout and checker to synthetic copies of merge base e15f4f188, the #1510 landing and the two complete PR trees. Placed each input's verbatim index entry before its comment. The two orders built trees af5faf13253118d73a63f973914bcca34c24b939 (#1511 then #1513) and 0a57b6f98f24867f5b0df8cc0c397ba95f843e2c (#1513 then #1511). Each scratch checkout passed its decision_errors. Both contained the same README blob dbe90e0c8d2546bd951fb2c1fd2f6340b7c9c758, also obtained by plain merge-tree in either order. Relative to fa226992837b, the forward replay changed only this index and the two checker files.
- For each order, checked the first squash tree and merged it into the checked-out second head using Git::merge_into. Its tree, Git::merge_tree's preview, the final TrainStep::Clean tree and the final squash tree were identical. Distinct descriptions for the same 0401 record survived union on tree 53162a4f684516da2d55df43f606ecfa972c19ab and the checker rejected the duplicate link.
- The ordering regression and wrong-placement check failed on the legacy layout. The seven decision-slot tests and 43 repository-checker tests pass. The slot tests also cover concurrent renewal at both ends of a block and damaged empty markers. scripts/dev quick passed.
- A CommonMark render of the index with adjacent slots filled exposed all 398 decision links in source order.
- Independent round one identified unshared new slots, duplicate displayed numbers with different existing targets, and extension without a final line feed. Regressions reproduced all three. The common-base test builds two divergent Git histories for 0465 and 0466 and checks each before and after the shared empty extension. The duplicate-number and unterminated-line fixtures now pass. The final replay used the merge-base-aware repository decision check.
- scripts/dev mutants reported no changed Rust code to mutate. scripts/dev security reported Vidoc disabled.
- Independent round two approved e083e3b55 after checking all three fixes, running all 50 focused tests, repository contracts and both neighboring merge orientations against an unterminated base. The task delivery record supplies the protected run and landed SHA.
- Rollout train cc180a59-6b69-42b5-abcd-95051f326b91 (commit c1864c3fe, tree a718639ae967865193d7d9cf4b1fc63048f0ad77) combined the legacy #1511 row at line 478 with the 0401 marker at line 413. Its checker reported wrong placement and 0403 before 0401. Integration base a986928a8 contained neither 0401 nor 0402.
- The first cache regression ran the normal repository checker twice with one complete tree, different parents, and a fixed origin/dev. At f50a2d4ab it exited 1 then 0. After moving ancestry validation to the documented author command, the protected checker passed both commits (852bca4c and 999251da, identical tree 3526a8d67e05cc008eef4d98f2b9d46af9e177da). The author CLI rejected first use of unshared 0465 and 0466 and accepted each after it had a shared insertion point.
- Replayed the revised checker and layout with the same immutable heads. Both production train orders matched their author update and squash, at 492e6afccf9fa1a82770f3df6661e2a1f0828483 and 9002b688ab143bb3d042d4b5f7cf2fd76717f318. Both contained index blob fdba027b2576233eef86e22fa4f09f5d94a722d7. The competing-description tree dd9816a6ccadda42785906bcf71109157f80a0de failed the content-only decision check.
