# 0345. An approval carries across a follow-up of at most 20 product lines

## Status

Accepted (2026-11-15). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (APR-001 to
APR-005, fix 6); task #472. Amends point 3 of
[ADR-0066](0066-parallel-tasks-stop-colliding-at-merge.md) ("a review carries over Git's own
merges of the integration branch … anything else needs a new review"). [ADR-0141](0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md)
stays: findings in the change's own code are fixed before the merge.

## Context

An approval listed minors and nits; the author fixed them, and the new head needed another full
round before `delivery_merge` took it. Between the last approval and the merge went 11 % of a
pull request's hours here and 29 % in quartio-v2 (pull requests merged 12 and 13 Nov 2026).
Where more than 100 product lines changed since the reviewed commit, 20 of 63 re-reviews asked
for changes, two thirds for a real defect: a large follow-up does need its round, a nit fix of a
few lines rarely finds anything.

## Decision

- **`delivery_merge` keeps an approval across at most 20 product lines** after the reviewed
  commit, beside Git's own clean merges of the integration branch. It walks the first parents
  from the head to the reviewed commit: a merge must be a clean merge of the integration branch
  (its second parent on that branch, its tree what Git's merge gives); every other commit's
  lines are counted, added plus removed, commit by commit, so a line changed twice counts twice
  and a large change undone later is not nothing. They are read with plumbing (`git diff-tree
  -p -U0 --text`, checked against `--numstat -z`), so the clone's configuration (coloured or
  prefix-less diffs) and attributes (`-diff`) change nothing; a hunk takes exactly the lines its
  header counts. Commits are read as their objects say: no replace ref, graft or
  commit-graph file of the clone counts (every Git command of Yulius runs so). The integration
  branch is the commit its origin names now (GitHub's branch, a playground's own repository),
  read before the clone is fetched; no ref of the clone, which the agents can write even right
  after a fetch, is compared.
- **What counts**: every file but documentation, tests (a `tests/`, `test/`, `e2e/`,
  `__tests__/` or `fixtures/` folder; `*_test.*`, `test_*.*`, `*.test.*`, `*.spec.*`,
  `*.e2e.*`) and the lines of a Rust file's `#[cfg(test)] mod name {` to its closing brace,
  read as Rust tokens so that comments and strings neither open nor close one, only when
  `#[cfg(test)]` is the item's first attribute and every enclosing delimiter is the brace of a
  module without an attribute macro (a `mod tests;` declaration, a single test-only item, or a
  module in a macro's input counts). A test or document file the follow-up changed counts when
  a product line it added names it (its folder and name, as `#[path]`, `include!` or an import
  write it); an added Rust product line with `#[path]`, `include!`, `include_str!` or
  `include_bytes!` needs a review. A project sets its own in
  `yulius.toml`: `[review] product = [globs]` (only these count, tests and documents still not),
  `not_product = [globs]` (never these). The integration branch's table counts, so a pull
  request does not loosen its own count; a table that does not parse refuses.
- **A new review is needed** for more than 20 lines, for a file of the reviewed change that left
  the pull request's change, for a merge that resolved a conflict, merged another branch or
  changed more than Git's merge, when the reviewed commit is not on the head's first-parent
  way, when the integration branch could not be read or fetched, and when a product file's
  changed line has a NUL byte or is over 200 bytes (a binary file, a minified line). The refusal names the count and the files, or the reason.
- **The record says what was counted**: the tool's answer (`review`), the merge's and the armed
  merge's audit entries and the review summary say "reviewed at X; since then N product lines
  in M files: a 12, b 3"; the merge's review record (`delivery.review.models`) counts such a
  head as approved.
- **One rule, everywhere reviews and merges are described**: "After an approval, fix the
  remaining nits within 20 product lines and merge without another round: `delivery_merge` with
  the approved head counts them (tests and documents do not count). More than that, or a file
  leaving the change, needs a new round." A test holds every place to it. Since 2026-11-26
  (audit L10 F2) `delivery_merge`'s messages and one line in `delivery-pipeline` say it, and the
  test refuses a copy elsewhere.

## Alternatives

- **The total diff from the reviewed commit to the head.** A large change undone by a second
  commit would count as nothing, though the reviewer saw neither.
- **A bound in changed files, or a share of the change.** Lines are what the measurement and the
  person's decision name; a share lets a large pull request carry a large unreviewed follow-up.
- **The same after a request for changes.** Not built: the decision holds after an approval, and
  the measurement does not say how many such re-reviews blocked.
- **Trust the agent to say what it changed.** The count is the tool's, not the author's claim.

## Consequences

- A nit fix after an approval merges on the head's green Yulius CI without another round; the
  audit entry shows what went in unreviewed.
- A reviewer's minors in product code are fixed within 20 lines or need a round; tests and
  documents can grow freely after an approval as long as no added product line has a loading
  word (below), which makes them count.
- What the count catches: the clone's settings, attributes, replace refs and grafts change
  nothing; the integration branch is origin's commit; a fake test module (in a comment, a
  string, a macro's input, a module an attribute macro rewrites) counts; a Rust file under
  `src/` is product code whatever its folder; a test or document file the follow-up changed
  counts when a new product line names it, and every one does when a new product line has a
  loading word (a word containing `import`, `require`, `include` or `concat` in any case,
  `export … from`, `#[path]`, `mod name;`, a manifest's `path =` or `file:`), whether or not it
  loads anything (`arr.concat()` counts too: when in doubt, a line counts); a
  new Rust `#[path]` or `include!` refuses.
- What it does not catch, by the person's rule ("tests and documents do not count"): a test or
  document file that product code already built in before the review (an `include_str!` of a
  document, a fixture a product module imports) changes behaviour when it changes, uncounted.
  The review that approved the inclusion saw it; a project that ships such files keeps them out
  of test folders and gives them a name that is not a document's (`[review]` can narrow what
  counts, never widen it to tests and documents). Nor does a line count say what one line
  does: deliberate concealment by the author is the review's and the conduct rules' to find,
  not a count's (SPEC-493 fix 6, five review rounds).
- A week later: `SELECT detail FROM audit_log WHERE action IN ('delivery.agent.merge',
  'delivery.agent.merge_armed') AND detail LIKE '%since then%product line%'` lists the merges
  that used the bound, with their counts; the analytics read's rounds per pull request (fix 1)
  shows whether rounds fell.
