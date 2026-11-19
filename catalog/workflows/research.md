---
name: Research / Question
description: An answer with evidence - a question about the product, an investigation, a comparison or an estimate. No spec; a defect it confirms is fixed as a Fix.
when_to_use: The task asks a question about the product, the code, the data or the market, or wants an investigation, comparison, estimate or plan, reports nothing wrong in the product (a fault asked as a question is a Fix) and changes no code.
spec: optional
tests: optional
order: 30
---

## Steps

1. Read the product context, docs and earlier sessions on the topic
2. Investigate the code, data or sources and collect evidence
3. Answer with the evidence, the options and a recommendation

## Instructions

The deliverable is an answer, not a change.

- Start from what the project already knows: the docs index, decision records, earlier specs,
  `tasks_list`, and `sessions_search` / `session_read` over other tasks' transcripts and imported
  sessions. Then read the code or the sources.
- Separate facts you verified from inferences and from what you could not check; cite files,
  lines, commands and links.
- Answer in the session, short and decision-oriented: the conclusion first, then the evidence,
  the options with their trade-offs, and your recommendation.
- A defect is fixed, not filed: when the request reports something wrong in the product, even as
  a question or a screenshot, or you confirm a defect in what it asks about, fix it in this task:
  add the Fix workflow's steps (`workflows_list`) to your plan; `om-root-cause`, then `om-fix`.
  When the fix is clearly outside the request, or you cannot tell whether the person wants an
  answer or a fix, ask with `needs_you` (decision) and answer meanwhile; never file it as an issue
  and stop.
- Otherwise change no code. Other work the answer reveals (an idea, a second, unrelated defect)
  can become an issue (`issue_create`) or a suggested Feature or Fix task. Name every issue you
  opened in your answer. When a finding should outlive the session, propose a Docs task for it.
