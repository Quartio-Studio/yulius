---
name: Research / Question
description: An answer with evidence - a question about the product, an investigation, a comparison or an estimate. No spec, no code changes.
when_to_use: The task asks a question about the product, the code, the data or the market, or wants an investigation, comparison, estimate or plan, and changes no code.
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
- Change no code. When the answer reveals work, record it as an issue (`issue_create`, with type,
  priority and area labels) or tell people which Feature or Fix task to start. When a finding
  should outlive the session, propose a Docs task for it.
