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

## Plan

- [x] Trace both Git paths and reproduce the failures on the recorded trees
- [x] Make distinct decision entries merge in numeric order and keep conflicts visible
- [x] Prove both orders on the author and protected candidate paths
- [ ] Review and deliver the verified repository change

## Acceptance criteria

- AC-1 (IDX-001, IDX-003). Replay the exact decision additions from #1511 and #1513 onto a common index with reserved locations. In each order compare the checked-out author merge, merge preview, train step and landed candidate with the expected entry bytes and tree.
- AC-2 (IDX-002). Two descriptions for one decision, two records for one number, an omitted decision and an out-of-order pair cannot pass. Unfilled locations stay invisible in rendered Markdown.
- AC-3 (IDX-003). Existing union behavior for the changes log remains in force. Protected dev delivery tests the actual candidate. Tasks #540, #548 and #556 retain ownership of their PRs.

## Test plan

Run each historical tree's decision_errors on its own records and repeat plain merge-tree in both directions. Use disposable clones for the new author layout and exercise the production Git::merge_into and Git::build_train on both queue orders. Test same-number conflicts and slot replenishment. Run focused repository contracts, quick checks and independent review. Cite protected CI through the task delivery evidence.

## Decisions

Use shared, hidden Markdown slots allocated ahead of authors in bounded blocks. Each marker stays in the file when its entry is inserted immediately before it, giving Git unchanged context even for consecutive decisions. The existing Git implementation works with these locations in both the contained author checkout and the confined server checkout. A custom executable merge driver would require separate registration and a change to host Git confinement. Sorting in a CI checkout would leave a different tree at integration.

## Evidence

- Each tree's own decision_errors reported zero errors on b95902970, 0bd899936 and the penultimate train tree 7b6e6658c7a7d4375b83d76608be0883df39851b. It reported exactly 0401 before 0400 on 8fd11ce29 and exactly 0402 before 0401 on fa226992837b. Plain git merge-tree --write-tree exited zero in both directions. The trees were 9a746c88ae564eba3d3593f9462235c2efc9a2c8 (sorted) and 944d315d11f01bc2d436787f23cb2a84cd345d7e (reversed).
- Linked a disposable Rust replay to the production yulius-git crate. Git::build_train with squash, starting at the recorded #1510 landing 1b651312a, produced exactly the historical failing tree aef31034982c1bd6c5e0f7c5592df541a42b7a5b for the unchanged #1511 and #1513 inputs.
- For the fixed replay, added the common slot layout and checker to synthetic copies of merge base e15f4f188, the #1510 landing and the two complete PR trees. Placed each input's verbatim index entry before its comment. The two orders built trees efe8e9eae1919e3c26acbf4835fbcefe4ca4f183 (#1511 then #1513) and c9102670502fde15e5e974f4350a5a3e61c3695b (#1513 then #1511). Each scratch checkout passed its decision_errors. Both contained the same README blob b29aac8eae1b25c1b69874b765fdf522c81b7299, also obtained by plain merge-tree in either order. Relative to fa226992837b, the forward replay changed only this index and the two checker files.
- For each order, checked the first squash tree and merged it into the checked-out second head using Git::merge_into. Its tree, Git::merge_tree's preview, the final TrainStep::Clean tree and the final squash tree were identical. Distinct descriptions for the same 0401 record survived union on tree 121ec0dc7686df15b101b501290ee1f638b894b3 and the checker rejected the duplicate link.
- The ordering regression and wrong-placement check failed on the legacy layout. The four decision-slot tests and 43 repository-checker tests pass. The slot tests also cover concurrent renewal at both ends of a block and damaged empty markers. scripts/dev quick passed.
- A CommonMark render of the index with adjacent slots filled exposed all 398 decision links in source order.
