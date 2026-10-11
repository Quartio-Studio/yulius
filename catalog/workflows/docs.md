---
name: Docs
description: Documentation brought in line with the code and with itself - product, architecture, decisions, guides. No behavior change, no spec.
when_to_use: Only documentation changes - product summary, architecture and diagrams, decision records, guides, READMEs, AGENTS.md - and behavior stays the same.
spec: optional
tests: optional
order: 40
---

## Steps

1. Read the documents and the code they describe
2. Update the documents to match the code and each other
3. Check links, diagrams and the docs index

## Instructions

Documentation is how the next agent learns the product, so it must be true and findable.

- Read the docs index first (AGENTS.md, product, architecture, decisions, specs, changes) and the
  code the change describes; when unsure how something works, check the code and
  `sessions_search` rather than guessing.
- Change one concern; keep each fact in one place and link to it instead of repeating it. Update
  Mermaid diagrams with the text. Superseded decisions keep their record and link to the new one.
- Check that every link and path resolves and that AGENTS.md still routes to every document.
- If you find that the code is wrong rather than the docs, record it with `issue_create` and leave
  the code alone. Commit and deliver following `git-hygiene` and `delivery-pipeline` (no spec,
  no independent review needed for documentation only).
