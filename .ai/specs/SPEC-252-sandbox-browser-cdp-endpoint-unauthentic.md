---
yulius-task: f6b06114-c5ec-41b6-92cc-f541df959716
status: in-progress
---

# SPEC-252: Sandbox browser DevTools over a pipe, not a loopback port

Status: in-progress
Date: 2026-10-21
Owner: Yulius task #238
Scope authorization: task #238 (started by task #235), GitHub issue #626; closes SPEC-002 risk
R-07's remaining path. Decision: [ADR-0191](../../docs/decisions/0191-the-sandbox-browser-speaks-devtools-over-a-pipe.md).

## Goal

No process other than Yulius can drive a sandbox's Chromium through the DevTools protocol: the
browser has no DevTools listener at all.

## Context

- Symptom (#626): every sandbox Chromium started with `--remote-debugging-port=0
  --remote-debugging-address=127.0.0.1` (`crates/yulius-browser/src/manager.rs`). The DevTools
  WebSocket has no authentication, so any process on the host's loopback could connect and
  navigate, read pages and use the browser's logged-in sessions (and CDP can make the uncontained
  browser write host files).
- Who reaches the host loopback: isolated sandboxes (ADR-22) do not; but `YULIUS_ISOLATION`
  defaults to `auto` (`crates/yulius/src/wiring/sandbox.rs`), and a run whose holder is
  unavailable, or whose sandbox has no base yet, uses the previous containment, which shares the
  host network namespace. Host processes (anything local) always could.
- Root cause: the transport. A TCP listener is reachable by every process in the network
  namespace; nothing tied the endpoint to the one client that needs it (Yulius). Tests pinned the
  flags (`chromium_args_are_loopback_isolated_and_sandboxed_by_default`) and the trust note in
  `crates/yulius-app/src/ports/browser.rs` documented the exposure as accepted.
- Consumers of the TCP endpoint: Yulius's own `CdpClient` (via `DevToolsActivePort`);
  `BrowserSession::cdp_endpoint()` (SPEC-001 U-10, "raw CDP endpoint for CDP-capable MCPs") has
  no production caller: no sandbox MCP server receives the URL. Opt-in live tests read it to
  drive the page directly. `devtools_ports()` only kept those ports out of previews.

## Requirements

- CDP-001. The sandbox Chromium starts with `--remote-debugging-pipe` and never with
  `--remote-debugging-port` or `--remote-debugging-address`; it opens no TCP (or other socket)
  DevTools listener.
- CDP-002. Yulius speaks CDP to it over the inherited pipe pair (Chromium reads requests on fd 3
  and writes responses and events on fd 4; messages are JSON terminated by a NUL byte) with the
  same bounds as the WebSocket client: bounded writer queue, bounded pending map, per-call
  timeout, and a maximum message size (a longer message closes the connection).
- CDP-003. Startup stays bounded by `startup_timeout`: a browser that exits first reports
  `ExitedEarly` with its stderr tail, one that never answers `Browser.getVersion` reports
  `StartupTimeout`; neither leaves a process behind.
- CDP-004. The raw endpoint accessors go: `BrowserSession::cdp_endpoint`,
  `BrowserPort::devtools_ports`, `BrowserManager::devtools_ports`,
  `BrowserInstance::{cdp_ws_url, cdp_http_url, devtools_port}` and `DEVTOOLS_PORT_FILE`.
  In-process callers (tests) use `BrowserInstance::cdp()`.
- CDP-005. The trust notes (`ports/browser.rs`, the browser crate docs), SPEC-002 R-07 and its
  mentions, `docs/operations.md`, `docs/architecture.md` and SPEC-001 U-10 say what is true now.

## Plan

- [ ] Spec and ADR-0191 record the pipe decision
- [ ] CdpClient speaks CDP over a pipe pair
- [ ] The fake Chromium answers over its pipe
- [ ] Sandbox Chromium launches with --remote-debugging-pipe and no TCP listener
- [ ] Raw DevTools endpoint accessors removed from the port, wiring and previews
- [ ] Trust notes, R-07 and operations docs say the endpoint is gone
- [ ] Independent review answered, gates green, delivered

## Acceptance criteria

- Another sandbox's process (isolated or not), or any host process, cannot drive a sandbox's
  browser through DevTools: there is no DevTools socket to connect to (CDP-001).
- The sandbox's own browser tooling keeps working: Yulius's browser MCP tools and viewers run over
  the pipe (the manager, instance and server browser tests pass against the fake through the pipe;
  the opt-in real-Chromium smoke test passes). No sandbox MCP server consumed the raw endpoint, so
  none breaks; a raw CDP endpoint for such servers (U-10) would come back only as a per-sandbox,
  authenticated proxy (ADR-0191).
- The trust-model note in `ports/browser.rs` is updated.
- Tested: the launch arguments carry the pipe flag and no port flag; the started browser writes
  no `DevToolsActivePort` and owns no listening TCP socket (real Chromium, Linux).

## Test plan

- Unit (`manager.rs`): `chromium_args` has `--remote-debugging-pipe` and no
  `--remote-debugging-port`/`--remote-debugging-address` (fails before the fix).
- `tests/cdp.rs`: calls, events, screencast acknowledgements and timeouts over an in-memory pipe
  (`tokio::io::duplex`); an over-long message closes the connection; EOF closes it.
- `tests/manager.rs`: the fake Chromium (a shell script bridging its fds 3/4 to the in-process fake
  through `python3`, a declared prerequisite) is driven through the pipe; it never sees a port
  flag and no `DevToolsActivePort` exists; startup failure (exits at once, never answers) ends
  with `ExitedEarly`/`StartupTimeout` and no process left; restart, idle reaping and SIGKILL
  escalation unchanged.
- `tests/real_chromium.rs` (opt-in, `YULIUS_TEST_CHROMIUM`): the smoke test passes over the pipe
  and, on Linux, no process of the browser's group has a listening TCP socket.
- `crates/yulius` browser and preview tests pass without `devtools_ports`.

## Decisions

- Pipe over an authenticating proxy or Chromium in the holder: see ADR-0191 (no listener at all is
  the strongest property and the smallest change; Playwright and Puppeteer use the same mode).
- fds 3/4 are set up with the `command-fds` crate (its `pre_exec` is the only `unsafe`, inside the
  dependency; it uses the workspace's `nix` 0.31), not a `sh -c 'exec 3<&0 4>&1 …'` wrapper:
  no extra process image or shell in the launch path.
- Not done here (said on #626): a per-sandbox authenticated CDP endpoint for CDP-capable MCP
  servers (U-10); nothing consumes one today.
