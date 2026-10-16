# 0150. The Superagent uses agent secrets as a task does, and restarts to get new tools

## Status

Accepted. Specified in [SPEC-186](../../.ai/specs/SPEC-186-the-superagent-uses-the-agent-vault-like.md);
task #171. Supersedes, in [ADR-0144](0144-the-superagent-reads-the-agent-vault-and-verifies-values-by-hash.md),
"The rest stays a task's" and the alternatives it left for later or rejected (the egress proxy,
`secret_copy_to_env`, `secret_request_hosts` for the Superagent); its `secret_verify` stands.
Settles the decision [ADR-0121](0121-the-superagent-gets-the-sandbox-mcp-servers-and-the-redaction.md)
left open (the Superagent behind the proxy). Amends SPEC-037 MGT-006 again.

## Context

A person moved 14 deploy secrets (API tokens, Ed25519 PEM keys, OpenSSH private keys) into a
project's agent vault and asked its Superagent to use and check them. It could not: ADR-0144 gave
it only `secrets_list` and `secret_verify`, and the Superagent at hand had been started before that
update, so it saw neither. The person asked for the Superagent to have the agent vault "as well",
meaning as the agents have it.

Three things stood in the way:

- The Superagent is not behind the egress proxy, has no worktree and no project browser, and runs
  as its person.
- An agent secret never spans lines, so a multi-line key is stored with literal `\n`, and
  `dotenv::line` refused any value holding `\`: no one, task or Superagent, could put such a key
  into a `.env`.
- Claude Code reads an MCP server's tools once, when it connects; a `--resume` keeps that view.
  Measured in the 2.1.292 binary: it acts on `notifications/tools/list_changed` only for a server
  whose `initialize` result advertised `tools.listChanged` (Yulius said `false`), receives it on a
  `GET /mcp` stream (Yulius answers 405) and reconnects that stream at most twice.

## Decision

- **Behind the proxy with its own bearer.** A Superagent starts with the proxy variables naming its
  `ck_` bearer (secret environment), `NO_PROXY`, the CA bundle in its shell's run directory (bound
  read-only into a contained session) and a placeholder variable per agent secret of its sandbox
  that its environment does not set already. The proxy takes a live Superagent's bearer as a client
  of its sandbox (`EgressCaller::Superagent`): its person enabled and allowed to work there, checked
  as its MCP calls are and cached as task tokens are, but refused at once when the Superagent's
  shell has ended. It never uses a production secret. Uses are audited once per Superagent process,
  secret and host, as `cockpit:<login>`. A header cannot carry line breaks, so a multi-line key goes
  as stored.
- **`.env` copies into the repository it works in.** `secret_copy_to_env` is served to the
  Superagent; its target folder is the sandbox repository, with every check a task's copy has. A
  secret no person allowed is asked for as `<login>'s Superagent` and nothing is written; no task
  is flagged, the person is in front of it and gets the Secrets page link.
- **Host requests.** `secret_request_hosts` is served to the Superagent, recorded and audited the
  same way, asked by `<login>'s Superagent`.
- **No browser.** The Superagent has no project browser tools, so `browser_fill_secret` is not
  served; a sign-in in the project browser stays a task's.
- **Asked before each change.** The two tools that change state are not among the tools it runs
  without asking: Claude Code asks the person first. `secrets_list` gives it the task's records
  (placeholders and what the proxy adds included) and its own guidance; its MCP instructions say
  what it can do.
- **Keys as their files.** A value that is a key block (armour `-----BEGIN …-----` opening its first
  line and `-----END …-----` closing its last once its literal `\n` are line breaks, and no other
  `\`, `'`, `\r` or `${`) is written to a `.env` as the key file in single quotes across lines,
  ending with one line break, whether or not the stored value ends with `\n`. `sh`, Node `dotenv`,
  `python-dotenv` and Docker Compose read single quotes literally, so the variable holds the file
  byte for byte. Copying again replaces the whole multi-line value. Each line of a key block is a
  redaction needle, so the copied file reads as `[secret:NAME]`.
- **New tools by a restart, not a notification.** Each Superagent's shell record keeps a launch
  digest: SHA-256 of its MCP instructions, its tool descriptors and `COCKPIT_LAUNCH_REVISION`
  (bumped when its environment or settings change). An adopted Superagent whose digest differs, or
  is absent (started before this), starts again under its start lock once its conversation is idle
  (no turn open, no background agent or unexpired Monitor), resuming its newest conversation on a
  subscription with capacity, without redoing a turn. Its viewers get a notice and the exit status
  `switched`, which the panel follows. Mid-turn it waits for the turn to end; a start that cannot
  happen (no capacity, access lost, its process not ended) is tried again a minute later.

## Alternatives

- **The person's workspace token as the proxy credential.** It is in all of the person's processes
  (terminals too) and outlives the Superagent, so their terminals could use agent secrets by
  placeholder. Rejected.
- **`.env` copies into a Superagent worktree of its own.** The Superagent works in the sandbox
  repository; a separate folder would hold keys where its commands do not run. Rejected.
- **A person's two-factor unlock for each Superagent copy.** The value never reaches the person's
  screen through the copy (the file reads as `[secret:NAME]` in the Superagent and in tool
  results), and an `env_files` allowance already lets every agent of the project write it to an
  ignored file of the sandbox. Adding the unlock would make the Superagent less able than a task
  for no gain. Rejected.
- **`notifications/tools/list_changed`.** Sessions running today never advertised for it, so they
  would ignore it; the notification stream reconnects twice, so an update longer than two retry
  intervals would lose it; and the proxy variables can only change with a new process anyway. It
  would serve only sessions started after this change, for tool changes alone. Not now.
- **Restart every Superagent after every update.** A restart costs the person's background
  processes; a digest restarts only the ones whose view changed.
- **Decode every literal `\n` when writing a `.env`.** A JSON service-account file kept on one line
  holds `\n` inside strings, which must stay escaped. Rejected for anything but key blocks.
- **Keep the stored value's lack of a final line break.** PEM and OpenSSH files end with one, and
  OpenSSH refuses a key without it. Rejected.

## Consequences

- The Superagent can deploy, sign requests and run CLIs with the project's agent secrets without
  seeing them. All of its non-Anthropic traffic goes through Yulius (bound hosts intercepted,
  others tunnelled). While Yulius restarts its requests fail, as a task's do; its Bash commands
  already wait for Yulius (ADR-0121).
- A `.env` in the sandbox repository holds real values, as a task worktree's does: the person's
  plain terminal reads it unredacted, which an `env_files` allowance already allowed for any agent's
  copy. Values stay redacted in the Superagent and in every tool result.
- A key's last body line shorter than 8 characters is not a redaction needle on its own, as no
  value shorter than that is; the rest of the key is.
- A Superagent whose conversation's last turn never closed (its process was killed mid-turn)
  restarts after the next turn ends. Text typed but not sent when it restarts is lost; the notice
  says why it restarted.
- A change to the Superagent's environment or settings must bump `COCKPIT_LAUNCH_REVISION`, or
  running Superagents keep the old launch until they start again for another reason.
