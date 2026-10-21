---
yulius-task: f6b06114-c5ec-41b6-92cc-f541df959716
status: done
---

# SPEC-252: Sandbox browser DevTools over a pipe, not a loopback port

Status: done
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

- [x] Spec and ADR-0191 record the pipe decision
- [x] CdpClient speaks CDP over a pipe pair
- [x] The fake Chromium answers over its pipe
- [x] Sandbox Chromium launches with --remote-debugging-pipe and no TCP listener
- [x] Raw DevTools endpoint accessors removed from the port, wiring and previews
- [x] Trust notes, R-07 and operations docs say the endpoint is gone
- [x] Independent review answered and gates green

## Acceptance criteria

- Another sandbox's process (isolated or not), or a host process of another user, cannot drive a
  sandbox's browser through DevTools: there is no DevTools socket to connect to (CDP-001), and
  contained runs cannot see the browser's process to open its pipe (own PID namespace). Yulius's
  own user (including an agent run without containment, host mode) or root still could through
  `/proc/<pid>/fd` (accepted: that user owns the profile).
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

## Evidence

- Independent review (fresh session, `om-code-review`) of `ddc8668b`: APPROVE, with low findings
  only, all in own code, all fixed: the trust notes overclaimed (Yulius's user or root can open
  `/proc/<pid>/fd`; sandboxes cannot, having their own PID namespace), so the wording is now
  precise; R-15, `operations.md` and SPEC-002's ADR-22 text still named raw CDP or DevTools ports
  (corrected); `ExitedEarly` could lose stderr read before the reader drained it (fixed with a
  bounded wait for stderr's end; the regression test failed first, commit `98abe348`); tests added
  for a pipe that closes while Chromium lives (`BrowserError::Cdp(Closed)`, process reaped) and a
  message of exactly `max_message_bytes`; nits: constant placement, `Lagged` in the projects
  test, `deny.toml` names `command-fds`. Not changed: the fake's accept loop ending on an accept
  error (test-only, it never fails on loopback).
- Merged `origin/dev` at `696eb6aa`. It conflicted in `crates/yulius/tests/projects.rs`, where
  dev had fixed the same flaky test the same way (waiting for the notice), so dev's version was
  kept and this change no longer carries that test fix. Yulius CI on `2c216e1d` once failed only
  `yulius-git::git_ops ci_checkouts_created_at_once_all_succeed` (`git worktree list` read a
  concurrent checkout's half-written `commondir`). That is open issue #666, outside this change;
  the evidence was added there.
- Confirmation review of `9c38e5fa`: APPROVE. One minor finding, fixed: the wording now names runs
  without containment (host mode) beside Yulius's user, and the ADR index line no longer
  overclaims. Nits, fixed: the `NeverReady` case no longer reads `ClosesPipe`'s pid file; SPEC-002's
  ADR-22 text no longer lists DevTools ports; an awkward line break in `architecture.md`.
- `scripts/dev ci` on `5cdd53f5` (after the review fixes): passed, `exit=0`, nextest 2368 run,
  2368 passed, 17 skipped. Merged `origin/dev` (`dbafe015`, secrets only) cleanly; `cargo check
  --workspace --all-targets --all-features` passed.
- `scripts/dev ci` (the whole gate) on `aef4c8c6`: passed, `exit=0`, nextest 2367 run, 2367
  passed, 17 skipped. The first run (on `4b5c986f`) failed one test outside this change,
  `projects::a_scheduled_start_without_claude_waits_and_says_why` (added by #760). It failed 1 of
  8 isolated runs. Cause: `pause` sets `waiting_capacity` (`capacity.rs`) before it publishes the
  notice, and the test drained notices once with `try_recv`. Fixed in the test (it waits for the
  notice; commit `aef4c8c6`). With a 200 ms delay injected before the notice, the old test fails
  and the fixed one passes; 12 of 12 isolated runs passed.
- `cargo test -p yulius-browser --features fake`: unit 52, cdp 10 (5 new pipe tests), instance
  15, manager 7, all passed. The manager tests drive the fake Chromium through its relayed pipe:
  `--remote-debugging-pipe` in argv, no port/address flag, no `DevToolsActivePort`, one
  `Browser.getVersion` per start; early exit → `ExitedEarly` with stderr, a silent pipe →
  `StartupTimeout`, no process left behind.
- Real browser (opt-in, local, Google Chrome 155.0.8059.39 through a `--no-sandbox` wrapper,
  since the host runs as root): `YULIUS_TEST_CHROMIUM=… cargo test -p yulius-browser --test
  real_chromium`: passed over the pipe; no process of the browser's group holds a listening TCP
  socket. Negative control: the same test with `--remote-debugging-port=0` added fails on that
  check (`the browser listens on no TCP port: ["pid …: socket:[…]"]`). Without `--no-sandbox` as
  root, Chrome exits and the manager reports `ExitedEarly` with its stderr.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/browser/)'`: 21 passed.
  Preview tests: 25 passed.
- `scripts/dev security`: **not run** (exit 2). Vidoc answered HTTP 429: the instance key has used
  its 50,000,000 tokens. Unrun, not passed.
- Not tested here: the opt-in live display/UI tests (`live_display`, `live_ui_browser`, ignored;
  they need Xvfb, ffmpeg and a display stack). They compile against `ChromiumBrowsers::cdp`. A
  real Chromium on macOS was not run (`scripts/dev macos` compiles it).
