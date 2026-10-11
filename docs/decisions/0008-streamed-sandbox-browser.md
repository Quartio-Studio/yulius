# 0008. The sandbox browser is the real Chrome streamed as H.264

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-09, ADR-14, ADR-15); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

A DOM mirror and screencast were slow and broke captchas; users must help agents through captchas and sign-ins.

## Decision

A headful Chrome per sandbox on its own Xvfb, shown as H.264 (ffmpeg x11grab, libx264) over a WebSocket and decoded by WebCodecs; user input is real (XTest); CDP stays for agents. Preview origins remain only to open a dev server live in a new window.

## Alternatives

Fixing the DOM mirror; CDP screencast only; VNC/noVNC; WebRTC; in-process encoder.

## Consequences

Video-latency scrolling and working captchas. Needs `xvfb` and `ffmpeg`; about one core per watched browser; the page is pixels. Falls back to headless with JPEG frames.
