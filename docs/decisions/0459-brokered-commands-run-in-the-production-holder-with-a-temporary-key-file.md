# 0459. Brokered commands run in the production holder with a temporary key file

## Status

Accepted (2026-11-22). Specified in
[SPEC-626](../../.ai/specs/SPEC-626-brokered-commands-a-cli-that-needs-a-key.md); task #610. The
owner approved the feature after a review of iTermX's broker. Amends SPEC-002 ADR-20 (agent
secrets): an agent secret can also reach a brokered command's run, as a file outside the
sandbox. Extends [ADR-0208](0208-production-cd-runs-use-a-production-holder-with-its-own-docker-engine.md)
(the production holder runs these commands too) and keeps
[ADR-0324](0324-a-production-secret-reaches-no-run-and-has-no-hosts.md) (a production secret
reaches no run, a brokered one included).

## Context

- The egress proxy keeps an agent secret out of the agent for HTTP requests to its bound hosts.
  A CLI that reads a key file (a deploy tool, a cloud SDK with a service-account JSON, a signing
  tool) gets the key only as a file in the sandbox, which the agent can read.
- Each sandbox already has a production holder: its own namespaces and network, no agent run
  joins it, it runs on the host's read-only system, and its files are under a data directory
  that agent runs see as an empty tmpfs.
- An agent's arguments are untrusted. A CLI pointed at another file, or given its key flag
  twice, would send whatever key it finds.

## Decision

- **A project's administrators define named commands**: the binary, which agent secret, which
  tasks may run it, the approval mode and minutes, fixed arguments and env with `{keyfile}` and
  `{home}` (`{secret}` in env only), allowed subcommands, flags Yulius owns, whether `owner/repo`
  arguments pass, and a time limit. Each change is audited without values.
- **Agents run them by name**: `command_run` (and `yulius run NAME ARGS` in the sandbox) with
  their own arguments. The Superagent and agents list them with `commands_list`.
- **The run happens in the project's production holder** on the host's read-only system, without
  Docker. It gets a fresh home as its working directory, an environment of nothing else, and the
  key in a 0600 file under a random directory outside that home. Its files go when it ends, and
  a starting server removes those of a run it did not see end. On a server without isolation it
  uses the containment that production CD runs get there.
- **Arguments that could reach another file or the key are refused**: anything with `/` or a
  leading `~` (with `slashes`, only absolute paths, `~`, `..` and `=/`, `@/` forms), the owned
  flags in every form, and a first non-flag argument outside the allowed subcommands.
- **Bounded and blanked.** A time limit, 16 MiB of files, and 96 KiB of each stream back. stdout,
  stderr and the command line are redacted of every agent secret the project can see, the key
  and its lines included, before they are stored or returned. Every run, refusal and answer is
  audited.
- **Approval** is `always`, `send` (when the agent says it changes something, or an argument is
  one of the command's send flags) or `never`. A waiting run flags the task Needs you with the
  full command line. People who manage the project's secrets answer from a browser session or
  by replying `approve` or `reject` to the Telegram notice. The answer is a conditional write
  that holds only before the deadline. No answer in time is a no.
- **Keys**: a project secret, or a global one switched on for the project. Never a personal
  secret (a definition is the administrators', a personal secret only its owner's), never a
  production secret. Saving a definition sends the value somewhere new, so
  [ADR-0186](0186-agent-secrets-are-project-global-or-personal.md) and
  [ADR-0317](0317-a-guarded-secret-goes-somewhere-new-only-after-two-factor.md) apply to it: only a
  person who manages the secret saves it (a global secret's: an instance administrator), and a
  secret people allow use by use needs a fresh two-factor unlock in a browser session, never an
  API token. The definition keeps the secret's id; its runs get no other secret of that name.

## Alternatives

- **The key in the sandbox (today).** The agent can read it, and injected text can make it send
  the key elsewhere.
- **Run in the server's own namespaces.** The server holds every project's keys and the instance's
  credentials. A CLI bug or a crafted argument would reach all of them.
- **A third holder per project.** The same separation from agents as the production holder, for
  ADR-0208's machinery again.
- **`{url:SERVICE}` through the egress proxy (iTermX).** The production holder has no route to the
  server's loopback. A key in a URL or header already works for agents through the proxy's
  placeholders.
- **Telegram inline buttons.** The adapter reads only messages. A reply to a notice is the
  channel's existing way of acting on a task.

## Consequences

- An agent deploys with a key it never holds, and a person sees every changing run's full command
  line before it runs.
- A brokered run shares the production holder's network with production CD runs. Its own pid and
  mount namespaces and an administrator-defined command bound what it can do there.
- A schema change: migration 0089 (command definitions and runs).
