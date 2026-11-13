# 0322. The host's own Git runs confined: Git's programs only, its sandbox's folders only

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 10, finding G1); task #465. Closes what
[0314](0314-the-host-starts-bubblewrap-by-its-own-path-and-writes-git-files-through-handles.md)
named as the remaining hole of
[0114](0114-agents-hold-a-token-github-refuses-on-production.md) (#54 part B), for what Git
executes and for the files it reads and writes. The connection of a transfer that carries the
server's token is finding G2, a slice of its own.

## Context

The server runs Git as the service user, outside every containment: status and diffs for the
pages, worktrees, commits of its own, fetches, pushes and merges for delivery. It runs it in a
sandbox's clone and worktrees, where every file is the agents' to write, `.git/config`
included, at the same paths on the host. Three constant options were its hardening
(`protocol.ext.allow=never`, `core.fsmonitor=false`, `core.hooksPath=/dev/null`).

A repository's configuration can make Git run a command in many more ways, and a file of the
repository can be a link. The audit ran the server's adapter, as it was, against such a clone
(`git_confined` tests with the confinement taken out):

- the server's Git ran the commands the clone named as `filter.<name>.process`,
  `core.sshCommand` (with the remote's address changed to an `ssh:` one), and the remote's
  `uploadpack` and `receivepack`: as the service user on the host, with the environment of
  the server's Git, which for a fetch holds the token;
- a fetch wrote Git's `FETCH_HEAD` through a link over a file outside the sandbox (the
  instance's key or database, as far as Git is concerned);
- a link at `.git/shallow` made Git repeat a line of a file outside in its error text;
- a remote address changed to another sandbox's origin or clone made the server push the
  branch there and merge that project's work into the worktree.

Git offers no way to ignore a repository's own configuration, and the list of keys that run
something grows with Git (filters, diff and merge drivers, text conversion, editors, signing
programs, credential helpers, proxies, the other side of a transfer, a command to list an
alternate's refs).

## Decision

- **Every Git command of the server is started through `yulius git-confined`**
  (`yulius_git::confine::exec_confined`, the adapter's `Confinement`), which first takes from
  its own process, with the kernel's Landlock, what Git does not need, and then becomes Git.
  The restriction is inherited by everything Git starts and cannot be lifted.
  - **Execute:** Git's executable (found by the server's `PATH`, links followed), the
    programs of its `exec-path` and the program loader. Nothing else: a command from the
    configuration is run through `/bin/sh`, which is not among them, and a single word names
    a program, which is not either.
  - **Write:** the folders the command works in, and `/dev/null`.
  - **Read:** those folders, the system's programs, libraries and configuration (`/usr`,
    `/lib*`, `/bin`, `/sbin`, `/etc`, the resolver's files) and the service user's own Git
    configuration. Not the instance's data, not another sandbox.
- **The folders a command works in are its sandbox's.** For a path in
  `<data>/sandboxes/<id>` or `<data>/import-worktrees/<id>`, both folders of that id (clone,
  task worktrees, import worktrees). For a path elsewhere (a playground's origin, a CD
  mirror, a bundle), that folder, or the nearest existing one for what is still to be made.
  What a command touches besides its own folder is named where the command is made
  (`Invocation::touching`): a clone's destination, a worktree's path, a bundle's file, the
  other repository of a transfer.
- **The shell only for a transfer between two folders that Yulius names**
  (`Invocation::local`). Git starts the other side of such a transfer as
  `sh -c "git-upload-pack '<path>'"`, so there the shell may be executed (and still no other
  program). For that command the transports are pinned to `file` (a rewritten address cannot
  make it an `ssh:` one, whose command the configuration names), the command for listing an
  alternate's refs is emptied, and none of the adapter's own configuration is passed, which
  is where a token travels.
- **A remote's name stands for a folder only when it is the sandbox's own origin.** A
  transfer by a remote's name (`origin`) reads the address from the clone's configuration,
  which the agents write. An address that is a folder of this machine is taken only when it
  is `<data>/playgrounds/<id>.git` for the sandbox `<id>` the command works in; the command
  then names that folder itself, so the remote's own settings (a program for the other side)
  do not apply. Any other folder is not reached and the shell is not allowed: the transfer
  fails. A playground's scratch clone pushes to its origin by a path the application gives
  (`GitPort::push_to_folder`).
- **No Landlock, no Git.** On a kernel that does not enforce Landlock the launcher refuses
  to start Git and says why; `yulius doctor` checks it ("git confinement"). macOS, where
  runs are not contained either, runs Git as before.
- The three constant options stay: they answer before a process is started.

## Alternatives

- **More constant options.** The list is Git's to extend; drivers are named by the
  repository (`filter.<any name>.clean`), so their keys cannot be listed at all; and a link
  is no configuration.
- **Reading the configuration first and refusing dangerous keys** (or scrubbing them). The
  agent writes the file again between the look and the command.
- **Running the server's Git inside the sandbox's containment.** What the configuration runs
  would then run where the agent already is. But the token the server's fetch carries would
  be there too, the sandbox's holder would have to run for every status, and it cannot be
  shown inside a sandbox. The token is G2's subject.
- **A Git directory of the server's own for each clone** (trusted configuration, the clone's
  objects). The right shape for transfers that carry a token (G2); for the working tree
  commands (status, diff, merge, checkout) Git reads the clone's own files whatever the
  directory.
- **Allowing the shell everywhere and confining only files.** A shell with Git is a network
  client on the host, and a filter's script would run with the token in its environment.
- **Trusting the address a clone's configuration gives when it is a folder.** It names
  another sandbox's origin as easily as its own.
- **seccomp instead of Landlock.** It filters system calls, not paths: "may execute this
  file, not that one" is what Landlock says directly, and it needs no privilege.

## Consequences

- A clone whose configuration names a filter, a diff or merge driver, a signing program or
  the like makes the server's command that would run it fail (or go on without it, as Git
  decides): the Git page, a commit of Yulius's own or a merge in that worktree says so. The
  agent's own Git, inside its sandbox, runs them as before.
- A host needs Linux 5.13 or later with `landlock` among its security modules (Ubuntu 22.04
  and later and Debian 12 have it). Without it every Git command of the server fails with
  that reason, and `yulius doctor` reports it.
- One more process per Git command (the launcher, which becomes Git).
- A clone whose `origin` is a folder other than its sandbox's own is not fetched or pushed by
  the server. Sandboxes of GitHub repositories and playgrounds are unchanged.
- Git on a path the service user's `PATH` does not name, or installed outside `/usr`, is
  found by `PATH` and its `exec-path`; a build that keeps other helpers elsewhere fails
  there, and the doctor's check shows it.
- Still open, finding G2: for a transfer with GitHub, the clone's configuration still shapes
  the connection that carries the server's token (`http.<url>.proxy`, `http.<url>.sslVerify`:
  a key for a URL beats the command line), so a clone can send that connection through a
  proxy of its own without a certificate check.
- Tests' stand-ins for GitHub are folders, so their adapter allows the shell in every
  command (`with_local_origins`, `with_github_base`); the tests of this decision use the
  adapter as the server builds it.
