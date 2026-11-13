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

Two rules, and one mechanism that keeps both.

- **No command that reads a clone's configuration carries the server's token.** The runner
  leaves `GIT_CONFIG_COUNT` and its numbered keys and values out for every command whose
  folder is a sandbox's, for a command without a folder that writes into one (an `init`),
  and for transfers between folders: the token is in the environment of a courier's commands
  and of commands in folders of the server's own (a scratch clone in `<data>/changes`, a CD
  mirror), nowhere else.
- **No command that reads a clone's configuration may start the shell.** A transfer between
  two folders of the machine starts the other side through the system's shell, so ADR-0322
  allowed the shell to such a transfer, also when it ran in the clone. There the shell ran
  what the clone's configuration names for a transfer. Three such ways were found in this
  change and its review, one after the other (the command of a "promisor" remote that Git
  fetches a missing object from; the command of a submodule's remote, in a push that goes
  into submodules; a helper for the password of a bundle that `fetch.bundleURI` downloads
  before a fetch, whatever transports the command allows) and a fourth in a new clone (a
  filter added to its configuration while it is made). A list of what Git starts today does
  not hold, so there is none: the runner gives no command in a sandbox's folder the shell,
  and refuses a transfer between folders there before anything is started.
- **A courier** makes every transfer of a sandbox's clone, with GitHub and with a
  playground's origin folder alike. It is a bare repository the server makes for that one
  transfer outside every sandbox's folder (`<data>/git-couriers/<id>-…`,
  `yulius_git::courier`), whose configuration nobody else writes. Its commands work on the
  clone's objects (`GIT_OBJECT_DIRECTORY` names the clone's object store, and the courier's
  `shallow` file is a link to the clone's), so nothing is copied and what a fetch brings is
  in the clone at once. Refs travel as data.
  - **Fetch:** the server lists the clone's remote-tracking refs and tags and writes them
    into the courier (a `packed-refs` file), so that only what is new is asked for; the
    courier fetches the remote's branches from the address (with the adapter's
    configuration for an address on the web; as a transfer between folders, with the shell
    and without the token, for the origin's folder); the server lists the courier's refs
    and has the clone's Git set them (`update-ref --stdin`, in two steps as a fetch does
    it: the branches the remote dropped are deleted first, so that `a/b` can take the place
    of `a`; then a moved branch is set, a new one and a tag the clone has none of made; each
    change names what the clone had, so a ref that changed meanwhile is not set back). A
    clone without a note of the remote's default branch (`refs/remotes/<remote>/HEAD`) gets
    one, asked from the remote.
    The courier's fetch tidies nothing (`gc.auto=0`, no maintenance): the objects are the
    clone's, and the courier knows only the remote's branches.
  - **Push:** the clone's commit is pushed from the courier by its id; never forced.
  - **Listing** the remote's branch is asked from a courier that works on no objects.
  - **A folder is written only by a push.** A fetch and a listing with a playground's
    origin read that folder and may write nothing there (the launcher's read-only reach):
    the courier's `shallow` file is a link to the clone's, a file of the agents', and with
    a link of their own in its place Git, which follows it, would have written where the
    history ends into the origin.
  - **A new clone** in a sandbox's folder is the sandbox's agents' to write from its first
    file on, and `git clone` connects, and checks files out, in a repository whose
    configuration it has already written. So it is not made by a transfer: it is an empty
    repository with the address as its origin's, filled by a fetch through a courier
    (every tag with it), whose default branch is then checked out by a command in the clone
    like any other. As with `git clone`, the folder is new or empty, and a clone that fails
    leaves nothing of itself: one that kept its `.git` would be taken for a clone to
    refresh, and never be checked out.
  The courier is removed when the transfer ends; one left by a server that stopped is removed
  by a later transfer after a day.
- **What still starts the shell** is a transfer between two folders made in a folder of the
  server's own: a courier with a playground's origin, a CD mirror or a new origin taking
  commits from a clone or an upload. Of a clone, such a transfer runs only Git's serving of
  a fetch (`upload-pack`), which Git keeps safe for a repository it does not trust and which
  starts nothing configured.
- **The address is the clone's, the connection is not.** The remote's address is read from
  the clone: `remote.<name>.url`, the last value when there are several. The other keys Git
  would make an address of (`url.<base>.insteadOf`, `pushurl`, `pushInsteadOf`, a file in
  `.git/remotes`) are not read: the server's transfers go to that one address. A remote that
  has none is refused in a sandbox's clone. A folder is transferred with only when it is the
  sandbox's own origin (ADR-0322) or the caller names it; with any other address the
  courier's commands start no shell, so another folder of the machine is neither served nor
  reached. The header is for GitHub's address only, so another host gets no token.
- **What the server itself commits and pushes is made in a folder of its own.** The first
  commit of an empty repository was made in a scratch clone in the sandbox's folder, where
  the sandbox's agents could add to what it holds; it is made in `<data>/changes` now, as a
  change from the remote control is. A new project's scaffold stays in the folder of the
  sandbox that is being made: it has no record yet, so nothing of it runs.
- **The objects must be the sandbox's.** Where the clone says its objects and its `shallow`
  file are (`git rev-parse --git-path`) counts only inside the sandbox's own folders, both
  sides with their links followed (a data folder may be a link); otherwise no courier is
  made. The courier's Git is confined to those folders besides (ADR-0322).
- **Nothing is started with an argument of the clone's** (from the review of ADR-0322's
  change). The program loader is among what a confined Git may execute, and the loader runs
  any file it is given: one of the clone's. Git starts a configured single word directly,
  with its arguments, and that needs no shell. The ways known to get one started with a
  path of the clone's choosing are switched off for every command, in one place (`runner`):
  `--no-ext-diff --no-textconv` on every command that can make a diff, no `ssh` or `git`
  transport, no signing and no showing of signatures, no editor, no command for an
  alternate's refs, and no program to ask for a password (`GIT_ASKPASS` is set and empty,
  which comes before a clone's `core.askPass`: that program would be started with the
  question, which holds the address). This one is a list of what Git does today: a later
  Git that starts another configured word with a path needs another line here.
- **In depth, for every command:** no fetch of a missing object (`GIT_NO_LAZY_FETCH=1`;
  Git knows the variable since the releases of May 2024, 2.45.1 and the maintenance
  releases beside it, and Ubuntu 24.04's 2.43.0 has it; `yulius doctor` asks the host's Git
  and fails a host whose Git fetches the object all the same), no going into submodules
  (`push.recurseSubmodules=no`, `submodule.recurse=false`), no credential helper, no bundle
  address, no push that fetches first. With the rule on the shell none of these is what
  keeps a clone's command from running; each is one thing less a command in a clone does
  with an address of the clone's.

## Alternatives

- **The server's own keys for the exact address.** A key for a longer URL prefix beats them,
  and the address itself is the clone's.
- **A lasting mirror per sandbox** instead of a courier per transfer. It would hold every
  object twice and need a lifecycle of its own (quota, removal, repair); working on the
  clone's objects makes a courier cost a few files.
- **The courier borrowing the clone's objects as an alternate, and the clone fetching from
  the courier.** The first shape of this change. The clone's fetch from the courier is a
  transfer between folders made in the clone: the shell, and the clone's configuration.
- **Switching off what a clone's configuration can make the shell run, one by one.** The
  second shape. Each round of the review found another.
- **Local transfers without the shell** (the two sides started by the server and joined by
  pipes, Git's `fd::` transport). It needs descriptors passed through the launcher and a
  second process per transfer, and a fetch that follows tags may open a second connection,
  which that transport cannot.
- **Moving a finished clone into the sandbox's folder**, or copying a fetch's packs there
  by the server itself. A move between folders of different quota projects is refused by
  the file system, and the server's own copy would follow links that a confined Git cannot.
- **The address from the application** (the sandbox's record) instead of the clone's
  configuration. It would also stop a clone pointed at another GitHub repository; that is a
  change of four port methods and every caller, and gives no secret away today (the token
  goes to GitHub either way). Left for the slice that looks at what the token may do.
- **Running the transfer inside the sandbox.** The token would be where the agents are.
- **A credential helper instead of a header.** Git starts helpers through the shell, which a
  confined Git cannot execute, and the clone's `credential.*` keys would apply.

## Consequences

- A fetch through a courier is up to seven Git commands instead of one (the clone's
  address, where its objects are, the courier's `init`, the clone's refs, the fetch, the
  courier's refs, and the clone's `update-ref` when something changed), and two more for a
  clone without a note of the remote's default branch; a push is five. None copies an
  object twice. The courier is a repository of a few files for their time.
- A new clone is `init`, `remote add`, such a fetch and a `checkout`, instead of one
  `git clone`. What a clone is to Git stays: the default branch checked out and following
  the remote's, the remote's branches and tags, the note of its default branch, and, of a
  repository whose own history ends early, where it ends. One difference: the clone of an
  empty remote is on `main`, where `git clone` takes the name the remote would give its
  first branch; Yulius asks GitHub for that name before the first commit.
- The list of a clone's remote branches and tags is read in one answer (at most 16 MiB,
  the runner's bound for any command's output): a clone with more refs than fit is not
  fetched. No repository Yulius has met is near it.
- `yulius doctor` asks the Git on its own `PATH`; run as the service's user, it asks the
  Git the server starts.
- A refresh of a clone's reflogs differs: the remote-tracking refs are set by `update-ref`,
  not by a fetch, and `FETCH_HEAD` is the courier's, not the clone's. Nothing of Yulius
  reads either.
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
- The tests' stand-in for GitHub (an address rewritten to a folder in the server's own
  configuration) gives the shell to every command but those in a sandbox's folder, so the
  tests keep the rule too.
- **Not confined, G2 and G5 stay open:** Git on macOS (a developer's machine) and in tests
  of the adapter's crate runs transfers where it always did, with the token, in the clone,
  and nothing keeps it from the shell. The courier and both rules need the confinement's
  knowledge of which folder is a sandbox's.
- Not covered: what the server's token may do at GitHub when the clone's address names
  another repository there (the address is still the clone's).
