# 0321. A sandbox's browser goes out through a proxy of its own and reaches what its sandbox reaches

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 9, finding E2); task #465. Completes SPEC-001 BRW-008 (the sandbox browser never loads
Yulius itself) and applies the destination rule of SPEC-002 ADR-20 (the egress proxy) to the
browser. The firmer fix [0318](0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md)
names, Chrome inside its sandbox's namespaces, stays open.

## Context

A sandbox's processes live in the sandbox's network namespace: the host's loopback is not
theirs (`slirp4netns --disable-host-loopback`), and what they send out goes through the egress
proxy, which refuses loopback, link-local and other local destinations after resolving a name.
The sandbox's Chrome does not live there. It runs on the host as the service user, so that a
viewer's real input and the virtual display work (ADR-0008), and it connected by itself. Any
agent, or any page an agent opens, could therefore load through the browser tools what no
process of its sandbox reaches:

- **other sandboxes' dev servers**: the host relays each sandbox's port range from its own
  loopback, so `http://localhost:<port of another project>` showed that project's work;
- **the host's own services** on the loopback, and a cloud's metadata address
  (`169.254.169.254`), whose answer holds the machine's credentials;
- **Yulius's internal listener** by any name the address guard does not list.

The guard (BRW-008) is a list of URL patterns handed to Chrome's `Fetch` interception:
Yulius's public origin and every listed loopback spelling of its listeners. Patterns match
text. A name that resolves to the loopback, another sandbox's port or a link-local address
matched none. Proven on Google Chrome 157 with this slice's opt-in test and the proxy switched
off: a browser of sandbox `s1` showed the page of a server in `s2`'s ports.

What Chrome does with a proxy was measured on Chrome 157: `--proxy-server` alone leaves
`localhost` and `127.0.0.1` out (they went to the origin directly); with
`--proxy-bypass-list=<-loopback>` they go to the proxy like every other address.

## Decision

- **Every browser behind a proxy of its own.** `BrowserConfig::network` (a `BrowserNetwork`)
  opens, for each browser that starts, an HTTP proxy on the loopback. Chrome is started with
  it for every address (`proxy_args`: `--proxy-server`, `--proxy-bypass-list=<-loopback>`),
  resolves no name itself (`--host-resolver-rules=MAP * ~NOTFOUND`), and sends no UDP past it
  (WebRTC's non-proxied UDP and QUIC are off). The proxy lives as long as the browser: a stop,
  a crash's restart and a failed start close it.
- **No proxy, no browser.** When the proxy cannot be opened (the egress proxy is not running
  yet, no port), the browser is not started (`BrowserError::Network`). A browser that connects
  by itself is never the fallback.
- **The listener says whose browser asks.** A browser sends a proxy no credential of its own
  accord, and answering a challenge over DevTools would pause every request of every page. So
  each sandbox's browser has its own listener, served by the egress proxy as that sandbox's
  browser (`EgressProxy::handle_browser`, `serve_browser`, wired by `BrowserProxies`).
- **It reaches what its sandbox's processes reach** (`BrowserEgress::reaches`, applied to the
  addresses the proxy itself resolved and then connects to):
  - every address the egress proxy lets an agent run reach (not loopback, unspecified,
    link-local, multicast or broadcast, nor their IPv4-mapped forms);
  - on the loopback, the ports of its own sandbox, which the host relays into the sandbox (a
    dev server at `localhost:<port>` works as before);
  - of Yulius's internal listener, the welcome page and its port list, by plain request only:
    the proxy reads the path, and a tunnel to that listener, whose path it could not read, is
    refused; of its public listener, at that listener's own address, nothing, whatever the
    sandbox's ports are.
- **A name is not an address.** The loopback's ports and the welcome page are reached only by
  an address written out or by `localhost` and the names below it, which are the loopback by
  definition and are not asked of a resolver (hosts without one for them exist). A name a
  resolver answers with a local address reaches nothing: its owner chooses the answer, and
  could give a public address for a page and the loopback for that page's next request, which
  the page would then read as its own origin. Chrome's own defence against that does not
  apply here, since every connection it makes ends at the proxy.
- **Nothing of an agent run's proxy applies.** No interception, no secret added, no answer
  redacted or judged by its encoding, and the policy of agent runs is not asked: the page is
  the sandbox's, and a secret injected into a page's requests would be the page's script's to
  use. A browser's connections count toward its sandbox's share of the proxy (SPEC-353) as one
  more run, and a plain request waits for its answer as an agent's does (SPEC-369: the bound
  restarts while its body is still being sent).
- **The address guard stays.** It keeps Yulius's public origin and previews out of its own
  browser, which is not a matter of local addresses, and answers before a connection is made.

## Alternatives

- **Chrome inside the sandbox's namespaces** (entering the holder's user and network
  namespace, or bubblewrap). Then the browser's network is the sandbox's by construction, and
  Chrome's own windows reach no host file either. It needs Chrome's own sandbox to work
  inside another user namespace, the display and its encoder to cross the boundary, and a
  host to prove it on; nothing of that can be shown inside a sandbox, where namespaces cannot
  be made. A hole this plain should not wait for it. The proxy stays useful under it (the
  welcome page, the share).
- **More patterns in the address guard.** Text again: the next spelling, a name someone
  registers that resolves to `127.0.0.1`, an address that changes between the check and the
  connection. Only what resolves and connects in one place can say where a request goes.
- **Checking the address in the browser tools** (`browser_open`, `browser_navigate`). A page's
  redirects, frames, scripts and a viewer's address bar never pass the tools.
- **The internal listener as the proxy, with the run's token.** Chrome asks the user for proxy
  credentials; over DevTools it would mean intercepting every request to answer the
  challenge, and requests to the listener's own authority are the internal API, not proxied.
- **A browser kind among the proxy's callers** (`EgressCaller`). Every rule of agent runs
  (which secrets apply, whose personal ones, liveness of a run) would need a branch saying
  "not for a browser". A separate entry that never asks the policy cannot get one wrong.
- **Refusing the host's private network too** (`10.0.0.0/8` and the like). A sandbox's
  processes reach it, so the browser reaching it gives an agent nothing new, and dev setups
  use it. The rule is the egress proxy's, in one place, if that changes.

## Consequences

- An agent, a page or a viewer no longer reaches through the sandbox browser: another
  sandbox's ports, a service on the host's loopback, a link-local address, or Yulius's
  internal listener beyond the welcome page. A plain request there is answered 403 with the
  sandbox's own port range named; a tunnel (HTTPS, a WebSocket) fails to connect.
- A server inside the sandbox on a port outside its range was never reachable from the
  browser (the host relays the range only) and now says so instead of showing whatever the
  host has on that port.
- Names are resolved by Yulius, not by Chrome: a host name that only Chrome's own resolver
  features knew (DNS over HTTPS set in the profile) resolves as the host's resolver says.
- WebRTC works only where it gets through a proxy (TCP), and QUIC is off: pages fall back to
  HTTP/2 or HTTP/1.1.
- Chrome's own background requests (updates, component downloads) go through the proxy too
  and count toward the sandbox's share.
- Any local process of the host can use a browser's proxy while that browser runs: it gives
  the internet, which the host has anyway, and one sandbox's dev ports, which the host has on
  its loopback anyway. Sandboxed runs cannot reach it (it is on the host's loopback).
- A developer's machine without sandboxes gets the same rule: the project's port range on the
  loopback, not the rest of the machine's.
- A dev server reached by a name that points at `127.0.0.1` (a wildcard DNS service, an entry
  in a DNS zone) no longer loads; `localhost`, a name below it or the address does.
- A managed Chrome policy on the host that sets a proxy (`ProxyMode`, `ProxySettings`) wins
  over the switches and would undo this; Yulius's own policy file sets none, and
  `docs/operations.md` says not to add one.
- A browser's proxy takes a free port of the host. New sandboxes' port ranges are given from
  20000 upward and are never given again, so after some hundreds of sandboxes they reach the
  ports the system hands out, and a proxy can sit on a port of another sandbox's range, which
  is then not relayed while that browser runs (issue #1249; the allocation is older than this
  change). A page cannot use such a listener as a proxy of its own: what reaches it through
  another browser's proxy is a path-only request or a tunnel's bytes.
- Not covered: what a viewer or page does to the public internet from the host's address
  (as before), and Chrome's own windows (ADR-0318's policy).
- Not tested: WebTransport and other protocols over UDP (the switches turn QUIC and
  non-proxied WebRTC off; their effect was not measured).
