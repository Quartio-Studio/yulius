---
yulius-task: 1fb06e11-08e1-4c37-90d0-3219e19122ad
status: in-progress
---

# SPEC-483: Deep security and quality audit: fix what it finds

Status: in-progress
Date: 2026-11-13
Owner: Yulius task #465
Scope authorization: the request of task #465: "Execute a deep security and quality audit and fix
everything. After this iteration, the system must be production-quality, secure and efficient.
Especially: (1) fix all ways we could hack the system (2) look for hidden state machines (3) look
for duplicated code, useless abstractions, overengineering and simplify (4) look for dead code and
dead solutions and remove it (5) look for contradictory logic in both code and product."

## Goal

Every confirmed finding of an audit of the whole repository is fixed and merged into `dev`: no
known way for an agent, a signed-in person with little access or an outside client to get what
Yulius's rules deny them; lifecycles decided by one rule; no logic kept twice; nothing kept that
no code path reaches; documents, texts shown to agents and people, and code that say the same.

## Context

- **How the audit ran.** Eleven read-only auditors, one per area (public API and access; internal
  listener and egress proxy; containment and host access to agent-owned files; vault; GitHub,
  Git, delivery, CI and CD; browser, previews, imports, updates and moves; web UI; domain, store
  and contracts; the two halves of the application layer with the composition crate; product
  documents), each with the same brief: threat model, what counts as a finding, read end to end,
  report with file and line. Each finding is verified here before it is fixed: the code path is
  read again, and a test that fails without the fix is written first. Their notes are kept
  outside the repository (`$HOME/.cache/yulius-audit-465/`).
- **Slices.** The fixes are delivered as pull requests of one concern each, most harmful first:
  1. the redaction fingerprints (V1, V2): merged, pull request #1240;
  2. what the host runs and writes where an agent can write: `bwrap` found on a run's PATH, the
     Git attributes and hook files written by path (C1, D1, D2): merged, pull request #1241;
  3. what the host reads where an agent can write: transcripts, `upper/etc/passwd`, numbered
     directories, disk usage (C2, C3, C5, I4);
  4. access rules of the public API and the merge gate (A1-A4, W1, D3, P8);
  5. the sandbox browser (E1-E3);
  6. the vault's remaining rules (V3-V6);
  7. delivery and CI decided once (D4-D6, QB1);
  8. lifecycles decided once (QA1-QA7, E6, E7, QC8, W2, W3);
  9. documents and shipped texts against the code (P1-P14, QB9);
  10. dead code (W7, D7, E5, QA9, QB4, QB6, QC2, QC5, QC7);
  11. duplicated code (W4-W6, QB2, QB3, QB7, QB8, QC3, QC4);
  12. efficiency (QA8, QB5, QC1, QC6).
  The Plan grows by a slice's steps when the slice before it is merged. The ledger below lists
  every finding with its slice and state.

### Slice 1: the redaction fingerprints

- **What they are.** The Bash wrapper and the `PostToolUse` hook run inside the sandbox and
  redact agent secrets from command output and tool results. They get fingerprints of every
  value and its encodings from `POST /internal/scrub/fingerprints` (the run's token) and from
  the run's `scrub.json` (`App::scrub_fingerprints_for`, `crates/yulius-app/src/vault/runtime.rs`).
- **The defect (critical).** A fingerprint held `rolling`, a polynomial hash of the needle
  modulo 2^61 − 1 at the set's `base` (`redact::rolling_hash`), and the server drew a new random
  `base` on every request. The hash is the needle's polynomial evaluated at `base`; as many
  requests as the value has bytes give as many points, and the bytes follow by solving the
  linear system. Any process with the run's token could read every agent secret that reaches
  the run this way, production secrets and the instance Vidoc key included. The accepted risk
  (ADR-0014, `.config/vidoc.toml`) was a guessing oracle for weak values, not this.
- **Guests.** A playground a guest joined gives no run an agent secret (SPEC-410 GST-004), but
  `scrub_fingerprints_for` still answered names, lengths and digests of the project's, the
  global and the instance's values. Its runs do keep the values its MCP servers hold themselves
  (`mcp_secret_values`), which must stay redacted.
- **Two-factor seeds.** A run's set held a secret's seed, which never enters a sandbox (Yulius
  types the code).
- **Sessions across an update.** An update freezes and thaws sessions; task runs and Superagent
  shells outlive it and keep the `scrub.json` their launch wrote. A contained session also keeps
  the `yulius` binary it started with (bound read-only by inode; the helper installs by rename),
  so its wrapper keeps reading the old format only: a new answer or a new file would refuse
  every command of every live session after the update.

### Slice 2: what the host runs and writes where an agent can write

- **Whose files.** A sandbox's directory (home, clone, worktrees) is mounted read-write into
  its runs at the same paths as on the host; the server and the process hosts work there as
  the service user, outside containment (ADR-0174).
- **C1 (critical): `bwrap` by name.** A contained run is `bwrap …` with the run's environment
  (`yulius-ptyhost` `containment.rs`, `host.rs`; `yulius sandbox-exec`, `enter.rs`). A task
  run's `PATH` begins in `<sandbox>/home/.cargo/bin`. A `bwrap` planted there starts instead
  of bubblewrap, as the service user, before containment.
- **D1 (critical): `info/attributes` by path.** `Git::set_managed_attributes`
  (`yulius-git` `merges.rs`), run at every task start (`keep_union_merges`), read and replaced
  the clone's `info/attributes` by path, in a folder asked of Git in the agent's worktree: a
  host file copied to the agent, a host file overwritten with the agent's lines, a FIFO.
- **D2 (high): the guard by path.** `Git::install_pre_push_guard` (`hooks.rs`) wrote fifteen
  files through fixed temporary names into the new worktree's Git directory, which exists
  before the checkout ends.

### Slice 3: what the host reads where an agent can write

- **C3 (medium): transcripts by path.** A session's transcript is
  `<sandbox>/home/.claude/projects/<slug>/<session>.jsonl`. `transcripts::read_page`,
  `read_tail` and `read_for_index` (`yulius-app`: the transcript pages, the agents'
  `session_read`, the search index) and the harness's `TranscriptTail` (the run watcher)
  opened it with `File::open`: a FIFO there blocks the opening thread for good, a link or a
  linked folder on the way is read as the session's transcript.
- **C2 (high): `upper/etc/passwd`.** `Manager::prepare` took the system layer's `passwd` and
  `group` when `is_file()` said so and `identity.rs` read them whole before looking at their
  size, then merged the lines into the identity files of the next shell or Cockpit.
- **I4 (low): `record_number`.** `names_on_disk` listed `<worktree>/<dir>` by path; `dir` is
  the agent's argument and may pass through a link.
- **C5 (low): disk usage.** `usage::measure` walked by path (`symlink_metadata`, then
  `read_dir` of the same path).
- **D2's sibling: the review.** `review.rs` read an untracked file by path after a
  `symlink_metadata` check.
- **From the review of slice 2** (minor, taken here because the files are the same):
  `git_dir_below` did not compare the common directory Git reports with the clone's `.git`;
  `handles::read_in` kept set-user-id bits for the file that replaces it; one test case could
  not fail; the change log's claim was wider than the fix.

## Requirements

- **FPR-001** The fingerprint set of a sandbox and holder is the same on every request and
  launch, and after a restart, until an agent secret changes: the salt is derived from the
  server's workspace secret and the sandbox id; nothing else is random.
- **FPR-002** A fingerprint is `{secret, len, tag, digest}`. `tag` is the top `tag_bits(len)`
  bits (3 for 8 to 15 bytes, one more per doubling of the length, at most 16) of a 16-bit hash
  of the needle's first 8 bytes, with the case of letters and the two base64 alphabets folded.
  The set has no `base`, a needle no `rolling`; the format version is 2.
- **FPR-003** The redactors find every form of a value as before (whole texts, streams, chunk
  boundaries, the corners of the data) and leave other text alone, a text sharing a tag
  included.
- **FPR-004** A run's set covers what can be in its sandbox: no two-factor seed; in a
  playground a guest joined, no agent secret and no Vidoc key, only its MCP servers' own
  values. A store that cannot say whether a guest joined gives an error, never an empty set.
  The sessions running when a guest joins get their files rewritten at the join.
- **FPR-005** A starting server rewrites, in place and only when it differs, the fingerprint
  file of every live task run and of every Superagent; one that cannot be rewritten does not
  stop the others.
- **FPR-006** Redaction of 1 MB of output is not slower than before.
- **FPR-007** A session launched before version 2 keeps redacting: its file stays in the old
  format, written with `base` 0 (the old rolling hash is then a needle's last byte plus one),
  at a start, at a guest's join and whenever a wrapper that does not name version 2 asks the
  endpoint, which itself answers version 2 only. When its fingerprints cannot be made, a file
  this server wrote stays as it is and one an older server wrote becomes a bare
  `{"version":1}`, with which its wrapper runs nothing and after which the format is still
  known.

- **HWR-001** A contained run is started with the `bwrap` of the process host's own `PATH`
  (the first absolute directory with an executable file of that name); the launch fails when
  there is none. `sandbox-exec` refuses a program without an absolute path.
- **HWR-002** `set_managed_attributes` takes the clone and reaches `info/attributes` through
  no-follow handles below the clone's `.git`: a link at the file, its folder or any temporary
  name is neither read nor written through; a FIFO is refused at once; more than 256 KiB or a
  file that is not text is refused; the file's other lines and its mode are kept.
- **HWR-003** `install_pre_push_guard` takes the clone and the worktree and writes the hooks
  through no-follow handles below the clone's `.git`; a hooks folder that is a link, or a
  worktree whose Git directory is not the clone's or one of its `worktrees/<name>`, is refused.
- **HWR-004** The union merge and the guard work as before for a task (SPEC-083 MRG-001,
  DSC-007).

- **HRD-001** A transcript is opened from its sandbox directory through folder handles opened
  with `O_NOFOLLOW`, the file with `O_NOFOLLOW | O_NONBLOCK`, and only a regular file is read;
  its size comes from the same handle. A path outside the sandbox directory (the server's own
  copy) is opened the same way at its name. The harness's tail opens its file that way at its
  name on every poll. A transcript that is missing or no regular file reads as none (no
  500). Continuing an imported session copies its transcript and side files through handles.
- **HRD-002** The sandbox adapter reads `upper/etc/<name>` through handles (no link followed,
  never waiting), at most 1 MiB of a regular UTF-8 file, else the base's; what hides the base's
  file in the sandbox gives nothing. The join record carries the text.
- **HRD-003** `record_number` lists a numbered directory from the sandbox directory down with
  `open_dirs_below`; a link on the way lists nothing; a checkout in a folder of the server's
  own is listed from there.
- **HRD-004** `usage::measure` opens each folder from its parent's handle and looks at its
  entries from that handle; the counts and the largest consumers are as before. Folders
  nested deeper than 128 are counted, not listed, and the measurement is marked truncated.
- **HRD-005** The review reads an untracked file through handles from the worktree's folder;
  a link at its name or on the way, a FIFO, `..` and an absolute path are listed without lines.
- **HRD-006** `install_pre_push_guard` refuses a worktree whose common directory is not the
  clone's `.git` (same device and inode) and sets `core.hooksPath` to a path made from the
  clone's; a file replaced through `handles` gets the permission bits only.

## Plan

One item per slice (a slice's own steps are its requirements and tests); a slice is ticked when
its change is reviewed and ready to merge, and the next is added when it is merged.

- [x] Slice 1: redaction fingerprints say a few bits and are fixed per sandbox (merged, #1240)
- [x] Slice 2: bubblewrap by the host's path, the clone's Git files through handles (merged, #1241)
- [x] Slice 3: the host's reads in a sandbox go through handles, bounded and never waiting, reviewed

## Acceptance criteria

- **AC-1** Three requests in a row to `/internal/scrub/fingerprints` answer the same body; it
  has the fields `version`, `salt`, `names`, `needles` only, each needle `secret`, `len`, `tag`,
  `digest` only, and every tag is below `2^tag_bits(len)`. Two sandboxes have different salts;
  a second server on the same data directory has the same salt; a changed value changes its
  digests and nothing else.
- **AC-2** The attack that recovered a 28-character value from 28 requests on `dev` has nothing
  to work on: no `base`, no `rolling`, equal answers.
- **AC-3** A fingerprint set made by the server redacts a value in the wrapper's matcher
  (`scrub_client::matcher`), in every encoding the tests cover.
- **AC-4** After a guest joined a playground, the maker's and the guest's tokens get the name
  of the playground's MCP server value only, and the wrapper's matcher redacts that value and
  not the project secret's; the maker's running session's file says the same; before, the
  maker's set named the project secret too. With the guests' table unreadable the set is an
  error.
- **AC-5** After `refresh_fingerprint_files`: a file of this server's format holds exactly what
  the endpoint answers; a file of an older Yulius is version 1 with `base` 0 and the same
  inode, and the old wrapper's search (and the real wrapper of `dev`, Evidence) redacts with
  it; a link in a run's place is refused and its target untouched, and the other runs' files
  are still rewritten. The same for a Superagent's file.
- **AC-6** A request without a version (the old wrapper's) is answered in version 2, and the
  asking session's old-format file then also covers a secret added since.
- **AC-7** The new wrapper refuses a file in the old format (`Invalid`), so nothing runs
  unredacted while the server cannot be reached.

- **AC-8** With a `bwrap` that leaves a marker in `<sandbox>/home/.cargo/bin` and that folder
  first on the run's `PATH`, a contained launch leaves no marker.
- **AC-9** `set_managed_attributes`: a link at the old temporary name keeps its target; a link
  at `attributes` is refused and its target not copied; a FIFO and a 300 KiB file are refused;
  `info` as a link out of the repository is refused with nothing written there; the existing
  union merge test passes with the clone named.
- **AC-10** `install_pre_push_guard`: links at three old temporary names and at one hook's name
  keep their target, the hooks are regular files and the guard refuses a push to a protected
  branch; a hooks folder that is a link out of the repository is refused with nothing written.

- **AC-11** `transcripts::open`: a regular transcript is read; a link at its name, a FIFO (within
  seconds), a linked folder on the way are `InvalidInput`; a missing one is `NotFound`; a file
  outside the sandbox directory is read. `TranscriptTail`: a FIFO and a link fail `poll` and
  `open_at_end` at once.
- **AC-12** `system_file_text`: the upper layer's regular file wins over the base's; a link to
  a host file, a FIFO, a file over 1 MiB and `etc` as a link give nothing; no `etc` or no file
  gives the base's.
- **AC-13** `names_on_disk`: a real directory lists its names; a linked directory, a linked
  checkout and a missing directory list none; a checkout outside the sandbox lists its own.
- **AC-14** `usage::measure`: a linked folder and a linked root are not walked; the existing
  counts and largest-consumer tests pass.
- **AC-15** `untracked_start`: a regular file is read; a link, a linked folder, a FIFO, a
  missing file, `..` and an absolute path give nothing.
- **AC-16** The guard refuses a worktree whose `.git` file names a directory of the same shape
  elsewhere and writes nothing there; a set-user-id bit on a hook is gone after a reinstall.

## Test plan

- Domain (`vault/redact.rs`): tags (bits per length, only the first 8 bytes count, the cases
  and alphabets fold, spread over all values); corners (an 8-byte needle ending the data, data
  shorter than the window, two secrets with one value, the longest of two needles at one
  place); a text sharing a tag is not redacted; fingerprints too short or of an unknown secret
  are ignored; the existing stream and whole-text tests run on the new matcher.
- Harness (`scrub_client.rs`): the existing wrapper and hook tests on version 2 sets;
  `a_fingerprint_file_in_the_format_before_version_2_fails_closed` (AC-7).
- Integration (`crates/yulius/tests`): `secrets::repeated_fingerprint_requests_answer_the_same_set`
  (AC-1, AC-2), `secrets::agents_list_names_and_read_redacted_results` (AC-3),
  `playground_guests::a_playground_with_guests_gives_no_run_an_agent_secret` (AC-4),
  `secrets_scopes::a_starting_server_rewrites_the_fingerprint_files_of_live_runs` (AC-5, AC-6),
  `secrets_scopes::an_old_fingerprint_file_survives_a_failed_rewrite_in_its_format` (FPR-007),
  `secrets::older_agent_secrets_reveal_their_user_name_and_seed` (FPR-004: no seed in a run's
  set, the seed still in Yulius' own redaction),
  `cockpit::a_superagent_from_before_version_2_keeps_redacting_from_its_file` (AC-5, AC-6).
  `support::old_wrapper_redacts` is the old wrapper's search written out, so these tests hold
  the old format to what that wrapper computes.
- Probes outside the suite (Evidence): the defect on `dev`; the wrapper built from `dev` against
  this server's answer and file; speed in a release build on 1 MB with 10, 40 and 200 secrets,
  before and after.

- Slice 2: `yulius-ptyhost` `tests/containment.rs`
  `a_bwrap_planted_on_the_runs_path_never_starts_a_run` (AC-8; no skip without namespaces),
  `host::tests::a_host_program_is_an_executable_file_in_an_absolute_directory`,
  `yulius-sandbox` `enter::tests::a_bare_program_name_is_refused_before_anything_is_started`;
  `yulius-git` `tests/git_ops.rs` `managed_attributes_follow_no_link_an_agent_planted` (AC-9),
  `the_push_guard_follows_no_link_an_agent_planted` (AC-10), the existing guard, scrub and
  union tests (HWR-004). Each new test failed on `dev` first (Evidence).

- Slice 3: `yulius-app` `transcripts::tests::a_transcript_is_opened_without_following_links_or_waiting`
  (AC-11), `record_numbers::tests::a_linked_directory_lists_no_names` (AC-13); `yulius-harness`
  `tests/tail.rs` `a_fifo_or_a_link_in_the_transcripts_place_is_refused_at_once` (AC-11);
  `yulius-sandbox` `manager::tests::a_sandbox_s_passwd_is_read_without_following_a_link`
  (AC-12), `usage::tests::a_linked_root_or_folder_is_not_walked` (AC-14); `yulius-git`
  `review::tests::an_untracked_file_is_read_without_following_a_link` (AC-15),
  `tests/git_ops.rs` `the_push_guard_follows_no_link_an_agent_planted` (AC-16, extended) and
  `managed_attributes_follow_no_link_an_agent_planted` (its linked-file case now asks with
  lines, so a writer that followed the link would copy the host file).

## Decisions

- [ADR-0313](../../docs/decisions/0313-redaction-fingerprints-are-fixed-per-sandbox-and-say-a-few-bits.md):
  a fixed set per sandbox; a tag of 3 to 16 bits of a hash of the first 8 bytes instead of the
  rolling hash; a run's set without seeds and, in a playground with guests, without agent
  secrets; live sessions' files rewritten at start, and kept in the old format for sessions
  launched before version 2. It tables what a holder of a set can still compute. Rejected
  there: limiting requests, a fixed base, bits of a whole-needle rolling hash, more or fewer
  tag bits, redacting on the server, leaving production secrets out, restarting sessions, the
  old format on the endpoint.
- [ADR-0314](../../docs/decisions/0314-the-host-starts-bubblewrap-by-its-own-path-and-writes-git-files-through-handles.md):
  bubblewrap by the process host's own `PATH`; the clone's attributes and guard files through
  no-follow handles, the clone named by the caller. The server's own `git config` writes and
  what Git itself writes or runs in an agent-writable clone are ADR-0114's remaining hole
  (finding G1, a later slice).
- [ADR-0315](../../docs/decisions/0315-what-the-host-reads-in-a-sandbox-it-reads-through-handles.md):
  the five readers through handles; the adapter hands on the text of `passwd`, not its path.
- The tag hash has no key: the redactor needs whatever the server uses, so a key would be in the
  set. Equal tags do show that two sandboxes hold a value with the same beginning; the digests,
  salted per sandbox, do not show that it is the same value.
- The audit's slices are separate pull requests of this task instead of chained tasks: the
  findings share one ledger and one reviewer's context, and a slice's fix often changes how the
  next is best made.

## Findings ledger

State: `fixed` (merged), `slice N` (planned), `accepted` (a recorded decision, named).

| # | Area | Finding | Severity | State |
| --- | --- | --- | --- | --- |
| V1 | vault | Fingerprints with a per-request rolling-hash base give every value away | critical | fixed (slice 1) |
| V2 | vault | A playground with guests still gets fingerprints | medium | fixed (slice 1) |
| C1 | containment | A run's `bwrap` is looked up on the launch PATH, whose first entries are in the sandbox home | critical | fixed (slice 2) |
| D1 | git | `set_managed_attributes` reads and writes `.git/info/attributes` by path (host file read and write through links) | critical | fixed (slice 2) |
| D2 | git | The pre-push guard is written by path into the new worktree's Git directory; `git config --worktree` (G1) and `review.rs` (slice 3) open by path too | high | fixed (slice 2) |
| G1 | git | The host's own Git works by path in the agent-writable clone: its `git config` writes and index writes follow links, and a repository's configuration can make it run commands (ADR-0114's remaining hole, #54 part B, #486) | critical | slice 3a |
| C2 | containment | `upper/etc/passwd` and `group` are read through links, unbounded, into people's `/etc/passwd` | high | slice 3 |
| C3 | host reads | Transcripts in the sandbox home are opened by path (FIFO blocks threads, links followed) | medium | slice 3 |
| C5 | host reads | `usage::measure` walks by path | low | slice 3 |
| I4 | host reads | `record_number` lists a directory through a link | low | slice 3 |
| A1 | access | A `read` API token reads the working tree (ignored `.env` included) over REST GET | medium | slice 4 |
| A2 | access | GitHub sign-in is not bound to the browser that started it | low | slice 4 |
| A3 | access | Revoking an API token leaves its open terminal sockets typing for up to 30 s | low | slice 4 |
| A4 | access | `GET /api/auth/github/start` stores states without a limit | low | slice 4 |
| W1 | web UI | The brand lab ships in the binary: `?lab=` injects CSS, which makes a device approval one blind click | high | slice 4 |
| D3 | delivery | The merge gate never pins a task pull request's base to the integration branch | high | slice 4 |
| P8 | delivery | A production-stage environment run starts for a writer with `confirm: true` (ADR-0110: an administrator typing the slug) | medium | slice 4 |
| E1 | browser | A viewer's mouse and keys drive a Chrome on the host: file chooser, context menu and DevTools reach host files | critical | slice 5 |
| E2 | browser | Browser tools reach other sandboxes' ports, host loopback services and link-local addresses | high | slice 5 |
| E3 | browser | `browser_fill_secret` checks a cached URL, then types after waiting for the input lock | high | slice 5 |
| V3 | vault | An edit re-seals the value it read: a concurrent rotation is undone | medium | slice 6 |
| V4 | vault | Redaction that cannot load the secrets passes text through in the terminal and the proxy, withholds elsewhere | medium | slice 6 |
| V5 | vault | Reading a value needs two-factor; sending it to a new host does not | medium | slice 6 |
| V6 | vault | "May this production secret reach this run" is decided four ways | low | slice 6 |
| D4 | CI | `ci_rerun` picks other jobs than the run it repeats | medium | slice 7 |
| D5 | delivery | "Checks green at this head" has five copies and three rules; a promotion never asks Yulius CI | medium | slice 7 |
| D6 | delivery | An armed merge outlives its task's run, and a restart decides it the other way | medium | slice 7 |
| QB1 | interface | The internal listener maps errors to statuses by a second table; `yulius wait` reads Yulius's own 502 as an outage | medium | slice 7 |
| QA1 | lifecycle | A start that has to queue forgets how it was to start | medium | slice 8 |
| QA2 | lifecycle | A failed re-login turns `relogin` into `error`, and the next usage reading into `ok` | medium | slice 8 |
| QA3 | lifecycle | After a restart a hand-over no longer restarts a run holding the previous owner's personal MCP servers | medium | slice 8 |
| QA4 | lifecycle | The `/goal` still to be typed into a resumed session is lost at a restart | medium | slice 8 |
| QA5 | lifecycle | A background probe without an answer means "busy" everywhere but at a subscription switch | medium | slice 8 |
| QA6 | lifecycle | "May Yulius type now" has three rules; `/compact` is typed behind a person's draft | medium | slice 8 |
| QA7 | lifecycle | A switch that times out records the old run lost without proof | medium | slice 8 |
| E6 | move | A move's report state is a string with three readings of "lost" | medium | slice 8 |
| E7 | move | An unreadable move mark does nothing and stays | low | slice 8 |
| QC8 | store | `tasks.closed_at` is never cleared | low | slice 8 |
| W2 | web UI | Sidebar groups (client) and counts (server) decide a paused task differently | medium | slice 8 |
| W3 | web UI | A 403 on a live edit leaves the document unsaved for good | medium | slice 8 |
| P1-P7, P9-P14 | documents | Contradictions between documents, shipped texts and code (promotion, hand merges, spec template, docs and chore workflows, UI copy, API table, guard text, briefing, stale engineering documents) | medium | slice 9 |
| QB9 | documents | Undocumented options and routes, stale architecture sentences | low | slice 9 |
| W7 | web UI | Dead exports and CSS rules | low | slice 10 |
| D7 | GitHub | `webhook_secret` is stored and never read | low | slice 10 |
| E5 | previews | The preview bridge has no listener since ADR-15 | low | slice 10 |
| QA9, QC2, QC5, QC7, QB4, QB6 | dead code | Store functions, task events and reasons, contract items, a one-shot workflow shim, Cargo features nothing uses | low | slice 10 |
| W4, W5, W6 | web UI | The unsaved-changes guard five times, the pull request chip three times, byte and commit formatters | low | slice 11 |
| QB2, QB3, QB7, QB8 | duplication | Three loopback HTTP clients, the device contract twice, the Superagent's tools dispatched twice, five bearer parsers | low | slice 11 |
| QC3, QC4 | duplication | RFC 3339 parsing four times with different inputs; the GitHub-token scrub twice | low | slice 11 |
| QA8, QC1, QC6, QB5 | efficiency | Store round trips per task, unindexed capacity scans, the watched transcript re-read every 1.5 s | medium | slice 12 |
| C4 | containment | The engine leaf's limits are writable from a nested cgroup namespace | low | slice 5 |

## Evidence

### Slice 1 (2026-11-13)

- **The defect, on `dev` d044172cb.** A temporary test asked `/internal/scrub/fingerprints` 28
  times with a run's token, built the 28 equations from `base` and the first needle's `rolling`
  and solved them modulo 2^61 − 1: `recovered from 28 requests: sk_test_51NzQ9vaultValue8f3a`,
  the test's 28-character value.
- **The fix.** `secrets::repeated_fingerprint_requests_answer_the_same_set` (which replaced that
  test) passes: equal answers, no `base`, no `rolling`.
- **The old wrapper, for real.** In a checkout of `dev` d044172cb, a temporary test of
  `yulius-harness` gave the wrapper there the answer and the fingerprint file this server made
  (dumped from `secrets_scopes::a_starting_server_rewrites_the_fingerprint_files_of_live_runs`):
  the answer alone: `Unavailable { server: Unusable, file: None }`; with its file: `loaded:
  version 1 base 0 names ["MY_TOKEN", "SECOND"]`, a text with the raw and the hexadecimal form
  of one value and a second value came back as `A=[secret:MY_TOKEN] B=[secret:SECOND]
  hex=[secret:MY_TOKEN]`, and `run_scrub_exec` of `echo run=<value>` printed
  `run=[secret:MY_TOKEN]` with exit 0.
- **Speed** (release build, 1 MB of text, a probe test in `yulius-harness`, secrets of 24 to 43
  characters and every tenth of 3000):

  | Secrets | Needles | `dev` | This change |
  | --- | --- | --- | --- |
  | 10 | 60 | 460 ms | 126 ms |
  | 40 | 240 | 781 ms | 205 ms |
  | 200 | 1200 | 819 ms | 314 ms |

- **The old wrapper's speed on the file it now gets** (the same probe in the checkout of `dev`,
  `base` 0 and the last byte as its hash): 614 ms, 896 ms and 923 ms for 10, 40 and 200
  secrets, against 460, 781 and 819 ms with the file it had.
- **Gate.** `scripts/dev ci` at 106bbb5e1: exit 0; `4112 tests run: 4112 passed, 16 skipped`; doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed. (At 6535968fe, before round 2:
  exit 0, `4112 tests run: 4112 passed, 16 skipped`.)
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (head 4b5e66fdb): request changes, one blocker and four major findings,
  all in this change's code, all taken:
  - the rustdoc link to a private function failed the gate: backticks;
  - "a store error answered an empty set": the guest question is asked with `?`
    (`matcher_for`), tested with the guests' table dropped;
  - "the empty set drops redaction of values its runs still hold": a guest playground keeps
    its MCP servers' values, tested through the wrapper's matcher; `.env` leftovers are stated
    in the ADR;
  - "a holder of the set learns more than the ADR says": tags went from 4 to 3 bits with the
    hexadecimal cases and base64 alphabets folded, two-factor seeds left a run's set, and the
    ADR tables the search that is left by value shape (16 letters and digits: 2^62);
  - "live contained sessions stop working after the update": a session from before version 2
    keeps a file in the old format with `base` 0, proved with the wrapper of `dev`;
  - minor: the Superagent branch and a failing rewrite are tested, the assertions that could
    not fail are gone, the launch file equals the endpoint's answer and the salt survives a
    restart (tests), files are rewritten at a guest's join, SPEC-002 no longer says "the
    rolling hash", the matcher's corners are tested, this section exists, a failed rewrite
    empties an old file; nits: `checked_add`, the salt without a silent zero, a window hashed
    once per length.
- **Review, round 2** (head cb3b3587a): request changes, one major and seven minor findings in
  this change's code, all taken; every finding of round 1 confirmed as answered, the ADR's table
  re-derived by the reviewer:
  - "the two-factor seed exclusion has no test": the fixture with a seed now shows the run's
    set leaving the seed alone and Yulius' own redaction replacing it;
  - "an emptied old-format file loses its format": a failed rewrite leaves a file of this
    server as it is and turns an older server's into `{"version":1}`; the test fails, heals
    and fails again;
  - "the ADR understates the base-0 file": the ADR says what it tells (about the last three
    characters), for which values, and the search that is left; operations and the change log
    say to rotate once the sessions from before the update have ended;
  - "20 random characters is wrong for hex": 20 letters and digits or 32 hexadecimal digits;
  - "the cache bump at a guest's join sits after a fallible call": moved to the join itself;
  - the Vidoc ignore's line and comment; the run that cannot be rewritten is seeded first and
    last; the old wrapper's timing is measured (above);
  - nits: one `agent_context` call decides the branch, the Plan item says what the code does,
    the read is bounded, the corner test names the secret, and #1239 tracks the removal of
    the old format's writer. The Global Superagent has no fingerprint file and no test of
    that absence.

### Slice 2 (2026-11-13)

- **The defects, each by a test that failed before its fix** (on the code of `dev`):
  - `a_bwrap_planted_on_the_runs_path_never_starts_a_run`: failed, the stand-in `bwrap` in
    `<sandbox>/home/.cargo/bin` had left its marker.
  - `managed_attributes_follow_no_link_an_agent_planted`: failed at its first check, the file
    outside the repository held `*.png binary` and Yulius's block instead of `kept`.
  - `the_push_guard_follows_no_link_an_agent_planted`: failed, the file outside the repository
    held the `pre-push` script instead of `kept`.
- **After the fix** all three pass, with the 95 tests of `yulius-git` (the guard, the scrub and
  the union merge among them) and `delivery::the_changes_log_merges_by_union_in_any_repository`.
- **Gate.** `scripts/dev ci` at 70e27996a (on `dev` b4eaca1e5 merged in): exit 0; `4117 tests
  run: 4117 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested here.** A joined run (`yulius sandbox-exec` after the holder's namespaces): this
  sandbox has no holder to join; the refusal of a bare name there is a unit test, and the
  process host's lookup, which every launch passes first, is tested end to end.

### Slice 3 (2026-11-13)

- **Tests.** The seven tests named in the Test plan pass. Against the code before the fix:
  `the_push_guard_follows_no_link_an_agent_planted` failed without the comparison of the common
  directory (run with the comparison switched off: the install into the directory elsewhere was
  not refused); `managed_attributes_follow_no_link_an_agent_planted` failed on `dev` in slice 2.
  The transcript, tail, `passwd`, listing, usage and review tests were written with their
  fixes: on `dev` the readers they cover take a path (`File::open`, `is_file()`,
  `read_dir(path)`), their FIFO cases would not return, and their link cases read the target;
  they were not run against `dev`.
- **Gate.** `scripts/dev ci` at fc7de6a02 (on `dev` 5f581c3f3 merged in): exit 0; `4123 tests
  run: 4123 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 2** (head 7248a0cec): approve, no blocker or major; its four minor findings
  and two of its nits are taken in this slice (HRD-006, the test case, the change log's
  wording, `ok_or_else`, a test that cannot replace its own process); "each new test failed on
  `dev` first" in the slice 2 Test plan holds for the three tests its Evidence names, not for
  the two unit tests of functions that are new; the directory is not synced after a rename in
  `replace_in`, as before the change.
- **Not tested here.** A sandbox with a real system layer and holder: `system_file_text` is
  tested on folders laid out like one.

