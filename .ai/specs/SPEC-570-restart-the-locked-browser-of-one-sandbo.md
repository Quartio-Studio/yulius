---
yulius-task: a4d245ef-f76a-4ec5-aa11-b3942597e259
status: in-progress
---

# SPEC-570 Restart a sandbox browser with locked input

## Goal

Fixes #1410.

A person using a sandbox browser can recover clicks and keys after the host policy takes effect while an earlier file chooser may still be open. [Issue #1410](https://github.com/Quartio-Studio/yulius/issues/1410)

## Context

At dev `f97bf8ccc` the adapter detects this condition, but both lock reasons reach the panel as `input_locked`. Recovery in the browser integration test uses an internal stop and a new viewer. The panel has no restart action. Task #548 corrected Chrome's default app dialog in the same base. The policy and chooser checks come from [ADR-0318](../../docs/decisions/0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md).

## Requirements

- BRR-001 Authorize a restart with the sandbox viewer's existing write permission. Reap that sandbox's Chrome and display before starting a fresh instance with the same profile and reopening its visible web tabs. Keep other sandboxes running. Report a failed stop or launch. Refuse a restart while another operation holds the sandbox slot.
- BRR-002 Distinguish the restart lock on the viewer connection. Show the reason and a persistent **Restart browser** action. On success reconnect the panel and accept input once the new instance verifies policy. On failure show the error and allow recovery. The missing-policy case continues to explain host setup.
- BRR-003 Document the action for sandbox operators and the API contract.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Restart the selected browser under its existing sandbox authorization
- [x] Recover viewer input through the locked browser panel
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1 A writer replaces one process, preserves the profile and reopens the permitted tabs and selects the previously active one. A reader, anonymous request and inaccessible sandbox are refused before any browser changes.
- AC-2 A marked browser refuses input after policy arrives. After a successful restart the replacement checks the policy and accepts the viewer's click. Missing policy still locks input.
- AC-3 A person sees the restart action in the panel, invokes it and types into the recovered page. A failed restart can be retried. Operations names the action.

## Test plan

Exercise the HTTP route with the integration harness and two supervised browser processes, inspect their PIDs and profile files, and exercise the existing chooser regression through the new API. Check reopened tabs with an isolated Chrome profile. Use the Chromium panel suite for both lock reasons, retries and input. Run quick checks, security, mutation testing, independent review and Yulius CI.

## Decisions

Use the existing browser grant, per-sandbox process slot, bounded stop and profile location. Reserve the new action for the specific lock reason in the panel. Launch on a blank page and reopen visible web addresses through the installed guard. Chrome’s startup restore can load pages before Yulius can intercept them. Cookies and site storage persist in the profile. Each page gets a new document and tab ID.

## Evidence

- At upstream dev `f97bf8ccc`, the API regression received 404 on the read-only request where the sandbox browser route must return 403. Chromium showed the existing lock message and found zero buttons named **Restart browser**.
- Google Chrome 157.0.8103.50, on a disposable profile, restarted with a different PID, the same persistent cookie and exactly two loopback tabs. The first tab was active before and after restart. The opt-in `restarting_real_chrome_reopens_the_active_page_with_its_cookie` test passed.
- The manager regression forced a failed replacement launch, recovered through the ordinary browser access path and saw the address guard installed before either restored navigation. Five focused Chromium panel tests passed, including a failed restart followed by typing into the replacement and a successful restart whose HTTP reply was lost. The first full Chromium suite passed 1,176 tests with one skipped.
- The independent reviewer requested three fixes on `667676000`. Regressions first reproduced a dropped page-created tab, lost ownership after replacement setup and cleanup both failed, and a disconnected panel after a lost HTTP reply. After the fixes all 13 manager tests and all five panel tests passed. The tab cap is applied to permitted addresses before stopping. A failed stop keeps the process and display owned and unavailable until cleanup succeeds. A lost reply reconnects once to the current browser.
- The authorization regression explicitly uses a cookie session for the CSRF check. Its initial fixture supplied a CLI bearer, which legitimately accepts a request without the cookie session's CSRF header.
- Focused nextest ran 47 browser tests and all passed. They include writable versus read-only access, cookie CSRF, inaccessible and missing sandboxes, other sandbox process identity and live input before and after replacement.
- Round two fault injection on `ccf763c95` showed that failed readiness or target discovery followed by a failed display stop could allow a third launch. A process-level regression also recorded two group signals after two calls to stop an already reaped child. The slot now records the starting display and process immediately on acquisition, and successful process-group cleanup is remembered. All 61 browser unit tests and 13 manager tests passed. The manager regression covers executable spawn, DevTools readiness, discovery, target listing, guard setup and restoration failures. Real Chrome again retained its active URL and cookie.
- Round three identified an omitted spawn case in the fault table. The test now exercises a missing interpreter. The reviewer also confirmed a pre-existing Xvfb failure in `start_xvfb`, before the manager acquires the display, on dev `d809931d5`. It is tracked in [#1519](https://github.com/Quartio-Studio/yulius/issues/1519).
