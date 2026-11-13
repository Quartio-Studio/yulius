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

- **A courier.** A transfer of a sandbox's clone with a remote that is not the sandbox's own
  origin folder is made in a bare repository the server makes for that one transfer outside
  every sandbox's folder (`<data>/git-couriers/<id>-…`, `yulius_git::courier`), whose
  configuration nobody else writes. It borrows the clone's objects as an alternate, so
  nothing is copied to make it.
  - **Fetch:** the server lists the clone's remote-tracking refs and writes them into the
    courier itself (a `packed-refs` file: data, no process of the clone's works in the
    courier); the courier fetches the remote's branches from the address with the adapter's
    configuration; the clone takes them from the courier (a transfer between two folders, no
    token), pruning as before. A clone without a note of the remote's default branch
    (`refs/remotes/<remote>/HEAD`) gets one, as a fetch by the remote's name gives it; the
    courier asks the remote for it. Every step with the token comes before the clone's Git
    works with the courier.
  - **Push:** the clone's commit is pushed from the courier by its id; never forced.
  - **Listing** the remote's branch is asked from a courier that borrows nothing.
  - **A new clone** in a sandbox's folder is made in a courier and copied from there (a
    clone between two folders), then given the remote's address: a new repository's
    configuration is written before its objects arrive, and the sandbox's agents could add
    to it meanwhile.
  The courier is removed when the transfer ends; one left by a server that stopped is removed
  by a later transfer after a day.
- **The address is the clone's, the connection is not.** The remote's address is read from
  the clone: `remote.<name>.url`, the last value when there are several. The other keys Git
  would make an address of (`url.<base>.insteadOf`, `pushurl`, `pushInsteadOf`, a file in
  `.git/remotes`) are not read: the server's transfers go to that one address. A remote that
  has none is refused in a sandbox's clone; it is not named to Git, which would find an
  address by those keys and make the transfer in the clone. Every address that is not the
  sandbox's own origin folder (ADR-0322) goes through a courier, whose commands start no
  shell: another folder of the machine is neither served nor reached. The header is for
  GitHub's address only, so another host gets no token.
- **No command in a sandbox's folder gets the adapter's configuration.** The runner leaves
  `GIT_CONFIG_COUNT` and its numbered keys and values out for every command whose folder is
  a sandbox's, for a command without a folder that writes into one (a clone), and for
  transfers between folders: the token is in the environment of the courier's commands and
  of commands in folders of the server's own (a scratch clone in `<data>/changes`, a CD
  mirror), nowhere else.
- **What the server itself commits and pushes is made in a folder of its own.** The first
  commit of an empty repository was made in a scratch clone in the sandbox's folder, where
  the sandbox's agents could add to what it holds; it is made in `<data>/changes` now, as a
  change from the remote control is. A new project's scaffold stays in the folder of the
  sandbox that is being made: it has no record yet, so nothing of it runs.
- **The borrowed objects must be the sandbox's.** Where the clone says its objects are
  (`git rev-parse --git-path objects`) counts only inside the sandbox's own folders, both
  sides with their links followed (a data folder may be a link), and only when the path is
  one line of the file that lends it; otherwise no courier is made.
- **Nothing is started with an argument of the clone's** (from the review of ADR-0322's
  change). The program loader is among what a confined Git may execute, and the loader runs
  any file it is given: one of the clone's. Git starts a configured single word directly,
  with its arguments. The ways known to get one started with a path of the clone's choosing
  are switched off for every command, in one place (`runner`): `--no-ext-diff
  --no-textconv` on every command that can make a diff, no `ssh` or `git` transport, no
  signing and no showing of signatures, no editor, no command for an alternate's refs, and
  no program to ask for a password (`GIT_ASKPASS` is set and empty, which comes before a
  clone's `core.askPass`: that program would be started with the question, which holds the
  address). This is a list of what Git does today, not a property of Git: a later Git that
  starts another configured word needs another line here.
- **Where the shell may be started, the clone's configuration starts nothing through it.**
  A transfer between two folders may start the shell (ADR-0322), and in a sandbox's clone
  that command reads the clone's configuration. Three ways it made the shell run a command
  of the clone's were found in this change and its review, and each is switched off: the
  fetch of a missing object (next point); a push that goes into submodules
  (`push.recurseSubmodules`, `submodule.recurse`: a submodule is pushed by its own
  configuration, with its own command for the other side), pinned off for every command;
  and a filter for the files of a new clone, added to its configuration while the clone is
  made: a clone into a sandbox's folder checks nothing out (`--no-checkout`), and its files
  are checked out by a command of their own, in the clone, which may start no shell. This
  too is a list, and the commands it covers are few (a fetch, a push, a listing and a clone
  without files, each with a folder Yulius names): see Consequences for what replaces it.
- **A missing object is missing.** A clone can name a "promisor" remote that Git fetches
  missing objects from, with a command for the other side; in a transfer between two
  folders, where a confined Git may start the shell, the shell ran that command (a push of
  a commit whose tree no repository has). Every command runs with `GIT_NO_LAZY_FETCH=1`.
  Git knows the variable since the releases of May 2024 (2.45.1 and the maintenance
  releases beside it; Ubuntu 24.04's 2.43.0 has it); `yulius doctor` asks the host's Git and
  fails a host whose Git fetches the object all the same.

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

- A fetch through a courier is six Git commands instead of one (the clone's address, where
  its objects are, the courier's `init`, the clone's refs, the fetch with the token, the
  clone's fetch from the courier), and two more for a clone without a note of the remote's
  default branch; a push is five. The courier is a repository of a few files for their time.
- A first clone copies the repository twice on the machine's disk for its time (files are
  linked, not copied, where the file system allows).
- `<data>/git-couriers` exists on a server; it is empty between transfers.
- A clone whose configuration names a proxy or its own certificates for GitHub no longer
  affects the server's transfers; the agents' own Git, in the sandbox, uses them as before.
- A merge after a fetch, a commit, a diff and every other command in a sandbox's folder run
  without the token in their environment.
- A clone that says its objects are elsewhere (a worktree whose `commondir` was rewritten)
  gets no transfer with a remote on the web, and a clone whose `origin` has no address of
  its own gets none at all.
- A clone that is a partial one (objects left at the remote on purpose) is not completed by
  the server's Git: its commands fail on the missing object. Yulius makes no such clone.
- A repository whose diff needs a text conversion or an external program shows without it in
  the server's pages (as before for most of them), and a signed-commits setting of the clone
  does not make the server's own commits signed.
- **Open, finding G5 of SPEC-483:** a command that reads a clone's configuration can still
  start the shell (a transfer between a clone and its playground's origin, the clone's fetch
  from a courier, a new clone's copy). What that configuration could make the shell run is
  switched off case by case above, by what Git does today. The rule that needs no list is
  that no such command may start the shell at all: the transfers made from the server's
  side, with the clone's objects shared and its refs written as data. That is the next
  change.
- **Not confined, G2 stays open:** Git on macOS (a developer's machine) and in tests of the
  adapter's crate runs transfers where it always did, with the token, in the clone. The
  courier and the rule on the adapter's configuration need the confinement's knowledge of
  which folder is a sandbox's.
- Not covered: what the server's token may do at GitHub when the clone's address names
  another repository there (the address is still the clone's).
