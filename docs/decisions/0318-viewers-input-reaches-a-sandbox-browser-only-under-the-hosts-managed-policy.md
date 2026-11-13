# 0318. A viewer's input reaches a sandbox browser only under the host's managed policy

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 6); task #465. Narrows [0008](0008-streamed-sandbox-browser.md) (viewers' real input on
the virtual display); extends what `yulius host setup` keeps on a host
([0162](0162-one-host-setup-command-brings-and-keeps-docker-in-sandboxes.md)).

## Context

A sandbox's Chrome runs on the host as the service user, headful on its own virtual display,
and a viewer's mouse and keys are real X input there (XTest), so captchas and sign-in popups
work. Real input reaches more than the page. The audit of task #465 clicked a page's
`<input type=file>` through `BrowserInstance::dispatch_mouse` in Google Chrome 157: GTK's
"Open File" window opened over the whole display. In it a viewer types any path the service
user reads (the instance's master key, its database, another sandbox's repository), and the
page they opened reads the file. Save as, printing to a file, DevTools and `file:` or
`view-source:` pages are further windows of Chrome itself; `--kiosk` hides the toolbar, not
these. A viewer is anyone who may work in one sandbox, a playground's guest included.

What Chrome offers against all of its own windows at once, in every tab, frame and popup, is
its managed policy: files in a directory only root writes, read by every Chrome on the host.
Probes on this machine: with `AllowFileSelectionDialogs: false` the same click opened nothing
and `showOpenFilePicker()` answered `NotAllowedError`; a navigation to a blocked address fails
with `net::ERR_BLOCKED_BY_ADMINISTRATOR`; and `DeveloperToolsAvailability: 2` refuses the
DevTools protocol too (`Target.attachToTarget`: "Not allowed"), which Yulius drives the
browser with.

## Decision

- **The policy.** `yulius_browser::policy::managed_policy` is a file `yulius.json`: no file
  selection dialog, no downloads, no printing, no `file:`, `view-source:`, `chrome:` or
  `devtools:` page (the developer tools' window is one), the File System Access API blocked,
  no extension installs, and a marker address blocked
  (`yulius-browser-policy-v<version>.invalid`).
- **Host setup writes it.** `yulius host setup`, and the update's `yulius host check`, write
  it into the managed policy directories of Google Chrome and Chromium
  (`/etc/opt/chrome/policies/managed`, `/etc/chromium/policies/managed`,
  `/etc/chromium-browser/policies/managed`) as the item "browser policy"; the update helper's
  unit may write those directories once the setup made them. `yulius doctor` reports it.
- **The browser is asked, not the file.** Which file a Chrome reads depends on its flavor and
  packaging. A browser on a virtual display is asked itself: a hidden tab is sent to the
  marker address, and the navigation fails as blocked by the administrator, or otherwise
  (`BrowserInstance::viewer_input_allowed`). The marker carries the policy's version, so an
  older Yulius's file, without what a newer one adds, does not pass.
- **No policy, no clicks and keys.** While the answer is no, a viewer's button presses and key
  presses on the display are refused (`BrowserError::InputLocked`, `input_locked` on the
  viewer's socket) and the page tells the viewer why and what an administrator runs, once a
  minute at most. Moving the pointer, scrolling, the tab bar, the address bar and the agents'
  tools work as before. The question is asked again after 5 seconds, so input comes back
  without a restart once the policy is written (Chrome reloads its policy files). A headless
  browser, which shows no window of its own, is unchanged.

## Alternatives

- **Intercepting the file chooser over DevTools** (`Page.setInterceptFileChooserDialog`). It
  covers `<input type=file>` in the tabs Yulius attached to, not frames of other sites, popups
  before they are attached, the File System Access pickers, Save as, printing or DevTools.
  Each would be its own interception and the next Chrome adds a window. Rejected as the lock;
  nothing forbids adding it for a friendlier answer to uploads later.
- **Filtering keys and the right button.** An allow-list of shortcuts breaks web applications'
  own (Ctrl+S in an editor), a deny-list misses the next accelerator, and neither stops what a
  page opens itself. Rejected.
- **Chrome in a private file system view** (bubblewrap, or the sandbox's own namespaces): then
  no window of Chrome's reaches the host's files, whatever opens. It is the firmer fix and
  planned with the browser's network (finding E2). It could not be proven here (namespaces
  cannot be made inside a sandbox, and Chrome's own sandbox inside one needs a host test), and
  a critical hole should not wait for it. The policy stays useful under it.
- **Disabling DevTools by policy.** Refuses the protocol Yulius needs (see Context). The
  tools' window is blocked as a `devtools:` page instead.
- **Trusting the file.** Comparing `/etc/…/yulius.json` with the expected text says nothing
  about a snap, another flavor or a Chrome started with other policy sources. Rejected for the
  gate; the doctor and host setup still report the files.
- **Starting no browser without the policy.** Agents' tools and watching need no input from a
  viewer and reach none of Chrome's windows through real input. Rejected.

## Consequences

- After the update that brings this, a host shows its sandbox browsers but takes no clicks and
  keys in them until `sudo yulius host setup` ran once (the update helper cannot create the
  policy directories; later versions of the policy it writes itself). The page, the update's
  host check and `yulius doctor` say so.
- The policy applies to every Chrome and Chromium on the host, not only Yulius's: on a
  machine where people also use Chrome themselves, their uploads, downloads and printing are
  off. Yulius's hosts are servers; a developer's workstation runs the sandbox browser headless
  or accepts it.
- Viewers cannot upload a file through a page in the sandbox browser, download one or print;
  agents never could through real input. A page's upload button does nothing.
- A probe opens and closes one hidden tab; it is never listed among the tabs. Nor are Chrome's
  own pages any more (Chrome 157 reports its address bar's popup,
  `chrome://omnibox-popup.top-chrome/`, as a page: it showed as a tab).
