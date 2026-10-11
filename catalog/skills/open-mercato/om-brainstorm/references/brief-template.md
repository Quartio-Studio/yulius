# Brief template

The brief `om-brainstorm` writes in workflow step 6 (conclusions 2-4), read cold by whoever takes the next step. Keep the Problem, Agreed direction, Resolved unknowns and Non-goals headings. Keep answers concrete and remove repetition; omit optional affected-area detail when unknown. A necessary empty section gets one short statement, not a placeholder table.

```markdown
# {one-line goal}

- Date: {YYYY-MM-DD}
- Category: {feature | bug | refactor | security | dependencies | documentation}
- Priority signal: {low | medium | high | extreme} — {one-line why}
- Risk signal: {low | medium | high} — {one-line why}
- Next step: {conclusion number and what it starts}

## Problem

{2-5 sentences, sharpened; evidence it matters}

## Agreed direction

{what to pursue — and what was explicitly rejected, including why "build nothing" lost}

## Resolved unknowns

| Question | Answer (and its source, or "reversible default") |
|----------|--------------------------------|
| {…} | {…} |

## Non-goals

- {explicit exclusions, so nobody gold-plates}

## Affected areas (if known)

- {only what the sources established, never guessed}
```

Notes:

- **Priority and risk signals** become the issue's labels on conclusion 2; otherwise they are
  context for whoever implements it.
- **Resolved unknowns** is the load-bearing section: it pre-answers the spec's Open Questions.
  Mark each answer with its source, or as a reversible default that a later task may revisit.
- **Affected areas** stays honest: only what was established. The next step re-derives the rest
  from the code.
