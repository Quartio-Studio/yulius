# 0456. The tunnel custodian runs the departing server's own image

## Status

Accepted for SPEC-617.

## Context

On an orderly Linux restart the departing server hands its established opaque tunnels to a short-lived custodian, `yulius sandbox-host hold-tunnels`, which holds them for the successor (SPEC-577). The server started the custodian from the executable path that it resolved at startup. When that file was missing or still being written at shutdown, the spawn failed with `NotFound` or `ExecutableFileBusy`. The handoff was then dropped and every tunnel closed. In issue #1676 this happened during a concurrent build that rewrote `target/debug/yulius`. A package upgrade or a manual install can also change the file at that path before the service restarts. The binary at the path can also be a different version from the server that deposits.

## Decision

The departing server starts the custodian from `/proc/self/exe`, the image it is running. The custodian is always the depositor's own build. Removing, replacing or rewriting the install path no longer affects the handoff. The library function `custodian::deposit` still takes the executable as a parameter, so tests can supply the built binary.

## Alternatives

Re-resolving `current_exe()` at shutdown fails the same way once the path is unlinked, because Linux reports it as deleted. Copying the binary to a private location at startup costs disk space and time on every start. Retrying the spawn would only hide a missing or half-written file.

## Consequences

The update helper stops the service before it renames the new binary into place, so updates hand over exactly as before. After a package upgrade that replaced the binary while the server ran, the old server's own build holds the tunnels. The new server then adopts them through the custodian protocol, as it already did after an update. `/proc` must be mounted for the server, as it already is for its other Linux features.
