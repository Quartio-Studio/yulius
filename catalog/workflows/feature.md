---
name: Feature
description: New or changed behaviour, from a spec through review and tests to production.
when_to_use: The task adds or changes behaviour that users or other code rely on.
spec: required
tests: required
order: 10
---

## Steps

1. Write the spec
2. Implement
3. Review the change
4. Test
5. Deliver to the integration branch
6. Release to production

## Instructions

Start with the task spec: goal, context, requirements, a plan checklist and acceptance
criteria, in the project's spec directory. Keep the plan in sync with the steps. Implement
only what the spec describes, prove it with tests, review your own diff, then deliver through
the project's delivery profile and keep the project's docs current.
