# 0338. The web UI runs under a strict content security policy

## Status

Accepted. Specified in
[SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md) (finding W1b,
slice 38), task #465.

## Context

Yulius's pages sent `Content-Security-Policy: frame-ancestors 'none'` and nothing else. The UI
inserts server and user data only as text and attributes (`dom.js`), so no injection is known; but
one slip there, or in a vendored library, would have run as the signed-in person, with their
session and their sandboxes' terminals. A policy is the second wall.

The UI loads everything from its own origin (fonts are self-hosted, no CDN). Monaco and the shader
library check whether `eval` is allowed and fall back to `importScripts` and a non-eval path
when it is not. xterm and Monaco add `<style>` elements at run time. Monaco's language workers start
from `blob:` URLs, pasted and attached images are shown from `blob:` and `data:` URLs, and the
browser panel draws the sandbox Chrome's frames on a canvas. Previews open in new windows, never
in a frame.

## Decision

Every public response not answered by a route with its own policy carries:

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self';
worker-src 'self' blob:; frame-src 'none'; object-src 'none';
base-uri 'none'; form-action 'self'; frame-ancestors 'none'
```

- **One file**: `crates/yulius-server/src/content-security-policy.txt`. The server includes it
  at build time; `e2e/serve.py` sends the same file, so every e2e test runs under the
  production policy. The browser only logs a refused load, so the suite's page instrumentation
  (`e2e/tests/support.ts`) records each `securitypolicyviolation` as a page error, and the test
  fails on it. The one difference: a brand lab page (`?lab=`, mock only, never shipped) may load
  the Google Fonts families it previews (`fonts.googleapis.com` styles, `fonts.gstatic.com`
  fonts).
- **Scripts only from Yulius**: no inline script, no `eval`, no other origin. Files people attach
  are served as images or `application/octet-stream` with `nosniff` and their own `sandbox`
  policy, so no same-origin URL hands back user content as a script.
- **Inline styles are allowed**: xterm and Monaco create `<style>` elements and cannot be given a
  nonce. A style cannot run code; the risk left is CSS-based reading of the page, which needs an
  injection that the script policy already limits.
- Routes with a stricter policy keep theirs: preview pages, the proxy's refusal page and
  attachments (`sandbox`).

## Alternatives

- **Nonces or `'strict-dynamic'`**: `index.html` is embedded, cached and validated by ETag; a
  per-response nonce would make every page load a full download and a rewrite of the shell. With
  `script-src 'self'` and no inline script, a nonce adds nothing.
- **Hashes for styles**: the styles xterm and Monaco add change with the theme and size.
- **Trusted Types**: Monaco supports them, xterm and our own code would need a policy. A later
  step once the libraries agree; the script policy comes first.
- **A report endpoint**: an unauthenticated write path for every browser; the console and the
  e2e suite already show refusals during development.
- **Keeping `frame-ancestors 'none'` only**: one bug in the UI or a vendored file away from
  running code as the signed-in person.

## Consequences

- Anything new the UI loads from elsewhere (a font service, an image host, an iframe) is refused
  until this one file says otherwise, and the e2e suite fails until it does.
- The brand lab is not shipped (`lab.js` is not embedded), so on a real instance it does not
  exist; the e2e server allows its two font hosts on a lab page only.
- `connect-src 'self'` covers the page's WebSocket (`wss://` on the same host) in every current
  browser (CSP Level 3). Browsers from before that rule (Safari before 15.4) refuse the
  WebSocket and show the UI without live updates.
- The policy is set once in the server's header middleware: an HTML page added to a route must
  either fit it or set its own.
