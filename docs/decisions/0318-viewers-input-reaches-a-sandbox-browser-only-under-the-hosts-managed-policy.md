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
  selection dialog, no downloads, no printing, the File System Access API blocked, no
  extension installs, and only web addresses: every address is blocked but `http:`, `https:`,
  `ws:`, `wss:`, `about:`, `data:`, `blob:` and Chrome's built-in extensions (its PDF viewer). That leaves no
  `file:`, `view-source:` or `chrome:` page, no `devtools:` page (the developer tools' window
  is one) and no address of another program, for which Chrome offers to run `xdg-open` on the
  host (on Chrome 157 a click on such a link kept the page and handed the address over; under
  the policy the tab shows it blocked). A marker address
  (`yulius-browser-policy-v<version>.invalid`) is blocked as the more specific rule.
- **Host setup writes it.** `yulius host setup`, and the update's `yulius host check`, write
  it into the managed policy directories of Google Chrome and Chromium
  (`/etc/opt/chrome/policies/managed`, `/etc/chromium/policies/managed`,
  `/etc/chromium-browser/policies/managed`) as the item "browser policy". The update helper's
  unit lists those directories as writable (`deploy/systemd/yulius-update.service`; for a unit
  from before, the setup writes a drop-in), so its re-check writes a newer version of the
  policy into the directories the setup made. `yulius doctor` reports it.
- **The browser is asked, not the file.** Which file a Chrome reads depends on its flavor and
  packaging. A browser on a virtual display is asked itself: a hidden tab is sent to the
  marker address, and the navigation fails as blocked by the administrator, or otherwise
  (`BrowserInstance::viewer_input_allowed`). The marker carries the policy's version, so an
  older Yulius's file, without what a newer one adds, does not pass. The probe's tab is known
  by the id Chrome gives it, never by its address (a page can open the same address: such a
  window is an ordinary tab). While the probe waits for that id, a new page no page opened
  is not listed yet; once the id is known, or the probe is given up or dropped, what waited is
  listed unless it is the probe's. The probe is given 3 seconds for its tab and 3 for the
  navigation.
- **No policy, no clicks and keys.** While the answer is no, a viewer's button presses and key
  presses on the display are refused (`BrowserError::InputLocked`, `input_locked` on the
  viewer's socket) and the page tells the viewer why and what an administrator runs, once a
  minute at most. Moving the pointer, scrolling, releases, the tab bar, the address bar and
  the agents' tools work as before. A "no" is asked again after 5 seconds, so input comes back
  without a restart once the policy is written (Chrome reloads its policy files); a "yes"
  after 5 minutes, as the files can also be taken away under a running browser. A headless
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
  keys in them until `sudo yulius host setup` ran once: the update helper's unit is read-only
  outside the paths it lists, so it can neither create the policy directories nor, with a unit
  from before, write them. The setup makes the directories and the drop-in; from then on the
  helper's re-check writes later versions of the policy itself. The page, the update's host
  check and `yulius doctor` say what to run.
- The policy applies to every Chrome and Chromium on the host, not only Yulius's: on a
  machine where people also use Chrome themselves, their uploads, downloads and printing are
  off. Yulius's hosts are servers; a developer's workstation runs the sandbox browser headless
  or accepts it.
- Viewers cannot upload a file through a page in the sandbox browser, download one or print;
  agents never could through real input. A page's upload button does nothing.
- A probe opens and closes one hidden tab; it is never listed among the tabs. Nor is a page
  of Chrome's own that is created as one (Chrome 157 reports its address bar's popup,
  `chrome://omnibox-popup.top-chrome/`, as a page: it showed as a tab).
- Another file in the same policy directory that sorts after `yulius.json` can set one of its
  settings differently (Chrome merges policy files by key, the later file winning) while the
  marker still answers. Only root writes there; `yulius doctor` and host setup report
  Yulius's own file, not others'.
