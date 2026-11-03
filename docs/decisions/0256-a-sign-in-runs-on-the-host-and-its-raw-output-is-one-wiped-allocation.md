# 0256. A sign-in runs on the host and its raw output is one wiped allocation

## Status

Accepted (2026-11-03, task #383, [SPEC-404](../../.ai/specs/SPEC-404-vidoc-triage-account-login-806-751-731-7.md)).
Refines SPEC-002 ADR-05 and [0184](0184-a-captured-token-ends-the-sign-in-and-a-closed-dialog-cancels-it.md).

## Context

A subscription sign-in runs `claude setup-token` in a server-owned PTY. The sign-in prints the
long-lived token once, so the server keeps a bounded raw copy of the output (`LOGIN_RAW_MAX`) to
extract it, and masks the token for viewers (`accounts.rs`).

Vidoc reported two kinds of finding about it:

- The sign-in inherits the server's `PATH` and runs without containment (#806, and #729 in other
  words).
- The raw copy is kept "without scrubbing" (#731).

On the second point, `finish_login` did zero the buffer it took. But the buffer was a `Vec` that
grew by doubling, so each growth freed an earlier allocation that held the output, unzeroed. Its
bound was kept by appending first and then dropping the oldest bytes, which left old output past
the end, where zeroing the contents does not reach.

## Decision

- **The sign-in runs on the host.** Its process gets the server's own `PATH`, a fixed argv
  (`claude setup-token`), new empty private HOME and `CLAUDE_CONFIG_DIR` folders (no settings, no
  hooks), and keystrokes from instance administrators only. `Containment::None` is deliberate.
- **The raw output is one allocation.** It is allocated at `LOGIN_RAW_MAX` when the sign-in
  starts and never grows. A chunk drops the oldest bytes before it is appended.
- **The allocation is wiped whole.** It is overwritten with zeros up to its capacity once the
  token has been extracted, and again when it is dropped. The lossy UTF-8 copy made for the
  extraction is scrubbed, and so is each raw output chunk once it has been copied and masked.
- All of this is safe code. `unsafe` is forbidden, and resizing to the capacity and then filling
  it reaches every byte.

## Alternatives

- **Bubblewrap containment for the sign-in.** Containment in Yulius is built per sandbox: its
  root, home and namespace holder. A sign-in belongs to no sandbox. It would need the `claude`
  install and its runtime bound read-only, and a way out to Anthropic, only to guard against an
  instance administrator typing into a code prompt. Rejected.
- **A fixed, server-chosen `PATH`.** It would miss a `claude` the operator installed elsewhere
  (docs/operations.md). And whoever can change the server's `PATH` already controls the server.
  Rejected.
- **The `zeroize` crate.** It adds a dependency for two lines of safe code here. Rejected.

## Consequences

- Each running or retained sign-in holds a 64 KiB allocation, at most 32 of them (2 MiB).
- After the sign-in ends, the raw output reads as zeros. Nothing reads it after that.
- Plaintext copies outside `accounts.rs` remain: the PTY client's read buffer, the masker's held
  bytes and the sealed `Secret`. The credential has to be kept anyway. The others are short-lived.
- A repeat of #806, #729, #751 or #730 in the same wording matches its `[[ignore]]` entry in
  `.config/vidoc.toml`.
