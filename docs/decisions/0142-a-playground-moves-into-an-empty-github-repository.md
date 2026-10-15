# 0142. A playground moves into an empty GitHub repository its mover could import

## Status

Accepted. Specified in [SPEC-176](../../.ai/specs/SPEC-176-fix-410-move-a-playground-to-a-github-re.md)
(#410). Amends [0081](0081-a-playground-keeps-its-git-on-the-server.md): a playground can stop
being one.

## Context

A playground (ADR-0081) keeps its Git in a bare repository on the server and is marked by an empty
repository owner. Once the instance connects GitHub, a team wants the work it did there to become
an ordinary GitHub sandbox, keeping its tasks and history. Moving it raises three questions. Where
does the repository come from: Yulius creates it, or the person brings an empty one? Who may move
it? And how does the move fail without breaking the playground?

The GitHub App asks for Administration: write, so the App can create repositories in an
organisation. Creating a project from a template (PRJ-004) is for instance administrators. Anyone
signed in can make a playground. Access to a GitHub sandbox mirrors GitHub (ACC-007). A repository
the App creates in an organisation gives nobody but the organisation's owners a permission on it.

## Decision

1. **The person brings an empty repository of the instance owner.** Yulius never creates one for a
   move. The mover must be an admin of the playground and pass the import check (IMP-002):
   `admin` or `maintain` on that repository on GitHub, or, for the superadmin, a repository the
   App installation can access.
2. **"Empty" means no branch and no tag**, or only the playground's branches and tags at their
   commits or behind them: a move that failed after its push is retried, and the non-forcing push
   fast-forwards what merged since.
3. **The move runs in this order:** an atomic, non-forcing push of every branch and tag of the
   bare `origin`, with the installation token (after the default branch, decision 5). Then the
   clone's own transport settings are removed, its `origin` is repointed to the repository, and
   its `origin/*` refs are refreshed from the bare `origin` by path, which GitHub now matches, so
   no token passes through the clone. Then a compare-and-set on the sandbox record writes the owner, name and
   repository id, and holds only while it is still a playground. That write is the commit point:
   when it fails, the clone's `origin` is pointed back. Only after it is the bare `origin`
   deleted. The move is audited as `sandbox.moved_to_github`.
4. **Live or queued tasks and active CI runs hold it** (409), checked before the push and again
   before the record update, and it holds the sandbox's armed-merge pass. It runs detached from
   its request, as a shell start does (ADR-0053), so a client that goes away never leaves it half
   done. Stopped and finished tasks stay and continue as GitHub tasks: their briefing is built per
   attempt.
5. **The playground's default branch is pushed alone first**, before the atomic push of the rest:
   GitHub makes the first branch pushed into an empty repository its default, so GitHub's default
   matches the sandbox's.

## Alternatives

- **Yulius creates the repository** (the App has the permission). It is one click. But it would
  let anyone who can make a playground create repositories in the organisation, which PRJ-004
  reserves for instance administrators. It also leaves the maker without a GitHub permission on the
  new repository, so under ACC-007 they would lose their project at the moment of the move. Making
  it safe needs a policy (instance administrators only, or Yulius adding the mover as a repository
  admin) that the issue did not ask for. A person would decide it, and the App's existing
  Administration: write is the only permission it needs.
- **Both: create when absent, else take an empty one.** It doubles the paths and still needs the
  policy above.
- **Keep the bare `origin` as a backup after the move.** Its code is on GitHub then, and a stale
  second copy on the server is exactly what SPEC-171's purge exists to remove.
- **Migrate live tasks.** A running agent was given a playground's tools, briefing and credentials,
  and an armed playground merge would land on the old `origin`. Stopping them first is one click
  each and leaves nothing half-moved.

## Consequences

- A team creates the empty repository on GitHub first (and adds it to the App when its
  installation is limited to selected repositories). The dialog says so.
- After the move, its maker sees the sandbox only through their GitHub permission on the
  repository. That is why the mover needs `admin` or `maintain`.
- The clone's configuration is the playground's agents' to write. The move passes no token through
  it (it refreshes the clone from the bare `origin` by path, which GitHub then matches) and removes
  the clone's own transport settings, so they do not steer the GitHub sandbox's later
  token-bearing fetches. Those fetches still read the rest of the clone (#54 part B, ADR-0114).
- No ruleset is created. As for an import, `production_protect` or onboarding adds them.
- The integration tests drive the real Git adapter against local bare repositories through a test
  seam that rewrites `https://github.com/` (`GitAdapter::with_github_base`), off in production.
