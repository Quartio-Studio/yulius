# Review report output format

Use one decision-oriented review as the authoritative finding record. Keep
`Verdict`, **approve / request changes**, and the severity headings
**Blocker / Major / Minor / Nit** explicit so the author can act on the result
mechanically.
Aim for 150–300 words before the validation table; expand for actionable
findings and evidence, never to fill a section. A short review still runs the
full checklist.

```markdown
# Code Review: {PR title or change description}

## Verdict
{approve | request changes} — {the concrete reason and next action}

## Summary
{Who can do what after this change, or which failure it fixes; one or two
sentences. Name the main area and any shared surface that changes the decision.}

## Direction and scope
{Only when a product/architecture choice needs attention: the choice, current
consumer or cited plan, durable commitment, and recommended action. Distinguish
an observed fact from an inference or something not checked.}

## Findings

### Blocker
- `{file:line}` ({own code | pre-existing}) — {trigger → wrong behavior or broken rule → concrete fix}.
  {Evidence or repo rule link when needed to establish the claim.}

### Major
- `{file:line}` ({own code | pre-existing}) — {trigger → consequence → concrete fix}.

### Minor
- `{file:line}` ({own code | pre-existing}) — {specific improvement and its benefit}.

### Nit
- `{file:line}` ({own code | pre-existing}) — {optional suggestion and its reason}.

## Validation Gate

| Command | Status | Evidence or limitation |
|---------|--------|------------------------|
| {each configured command, in order} | PASS / FAIL / NOT RUN | {failure, useful evidence link, or why not run} |

## Breaking Changes
{Only touched contracts with a material consequence: what becomes a dependency,
who consumes it, and any migration/deprecation path or uncovered risk.}

## Test Coverage
{Changed behavior proved by the tests; material gaps with exact cases and files.
Link evidence; do not repeat a missing-test finding already listed above.}
```

- Mark each finding own code (code or tests the change added or modified) or
  pre-existing (on the integration branch before it, with the evidence). An
  own-code finding is fixed in the pull request, never deferred to an issue.
- Keep Verdict, Summary, and Validation Gate. Omit empty findings, optional
  sections, passing checklists, and routine praise. Per-command results remain
  mandatory; NOT RUN is a limitation, never evidence of a pass. Any unexecuted
  configured command keeps the verdict at request changes until validation runs.
- Separate direction/scope choices from verified defects. A planned consumer
  or an unavailable roadmap is a decision dependency, not automatically a bug.
  Report a repository-rule violation only with the applicable rule and code
  evidence; preference alone does not establish a major or blocker.
- Check current consumers before claiming there are none. Name the searched
  symbols, paths, revision, and relevant exclusions; say “not found in this
  search” rather than claiming absence across an ecosystem you did not inspect.
- State whether consequential claims were observed, inferred, or not checked
  where the distinction matters. Do not infer runtime behavior solely from a
  screenshot or claim a broad impact from line count alone.
- Add a small Mermaid diagram only when connections explain the decision more
  clearly than prose. Label nodes existing/new/planned, label each relationship,
  and give a one-sentence takeaway. Do not duplicate the same inventory in prose.
- Re-reviews lead with what changed since the prior review. Link resolved
  findings and retain every unresolved actionable finding with its severity.
  Keep earlier findings' source link and disposition without repeating the same
  issue in several sections.
- When the review is posted on the pull request, the reviewer's final reply links
  it and gives verdict, validation limit and next action in 3-6 lines. For a local
  review, return the report itself. Do not paste a posted report a second time.
- Start a posted review with the reviewed head commit (`Reviewed head: <sha>`), so
  a later push is visibly not covered by it.
- Never include secrets, credentials, or tokens, including in quoted code.
