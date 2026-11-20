---
yulius-task: ce65d233-0ed5-403b-b36c-33036e6d1952
status: done
---

# SPEC-567. Prevent Chrome's default app policy dialog

## Goal

Fixes #1411. Fixes #1517. Fixes #1518.

Sandbox browsers open a usable display when Chrome applies Yulius's extension blocklist, including when a profile already contains Google Docs Offline. [Issue #1411](https://github.com/Quartio-Studio/yulius/issues/1411)

## Context

Task #490 saw Chrome's native administrator dialog after host setup. On dev `e15f4f188`, a fresh Google Chrome 157 profile installed Docs Offline. Reopening that profile and adding Yulius's policy while Chrome ran changed the extension from enabled to disabled and displayed the dialog on Xvfb. Restarting the same browser with policy already loaded cleared the display. Existing tests check the wildcard blocklist and the launch arguments independently. [ADR-0318](../../docs/decisions/0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md)

## Requirements

- **APP-001** Fresh sandbox profiles skip Chrome's default app installation.
- **APP-002** A sandbox browser launched on its virtual display with a pre-policy profile shows no extension-blocked dialog when policy arrives or when it starts with policy already installed.
- **APP-003** The host policy continues to block every extension install. Chrome's built-in PDF viewer and the existing profile data remain usable.

## Plan

- [x] Verify current dev on a private Xvfb display with fresh and older profiles
- [x] Capture the launch regression and preserve the wildcard policy
- [x] Verify the corrected launch with real Chrome and document its scope
- [x] Obtain independent approval for delivery to dev
- [x] Preserve the parked-screen fixture across a late viewer resize
- [x] Verify saved assistant names when transcript entries are offscreen
- [x] Validate the ordered ADR index on the landed prefix and obtain delivery approval

## Acceptance criteria

- **AC-1** Real Google Chrome installs Docs Offline in a disposable unmodified profile. Capture Xvfb before policy, after live policy arrival and after restarting the older profile.
- **AC-2** New sandbox browser launches skip default apps. On Xvfb, a fresh profile contains no Docs Offline after an extension update check, including before the host policy arrives.
- **AC-3** Using copies of the pre-policy profile, observe the test page without a native box on live policy arrival and on launch with policy already installed. Verify policy arrival via the browser's marker.
- **AC-4** The existing policy test passes with the wildcard extension blocklist. A PDF still renders in Chrome's built-in viewer.

## Test plan

Commit a failing launch-argument regression before changing the browser adapter. Compare full Xvfb framebuffers in a disposable container running branded Google Chrome, using a profile Chrome itself populated before applying policy. Exercise fresh and previously populated profiles. Check the browser's extension inventory, profile on disk, policy marker and PDF viewer. Keep all experimental policy files in the container. Record the independent review and Yulius CI run for delivery.

PR CI also exercises the mock terminal shown to owners of parked tasks. Reproduce the loss of row 89 after a viewer's delayed resize, then verify the saved grid and the subsequent live-grid transition. For the saved assistant name, scroll a long transcript until Chromium skips painting the first answer. Check the exact prefix for each harness and exercise a deliberately incorrect saved name. Run the repository checker on the train that includes ADRs 0401 and 0402.

## Decisions

Skip default apps on sandbox launches. On virtual displays also disable ordinary extensions before Chrome loads the profile. The default-app switch alone reproduced the dialog for the previously installed extension. Scope the broader switch to the display whose viewer input depends on the host policy. A launch setting takes effect the next time the browser starts. [ADR-0402](../../docs/decisions/0402-sandbox-browsers-skip-chromes-default-apps.md)

## Evidence

Used Google Chrome 157.0.8103.50 in a disposable Linux container with an Xvfb framebuffer at 960×640. The browser accepted the arguments emitted by `BrowserConfig::chromium_args` over the test profiles, with the container-only `no_sandbox` setting. The policy directory, Chrome homes and X server were private to the container. The marker navigation returned `net::ERR_BLOCKED_BY_ADMINISTRATOR` in each policy case.

| Chrome 157 on Xvfb | Result on the full display |
| --- | --- |
| Dev `e15f4f188`, fresh profile then reopened before policy | Chrome installed Docs Offline 1.112.1. The reopened profile showed it enabled. [Clear display](evidence/SPEC-567/clear-display.png) |
| Dev, reopen the older profile and add policy live | Chrome reported Docs Offline disabled. The native box with the OK button stayed visible, partly off the left edge of Xvfb. [Dialog](evidence/SPEC-567/dev-policy-dialog.png) |
| Dev, restart the same profile with policy | The display cleared. [Restart capture](evidence/SPEC-567/clear-display.png) |
| Fixed, fresh profile with policy added live or already installed | Chrome reported no ordinary extensions. The fresh pre-policy browser completed an extension update check. Neither profile contained Docs Offline in preferences or on disk. [Clear display](evidence/SPEC-567/clear-display.png) |
| Fixed, copies of the older profile | All three captures matched the clear display byte for byte: policy added live, restart after live policy, and first launch of an untouched copy with policy already installed. The original extension manifest remained on disk. [Clear display](evidence/SPEC-567/clear-display.png) |

A fresh headless launch with the default-app switch alone completed an extension update check and contained no Docs Offline on disk or in preferences. All five corrected Xvfb runs loaded the PDF test document through Chrome's built-in PDF extension and its document frame. The identical clear captures share one PNG. The test page and the profiles contained synthetic data.

- `cargo test --locked -p yulius-browser --lib manager::tests` on the old launcher failed both new regressions at the missing switches. Their test-only commit is `f00634c99`.
- `cargo test --locked -p yulius-browser --lib` on the fix passed all 59 tests, including the wildcard policy and both launch regressions. The generated policy was byte-identical to dev (SHA-256 `e6c303c0c9fbb1d736d5f5e3255a6a6c8a917def4596e3001804245bf7ff5418`).
- Scoped Clippy passed with all targets, all features and warnings denied. The real-browser checks cover Linux Chrome 157 on disposable profiles. Production sessions and other Chrome versions were outside the experiment. A browser already running old arguments picks up the fix on its next launch.

- A separate copy of the genuine pre-policy profile stored a synthetic persistent cookie and a local-storage value. A fixed launch with policy already loaded read both original values.
- The independent reviewer approved `241bf4168`, traced the production launch, passed all 59 browser library tests, checked formatting and inspected the Xvfb screenshots. The reviewer used the recorded real-Chrome results.
- `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled by Yulius and scanned nothing. The task's delivery record tracks the PR and landed-commit CI results and GitHub issue closure.
- `scripts/dev mutants` tested all four mutations of the changed argument builders. Four caught, zero missed, timed out, unviable or unreached.

### Delivery recovery

The original PR head passed quality and both Rust test shards. Yulius run `244e8360-1160-4e37-9d0e-28f732cd1df3` failed the queued parked-screen e2e assertion because row 89 had no node. On `0bd899936`, delivering a 44×28 resize after the first saved row appeared reproduced the identical assertion. The mock had announced the 220×90 capture to viewers once, then reported its live terminal state on the next resize. The production terminal hub reports the retained screen's dimensions on subsequent state messages. [CI fixture issue #1517](https://github.com/Quartio-Studio/yulius/issues/1517)

The three-PR train `fa2269928` failed the repository checker with 0402 before 0401. Its immediate parent listed 0400 then 0401. Swapping only those two intact entries in a detached copy of the failed train made `python3 scripts/check_repository.py` pass. Task #560 owns the general merge-order fix.

The test-first commit `fce570f41` forces the delayed resize. The corrected mock reads its retained capture for both state and snapshot replies until a live attempt takes over. All nine Chromium parking scenarios passed, including all four saved-grid statuses and the transition back to a live viewport.

Review of the fixture found that an incorrect live terminal could still pass by mirroring 28 rows. With the UI deliberately forced to mirror every waiting-capacity terminal, the initial recovery test passed. Offering the original 220×90 server size at the live transition and checking that the owner leaves mirror mode failed that mutant (one mirrored host, expected zero). All nine parking scenarios passed with the actual UI.

Yulius run `b1b56597-4952-48b4-97f6-097478bf80f3` on `f11a41632` read the correct Codex label in watch, on reprint and in search, then read an empty string on the saved page. On that page, each assistant entry carries its screen-reader span from creation. Chromium can skip painting an entry under the existing `content-visibility: auto` rule. With a long transcript scrolled to the end, the first Codex span had DOM `textContent` equal to `Codex` and rendered `innerText` equal to the empty string. The e2e locator reads the latter. The CI log records the empty value, while the local reproduction also measured Chrome's skipped state. [Assistant-label issue #1518](https://github.com/Quartio-Studio/yulius/issues/1518)

The regression commit `6ef139e03` reproduced the empty saved label for Claude, Codex and Antigravity. The corrected checks read each prefix directly from the DOM and require its exact case, including the imported Claude search hit. Forcing saved entries to use Claude in an isolated checkout then failed only the Codex and Antigravity saved-label assertions. This repair changes the test. The browser UI already sets the span to the session's harness name.

All 55 Chromium watching and Sessions tests passed on the repaired branch. `scripts/dev quick` passed. `scripts/dev mutants --base f11a41632` found no Rust changes in this test repair.

PR head `6b0af30da` passed its own [Yulius CI run](https://yulius.quartio.ai/s/yulius/ci?run=d888c0ef-0272-407e-8418-bf1e8cedadd2). The independent reviewer approved `f11a41632` after verifying the live and saved grid controls. The assistant-label correction since that approval changes tests and evidence.

The integration branch landed #1511 at `7645d73dc` and #1510 at `a97de183b`. Merging that dev into the task branch reproduced the repository checker's 0402-before-0401 failure. Commit `0fa27701f` orders the original index entries 0400, 0401 and 0402. The entries and decision files for 0401 and 0402 match their respective parent commits byte for byte. The combined tree passed `python3 scripts/check_repository.py`, `scripts/dev quick` and all 59 browser library tests.

The combined tree passed all 37 Chromium parked and watching cases. The previously approved browser code and parked-screen mock are byte-identical on this tree. Vidoc was disabled by Yulius. The task delivery record tracks the final PR and landed-commit checks.
