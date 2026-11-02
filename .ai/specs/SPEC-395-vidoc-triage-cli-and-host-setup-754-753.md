---
yulius-task: a8788eda-3729-4fbe-92ac-7c1618bc5602
status: in-progress
---

# SPEC-395: Triage of the Vidoc findings on the yulius CLI and host setup (#754 #753 #743 #742 #679)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #377
Scope authorization: task #377 (a person asked for each of the five issues to end closed, as a
false positive or as solved, without creating new issues).

## Goal

Each of the five Vidoc findings filed as #754, #753, #743, #742 and #679 is checked against `dev`
(f416b400) and either fixed test-first or dismissed with its reason, the guard or trust boundary
that stops it, the test that pins it, and one narrow `[[ignore]]` entry for its wording in
`.config/vidoc.toml`, so Yulius does not reopen the issue on the next scan.

## Context

- Vidoc files each finding once as an issue and reopens it when it reports the finding again after
  it was closed; a false positive needs an `[[ignore]]` entry (path exact, title a
  case-insensitive substring, SPEC-217, SPEC-273), one per wording (SPEC-262 VTR-005).
- Four findings are about subcommands of the `yulius` binary (`crates/yulius/src/main.rs`). The
  question for each is who can run it and what that caller gains. The binary is installed root
  owned, not writable by others (HST-010, `host_setup.rs:1307`), with no setuid bit and no file
  capabilities (none is set anywhere in the repository; on this host `stat` gives `-rwxr-xr-x`
  and `getcap` nothing). Every subcommand therefore runs with exactly its caller's rights, and the
  kernel checks what it does (cgroup moves, `setns`, file removal) against that caller.
- The fifth is `yulius host setup` (`host_setup.rs`), which an operator runs with `sudo` on the
  server; it writes `/etc/systemd/system/yulius.service` when none is installed.

## Requirements

- VCH-001 (#754, dismissed). `yulius sandbox-exec` (`main.rs:93-109`, dispatched at
  `main.rs:623-628`) is the internal step between the ptyhost or holder and `bwrap`: Yulius itself
  builds its argv (`yulius-ptyhost/src/host.rs:232-234`, `yulius-sandbox/src/manager.rs:904`,
  `yulius-contracts/src/sandbox.rs:166`); no web handler passes request data to it. Run by anyone
  else, it holds no privilege: `exec_bwrap_with` (`yulius-sandbox/src/enter.rs:93-123`) execs the
  given program as the caller, only handing it the seccomp filter (`--seccomp FD`, which bwrap
  installs); `--cgroup` writes the caller's own
  pid to `cgroup.procs` of a directory under `/sys/fs/cgroup` without `..`
  (`yulius-sandbox/src/cgroup.rs:680-688`), which the kernel allows only where the caller may
  write; `--join` does `setns` into namespaces named by inode (a reused pid is refused,
  `holder_ns`, `yulius-sandbox/src/enter.rs:43-52`), which the kernel allows only with
  `CAP_SYS_ADMIN` over them. A caller gains nothing it could not do with `bwrap` or `nsenter`
  directly. Not a trust boundary.
- VCH-002 (#753, dismissed; same reasoning, other code: not a duplicate of #754).
  `yulius --scrub-exec` (`main.rs:556-569`) is Claude Code's `CLAUDE_CODE_SHELL_PREFIX` in a run
  (SPEC-358): its argument *is* the agent's Bash command, which by design runs through the shell
  (`scrub_client.rs:258-273`, `[line] => sh -c line`), as the agent's own user in its own
  sandbox. Running a command is what it is for; it adds only output redaction
  (`scrub_client.rs:316-399`) and fails closed when the fingerprints cannot be loaded
  (`scrub_client.rs:105-126`). The `env` closure reads only `SHELL`, `YULIUS_SCRUB_FILE`, the task
  token and the server address (`scrub_client.rs:106-111`, `:236`, `hook_client::post_internal`);
  the child inherits the environment it would have had without the prefix, and the run's
  environment holds no secret value (secrets reach a run as proxy placeholders). Pinned by
  `secrets_redaction::the_bash_wrapper_redacts_every_form_of_a_value` and
  `secrets_redaction::the_terminal_and_the_run_environment_hold_no_value`.
- VCH-003 (#743, dismissed). `yulius remove` (`main.rs:78-85`, `remove_cli.rs`) runs as the
  agent, in its run, with the agent's rights: it lets an agent remove what it could already `rm`
  without stopping on Claude Code's prompt, so it is narrower than the agent's own shell, not a
  boundary. Even so, the validation the finding asks for is there: the last component must not be
  empty, `.` or `..` (`remove_cli.rs:263-272`), the directory is canonicalized, resolving `..` and
  links (`remove_cli.rs:277-282`), the last component is never followed (`symlink_metadata`,
  `remove_cli.rs:284`; a link is removed as a link), and `decide` (`remove_cli.rs:110-157`) then
  accepts only an untracked or ignored path inside the resolved task worktree (not `.git`, not the
  worktree itself) or a path on the contained run's own tmpfs, refusing mount points. Pinned by
  `remove_cli::tests::it_removes_the_tasks_paths_and_refuses_the_rest` (a link to a directory
  outside, `src/../../<outside>/x`, a link with a trailing slash, a link inside an ignored
  directory: none reaches the outside file), `shared_files_and_other_worktrees_are_refused`,
  `an_uncontained_runs_tmp_is_refused` and
  `mount_points_their_holders_and_the_root_links_are_refused`.
- VCH-004 (#742, dismissed). `yulius security-scan` (`main.rs:68-77`) forwards its arguments to
  Yulius's own scanner, `scripts/vidoc_scan.py` embedded in the binary
  (`security_scan.rs:10-11`), as separate argv entries of `python3 -I -c <scanner>`
  (`security_scan.rs:35-37`), never through a shell. The scanner's `argparse` accepts only the
  documented options (`vidoc_scan.py:964-974`) and exits on any other. `--report FILE` writes a
  JSON report with the caller's own rights, where the caller could write anyway. Its callers are a
  person or agent in their own checkout (the agent shim, `identity.rs:113-123`, pinned by
  `identity::tests::the_agent_yulius_shim_offers_wait_and_the_security_scan`) and the CI job a
  project's own `yulius.toml` declares, which runs contained. No one gains a capability by
  choosing its arguments. Not a trust boundary.
- VCH-005 (#679, dismissed). `unit_text` (`host_setup.rs:249-262`) is called once, at
  `host_setup.rs:1170`, after every value it formats (the public URL, the service user, the
  executable path and the data directory) passed `unit_safe` (`host_setup.rs:1155-1168`), which
  accepts only printable ASCII other than space, quotes, backslash, `%` and `;`
  (`host_setup.rs:265-270`): no newline, so no directive can be injected, and no systemd
  specifier. The service user is also checked when it is created (`host_setup.rs:957`). The
  values come from `sudo yulius host setup` options on the server (`main.rs:445-481`, passed at
  `main.rs:509-530`), typed by the operator who is already root; `public_url` is not taken from web
  configuration. A unit file that exists is never overwritten. New test (no test pinned the
  guard): `host_setup::tests::unit_values_a_unit_file_cannot_hold_are_refused`.

## Plan

- [x] Spec with the triage of all five findings
- [x] Host setup test pins that unsafe unit values are refused (#679)
- [x] Narrow [[ignore]] entries for the five dismissed wordings
- [ ] Gates, changes entry and evidence
- [ ] Independent review answered
- [ ] Issues commented and closed

## Acceptance criteria

- `.config/vidoc.toml` has one `[[ignore]]` entry per dismissed wording (five, the exact titles of
  #754, #753, #743, #742 and #679 at their files), each with its reason; no entry uses
  `function` or `line`, so a different finding in `main` or in the host setup is still reported.
- No product code changes. The new test fails when `unit_safe` accepts every value, and passes on
  this branch; the tests named in VCH-002..005 pass.
- Each issue carries a comment with the reason and file:line evidence on `dev` and is closed as not
  planned; no issue is created.

## Test plan

- `cargo nextest run -p yulius -E 'test(/host_setup::tests::unit_values/)'`, and the same with
  `unit_safe` returning `true` (mutation): fails, then passes once restored.
- `remove_cli::tests::`, `identity::tests::the_agent_yulius_shim_offers_wait_and_the_security_scan`
  and `secrets_redaction::` in the gate: the guards cited are pinned.
- `scripts/vidoc_scan.py` `parse_config` accepts `.config/vidoc.toml`.
- `scripts/dev ci`: the full gate.

## Decisions

- All five findings are false positives on `dev`; nothing is fixed. The four CLI findings assume
  an untrusted caller of a binary that holds no privilege beyond its caller's; the fifth assumes
  `unit_text` is reached unvalidated, and it is not.
- #753 and #754 are separate issues, not duplicates: different subcommands, code and fingerprints.
- One test is added for #679 because no test pinned the guard that refutes it; the other guards
  are already pinned, so no test is added for them (a test that cannot fail without a change would
  prove nothing new).
- Dismissed with title-only `[[ignore]]` entries, one per wording (SPEC-262 VTR-005): a `function`
  entry on `main` would accept any later finding in the whole dispatch.
- Not "fixed" by an allowlist of scanner options, a path filter in the CLI or an argv check in
  `sandbox-exec`: product changes that only silence a false positive are refused (SPEC-262), and
  each would duplicate a check the scanner, `remove_cli` or the kernel already makes.

## Evidence

To be filled when the gates have run.
