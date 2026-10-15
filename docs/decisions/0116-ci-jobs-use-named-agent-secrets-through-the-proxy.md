# 0116. CI jobs use named agent secrets through the egress proxy

## Status

Accepted. Specified in [SPEC-142](../../.ai/specs/SPEC-142-fix-466-yulius-ci-jobs-opt-in-to-named-a.md);
task #140, issue #466. Amends [ADR-0104](0104-agents-scan-their-branch-with-vidoc-before-delivery.md):
Yulius CI runs the Vidoc scan on pull request heads.

## Context

Agent sessions use agent secrets without holding them: they get `yulius-secret-NAME` placeholders,
and the egress proxy replaces a placeholder with the value only in requests to the hosts a person
bound to that secret (ADR-0098). Yulius CI jobs had no secret and no proxy, so a check that needs a
credential, such as the Vidoc scan, could only be an agent's discipline step, not a merge gate.

A CI job runs the code of a pull request head. A pull request can also change the `[[ci.jobs]]`
it is tested with: Yulius runs a pull request's own jobs when it changed them, so that a CI change
can be tested before it merges. So "the jobs of this run" are partly the pull request's choice.

## Decision

- **The grant is the integration branch's.** A `[[ci.jobs]]` entry may name agent secrets,
  `secrets = "NAME OTHER"` (space-separated, like `[cd.<env>] secrets`).
  When a job starts, Yulius gives it the list of the integration branch's entry with the same
  name, read where no job can write: the integration head GitHub reports, taken from the sandbox
  clone into the CD mirror only Yulius writes, every object checked and replacement refs ignored
  (as ADR-0109 does for deploys). CI jobs can write the sandbox clone (it is inside the sandbox),
  so what it shows (`origin/<integration>`, which chooses the jobs) only says whether to look; a
  grant planted there gives nothing. A pull
  request's own `secrets` line grants nothing, even when its own jobs run; the person's profile
  gate grants nothing. A grant reaches pull request code only after a reviewed change of
  `yulius.toml` merged into the integration branch.
- **Placeholders, never values.** A granted job gets a placeholder for each granted agent secret
  of its sandbox (and the instance-wide ones), except production-deploy (`cd_prod`) secrets, plus
  the proxy variables and the CA bundle, as an agent session does. A job without a grant gets none
  of these.
- **The job is its own proxy client, `ci:<run>`.** Each granted job gets a fresh random proxy
  credential, held in Yulius's memory from just before the job starts until its process ended, and
  never cached past that. The client it names may use only the job's granted secrets, and each only
  toward its bound hosts: other secrets' hosts are opaque tunnels, their placeholders stay literal,
  and `X-Yulius-Secret` naming them is refused. Each use is audited (`vault.agent.use`, actor
  `ci:<run>`), once per job, secret and host. Responses are redacted of every agent secret of the
  sandbox.

## Alternatives

- **Take the grant from the run's own jobs.** A pull request could grant itself any agent secret
  by editing `yulius.toml`. Rejected.
- **Refuse secrets to every pull request that changed its jobs.** Safer on paper, but the pull
  request's code already decides what the granted job runs (`scripts/dev` is its own), so it widens
  nothing, and any pull request touching `yulius.toml` would lose its security scan.
- **Put values in the job's environment, as CD does for its own targets.** CD runs only verified
  commits of the protected branches out of the agents' reach (ADR-0109); CI runs unreviewed pull
  request code. Rejected.
- **Reuse a task token.** A CI run belongs to no task or attempt, and its audit must name the run.
- **Store the credential with the job.** A stored token outlives a crash and needs a migration; an
  in-memory one ends with the job or with Yulius.

## Consequences

- Code in a pull request head can make a granted job send any request to a bound host with the
  secret: whatever the key allows there, pull request code can do, and the value can come back
  only if that host echoes it in a form the redaction does not know. Granting a name and binding a
  host is therefore a person's decision, taken in a reviewed `yulius.toml` change and on the
  Secrets page. Pull requests from forks are not tested by Yulius CI at all.
- As for agents, the proxy variables are advisory: a job can ignore them and reach the network
  directly, but it then holds only placeholders.
- Removing a name from the integration branch's `yulius.toml` takes effect for the next job.
- A granted job costs a fetch and a GitHub call when it starts; when either fails, the job runs
  without its secrets and its output says why.
- The sandbox clone stays writable by CI jobs, so a pull request can still change which jobs a
  later run of the sandbox executes (not what they may use). That is older than this decision and
  belongs to its own change.
- This repository's `security` job (`scripts/dev security`, granted `VIDOC_API_KEY`) runs on pull
  request heads only. Until the scan can run (#470: the stored key is rejected), its exit 2 does
  not fail the job; findings (exit 1) do. Exit 2 also covers causes a pull request controls (a
  malformed `.config/vidoc.toml`, a broken clone), so once a scan runs, the follow-up is to make
  exit 2 fail the job again (noted on #466).
