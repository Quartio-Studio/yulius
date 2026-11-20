---
yulius-task: ce65d233-0ed5-403b-b36c-33036e6d1952
status: done
---

# SPEC-567. Prevent Chrome's default app policy dialog

## Goal

Fixes #1411.

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

## Acceptance criteria

- **AC-1** Real Google Chrome installs Docs Offline in a disposable unmodified profile. Capture Xvfb before policy, after live policy arrival and after restarting the older profile.
- **AC-2** New sandbox browser launches skip default apps. On Xvfb, a fresh profile contains no Docs Offline after an extension update check, including before the host policy arrives.
- **AC-3** Using copies of the pre-policy profile, observe the test page without a native box on live policy arrival and on launch with policy already installed. Verify policy arrival via the browser's marker.
- **AC-4** The existing policy test passes with the wildcard extension blocklist. A PDF still renders in Chrome's built-in viewer.

## Test plan

Commit a failing launch-argument regression before changing the browser adapter. Compare full Xvfb framebuffers in a disposable container running branded Google Chrome, using a profile Chrome itself populated before applying policy. Exercise fresh and previously populated profiles. Check the browser's extension inventory, profile on disk, policy marker and PDF viewer. Keep all experimental policy files in the container. Record the independent review and Yulius CI run for delivery.

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
