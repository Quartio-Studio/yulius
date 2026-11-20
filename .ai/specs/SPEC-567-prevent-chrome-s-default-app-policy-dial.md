---
yulius-task: ce65d233-0ed5-403b-b36c-33036e6d1952
status: in-progress
---

# SPEC-567. Prevent Chrome's default app policy dialog

## Goal

Sandbox browsers open a usable display when Chrome applies Yulius's extension blocklist, including when a profile already contains Google Docs Offline. [Issue #1411](https://github.com/Quartio-Studio/yulius/issues/1411)

## Context

Task #490 saw Chrome's native administrator dialog after host setup. On dev `e15f4f188`, a fresh Google Chrome 157 profile installed Docs Offline. Reopening that profile and adding Yulius's policy while Chrome ran changed the extension from enabled to disabled and displayed the dialog on Xvfb. Restarting the same browser with policy already loaded cleared the display. Existing tests check the wildcard blocklist and the launch arguments independently. [ADR-0318](../../docs/decisions/0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md)

## Requirements

- **APP-001** Fresh sandbox profiles skip Chrome's default app installation.
- **APP-002** A sandbox browser launched on its virtual display with a pre-policy profile shows no extension-blocked dialog when policy arrives or when it starts with policy already installed.
- **APP-003** The host policy continues to block every extension install. Chrome's built-in PDF viewer and the existing profile data remain usable.

## Plan

- [x] Verify current dev on a private Xvfb display with fresh and older profiles
- [ ] Capture the launch regression and preserve the wildcard policy
- [ ] Verify the corrected launch with real Chrome and document its scope
- [ ] Obtain independent review and verify green delivery and issue closure

## Acceptance criteria

- **AC-1** Real Google Chrome installs Docs Offline in a disposable unmodified profile. Capture Xvfb before policy, after live policy arrival and after restarting the older profile.
- **AC-2** New sandbox browser launches skip default apps. On Xvfb, a fresh profile contains no Docs Offline after an extension update check, including before the host policy arrives.
- **AC-3** Using copies of the pre-policy profile, observe the test page without a native box on live policy arrival and on launch with policy already installed. Verify policy arrival via the browser's marker.
- **AC-4** The existing policy test passes with the wildcard extension blocklist. A PDF still renders in Chrome's built-in viewer.

## Test plan

Commit a failing launch-argument regression before changing the browser adapter. Compare full Xvfb framebuffers in a disposable container running branded Google Chrome, using a profile Chrome itself populated before applying policy. Exercise fresh and previously populated profiles. Check the browser's extension inventory, profile on disk, policy marker and PDF viewer. Keep all experimental policy files in the container. Record the independent review and Yulius CI run for delivery.

## Decisions

Skip default apps on sandbox launches. On virtual displays also disable ordinary extensions before Chrome loads the profile. The default-app switch alone reproduced the dialog for the previously installed extension. Scope the broader switch to the display whose viewer input depends on the host policy. A launch setting takes effect the next time the browser starts.
