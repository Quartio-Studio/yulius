# 0191. The sandbox browser speaks DevTools over a pipe

## Status

Accepted. Specified in [SPEC-252](../../.ai/specs/SPEC-252-sandbox-browser-cdp-endpoint-unauthentic.md);
task #238, issue #626. Closes SPEC-002 risk R-07; amends SPEC-002 ADR-09/ADR-15 (how Yulius
reaches the browser) and narrows SPEC-001 U-10 (no raw CDP endpoint for now).

## Context

- Every sandbox Chromium listened for the Chrome DevTools Protocol on `127.0.0.1:<random port>`
  without authentication. Any process in the host's network namespace could drive it: navigate,
  read pages, use its logged-in sessions, read a filled secret (SPEC-002 §8 R-15), and make the
  uncontained browser write host files.
- Isolated sandboxes (ADR-22) cannot reach the host's loopback, but `YULIUS_ISOLATION=auto` runs a
  sandbox without a holder or base under the previous containment, which shares it; host
  processes always could.
- The only client of the endpoint is Yulius itself. `BrowserSession::cdp_endpoint()` (U-10, raw
  CDP for CDP-capable MCP servers) had no production caller.

## Decision

- Chromium starts with `--remote-debugging-pipe` (no `--remote-debugging-port`/`-address`): it
  reads CDP requests on fd 3 and writes responses and events on fd 4, each a JSON message
  terminated by a NUL byte. It opens no DevTools socket, so there is nothing to connect to.
- Yulius creates the two pipes, maps their child ends to fds 3 and 4 with the `command-fds` crate
  (its `pre_exec` is the only `unsafe`, inside the dependency; it shares the workspace's `nix`),
  and keeps the other ends close-on-exec, so no other process inherits them.
- `CdpClient` has two transports with the same bounds: the WebSocket (tests, an externally managed
  browser via `BrowserInstance::attach`) and the pipe (`CdpClient::over_pipe`). A message longer
  than `max_message_bytes` closes a pipe connection, as it does a WebSocket.
- Startup waits for an answer to `Browser.getVersion` within `startup_timeout` instead of
  `DevToolsActivePort`; an early exit reports `ExitedEarly` with stderr.
- The raw-endpoint accessors (`cdp_endpoint`, `devtools_ports`, `cdp_ws_url`, `cdp_http_url`,
  `devtools_port`, `DEVTOOLS_PORT_FILE`) are removed; in-process code uses
  `BrowserInstance::cdp()`.

## Alternatives

- **An authenticating proxy** (a per-sandbox token on a WebSocket in front of the port). The port
  itself would stay open on loopback unless Chromium also moved to a pipe, so it adds a component
  without removing the exposure; it is the way to give a sandbox's own MCP servers raw CDP later.
- **Chromium inside the sandbox's holder** (its network namespace). Only for isolated sandboxes, the
  case that is already safe; it moves the display, encoder and profile handling into containment,
  a much larger change.
- **Keep the port and accept the risk.** The fallback containment is the default path for a
  sandbox without a base, and the browser holds people's logged-in sessions.
- **A `sh -c 'exec 3<&0 4>&1 …' chromium` wrapper** instead of `command-fds`: no dependency, but a
  shell in the launch path and stdin/stdout repurposed; the crate is small, Apache-2.0 and already
  on our `nix`.

## Consequences

- No process but Yulius can speak CDP to a sandbox browser, whatever the containment.
- A CDP-capable MCP server inside a sandbox cannot attach to its browser (U-10's raw endpoint); none
  did. Offering one needs a per-sandbox authenticated endpoint inside the sandbox (an issue comment
  on #626 records it).
- Previews no longer need to exclude DevTools ports.
- Tests: the fake Chromium bridges its fds 3/4 to the in-process fake server with `python3` (a
  repository prerequisite); opt-in live tests drive pages through `BrowserInstance::cdp()`.
