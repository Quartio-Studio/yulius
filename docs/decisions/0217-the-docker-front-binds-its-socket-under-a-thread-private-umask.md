# 0217. The Docker front binds its socket under a thread-private umask

## Status

Accepted. Specified in [SPEC-304](../../.ai/specs/SPEC-304-front-start-s-umask-race-under-in-proces.md);
task #290, issue #825. Refines how [0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md)'s
front creates `docker.sock`.

## Context

- The front's socket must be 0660 and is created that mode by the umask, never chmod'ed by name:
  the engine (and a container binding the directory) can write `<state>/docker/` and could put a
  link there between `bind` and `chmod`.
- `Front::start` set the process umask to 0o117, bound and restored it. The umask is shared by
  every thread of a process. Fronts started at once in one process (the crate's unit tests under
  `cargo test`) raced: a socket was bound under another call's restored mask (0700 or 0600), and
  the process could be left with 0o117, so later directories lacked their x bits.
- In production each front is its own `yulius sandbox-host docker` process that starts one front
  before serving anything, so only tests raced; but any thread creating a file while the mask was
  set would get it.

## Decision

- The socket is bound on a short-lived scoped thread that first calls `unshare(CLONE_FS)`: the
  thread gets its own copy of the umask (with cwd and root), sets 0o117 there and binds. The
  process umask is never changed. The bound listener goes back to the caller and to tokio.
- Thread creation failing is an error of `Front::start` (the front prints it), not a panic.
- On other systems (a development Mac running the tests) the umask stays the process's, set and
  restored around the bind as before: the front itself runs only on Linux, inside a holder's
  namespaces.

## Alternatives

- A process-wide mutex held across set/bind/restore: serializes fronts, but any other thread
  creating a file in the window still gets 0o117.
- Binding in a fresh private subdirectory, `chmod`, then renaming to `docker.sock`: needs a `chmod`
  by name, which a writer with the same uid (the engine) could redirect with a link. Doing it
  without following links takes `fchmodat2` (Linux 6.6) or libc's emulation through an `O_PATH`
  descriptor and `/proc/self/fd`: workable, but more moving parts than a private umask.
- `fchmod` on the socket descriptor: on Linux it changes the socket's own inode, not the file.

## Consequences

- On Linux, fronts started concurrently in one process each get a 0660 socket, and other threads
  keep the process umask; `cargo test -p yulius-sandbox --lib` no longer races on it. On a Mac
  the old race remains for in-process tests (the regression test is Linux-only).
- One thread spawn per front start (once per front process).
- A future helper that must create a file with a mode the directory's writers must not redirect
  can reuse this pattern instead of the process umask.
