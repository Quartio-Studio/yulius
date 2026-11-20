---
yulius-task: 96665b1d-0ed8-4442-9ddd-54d00e72a8d7
status: done
---

# SPEC-574 Protect interpreter settings when naming agent secrets

## Goal

Prevent newly assigned agent-secret names from setting interpreter options in later runs. Help vault managers move previously accepted names on their own schedule. Preserve host-bound GitHub CLI credentials.

## Context

Issue #638, tracked under task #541, identified exact interpreter names missing from the shared vault parser. The parser also reads stored secrets in the manager list, runtime and CI. Extending its unconditional reservations would break old rows. Production-copy approvals can create a row from a request saved before an upgrade. The Secrets page already marks old flattened keys for their managers. SPEC-004 GH-003 and SPEC-002's proxy contract explicitly support user-supplied GitHub tokens.

## Requirements

- **INT-001** Refuse newly assigned PYTHONPATH, PERL5LIB, RUBYOPT, JAVA_TOOL_OPTIONS and JDK_JAVA_OPTIONS agent-secret names, including renames. Retain the existing NODE_ prefix and initial-letter rules for NODE_OPTIONS and _JAVA_OPTIONS. Give the caller a usable alternative for a newly reserved name.
- **INT-002** Keep existing rows under their prior runtime, host, scope, redaction and editing rules. Show their managers a name warning and a way to remedy it. A rename to an allowed name ends the warning.
- **INT-003** Keep GH_TOKEN and GITHUB_TOKEN available to the GitHub CLI under their existing bindings and precedence.

## Plan

- [x] Specify the creation policy and GitHub override contract
- [x] Prove interpreter-name refusals at each creation boundary
- [x] Preserve existing secrets and report them to their managers
- [x] Obtain independent approval and verification for delivery to dev

## Acceptance criteria

- **AC-1 (INT-001)** The vault API, CLI agent import, agent file tool, Superagent value request and production-copy request refuse interpreter names. An approved copy requested before the upgrade also obeys the new policy and can still be dismissed. Each newly reserved name suggests an assignable name. A rename cannot acquire another protected name.
- **AC-2 (INT-002)** A previously accepted interpreter name still resolves to its placeholder and bound credential. Managers can list, rotate and rename it. Its card explains the effect and the rename. Other people receive only their existing access and no warning.
- **AC-3 (INT-003)** Both GitHub token names still create host-bound agent secrets and use the existing shim override, including when both are set. Report the conflict with #638's requested GitHub reservations to its owner.

## Test plan

Use synthetic credentials in HTTP, agent tool and CLI import tests. Seed a stored secret and a pending copy directly to model an upgrade. Exercise host policy and a manager and reader listing. Check the visible notice in Chromium. Run targeted Rust tests, repository quick checks, the web suite, security and mutation verification, then independent review and Yulius CI.

## Decisions

Apply the additional exact-name policy only when assigning an agent secret a new name. Existing scopes, values and authorizations persist. Changing the GitHub override contract needs an owner's product decision. Record the compatible policy in ADR-0408.

## Evidence

On the unchanged product, the seven Rust regression cases failed. The API created PYTHONPATH (201), approved a pending PERL5LIB copy (200), and supplied no manager warning. File and copy tools accepted interpreter names, a value request waited for input, and CLI import accepted RUBYOPT. Two Chromium scenarios failed at the missing warning and missing refusal. Regression commit f70d04d71.

After the assignment check, nine focused nextest cases passed, including task and Superagent launch environments, same-scope rotation, host-bound proxy substitution for both GitHub names, copy approval and dismissal, and the CLI plan. Two targeted Chromium scenarios passed. Repository quick checks passed. Vidoc was disabled by the instance, so nothing was scanned.

Independent review of a8bf22966 requested two changes. The shim assertion failed with `token=none` when the test expected the supplied GITHUB_TOKEN. The fake gh now reads both token variables in priority order, and the gh shim tests passed. Chromium also observed a 68-character suggestion for a 64-character reserved name. The mock now omits suggestions that fail the name grammar. Both interpreter scenarios passed again, including the length limit.

Independent round two approved 026c685dd549bf1e469c976351fc68cb914ac9aa. At this head the related nextest selection passed 101/101. The full Chromium suite passed 1,181 tests with one model-driven editor scenario skipped because no model key was configured. The review follow-up's two changed browser scenarios also passed in a separate focused run. Yulius records the CI and merge results in task #551's delivery evidence.

The bounded mutation run reported 57 listed, 5 caught, 0 missed, 0 timed out, 52 unviable and 0 not reached. Against the first reviewed head, the follow-up contained no mutable Rust production lines. No live GitHub credential was used in the proxy and shim tests. Issue #638 stays open for the owner's decision on its blanket GitHub token reservation.
