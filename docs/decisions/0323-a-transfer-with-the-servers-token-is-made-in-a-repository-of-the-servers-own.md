# 0323. A transfer that carries the server's token is made in a repository of the server's own

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 11, finding G2); task #465. Completes
[0322](0322-the-hosts-own-git-runs-confined.md), which confined what the server's Git executes
and touches and left the connection of its transfers open.

## Context

The server fetches from GitHub and pushes to it with a token of its own, passed to Git as a
header in the adapter's configuration (`http.https://github.com/.extraheader`, through
`GIT_CONFIG_*`), never on the command line or on disk. It ran those transfers in the sandbox's
clone, by the remote's name. A clone's configuration is its agents' to write, and Git gives a
key for a URL precedence over a general one whatever its source: with
`http.https://github.com/.proxy` and `http.https://github.com/.sslVerify=false` in the clone,
`git config --get-urlmatch` under the server's command line and environment answered the
clone's proxy and `false`. The connection, token included, would go through a proxy the agents
run on one of their sandbox's ports, with no certificate check. Confinement
(ADR-0322) does not change that: nothing is executed, and the network is not confined.

The token was also in the environment of every other command the adapter ran with it (the
merge after a fetch, a listing), in a folder whose configuration the agents write.

## Decision

- **A courier.** A transfer of a sandbox's clone with a remote on the web is made in a bare
  repository the server makes for that one transfer outside every sandbox's folder
  (`<data>/git-couriers/<id>-…`, `yulius_git::courier`), whose configuration nobody else
  writes. It borrows the clone's objects as an alternate, so nothing is copied to make it.
  - **Fetch:** the courier takes the clone's remote-tracking refs (a transfer between two
    folders, no token), fetches the remote's branches from the address with the adapter's
    configuration, and the clone takes them from the courier (no token), pruning as before.
  - **Push:** the clone's commit is pushed from the courier by its id; never forced.
  - **Listing** the remote's branch is asked from the courier.
  The courier is removed when the transfer ends; one left by a server that stopped is removed
  by a later transfer after a day.
- **The address is the clone's, the connection is not.** The remote's address is still read
  from the clone (`remote.<name>.url`), as Git reads it. Only an `https:` or `http:` address
  goes through a courier; the header is for GitHub's address only, so another host gets no
  token. A folder is taken only when it is the sandbox's own origin (ADR-0322).
- **No command in a sandbox's folder gets the adapter's configuration.** The runner leaves
  `GIT_CONFIG_*` out for every command whose folder is a sandbox's (and for transfers between
  folders): the token is in the environment of the courier's commands and of commands in
  folders of the server's own (a new clone, a CD mirror, a playground's origin), nowhere else.
- **The borrowed objects must be the sandbox's.** Where the clone says its objects are
  (`git rev-parse --git-path objects`) counts only inside the sandbox's own folders;
  otherwise no courier is made.
- **Nothing is started with an argument of the clone's** (from the review of ADR-0322's
  change). The program loader is among what a confined Git may execute, and the loader runs
  any file it is given: one of the clone's. Git starts a configured single word directly,
  with its arguments, so every way to get one started with a file's path is switched off for
  every command, in one place (`runner`): `--no-ext-diff --no-textconv` on every command that
  can make a diff, no `ssh` or `git` transport, no signing and no showing of signatures, no
  editor, no command for an alternate's refs.

## Alternatives

- **The server's own keys for the exact address.** A key for a longer URL prefix beats them,
  and the address itself is the clone's.
- **A lasting mirror per sandbox** instead of a courier per transfer. It would hold every
  object twice and need a lifecycle of its own (quota, removal, repair); the alternate makes
  a courier cost a few files.
- **The address from the application** (the sandbox's record) instead of the clone's
  configuration. It would also stop a clone pointed at another GitHub repository; that is a
  change of four port methods and every caller, and gives no secret away today (the token
  goes to GitHub either way). Left for the slice that looks at what the token may do.
- **Running the transfer inside the sandbox.** The token would be where the agents are.
- **A credential helper instead of a header.** Git starts helpers through the shell, which a
  confined Git cannot execute, and the clone's `credential.*` keys would apply.

## Consequences

- A fetch is three Git commands and a push two instead of one each; the courier is a
  repository of a few files for their time.
- `<data>/git-couriers` exists on a server; it is empty between transfers.
- A clone whose configuration names a proxy or its own certificates for GitHub no longer
  affects the server's transfers; the agents' own Git, in the sandbox, uses them as before.
- A merge after a fetch, a commit, a diff and every other command in a sandbox's folder run
  without the token in their environment.
- A clone that says its objects are elsewhere (a worktree whose `commondir` was rewritten)
  gets no transfer with a remote on the web.
- A repository whose diff needs a text conversion or an external program shows without it in
  the server's pages (as before for most of them), and a signed-commits setting of the clone
  does not make the server's own commits signed.
- Not confined, as before: Git on macOS and in tests of the adapter's crate runs transfers
  where it always did.
- Not covered: what the server's token may do at GitHub when the clone's address names
  another repository there (the address is still the clone's).
