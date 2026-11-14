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
     directories, disk usage (C2, C3, C5, I4): merged, pull request #1242;
  4. access rules of the public API and the merge gate (A1-A4, W1, D3, P8): merged, pull
     request #1243;
  5. the vault's rules on every path (V3-V5): merged, pull request #1244;
  6. the sandbox browser's own windows: a viewer's real input reaches Chrome's file dialogs,
     DevTools and `file:` pages on the host (E1): merged, pull request #1245;
  7. the sandbox browser's secret typing (E3): merged, pull request #1247;
  8. what a secret's keys can still reach: a focused frame of another site, a navigation
     under way, and its user name and one-time code typed without the rule (E8, E9, E10):
     merged, pull request #1248;
  9. the sandbox browser's network (E2): merged, pull request #1250;
  10. the host's own Git in a clone its agents write (G1): merged, pull request #1251;
  11. production secrets decided once (V6): slice 12 of the Plan: merged, pull request #1253;
  12. delivery and CI decided once (D4-D6, QB1): D4 is slice 13 of the Plan (merged, pull
      request #1254), D5 slice 14 (merged, #1257), D6 slice 15 (merged, #1263), QB1 slice 16 (merged, #1265);
  13. lifecycles decided once (QA1-QA7, E6, E7, QC8, W2, W3, D8): QA5 and QA7 are slice 17
      of the Plan (merged, pull request #1268), QA2 slice 18 (merged, #1271), QA1 slice 19
      (merged, #1272), QA3 and QA4 slice 20 (merged, #1274), QA6 slice 21
      (merged, #1276), E6, E7 and QC8 slice 22 (merged, #1277), W2, W3 and D8 slice 23
      (merged, #1279);
  14. documents and shipped texts against the code (P1-P14, QB9);
  15. dead code (W7, D7, E5, QA9, QB4, QB6, QC2, QC5, QC7): W7, D7 and E5 are slice 24 (merged,
      #1281), the rest slice 25;
  16. duplicated code (W4-W6, QB2, QB3, QB7, QB8, QC3, QC4);
  17. efficiency (QA8, QB5, QC1, QC6);
  18. the web UI's script and style policy (W1b), and the engine leaf's limits (C4): two small
      changes of their own, taken out of slice 9, which is one concern;
  19. the connection that carries the server's token in a clone its agents configure (G2,
      found in slice 10): taken as slice 11 of the Plan (the earlier number 11, V6, follows
      it): merged, pull request #1252;
  20. no command that reads a clone's configuration may start the shell (G5, from the review
      of slice 11): taken into slice 11 itself, whose own step it concerns: merged with it.
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

### Slice 4: access rules of the public API and the merge gate

- **A1 (medium).** The server refuses a `read` token by method only (`yulius-server` `auth.rs`);
  `files.rs` (`file_target`, `files_roots`), `shells.rs` `shell_list`, `preview.rs`
  `preview_info`, `cockpit.rs` `cockpit_link` need Work and never look at the scope;
  `cross_sandbox.rs` `superagent_requests` checks nothing.
- **A3 (low).** `revoke_api_token` (`yulius-store` `sessions.rs`) does not grow the access
  epoch; a write grant is re-confirmed only after 30 s.
- **A2, A4 (low).** `github_start` sets no cookie and is unlimited; `github_callback` checks
  the state only.
- **W1 (high).** `build.rs` embeds `lab.js`; `app.js` loads it for `?lab=`; `lab.js` builds a
  stylesheet from the address; `/device?code=…` approves with one click.
- **D3 (high).** `dlv_merge` and `merge_armed_task` read the pull request's base and use it
  as the integration branch; `check_merge` has no fact about it.
- **P8 (medium).** `dlv_start_environment_run` needs Work and `confirm: true` whatever the
  environment follows.

### Slice 5: the vault's rules on every path

- **V5 (medium).** A reveal asks for the caller's two-factor unlock (`fresh_or_status`:
  `reveal_agent_secret`, `answer_copy_request`). `update_agent_secret` (new `hosts`,
  `auto_allow: true`), `answer_host_request` and `answer_env_files` ask for write access only,
  by a session or a token; the remote MCP's `secret_update` and `secret_hosts_answer` call
  them. A secret with `auto_allow` on is bound to hosts by agents themselves
  (`requests.rs` `auto_bind`), so only one with it off is guarded by its bindings.
- **V4 (medium).** `terminal_redactor` turns a failed load into `None`; `scrub_output` stores
  it with the current generation and passes bytes through. `egress_redactor` turns it into an
  empty matcher; `EgressProxy::respond` then also skips its refusal of compressed answers.
  `terminal_snapshot` and `redact_for_sandbox` withhold.
- **V3 (medium).** `vault_update`'s `WHERE` compares hosts, `auto_allow` and `cd_prod` with what
  the edit read, not `ciphertext`; `update_agent_secret` seals the payload it read again.

### Slice 6: the sandbox browser's own windows

- **E1 (critical), reproduced on Google Chrome 157** with the opt-in test
  `yulius-browser` `real_chromium::a_viewers_click_on_a_file_input_opens_no_file_chooser`: on a
  virtual display a viewer's click is real X input (`display_mouse`, XTest), and a click on a
  page's `<input type=file>` opened GTK's "Open File" window, 1280x800 over the page, in a
  Chrome that runs on the host as the service user. The viewer types a path there (the
  instance's `master.key`, the database, another sandbox's repository) and the page reads the
  file. Anyone with Work access to one sandbox is a viewer, a playground's guest included.
  The same real input reaches Chrome's other own windows: Save as, Print to a file, DevTools
  (F12, Inspect), `view-source:` and `file:` pages; `--kiosk` hides the toolbar, not these.
- **What Chrome offers.** With a managed policy file (`/etc/opt/chrome/policies/managed/`, for
  Chromium `/etc/chromium/policies/managed/`) holding `AllowFileSelectionDialogs: false`, the
  same click opened nothing and `showOpenFilePicker()` answered `NotAllowedError` (probes on
  this machine). A policy directory is root's; `yulius host setup` already writes such files
  and the update helper's `yulius host check` re-applies the safe ones.
- **What does not work here.** A private file system view for Chrome (bubblewrap) would hide
  the host's files whatever window opens; inside a sandbox namespaces cannot be made, so it
  cannot be proven on this machine, and Chrome's own sandbox inside one needs a host test.
  It stays the firmer fix, for the slice that moves the browser into its sandbox (E2).

### Slice 7: the sandbox browser's secret typing

- **E3 (high).** `tool_browser_fill_secret` (`vault/runtime.rs`) takes the tab's address from
  `browser.tabs()`, a cache fed by Chrome's target events, checks it against the secret's
  hosts and calls `tool_type_secret`. `type_text` (`yulius-browser` `instance.rs`) focuses the
  field, waits for `input_lock`, reads the document only to record where the secret went, and
  types. `navigate_session`, `history` and `reload` take no lock; a screenshot holds it from
  its look at the page to the capture. So: a slow screenshot, a fill on a tab at the secret's
  host, a navigation of that tab; the secret is typed into the new page and the tool answers
  `filled` with the secret's host.

### Slice 8: what a secret's keys can still reach

- **E8 (high).** The rule of slice 7 is asked of the tab's document. The keys go to the
  element with the focus (`Input.insertText`), which can be in a frame of another site inside
  a page of the secret's host: a click into the frame before, or a fill without a selector.
  `Document::parse` already lists such frames for screenshots (`framed`), but the typing never
  looked at them. On Google Chrome 157 a page at `127.0.0.2` framing `localhost` had the
  secret typed into the frame's field and answered `filled`.
- **E9 (medium).** A navigation asked for before the fill (a clicked link, an Enter) is not
  one of Yulius's own that wait for the lock. On Chrome 157 `Page.getFrameTree` answers only
  when the new document is there (measured with servers answering after 1.5 and 4 s, to
  another site and to the same one), so slice 7's rule refuses the new page there; nothing in
  the protocol promises that, and `Page.frameStartedNavigating` names the address at once.
- **E10 (high; found by the review of this slice, on `dev` before it).**
  `tool_browser_fill_secret` types the fields `username` and `totp` with `browser.tool_type`:
  only the cached address of `browser.tabs()` is checked (E3's pattern), and none of this
  slice's checks apply. A one-time code is a credential while it lasts.

### Slice 9: the sandbox browser's network

- **E2 (high).** A sandbox's Chrome runs on the host (`BrowserManager::launch` spawns it as
  the service user) and connects by itself. Its only limit is BRW-008's `UrlDenyList`: URL
  patterns for Yulius's public origin and listed loopback spellings of its listeners, matched
  by Chrome's `Fetch` interception. Through `browser_open` and `browser_navigate`, a page's
  own requests or a viewer's address bar it reaches what no process of the sandbox reaches:
  another sandbox's dev servers (the host relays every sandbox's port range from its
  loopback), services on the host's loopback, `169.254.169.254`, and the internal listener by
  a name that resolves to it. The egress proxy already has the rule for agent runs
  (`egress_destination_allowed`, applied to what it resolved itself). Chrome 157 sends
  `localhost` past `--proxy-server` unless `--proxy-bypass-list=<-loopback>` is given
  (measured with a logging proxy, `target/audit/slice9/probe_proxy.py`).

### Slice 10: the host's own Git

- **G1 (critical).** `yulius_git::runner` starts `git` as the service user with three
  constant options (`protocol.ext.allow=never`, `core.fsmonitor=false`,
  `core.hooksPath=/dev/null`), in clones and worktrees whose `.git/config` and every other
  file the agents write. Run against such a clone (`git_confined` tests, the confinement
  taken out), the adapter's Git executed what the clone named as `filter.<name>.process`,
  `core.sshCommand` and the remote's `uploadpack` and `receivepack`; a fetch wrote
  `FETCH_HEAD` through a link over a file outside the sandbox; and with the remote's address
  changed to another sandbox's origin or clone, the server pushed there and merged from
  there. By hand: a link at `.git/shallow` made Git print a line of the file behind it
  ("fatal: bad shallow line: …"). A fetch's environment holds the token
  (`http.<url>.extraheader`).
- **Landlock** is what the kernel offers an unprivileged process for "execute these files
  only, touch these folders only" (ABI 8 on this machine; the `landlock` crate, no `unsafe`
  of ours). Measured by hand before building on it: a confined `git status`, `log`, a local
  fetch with the shell allowed and an HTTPS `ls-remote` work; a filter's command fails with
  "cannot exec 'sh -c …': Permission denied"; Git starts the other side of a transfer between
  two folders through `/bin/sh` always.
- **G2 (new, high).** A key for a URL in the clone's configuration
  (`http.https://github.com/.sslVerify=false`, `.proxy`) beats the server's command line and
  environment (`git config --get-urlmatch`, `target/audit/slice10/urlmatch-proof.log`): the
  connection that carries the server's token can be sent through a proxy in the sandbox's
  own ports without a certificate check. Not fixed by confinement; slice 19.

### Slice 11: the server's token on a connection the clone shapes

- **G2 (high).** `GitAdapter::git_with_token` passes the token as
  `http.https://github.com/.extraheader` in `GIT_CONFIG_*`, and `fetch`, `push` and
  `remote_branch_head` ran `git fetch origin`, `git push origin …` and `git ls-remote origin`
  in the clone or a worktree. The clone's configuration applies there, and a key for a URL
  beats a general one from any source (slice 10's proof). The merge after the fetch ran with
  the same environment.
- **The loader** (review of slice 10, and by hand): `/lib64/ld-linux-x86-64.so.2 <file>`
  runs any file it can read, and the loader must stay executable. With `diff.external` set to
  the loader and a new file in a folder that is a program, a bare confined `git diff`
  executed that file (`target/audit/slice10/loader-gadget.log`: "added.bin: '/dev/null':
  Permission denied" is the copied `env` trying to run its argument). The adapter's own diff
  commands already carried `--no-ext-diff --no-textconv`, each at its call site.
- **The loader through `core.askPass`** (review of this slice, reproduced by the reviewer
  under the launcher): a remote without `remote.<name>.url`, given an address by
  `url.<base>.insteadOf` or a file in `.git/remotes`, was named to Git, which made the
  transfer in the clone; a server of the agent's answering 401 made Git start the clone's
  `core.askPass`, the loader, with the question, whose text holds the address and so a path
  to a file of the clone's.
- **G3 (new, high; found while answering the review).** A clone can name a "promisor"
  remote (`extensions.partialClone`, `remote.<name>.promisor`) that Git fetches missing
  objects from, with `remote.<name>.uploadpack` as the command for the other side. In a
  transfer between two folders the confined Git may start the shell, and the shell ran that
  command: a push of a commit whose tree no repository has, to a playground's origin
  (`target/audit/slice11/lazy-fetch-proof.log`). There since slice 10.
- **G4 (new, medium; from the review's F5).** Two things the server does with a token or
  on its own authority were done in a sandbox's folder: a clone made anew (a sandbox's first
  clone, and any later one when the clone's `.git` is gone, which its agents can arrange)
  wrote the new repository's configuration and then connected with the token; and the first
  commit of an empty repository was made in `<sandbox>/seed`, where the agents could add
  files to a commit that goes to the default branch without a review.
- **Two more of G3's kind (review of this slice, round 2; both reproduced by the reviewer
  under the launcher, and here).** A push to a playground's origin went into the clone's
  submodules when the clone asked for it (`push.recurseSubmodules=on-demand`,
  `submodule.recurse=true`), and a submodule is pushed by its own configuration: its
  `remote.origin.receivepack` ran through the shell (there since slice 10). And the copy of a
  new clone from its courier (added in round 1) checked the files out in a command that may
  start the shell, while the new repository's configuration was already the sandbox's to
  write: a filter appended to it ran.
- **G5 (the class of G3, high).** Wherever a command that reads a clone's configuration may
  start the shell, each way that configuration can make the shell run something has to be
  found and switched off. Round 3 of the review found the next: `fetch.bundleURI` in the
  clone makes a fetch download a bundle over HTTP whatever transports the command allows,
  and when the address asks for a password Git starts the clone's credential helper through
  the shell (reproduced by the reviewer on the folder fetch; here by the new test against
  the code of round 2, `target/audit/slice11/n3-before.log`: "the clone's bundle address
  was connected to", four times). The commands concerned were a playground's fetch, push
  and listing (slice 10), and the clone's fetch from a courier and a new clone's copy (this
  slice). Closed here by a rule instead of a list: no such command may start the shell.

### Slice 12: a production secret that some runs still got

- **V6 (low).** `EgressClient::may_use` answered `true` for an agent run whatever `cd_prod`
  was, and `!cd_prod` for a Superagent and a CI job. A production secret has no hosts since
  SPEC-134, so the difference showed only for a row made a production secret before that,
  which kept its hosts: the proxy sent it there for an agent's run, while
  `browser_fill_secret`, `secret_copy_to_env`, a `${secret:NAME}` reference, the Superagent
  and CI refused it and `secrets_list` said "agents never use it, not through hosts".
  SPEC-308 put the state into the Edit dialog ("agents' requests to them still get it").
  The store's insert of a whole row did not check the pair.
- **From the review of slice 11 (a nit, pre-existing):** a CD mirror's fetch from a
  sandbox's clone, the copy of an upload's refs and a refresh of branches outside a sandbox
  got write access to the folder they only read.

### Slice 13: which jobs test a commit

- **D4 (medium).** "Which jobs test this head" was answered in five places: the scheduler
  for a pull request (`ci_sync_sandbox`: its own jobs when it changed them, with the trusted
  security job kept; its own when the integration branch has no CI and no broken
  configuration), a merge train (`form_train`: a copy), a playground task's merge
  (`ci/playground.rs`: a copy without the security job and without the broken-configuration
  case), a change from the remote control (`remote_files.rs`: "does it bring CI"), and a run
  again (`ci_rerun`), which took the integration branch's jobs whenever that branch has CI.
  A pull request that tightens a job and fails with it passed when run again (by a person,
  by the agent's `ci_rerun`, or by Yulius after an errored run), and `ci_gate_result` takes
  the newest run of a head as its verdict.

### Slice 14: what the checks at a commit say

- **D5 (medium).** "Are the checks green at this head" was decided in five places by three
  rules. `dlv_observe` (a task's delivery) and `git_overview` (the Git page) asked Yulius
  CI's run of the head and, without Yulius CI, GitHub's checks, each with its own copy.
  The remote control's merge (`remote_files.rs`) added two cases (a pull request that
  brings CI is pending; a broken configuration refuses). A promotion pull request read
  GitHub's checks only, with `ChecksState::None` passing: when a run's status was not
  published, a failed Yulius CI run at the integration head read as no checks and the
  promotion merged with `require_checks` on, although `docs/engineering/git.md` says the
  head "must have a successful `yulius/ci` status". A fast-forward promotion read no
  checks at all.
- **From the review of slice 13 (its sixth point):** `ci_gate_result` said "not configured"
  for a commit without a run in a project without CI, also when the commit brings jobs the
  scheduler is about to test: a merge that requires checks could go through before that
  first run existed. Only the remote control compensated.

### Slice 15: an armed merge and its task's run

- **D6 (medium).** The life of an armed merge was the pair of the slot (`slot.merge`) and
  the task's status, with no rule for "armed, and the task no longer runs".
  `merge_armed_task` never read the status: a task a person stopped or closed after its
  agent armed was still merged when its head was green and up to date; green and behind,
  every pass answered Keep for ever, because bringing a branch up to date
  (`keep_armed_current`) and joining a merge candidate both need a running, idle agent.
  `restore_armed_merges` used another rule: after a restart every task not exactly
  `Running` lost its armed merge, also one in `WaitingCapacity` or `Switching`, although
  the documents say armed merges are restored.

### Slice 16: the statuses of the agents' own listener

- **QB1 (medium).** The server had three hand-written tables from an application error to
  an HTTP status: `error::status_for` (the public API, and `/internal/scrub/*`),
  `routes/internal.rs` `status_of` (hooks, `yulius wait`, the Git credential helper: `Invalid`
  400, everything else but four kinds 502) and one in `routes/mcp.rs`. Two of them served
  the same loopback listener and disagreed (`Invalid` 400 or 422, `Internal` 502 or 500,
  `Capacity` 502 or 429). `yulius wait` takes 502 to 504 for "cannot be reached" and waits
  it out without counting it: a failure inside Yulius at `/internal/agent/wait` (any store
  error) kept an agent's wait going to its deadline, 90 minutes by default, and then told
  it "When it restarts, Yulius wakes this task" although Yulius never stopped.

### Slice 17: what a switch may end

- **QA5 (medium).** "Would a restart of this session lose background work" was asked in
  four places and answered in two ways when the look at the processes gave no answer (it
  timed out under load, or the process table could not be read). Disk hygiene, the restart
  on a newer Claude Code and the queue's rule take "not known" as work running. The turn's
  end that moves a session to another subscription took it as none
  (`shells_running_now`: `== Some(true)`) and ended the session, with the build, gate or
  review the agent had left running.
- **QA7 (medium).** A switch waits for the session it asked to end, half of the switch's
  two minutes. When that time passed it called `terminate`, threw the answer away, wrote
  the run down as lost itself and started the new session. A host that could not be
  terminated left two agents in one worktree on one conversation. A host that could be
  left the switch's own intent in the task's slot (the next run's end would have been taken
  for a switch: "switched", and the task left running with no run), and raced the run's
  reader: whichever recorded the loss first decided whether a new session started.
  Everywhere else a run whose host had to be killed goes through one path
  (`terminate_then_mark_lost`, SPEC-065 END-001), and a switch whose old run is lost starts
  none (SPEC-250 STW-006).

### Slice 18: a subscription that needs a new sign-in

- **QA2 (medium).** "This subscription works" was written four times (`ok` or `limited`:
  the pool's input, the capacity loop, the agent gate, the login failure) and "this one
  needs a sign-in" three more (`relogin` or `pending_login`: the usage round, the e-mail
  round, the usage write), and the two were not each other's opposite: `error` was in
  neither. A subscription Claude had refused (`relogin`, credential kept) whose new
  sign-in then failed or timed out became `error`; the background round asked `error` for
  its usage, a reading stored `ok`, and tasks were placed on the refused subscription
  again, each losing a turn to the refusal. The administration page had the rule a fifth
  and a sixth time, and asked an `error` subscription for its usage by itself.

### Slice 19: the start that waits in the queue

- **QA1 (medium).** "How the next run starts" (a new session or a resume, the prompt or
  continuation it types, a resume past a usage limit, the subscription asked for) was only
  the arguments of `start_run`. A start that met the session limit, a supertask's hold or
  an update's hold made the task `queued` and dropped them; the queue then started every
  task as a plain resume (or a plain new session) on the pool's choice. A task paused at a
  usage limit that became resumable while every slot was taken came back as a plain
  resume: the transcript not stepped back, the interrupted turn not continued, so a
  prompt-mode task sat at the limit's error reading Working. A person's held prompt
  carried by a pause was lost without a word. And "Continue on…" a chosen subscription
  ran on the pool's choice, against SPEC-447 MOV-002 ("never lands elsewhere unsaid").

### Slice 20: what a restart forgot about a run

- **QA3 (medium).** Whose personal MCP servers a live run was started with was only in the
  server's memory (`TaskRuntime::personal_mcp_of`). SPEC-332 MSC-009 restarts such a run
  when control passes, "so the previous owner's credential never stays with the new
  owner", and lets only its owner message it meanwhile. After a restart of Yulius (every
  update) the reattached run held the same servers and nothing knew: a hand-over restarted
  nothing, and the new owner typed into a session whose MCP configuration carried the
  previous owner's bearer.
- **QA4 (medium).** A resumed goal session (a switch, Continue, a resume past a limit)
  gets its `/goal` typed once its opening turn ended, by a task of the server (up to five
  minutes later). A restart in that time left the reattached session with no goal, no
  typist and no word: a goal task idled as "your turn". The same gap for a goal given as
  the launch prompt was closed with a mark in the run directory (SPEC-146).

### Slice 21: who may type into a session

- **QA6 (medium).** "May Yulius type into the session now" had three rules. A message
  (a person's, a pull request's news, the wake after an outage) is kept out while a
  person's draft may sit in the prompt box, however old, and while another message is
  being typed (SPEC-480, `type_into_session`). `/compact` and the line that wakes a
  compacted session asked only whether a person typed since the refusal, and the goal's
  typist only whether one typed since the run was attached; neither took the one-typist
  mark. A person who typed their next instruction while the turn ran had a draft in the
  box from before the refusal: `/compact …` was appended to it and Enter pressed, the
  draft was submitted garbled (what DRF-001 forbids), refused again, and the episode
  stayed "compacting" until it fell to a fresh session. And a message could land in the
  300 ms between a typist's line and its Enter.

### Slice 22: what a move's report, a move's mark and a close time say

- **E6 (medium).** The move helper's report was read into a string, "running", "lost",
  "ended" or "refused", matched with catch-all arms, and "lost" (the report says running
  and is more than 60 s old) meant three things: no move for `move_in_flight`, so an
  update or a restart could stop the server (SPEC-470 MVB-023 forbids it); a stopped
  move for `movable`, so the card offered to continue it; and a recorded move for
  `recorded`, which offered Abort. A helper still copying, whose report write waits
  behind a saturated disk, was all three. The helper's oneshot unit is `activating`
  while its command runs, and systemd already answers one question about the path unit.
  The helper's `run_command` also said the server tells an earlier run's checklist "by its
  time"; it does so by the request id since SPEC-470's last slice.
- **E7 (low).** `host_move::read_mark` read the move's mark as UTF-8 text and gave up
  when it was not: `paused_before_a_move` saw the mark (`exists()`), `revive_after_move`
  did nothing, so the lost tasks stayed lost and the mark stayed for every later start,
  against its own comment ("a mark that cannot be read is still a mark").
- **QC8 (low).** `tasks.closed_at` was set at every change to `closed` and never
  cleared: a task continued after a close kept its old close time, which the analytics
  export shows as "Closed" and the project list as `closed_at_ms`; a repeated close moved
  it. A task created without a start but chained or prepared (no caller does so today)
  got one too.

### Slice 23: one rule for a task's group, and an edit refused for lack of access

- **W2 (medium).** The sidebar's rule (`status.js`) and the server's (`AttentionFacts::group`,
  which Home's cards, the project tabs and the page title count) disagreed twice. A run paused
  by a person or by an update is Paused under Working in the sidebar whatever it waited for
  (SPEC-191: "It counts in the Working group"); the server had no pause fact, so a paused task
  that had a permission prompt, "Your turn", a held prompt or a browser request was counted as
  needing you. And the onboarding task of a `pending` sandbox, listed in the sidebar's Working
  group as "Waits for Start onboarding", was a closed task to the server and counted nowhere.
  The mock counts with the sidebar's rule, so no e2e test could see it.
- **D8 (medium), the case left.** SPEC-487 CHN-001 (#1270) made a merge that is armed keep an
  idle task out of Done, through `yulius_wait`; an achieved goal (`activity: done`) still
  came first, in the server's rule, in both "is it done" functions of `delivery.rs` and in the
  sidebar. An agent whose goal was judged achieved after it armed its merge was Done: a task
  chained after it started before the merge, and archiving it closed it, which ends the armed
  merge (slice 15) with a reason typed to nobody.
- **W3 (medium).** `livedoc.js` answered a refused edit (403: the person may no longer change
  the folder, such as a worktree whose owner changed) by setting `readOnly` and keeping the
  edit: the file read Unsaved for good, the page asked before every unload, more typing
  piled up unsent, a close waited its full 10 s and dropped it, and `readOnly`, never reset,
  had no reader.

### Slice 24: what nothing reaches

- **E5 (low).** ADR-15 removed the browser panel's frames: previews open in their own window
  (`browser.js`, `window.open(…, 'noopener')`), and no Yulius page has a `message` listener.
  The preview proxy still appended a bridge script to every proxied HTML document (which
  posted the page's address to a parent that does not exist), and for that fetched every
  document uncompressed (`Accept-Encoding` removed) and dropped its `Content-Length`; it
  removed the app's `X-Frame-Options` and `frame-ancestors` and allowed the Yulius origin to
  frame previews. The landing page told of "cookies in frames".
- **D7 (low).** `webhook_secret` was copied from the App manifest's answer into the stored App
  configuration and into `AppCredentials`; Yulius has no webhook route and reads it nowhere.
- **W7 (low).** `linkify` and `URL_RE` (`dom.js`), `kindLabel` (`stages.js`) and the style
  rules `.card-head`, `.card-body`, `.space-bottom`, `.input-priority`, `.group-needs
  .tl-what`, `.release-list` (and its `ul`), `.release-head`, the phone rule of `.hbar` and
  `.term-tab-add` had no user (`.st-blocked` and `.tone-stopped-abnormal`, also reported, are
  built from a status and a tone, so they stay).
- **W8 (low, review of slice 23).** A live document whose rejoin is refused for lack of access
  tried again every second for as long as the file stayed open.

### Slice 25: Rust code nothing reaches

Each item was searched again by name over `crates/`, `e2e/`, `tools/` and `scripts/` before it
went; two of the auditors' items had gained a user since and stay.

- **QC5 (task state machine).** `TaskEvent::StartFailed` and `SwitchAborted` were never fired
  (the supervisor writes a run's statuses itself), `FailReason::StartTimeout` and
  `START_TIMEOUT_MS` never written or enforced (the ptyhost's own spawn timeout ends a start
  as `failed_spawn_error`), and the module doc promised timeouts "enforced by the
  supervisor". `StopReason::SwitchTimeout`, also reported, is written since slice 17.
- **QC2 (contracts).** `UiEvent::SandboxUpdated` (never published) and its web handler,
  `IdResponse`, `SESSION_KINDS`, `CiRunDetail`, five import path constants and two path
  helpers, `DeviceApproveRequest`, `MoveRequest::same_values`, the hook reason `RESUME`, the
  hook input's `permission_mode` and the hook output's `systemMessage`, `continue`,
  `stopReason` and `suppressOutput`.
- **QC2 (domain).** `agent_cli::KEEP`, `Nudge::is_pace`, `spec_slug` (production calls
  `slug_or`), the usage helpers without a model family (`max_used_percent`, `window_reset`,
  `known_total`, the wrappers `current_used_percent` and `available_at`).
  `TokenUsage::is_unknown`, also reported, is read by the harness. `scope::pattern_matches`
  stays: the tests of `patterns_overlap` check it against it path by path.
- **QC7.** "Rotate now" without an account (`Trigger::Manual { account: None }`) was never
  asked; its branch and `Refusal::NoCandidate` were reached by tests only.
- **QA9.** The message inbox readers (`inbox`, `mark_delivered`, `mark_read`,
  `unread_count`; nothing writes messages since SPEC-028), `set_account_error` (tests only),
  `UsageSnapshot::turn_open_since` (written, never read) and `end_turn`'s unused time.
  `insert_message` stays as the only way tests seed the old rows a project's export carries.
- **QB4.** `retire_legacy_workflows` ran at every start for the two workflows seeded before
  the catalog (2026-10-07..10); every instance started since 2026-10-11 has retired them.
  The shutdown's CPU un-split, reported with it, also resets the sandboxes' CPU weights for
  whatever runs next (SPEC-362 CSW-004): it stays, with a true comment.
- **QB6.** `parse_port_range`, the `axum` feature `macros` and the `tower-http` features
  `trace` and `limit`, the routers' unused `App` argument, `SkillSource::is_bundled` and
  `same_repository`. `YULIUS_CD_KIND`, passed to every CD run and read by nothing in this
  repository, is documented instead: a project's deploy script may read it.
- Kept, as the audit's notes allowed: the write-only columns `step_events.from_status` and
  `vault_enabled.enabled_by`/`enabled_at` (who and when, for a person reading the database;
  dropping a column needs a migration for no behaviour).

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

- **ACR-001** Every access check of a request's caller applies its token's scope
  (`App::require_for`): a `read` token gets 403 where the action is above View. The
  Superagent's requests are refused to a read token.
- **ACR-002** Revoking an API token grows the access epoch.
- **ACR-003** The GitHub sign-in start sets a host-only, HttpOnly, SameSite=Lax cookie with the
  state for 10 minutes; the callback without that cookie, or with another state in it, answers
  `expired` and leaves the state usable; the cookie is cleared by every callback.
- **ACR-004** One client starts at most 10 GitHub sign-ins a minute and at most 1000 states
  wait; a start past either goes to the sign-in page with `busy` and stores nothing.
- **ACR-005** `lab.js` and `lab-proposals.js` are not embedded; the page loads the lab only in
  mock mode.
- **ACR-006** A task's pull request whose base is not the integration branch, or that comes
  from a fork, is not merged by a person, cannot be armed and is dropped when armed, with what
  to do; nothing is merged from or into another base on its behalf.
- **ACR-007** The command of an environment that follows production starts only for an
  administrator with the project's slug in `slug`; a manual checkpoint is confirmed as before.

- **VLR-001** For an agent secret with `auto_allow` off, binding a host it is not bound to
  (`PATCH`, the answer to a host request), allowing `.env` copies, allowing it for production
  deploys and switching `auto_allow` on need a browser session whose two-factor unlock is
  fresh: otherwise 403 `reauth_required`
  with the status (a session) or 403 `forbidden` naming the Secrets page (an API token), and
  nothing changes. A description, a name, a value, fewer or the same hosts, a dismissal and a
  revocation need neither. A secret with `auto_allow` on is unchanged; an answer given without
  the unlock because it was on holds only while it still is.
- **VLR-002** The Secrets page asks for the code on those answers and repeats the request;
  closing the dialog leaves the request waiting and an edit form open. The mock refuses as the
  server does.
- **VLR-003** While a sandbox's agent secrets cannot be read, its terminals show one notice and
  no output, and ask again after a second or when a secret changes (not at every chunk); once
  they can, output flows redacted.
- **VLR-007** Yulius types nothing into a terminal whose screen is withheld or misses output
  dropped while it was (until the next whole screen): a first message or a goal waiting to be
  typed is not typed and the task is flagged with what to do; a message for the task
  (`type_into_session`) is refused with the reason. The launch goal's watch types nothing and
  is unchanged.
- **VLR-004** The proxy reads the values to redact before it sends a request and answers 503,
  sending nothing, when it cannot.
- **VLR-005** An edit of a secret, an agent's or the team's, whose sealed value changed since
  it was read is refused (409) and changes nothing.
- **VLR-006** An imported session's transcript and side files are copied up to 1 GiB together,
  counted as they are read; past it the import is refused and the file in hand leaves no copy.

- **BRL-001** `yulius host setup` and `yulius host check` write the browser policy
  (`yulius.json`: no file selection dialogs, no downloads, no printing, the File System Access
  API blocked, no extension installs, only web addresses (so no `file:`, `view-source:`,
  `chrome:` or `devtools:` page and no address handed to another program), and a version
  marker) into the managed policy directories of Google Chrome and Chromium, and report it as
  every other item. The update helper's unit may write those directories: the shipped unit
  lists them, and the setup writes a drop-in for a unit from before.
- **BRL-002** A browser on a virtual display is asked whether that policy is in force
  (a hidden tab, known by its id, navigated to the marker address: blocked by the
  administrator, or not; 3 seconds at most) at the first button or key press; a "no" is asked
  again after 5 seconds, a "yes" after 5 minutes.
- **BRL-003** While it is not in force, a viewer's button presses, key presses and typed text
  on a virtual display are refused and the viewer is told why and what an administrator runs;
  moving the pointer, scrolling, releases, watching, the tab bar, the address bar and the
  agents' tools work as before. A headless browser, which shows no window of its own, is
  unchanged.
- **BRL-005** The probe's tab is never listed; a page that opens a window at the probe's
  address gets an ordinary tab. A target created as a page of Chrome's own is no tab; a tab
  that goes to one stays a tab.
- **BRL-004** `yulius doctor` says whether the policy is installed.

- **FSR-001** `tool_type_secret` takes the rule "may a page at this address get the secret" and
  applies it to the tab's document read with the input lock held; a page that may not gets
  nothing typed, no record of a typed secret, and an error naming where the tab is.
- **FSR-002** `browser_navigate`, `browser_open`'s navigation, a viewer's address bar, back,
  forward and reload take the input lock for their request to the browser. (A navigation
  already under way when a typing begins is FSR-003's case and finding E9.)
- **FSR-003** When the document changed while the secret was typed and the new one may not get
  it, the tool fails naming where the tab went, whatever else the typing ran into, and the
  secret's activity records it; a page
  that cannot be identified afterwards keeps SPEC-450's rule (every screenshot refused).
- **FSR-004** After focusing the field, with the lock held, the page is read. Unless its
  focus is in a text field (`input`, `textarea`, editable content) of a document the probe
  read, every frame of another origin the page holds (in its tree and as targets of their
  own) must be at an address the rule allows; the focus on a fenced frame is refused whatever
  the frames are. Otherwise, and when the page or its frames cannot be read, nothing is typed
  and the error names the frame's site.
- **FSR-005** The browser keeps the address of a navigation to another document Chrome
  announced for a frame (`Page.frameStartedNavigating`; `Page.frameRequestedNavigation` for
  this tab) until the frame arrived, stopped loading or was detached, the tab's session ended
  or 30 s passed; at most 256 frames per tab; of an address over 2048 bytes its site is kept
  (scheme, host and port), or none when that is too long as well (no secret is for it); never
  a cut address. A same-document navigation is not kept. Past the bound nothing is known
  of that tab for 30 s, and when events were skipped, of none.
- **FSR-006** The refusals reach the application as a page the secret is not for (403), the
  reports as a secret gone astray (with its activity entry).
- **FSR-007** Before the keys, after the focus: the document is read again, and one that is
  not the document the rule was asked of gets nothing typed (named when the rule refuses it).
  Then, last, a page with a frame (the main one or one in its tree) on its way to an address
  the rule refuses gets nothing typed and that site is named; when where they are going is
  not known, nothing is typed either.
- **FSR-008** After the keys, a focus that is not in a text field of the page, on a page
  holding a frame the rule refuses (or on a fenced frame), fails the tool naming that site.
- **FSR-009** A secret's user name and one-time code are typed under FSR-001..008, and are
  not remembered as typed secrets (finding E10).

- **BNR-001** With a network configured (`BrowserConfig::network`), a browser is started
  behind a proxy opened for it, with the switches that send every address through it (the
  loopback too), resolve no name locally and send no UDP past it; the proxy is closed when
  the browser stopped, crashed and was replaced, or failed to start; a browser whose proxy
  cannot be opened is not started.
- **BNR-002** The server configures it for every sandbox browser: a listener of its own per
  browser, served by the egress proxy with the sandbox's port range read from its record; a
  browser asked for before the egress proxy runs is not started.
- **BNR-003** Through its proxy a browser reaches, by the addresses the proxy resolved: what
  the egress proxy lets an agent run reach; on the loopback its sandbox's ports only; of the
  internal listener the plain paths of the welcome page only (no other path, no tunnel); of
  the public listener at its own address nothing. The loopback and the welcome page are
  reached only by an address or by `localhost` and names below it (which no resolver is asked
  for); a name resolved to a local address reaches nothing.
  Anything else is refused before a connection is made: 403 naming the sandbox's ports for a
  plain request, 403 to `CONNECT`.
- **BNR-004** A browser's requests are neither intercepted nor changed nor redacted (the
  headers of one hop and what an address carries before its host apart), need no credential,
  never ask the policy of agent runs, and count toward the share of the proxy's connections
  its sandbox's agent runs have. A plain request's answer is waited for with SPEC-369's
  bound, which restarts with every piece of its body sent on.

- **GCR-001** On Linux every Git command of the server's adapter is started through
  `<yulius> git-confined`, which restricts its own process with Landlock and then becomes
  Git: executing is allowed for Git's executable (found by `PATH`), its `exec-path` and the
  program loader; writing for the folders given and `/dev/null`; reading for those, the
  system's folders and the service user's Git configuration. Without an enforcing Landlock
  Git is not started.
- **GCR-002** The folders given are the command's own: for a path in
  `<data>/sandboxes/<id>` or `<data>/import-worktrees/<id>` the existing folders of that id;
  elsewhere the path itself or its nearest existing folder; plus what the command names
  besides (a destination, a bundle, the other repository of a transfer).
- **GCR-003** The shell is allowed only for a command that transfers with a folder Yulius
  names; that command's transports are pinned to `file`, its alternate-refs command is empty
  and it gets none of the adapter's `GIT_CONFIG_*` environment.
- **GCR-004** A transfer by a remote's name whose configured address is a folder is made
  with that folder, named by path, only when it is `<data>/playgrounds/<id>.git` of the
  command's own sandbox; otherwise it is made by the name, without shell or reach.
- **GCR-005** `yulius doctor` reports whether Git can be confined on the host.

- **TKR-001** A confined fetch, push or listing of a clone in a sandbox's folder is made in
  a courier: a bare repository in `<data>/git-couriers`, made for it and removed after it.
  The remote's address is `remote.<name>.url` of the clone, or a folder the caller names; a
  remote without such an address is refused. For a fetch and a push the courier's commands
  work on the clone's objects (`GIT_OBJECT_DIRECTORY`; its `shallow` file a link to the
  clone's), which must lie in the sandbox's own folders (links followed on both sides), and
  reach the clone's sandbox by the path the caller names. A fetch and a listing with a
  folder only read that folder; a push writes there.
- **TKR-002** A fetch through a courier leaves the clone's `refs/remotes/<remote>/*` as a
  fetch by name with pruning would, brings the tags that come with them (none for a refresh
  of the branches alone) without touching a tag the clone has, and gives a clone without
  `refs/remotes/<remote>/HEAD` the one the remote names; a push sends the commit of the
  given branch and is refused when it would rewind the remote; the listing answers the
  remote's branch. The refs travel as data: the clone's are listed and written into the
  courier, the courier's are listed and set in the clone by `update-ref --stdin`, the
  dropped ones in a step before the others (a branch can take the place of another under
  its name), each change naming what the clone had. The courier's fetch runs no
  maintenance.
- **TKR-003** A command whose folder is a sandbox's, a command without a folder that writes
  into one, and a transfer between folders get none of the adapter's `GIT_CONFIG_COUNT`,
  `GIT_CONFIG_KEY_<n>` and `GIT_CONFIG_VALUE_<n>`; a command in a folder of the server's own
  gets them.
- **TKR-004** Every command of the runner carries the constant options that switch off the
  `ssh` and `git` transports, signing (of commits, tags and pushes), signatures, the
  alternate-refs command, going into submodules (`push.recurseSubmodules`,
  `submodule.recurse`), credential helpers, a bundle address and a negotiating push, and
  `GIT_EDITOR=:`, an empty `GIT_ASKPASS` and `GIT_NO_LAZY_FETCH=1`; every `diff`, `log`,
  `show`, `diff-tree`, `diff-index` and `diff-files` carries `--no-ext-diff --no-textconv`
  right after its name, once (a call site's own copy among its options is dropped; the same
  word past `--` stays).
- **TKR-005** A command for a sandbox whose folder does not exist reaches that folder only,
  not the folder of all sandboxes, and makes nothing; which sandbox a path lies in is read
  from the path without its `.` and `..` parts.
- **TKR-006** A clone into a sandbox's folder is an empty repository with the address as
  its origin's, filled by a fetch through a courier with every tag of the remote, with the
  remote's default branch checked out and following it; of an empty remote, a repository
  without a commit. Its folder is new or empty, and a clone that fails removes it. The
  first commit of an empty repository is made in `<data>/changes`, never in the sandbox's
  folder.
- **TKR-007** `yulius doctor` fails a host whose Git, told `GIT_NO_LAZY_FETCH=1`, still
  starts a promisor remote's command for a missing object.
- **TKR-008** No confined command in a sandbox's folder (its own folder is one, or it has
  none and writes into one) may start the shell, also where the tests' stand-in for GitHub
  gives the shell to every other command; a transfer between folders there is refused
  before anything is started.
- **PSR-001** `EgressClient::may_use` is `false` for a production secret for every caller.
- **PSR-002** Migration 0055 sets `hosts` to `[]` for every row with `cd_prod = 1` and hosts,
  and writes one `audit_log` row per such secret (`vault.agent.hosts_cleared`, actor
  `yulius`, target the secret's id, a sentence with its name and the hosts it had, time in
  milliseconds; the Secrets page's Activity names the action); other rows are unchanged.
- **PSR-003** The store refuses to insert a secret with `cd_prod` and hosts.
- **PSR-004** The Edit dialog of a production secret has no Hosts input and no choice about
  hosts; the web mock holds no production secret with hosts.
- **PSR-005** A fetch, a listing and a clone between two folders reach the folder they take
  from read-only (`Invocation::local_reading`), by a path the caller gives and by a remote's
  name alike; a push reaches its destination read-write. A command without a folder of its
  own that takes from a sandbox's folder counts as one in a sandbox's folder (no token, no
  shell).
- **CGR-001** `App::ci_gate_for_head(sandbox, view, (head, what))` answers which jobs test a
  commit on its way into the integration branch: that branch's; the commit's own when it
  changed them since it branched off and they can be read, with the trusted security job
  kept; the commit's own when the integration branch has no CI, and, while that branch's
  configuration is broken, only for a commit that is the branch with a change merged
  (`Head::Merged`: a train's commit, a playground task's merge), not for a pull request's
  head (`Head::Proposed`); else none.
- **CGR-002** The scheduler (pull requests), a merge train, a playground task's merge, a run
  again of a run that is no branch's, and the remote control's "will Yulius CI test this
  pull request" use CGR-001. A run again of a branch's run takes the integration branch's
  jobs. A run again that nothing tests says why: the integration branch's broken
  configuration, the commit's own that cannot be read, or that the sandbox (a branch's run)
  or the commit has no CI.
- **CHK-001** `App::dlv_checks_at(sandbox, token, sha)` answers what the checks at a commit
  say: with a verdict of Yulius CI (`ci_gate_result` other than "not configured"), that
  verdict alone, with the word `error` for a run that could not happen; else GitHub's
  combined checks and what of them could not be read.
- **CHK-002** A task's delivery, the Git page, the remote control's merge, a promotion pull
  request and a fast-forward promotion use CHK-001. With `require_checks`, both promotions
  refuse failing, pending and unreadable checks at the integration head and go on passed
  checks or none. A fast-forward that is the profile's automatic promotion (after a merge,
  or after an integration environment of a merged change) also goes while that head's
  checks are pending; failing and unreadable checks stop it, and its note says so.
- **CHK-003** `ci_gate_result` is "pending" for a commit without a run when the integration
  branch has CI, or when the commit brings its own to a branch without any (the scheduler's
  rule for a proposal, CGR-001; whether the scheduler has a pull request to test it for is
  not asked, as it is not for a project with CI).
- **ARM-001** `TaskStatus::keeps_an_armed_merge` is true for `Queued`, `Starting`,
  `Running`, `Switching` and `WaitingCapacity`, false for every other state.
- **ARM-002** A pass (`merge_armed_task`) ends the armed merge of a task whose state does
  not keep one: decided, with "the task's run ended (<state>)" and what to do, and nothing
  is merged for it later. A pull request that is merged already is said to be merged
  first, as for any task.
- **ARM-003** A start (`restore_armed_merges`) puts back the armed merges of the tasks whose
  state keeps one, and forgets the others.
- **STS-001** Every route of the server maps an application error to its HTTP status by
  `error::status_for`: 401 unauthenticated, 403 forbidden or a bad CSRF token, 404 not
  found, 409 a conflict or a missing account, 422 invalid, 429 capacity, 502 an upstream
  failure, 500 an internal one. No file under `src/routes` holds another table; one kind
  is answered differently on purpose, by its own route: a guest link that ended is 410
  (SPEC-410). The routes that answer without a body log a failure inside Yulius as the
  others do (`error::bare_status`).
- **SWT-001** A turn's end that would move the session to another subscription takes a look
  at the background work that gives no answer as work running: the session stays, the Stop
  is answered as on a subscription that stays, and a later turn's end moves it. No caller
  of the two looks (`background_work_now`, `background_or_wait_now`) reads "not known" as
  "none". The session that stays for it is logged; a host whose look never answers moves
  no session at a turn's end (a move at a usage limit does not ask).
- **SWT-002** A switch whose old run does not end within `switch_end_ms` (half of the
  switch's two minutes) gives the run up and terminates its host. The switch starts no
  new run, is recorded `failed`, and the task rests as `stopped_switch_timeout` (SPEC-002's
  state machine), whoever records the run's end first: the switch, the run's reader that
  sees the host go, or the run's own exit. The task's end intent says the run was given up
  (`EndIntent::SwitchTimeout`), set in the step in which the switch sees that the run's end
  is still its own. A run whose end is already being recorded when the wait runs out is
  waited for once more and the switch goes on. A host that cannot be terminated leaves
  the run the task's live run (END-001): no session starts beside it, the task is
  `running` again, the switch is recorded `failed`, the task says in its terminal and to a
  person that the session could not be ended, and the next turn's end does not try the
  switch again within the cooldown; a run that ended by itself meanwhile rests the task
  instead, as `stopped_switch_timeout` from either status, and an end that is being
  recorded then is waited for before the run is asked about. No intent of the switch stays
  in the task's slot. A run that ends by itself before the switch has claimed its end
  (the task already reads `switching`) ends the task as it does without a switch, and the
  switch gives way. A stop that lands after the switch gave the run up rests the task
  too; whether it then reads `stopped_switch_timeout`, `stopped_lost` or
  `stopped_cancelled` follows who recorded the run's end. At a server start a task found
  `switching` with a run that ended while Yulius was down lost its switch with the
  server: it rests as `stopped_server_lost` and is among the tasks a move brings back, as
  one found with no run is.
- **SGN-001** A failed sign-in leaves a subscription that needs a new one (`relogin`) as
  it is, with the reason it had. One that works keeps its status with a note (as before);
  only a first sign-in that fails makes an `error`. Why a sign-in failed is told to its
  viewers in its `exit` frame (`cause`), also to one who opens it after it ended, and the
  sign-in's dialog shows that reason, never the subscription's own error. The write reads
  the status and writes it back under the lock of every other status write.
- **SGN-002** A subscription works while it is `ok` or `limited`
  (`AccountStatus::healthy`), and that is the one rule: tasks are placed only on one that
  works, only such a one is asked for its usage or its e-mail address by the background
  rounds, and a usage reading that arrives for any other stores its figures and leaves
  its status and its reason. Every other status ends with a sign-in that succeeds, and
  with nothing else. The web UI has the same rule once (`works` and `needsSignIn` in
  `usage.js`): the administration page offers and makes a reading for the subscriptions
  that work and offers a sign-in for the others, and the pool's panel and the "run on"
  menu call the others "needs a new sign-in".
- **QST-001** A start that has to wait in the queue is kept with its task
  (`tasks.queued_start`: the kind of start, its prompt, and the subscription when a person
  chose it), written in one step with the task's change to `queued`, and whoever starts
  the queued task later starts it as that start was to be; a task queued without one
  starts as before. The kept start is the task's only while it waits: the change that
  takes the task out of the queue removes it from the row. It survives a restart of
  Yulius. The pool's own choice of a subscription for a paused task is not kept: the pool
  chooses again when the slot frees. A subscription a person chose that would be refused
  when the task's turn comes (at its limit, needing a sign-in, removed) is given up: the
  pool places the task and its terminal says "The subscription chosen for this task
  cannot take it now, so Yulius chose one."
- **QST-002** A paused task's pause ends when its start is made (its run has a
  subscription) or waits in the queue, not before: a start that is refused (the queue is
  full) or finds no capacity after all leaves the pause open, with its kind and
  continuation, and the pool's next round tries again.
- **RST-001** A run started with its owner's personal MCP servers says so in its run
  directory (`personal-mcp-of`: the owner's id); a launch that cannot write it fails. A
  restarted server reads it for a run it reattaches, so the rules of SPEC-332 MSC-009
  hold across a restart: only that person messages the session, and a hand-over restarts
  it with the new owner's servers. A reattached run whose mark names someone who is not
  the task's holder (a hand-over's restart the old server did not finish) is restarted at
  once. A mark that exists and cannot be read counts as none and is logged.
- **RST-002** A `/goal` Yulius still has to type is marked in the run directory
  (`typed-goal-pending`, written before the spawn; it says whether the session opens with
  a turn) until the typist ends with an outcome: the goal recorded, the task flagged, or
  the prompt box left to a person who typed. A restarted server watches a reattached run
  that still has the mark, as it watches a launch goal: a goal Claude Code recorded
  before the restart ends the watch (the mark goes, nothing is said); otherwise, after
  `launch_goal_ms`, the task is flagged with "Yulius restarted before it set this task's
  goal" and the mark goes, so it is said once. A session whose opening turn has not ended
  (the mark says it opens with one, and no Stop of the run was answered) gets that turn
  back: its Stop ends it as it is, as before the restart, and the watch's time starts
  there, so the flag is not raised inside the turn and taken away by its end. A folder
  trust dialog still unanswered tells the person to type the goal after answering it.
- **TYP-001** A line of Yulius's own for a full session is not typed behind what may sit
  in the prompt box (`has_draft`: any key, a person's or a message Yulius typed, since the
  last prompt sent from the box; Yulius sees keys, not the box, so a key that left no text
  counts too). A prompt a person typed and sent is no draft: its submit emptied the box,
  and the session it filled is compacted. For `/compact <focus>` the compaction is held:
  the task reads Needs your action with the reason and what to do (send what was typed,
  or clear it and send any prompt), and its terminal says so. When its person types, the
  session is theirs as after any key since the refusal: the flag ends and the task waits
  for them; the prompt they send then meets the full context and is compacted in its
  turn. The hold lasts at most `compact_hold_ms` (ten minutes); then Yulius types its
  command all the same, the flag ends and the task works on (a task nobody watches must
  not wait for good for a key that left nothing). With nothing in the box the session is
  compacted. With text there the command goes out behind it as a prompt, which the full
  context refuses: that refusal is the compaction's failure, and the task continues in a
  fresh session at once; the flag's action and the terminal say this beforehand. The flag
  ends with the hold however it ends (its person types, the command is typed, the run
  ends, the terminal never goes quiet), and a hold cut by a restart of Yulius is taken up
  again by the new server. The Enter of a line Yulius typed empties the box, whatever was
  in it (Claude Code may run no prompt hook for a command: 2.1.292 ran none for
  `/compact`), so a key from before it keeps nothing out afterwards. The line that wakes a compacted session is not typed behind a
  draft (a key typed while the session was compacted): the terminal says that the
  session was not told to carry on and what to do, and the task waits for its person. A
  person who types after the refusal owns the session as before ("Yulius leaves the full
  session to you").
- **TYP-002** From a line Yulius types to its Enter, nothing else is typed into the
  session by Yulius: a message, `/compact` and its wake-up line, and the goal hold the
  run's one typing mark for that time; the goal holds it until Claude Code took it or its
  tries are over, since a submit the TUI dropped leaves the line in the box. A message
  that comes then is refused as while another message is being typed; a line of Yulius's
  own waits for the mark, at most 5 s, is typed then, and first asks again, with the mark
  held, whether a person typed meanwhile (a goal kept out that way says so, as one kept
  out by an earlier key does). Not under the mark: a person's own keys, the
  first message Yulius types (it has its own, which keeps keys and messages out), and the
  Enter of "Send anyway".
- **MOV-001** What the move helper's last run is doing is one of four states, matched
  exhaustively: running, lost, ended, refused. A report that says running is believed
  while it is at most 60 s old; an older one is lost only when systemd says the helper's
  service (`<server unit stem>-migrate.service`) is `inactive` or `failed`, and is
  running while systemd says it still runs (`activating`, `active`, `deactivating`,
  `reloading`) or cannot say. Systemd is asked only of such a silent report, each time;
  once it says the service ended, the report is read again (a helper writes its last
  report before its service ends, so one that ended between the two reads is ended, not
  lost). While running, no update or restart starts and nothing continues the move; once
  lost, a move can be continued or aborted as before.
- **MOV-002** A move's mark that exists is a mark, whatever its bytes: the first start
  that finds it queues the lost tasks and removes it.
- **CLT-001** A task's `closed_at` is the time of the close the task is in: set when it is
  closed (or created closed), kept while it stays closed, `NULL` from the change that
  takes it out of `closed`.
- **CLT-002** Migration 0058 (schema version 58) clears `closed_at` of every task that is
  not closed.
- **GRP-001** A run paused by a person or by an update (the pause the task's view shows) is
  in the Working group whatever it waits for: permission prompt, "Your turn", held prompt,
  browser request, failing checks. A stop asked for still makes it Ended.
- **GRP-002** The onboarding task a `pending` sandbox waits for (closed, never run) is in
  the Working group and counted in line (`counts.queued`), by the server and the sidebar.
- **GRP-003** A finished turn whose merge is armed is in the Working group, an achieved
  goal's too: the task's view says `merge_armed`, the list reads "Waiting for checks or
  merge", a task chained after it waits, a supertask waits for it, and archiving it is
  refused (the merge stays armed). This narrows SPEC-487 CHN-003 ("an achieved goal is Done
  as before") by the armed merge alone.
- **LVE-001** An edit the server refuses as `forbidden` (403) gives the typing up: the tab's
  text is the document's as it had it, nothing of the tab's waits (not saved, not busy, the
  tab not marked modified, a close ends at once), typing is not kept, teammates' steps since
  are caught up once (a refused catch-up asks nothing more), the Editor makes the file
  read-only and says "Read-only: you can no longer change this folder". Only a successful join
  (the file opened again) makes it writable again. A 403 for a stale session token
  (`csrf_invalid`) is not a refusal of access: the edit is sent again.
- **PRV-001** A proxied response reaches the browser with its body, `Content-Length` and
  `Content-Encoding` as the app sent them (the browser's `Accept-Encoding` reaches the app);
  the app's `X-Frame-Options` and CSP are kept (loopback sources mapped) and
  `frame-ancestors 'self'` is added; `/__yulius/bridge.js` is gone (404); the proxy's own
  pages carry no script and the same framing rule.
- **GHA-001** The stored GitHub App configuration and the App's credentials hold no webhook
  secret; a stored configuration that still names one parses as before.
- **DED-001** Removing the items of slice 25 changes no behaviour a person or an agent sees:
  the gate and the whole e2e suite pass with them gone, the API and its event stream keep
  every field they sent, and a stored row of an old shape (a workflow seeded before the
  catalog) stays an ordinary row.
- **LVE-002** A rejoin refused for lack of access (`forbidden`) is not tried again and gives
  the typing up as a refused edit does (LVE-001): the document stays read-only, with nothing
  waiting, until the file is opened again.

## Plan

One item per slice (a slice's own steps are its requirements and tests); a slice is ticked when
its change is reviewed and ready to merge, and the next is added when it is merged.

- [x] Slice 1: redaction fingerprints say a few bits and are fixed per sandbox (merged, #1240)
- [x] Slice 2: bubblewrap by the host's path, the clone's Git files through handles (merged, #1241)
- [x] Slice 3: the host's reads in a sandbox go through handles, bounded and never waiting (merged, #1242)
- [x] Slice 4: token scopes, the sign-in's browser, the merge's base and production hold on every way in (merged, #1243)
- [x] Slice 5: a secret allowed use by use goes somewhere new only after two-factor, redaction that cannot load withholds, an edit keeps a rotation (merged, #1244)
- [x] Slice 6: a viewer's input reaches the sandbox browser only while Chrome's file dialogs, DevTools and file pages are locked by policy (merged, #1245)
- [x] Slice 7: a secret is typed into the page the browser shows when the typing begins, and Yulius's navigations wait for it (merged, #1247)
- [x] Slice 8: a secret is typed neither into a focused frame of another site nor into a tab on its way to one (merged, #1248)
- [x] Slice 9: a sandbox's browser goes out through a proxy of its own and reaches what its sandbox reaches (merged, #1250)
- [x] Slice 10: the host's own Git runs confined: it executes Git's programs only and touches only its sandbox's folders (merged, #1251)
- [x] Slice 11: a transfer that carries the server's token is made in a repository of the server's own, reviewed (merged, #1252)
- [x] Slice 12: a production secret reaches no run by one rule, and no row keeps hosts for one (merged, #1253)
- [x] Slice 13: a run again tests a commit with the jobs the scheduler gives it, by one rule for all who ask (merged, #1254)
- [x] Slice 14: a promotion asks what every merge asks about the checks at a commit, by one answer (merged, #1257)
- [x] Slice 15: an armed merge lives and ends with the run that armed it, by one rule in a pass and at a start (merged, #1263)
- [x] Slice 16: the agents' own listener answers an error with the status every other route gives it (merged, #1265)
- [x] Slice 17: a switch ends no work it cannot answer for, and starts no session beside one it could not end (merged, #1268)
- [x] Slice 18: a subscription that needs a new sign-in stays out of the pool until one succeeds (merged, #1271)
- [x] Slice 19: a start that has to wait in the queue keeps how it was to start (merged, #1272)
- [x] Slice 20: what a run was started with is still known after a restart of Yulius (merged, #1274)
- [x] Slice 21: Yulius types into a session by one rule, whatever it types (merged, #1276)
- [x] Slice 22: a move's report, a move's mark and a task's close time each mean one thing (merged, #1277)
- [x] Slice 23: Home, the tabs and the sidebar count a task by one rule, and a refused edit leaves nothing unsaved (merged, #1279)
- [x] Slice 24: what nothing reaches goes: the preview bridge, the webhook secret, dead web exports and styles (merged, #1281)
- [ ] Slice 25: Rust code nothing reaches goes: dead events, helpers, store methods, a one-shot shim and unused features

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

- **AC-17** A writer's `read` token gets 403 "read-only" on GET `files/roots`, `files`,
  `files/content`, `files/live`, `shells`, `previews`, `cockpit/link` and
  `superagent/requests`, and 200 on the task list; the same person's operate token reads
  `files/roots`.
- **AC-18** The access epoch grows on `revoke_api_token`.
- **AC-19** A callback with a valid state and code but no cookie, or another start's cookie,
  answers `/signin?error=expired` without a session; the starting browser then completes.
  The start's cookie is `__Host-yulius_oauth=<state>` with HttpOnly, SameSite=Lax, Path=/,
  Max-Age=600 and Secure.
- **AC-20** The eleventh start within a minute goes to `/signin?error=busy&next=…` with no
  cookie; the store refuses a state past its bound and takes one again after an expiry.
- **AC-21** `/assets/lab.js` and `/assets/lab-proposals.js` answer 404 from the server while
  the UI still imports them; the e2e lab test passes in mock mode.
- **AC-22** A pull request pointed at `main` after it was armed is dropped on green ("goes
  into `main`, not `quartio`") and not merged; `delivery_merge` and a person's merge are
  refused; pointed back, it merges.
- **AC-23** A writer's run of a production-stage command is 403, an administrator's without
  the slug or with another is 422, with it 200.

- **AC-24** A new host and an allowed `.env` copy of a secret with `auto_allow` off answer 403
  `forbidden` to a token and 403 `reauth_required` to a session without the unlock, changing
  nothing; with the unlock 200; 11 minutes later `reauth_required` again. A description, the
  same hosts in another case, fewer hosts and a revocation pass with a token; a host removed
  is new again. A token binds a host of a secret with `auto_allow` on.
- **AC-25** The remote MCP's `secret_update` (a new host) and `secret_hosts_answer` (allow) are
  refused for such a secret with "allowed use by use" and "Secrets page"; its description and a
  dismissal pass.
- **AC-26** On the Secrets page, Allow of a host request, Allow of `.env` copies, the
  production allowance and Save of an edit that lets agents set a secret up open the
  two-factor dialog first and complete after the code; nothing changes before it. Closing the
  dialog leaves a host request waiting with its Allow, and an edit form open as it was filled.
- **AC-27** A terminal whose secrets fail to load shows the notice once and nothing of two
  chunks holding a value, asks again only after a second or for another generation; after a
  load it shows them redacted; without agent secrets output passes. A Superagent's real
  terminal with a secret that no longer opens shows the notice and none of a command's output
  that ran; once that secret is deleted the next output shows.
- **AC-28** A request through the proxy of a run whose secrets cannot be read answers 503
  ("nothing was sent") to a bound host and to a plain one, and the upstream sees nothing.
- **AC-29** An update of a row whose ciphertext changed since it was read changes nothing and
  reports no row; an overtaken team edit is a conflict that says reload.
- **AC-30** Allowing a secret for production deploys answers 403 `forbidden` to a token and 403
  `reauth_required` to a session without the unlock (the secret stays `not_allowed`), 200 with
  it; revoking needs neither.
- **AC-31** A host answer or `.env` allowance that names another `auto_allow` than the row has
  changes nothing.
- **AC-32** A file that states 0 bytes and holds 6 is refused with 5 left (no copy remains) and
  takes 6 from a budget of 10.
- **AC-34** A task whose first message Yulius types and whose session starts behind a withheld
  screen (a secret stopped opening while its trust dialog showed; a person answered it) gets
  nothing typed and is flagged "Agent secrets could not be read, so this terminal's screen
  was withheld and Yulius typed nothing." with what to do; no trust notice is given; a
  Superagent's message for it is refused with that reason; no transcript holds either text.
  A scrub that dropped output says its screen is unknown until a whole screen is loaded.
- **AC-33** The app's proxy policy has nothing to redact with for a sandbox one of whose secrets
  no longer opens, and still has for another sandbox.

- **AC-35** Host setup writes the policy file where it is missing or differs and leaves an
  equal one alone; the dry run says so and writes nothing; the check mode writes it.
- **AC-36** A browser on a (fake) display whose Chrome does not block the marker refuses a
  viewer's button press, key press and typed text with the reason and presses nothing on the
  display, while releases and moves pass; one probe serves them; after the policy appears,
  input is taken without a restart. A window a page opens at the probe's address is listed
  and shown; a page of Chrome's own is not.
- **AC-37** Against a real Chrome (opt-in): with the policy installed a click on a file input
  opens no window, the accelerators for Open, Save, Print, the page's source and the
  developer tools open no window and no page of Chrome's, and a link to another program's
  address is blocked as a page; without it the click is refused and opens none.
- **AC-38** A helper unit that may write the modules list but not the policy directories: the
  re-check says to run the setup, the setup writes the drop-in with the directories, and the
  re-check is then satisfied.

- **AC-39** A fill on a tab whose page is not one the secret may go to types nothing
  (`SecretPageRefused` with the page's host) and leaves screenshots working; on an allowed
  page it types. Through the MCP tool: a tab that moved to another site without Yulius having
  heard of it (still listed at the secret's host) gets nothing typed and the answer names the
  site.
- **AC-40** A page that moves on during the typing to another site answers `SecretAstray` with
  that site's host; the rule was asked of the page where the typing began and of the one it
  moved to; a move within the secret's sites is no error. Through the MCP tool the answer
  names the site, not the value, and a `vault.agent.fill` activity entry names both sites.
- **AC-41** A `browser_navigate`, a reload and a step back asked for while a secret is typed
  each return only after the typing ended.
- **AC-42** A fill whose focus is not in a text field of the page (a frame, an element that
  takes no text, nothing, an unknown or missing answer), on a page holding a frame of a site
  the rule refuses (in the frame tree, or as a target of its own), types nothing and answers
  `SecretFrameRefused` with that site; so does one whose frames cannot be listed or told (an
  error), one whose page cannot be read, and one with the focus on a fenced frame; with the
  focus in a field of the page, or with every such frame allowed, it types. Through the MCP
  tool the answer names the frame's site, not the value.
- **AC-43** A fill on a tab Chrome announced a navigation to another site for types nothing
  and answers `SecretPageLeaving` with that site: for a link, a step in the tab's history, a
  requested navigation of this tab, a frame of the page, an announcement that comes while the
  page is looked at, and an address without a host (named "an unknown site"). After Chrome
  said the frame arrived, stopped loading or was detached, for a navigation within the
  secret's sites, a same-document navigation, a navigation requested for another tab and a
  frame of another page, it types. Through the MCP tool the answer names the site.
- **AC-44** On a real Chrome (opt-in): a secret is not typed into a field of another site's
  frame, object or frame inside a closed shadow root, and is typed when that site is allowed
  or the focus is in the page's own field beside the frame; a fill right after a click on a
  link to another site types nothing.
- **AC-45** A document that changed between the look at the page and the keys gets nothing
  typed (`SecretPageRefused` with the new site, or "try again" within the secret's sites). A
  focus outside the page's fields after the keys (a frame, something else, a fenced frame),
  on a page holding a frame of a refused site, answers `SecretFrameAstray` with that site;
  with only allowed frames, or still in the field, it does not; through the MCP tool a
  `vault.agent.fill` activity entry names it.
- **AC-46** The record of announced navigations: an entry counts for 30 s, a settled frame's
  and an ended session's are gone, past 256 frames of a tab (none older than 30 s) nothing
  is known of that tab for 30 s while other tabs' are, after lost events of none, and of an
  address over 2048 bytes the site is kept (for one that would read as a bound host when
  cut, the site it really names; for a long query, its own site), none when the site is too
  long or there is none.
- **AC-47** Through the MCP tool, the field `username` is typed neither with the focus in
  another site's frame, nor into a tab on its way to another site, nor into a tab that moved
  on unheard; it is typed on the bound page, and a frame that took the focus is reported.
- **AC-48** The proxy's rule (`BrowserEgress::reaches`): the sandbox's ports on `127.0.0.1`,
  `::1` and `127.0.0.2` yes; a port outside, `0.0.0.0`, link-local and IPv4-mapped loopback
  and link-local addresses no; a public and a private address yes; the pages' listener only
  for a plain request whose path begins as the welcome page's, also bound to every address
  and with its port inside the sandbox's range; the public listener at its own address never,
  also with its port inside the range. For a name's answer (not `localhost`, a name below it
  or an address): no loopback port, not the welcome page; a public address yes. A refusal
  names the sandbox's ports, or says that none is known.
- **AC-49** Over real sockets: a plain request (by address, `localhost` and `app.localhost`,
  the host header as asked) and a tunnel (by address and `localhost`) to the sandbox's port
  arrive; a name that resolves to that port, or to the welcome page's listener, gets 403 by
  plain request and by tunnel and nothing arrives; to another loopback port
  (`127.0.0.1`, `localhost`, `[::1]`, `127.0.0.2`, `0.0.0.0`), `169.254.169.254`, `[fe80::1]`
  and `[::ffff:127.0.0.1]` both are answered 403 and nothing arrives; the welcome page is
  served, other paths of its listener and a tunnel to it are not.
- **AC-50** A browser's request arrives with its body and its own headers, without the
  proxy's hop headers and without a credential an agent's proxy would add; the answer comes
  back with the secret value its origin echoed and its `content-encoding`; the policy of
  agent runs was not asked once. A header the request's `Connection` lists is not sent on,
  the origin's own hop headers do not come back, and a user and password before the host
  reach neither the origin's `Host` nor an error text. With a share of one connection, an
  agent run's tunnel leaves none for the browser and the browser's none for a run. An upload
  slower in all than the head bound, sending throughout, gets its answer; a server that
  never answers gets 504 after the bound.
- **AC-51** `chromium_args` with a proxy hold the five switches before an operator's extra
  ones and none without; with a network, each browser's arguments name its own proxy, which
  is open while it runs and closed after a stop, a crash's restart and a failed start; a
  failing network starts nothing (`BrowserError::Network`).
- **AC-52** As the server wires it: before the egress proxy is bound no proxy opens; `s1`'s
  proxy reaches a server in `s1`'s ports and not one in `s2`'s, and the other way round;
  neither reaches a service of the host or the internal listener beyond the welcome page; a
  lease let go closes its listener; an unknown sandbox's proxy answers nothing; once the
  egress proxy is gone no proxy opens. The server's browser configuration has a network, its
  pages are the two the guard lets through, and the public listener is among those no
  browser reaches.
- **AC-53** On a real Chrome (opt-in): behind its proxy it shows its sandbox's server by
  `localhost` and `127.0.0.1` and the welcome page; navigations to another sandbox's port, a
  host service, the internal API, `169.254.169.254` and both over HTTPS show none of those
  pages; a page's `fetch`, image and WebSocket to them arrive nowhere while its own server
  answers.
- **AC-54** In the server's layout and with the adapter as the server builds it: a clone
  from the sandbox's own origin, a worktree, a commit, the review's changes, a push and the
  remote's head by the remote's name, a merge of the fetched integration branch and the
  worktree's discard all work.
- **AC-55** With the clone's configuration naming a marking command as a filter (clean,
  smudge, process), a diff driver, text conversion, external diff, merge driver, editor,
  pager, signing program, `ssh` command, file monitor, credential helper, alternate-refs
  command, a hook, and the remote's `uploadpack` and `receivepack`, with the remote's address
  an `ssh:` one and with the origin's folder rewritten to one: the server's status, commit,
  review, diff, log, push, fetch and merge leave no marker.
- **AC-56** A link at `FETCH_HEAD` to a file outside leaves that file as it was after a
  fetch; a link at `.git/shallow` to a file outside does not bring its text into any answer;
  another sandbox's objects named as an alternate do not make its file readable.
- **AC-57** With the remote's address set to another sandbox's origin, another sandbox's
  clone or a folder of the host, a push, a fetch-and-merge and the remote's head all fail,
  nothing arrives there and nothing is merged from there; with the sandbox's own origin the
  push arrives.
- **AC-58** The launcher: `git --version` runs; a Git that is not on `PATH` is refused with
  the reason (exit 126); the Git it starts runs no shell alias and no other program by path;
  with `--shell` a shell alias runs and another program still does not.
- **AC-59** `Confinement::reach` and `local_origin` over a data folder: a sandbox's two
  folders for every path in either, nothing of another sandbox, the nearest existing folder
  for what is to be made, the tests' data folders one level down; the own origin for the
  clone, a worktree and an import worktree, none for any other folder or address.
- **AC-60** The binary's whole test suite passes with every adapter confined in the
  server's layout (the tests' stand-ins for GitHub, which are folders, with the shell).
- **AC-61** With the clone's configuration naming a proxy (generally, for the remote's host
  and for its full address) and no certificate check, and the remote at an address where
  nothing listens: the server's fetch-and-merge, push and listing fail, no answer holds the
  token, the proxy was never connected to, and `<data>/git-couriers` is empty afterwards.
  With the clone's objects a link to a folder outside the sandbox that Git can read, a
  fetch and a push are refused for that reason, and the listing is still tried; with them
  at a path inside the sandbox that holds a line break, the transfers are tried. A worktree
  whose `commondir` names another sandbox's repository gets no transfer. With
  `remote.origin.url` unset and `origin` rewritten to the test's listener (and the loader
  as `core.askPass`), all three are refused for the missing address and the listener is
  never connected to.
- **AC-62** With the tests' stand-in for GitHub (an address rewritten to a folder in the
  server's own configuration only): a clone into a sandbox's folder that has what a clone
  has (the checked-out default branch following the remote's, its branches, a tag, a tag
  of a commit on no branch, the note of the default branch, the address), after a clone
  from an address where nothing listens left no folder and one into a folder that holds a
  file was refused; a push that arrives, the remote's head, a push that would rewind
  refused, a fetch that brings the remote's new commit and prunes a branch the remote
  dropped; a branch `a` replaced by `a/b` and that by `a` again, each fetched; a note of
  the default branch that is kept when the remote's changes and
  made when the clone has none; an empty remote cloned, fetched, and noted once it has a
  branch; no courier left after any of it. The same clone, push and fetch with the data
  folder reached through a link.
- **AC-63** The runner's command: in a sandbox's folder, and for a command that writes a
  new repository into one, no `GIT_CONFIG_COUNT`/`KEY_<n>`/`VALUE_<n>` of the adapter,
  another variable of its kept; in a folder of the server's own (also when it works on a
  sandbox's objects) and for a command that writes elsewhere, all of it; the constant
  options and environment on every command; the two diff options right after the name of
  each command that can make a diff, past the command's own `-c` pairs, not twice among the
  options, kept past `--`, and on no other command. A transfer between folders made in a
  courier gets the shell, the `file` transport only and no adapter configuration; made in
  a sandbox's clone or into a new repository there, it is refused; with the shell given to
  every command, a command in a sandbox's folder still gets none. A transfer that only
  takes from a folder reaches it read-only. A command given input reads all of it.
- **AC-64** With the clone naming the program loader as external diff, diff driver, text
  conversion, `ssh` command and signing program, and a new file in a folder that is a
  program: the server's status, review, diff, log, commit, fetch and push start nothing.
  Of these, only the diff cases could start the file (the loader refuses a signing
  program's first argument, and the `ssh` address goes through a courier, which does not
  read the clone's `core.sshCommand`), and the adapter's call sites carried the diff options
  before the runner did: the test guards the sum; what the runner adds is AC-63's.
- **AC-65** With the clone naming a promisor remote whose command for the other side leaves
  a marker, and a commit whose tree no repository has at the head of the task's branch: the
  server's push to the playground's origin, fetch-and-merge, review, diff and clone refresh
  leave no marker.
- **AC-66** After a first commit the remote refused, with a file of the sandbox's where the
  scratch clone used to be made (`<sandbox>/seed`): the next start pushes a first commit
  holding `README.md` only, the file is untouched, and `<data>/changes` is empty.
- **AC-67** `yulius doctor` reports "git missing objects" as ok on this machine; its probe
  answers that a Git with the variable taken away does not leave the object missing, and
  nothing for a Git that is not there.
- **AC-68** With a submodule in the task's worktree that knows a branch of its remote, has a
  commit beyond it and names a command for the other side that leaves a marker, and the
  clone asking for submodules to be pushed (by either key): the server's push to the
  playground's origin arrives and leaves no marker.
- **AC-69** While the server clones a playground's origin (400 files) into the sandbox's
  folder with the adapter as the server builds it, a thread keeps a filter for every file,
  whose command leaves a marker, in the new repository's configuration from the moment it
  exists: the clone ends with its files as committed, the filter in its configuration, and
  no marker.
- **AC-70** With the clone naming a bundle address at the test's listener (which asks for a
  password), a credential helper that leaves a marker (generally and for that address) and
  a negotiating push: the server's push to the playground's origin arrives, the listing
  answers it, a fetch-and-merge brings the origin's new commit, a refresh of the branches
  brings a new branch and no tag, a fetch by the remote's name brings the tag; the listener
  was never connected to and there is no marker.
- **AC-71** A playground's origin whose history ends early (a shallow copy, two of three
  commits) is cloned into the sandbox's folder: the clone has the two commits, the same
  `shallow` file as the origin, and passes `git fsck`; so after a second fetch, and so with
  the data folder reached through a link. With a link to a missing branch file of the
  origin where the clone's `shallow` file was, and the clone's remote branch deleted, the
  server's fetch creates nothing in the origin.
- **AC-72** For an agent run, a Superagent and a CI job granted the name: `may_use` is true
  for a secret that is no production secret and false for one that is; a CI job has no
  secret it was not granted.
- **AC-73** A database at schema version 54 with a production secret that has two hosts, a
  production secret without hosts and an agents' secret with a host: after opening, the
  first has none, the others are unchanged, and the audit log has one
  `vault.agent.hosts_cleared` row, of `yulius`, for the first secret, with its name and both
  hosts; the schema version is 55.
- **AC-74** Inserting a production secret with hosts is refused ("no hosts"); without them it
  is taken. With such a secret in a project: `browser_fill_secret` refuses it for what it
  is, and a CI job that names it is told it is never given to CI. (That the proxy gives it
  to no client is AC-72's: with no host bound, the integration tests' requests show nothing
  about the proxy's rule.)
- **AC-75** The Edit dialog of a production secret shows "None. This is a production secret:
  …" and no Hosts input; the e2e test of the secrets page passes without the old rows.
- **AC-76** The runner: `local_reading_if_path` makes a transfer with a folder a local one
  that reads it and touches nothing; with an address elsewhere it changes nothing. A
  repository of the server's own with a folder for its origin, through a launcher that notes
  its words: the clone, the fetch by the remote's name and the listing reach the origin with
  `--ro` and never `--rw`; the push with `--rw`.
- **AC-77** A pull request that changes one job's command and adds a job is tested with
  both; run again through the API, the new run has the same jobs and the changed command.
- **AC-78** The existing tests of a pull request's own jobs, of the security job it cannot
  drop, of a pull request that adds CI, of a merge train with its own jobs, of a playground
  task's checks and of the remote control's merge pass unchanged.
- **AC-79** With the integration branch's `yulius.toml` unreadable and a pull request that
  brings a job of its own: no run of the pull request's head, and the CI overview says what
  is broken.
- **AC-80** A pull request that changes no job, after the integration branch changed its
  job's command: its run has the integration branch's command of now.
- **AC-81** In a playground whose `dev` has CI: a task that changes the job's command arms
  its merge, and its run shows the task's command, not `dev`'s.
- **AC-82** In a playground whose `dev` has a `yulius.toml` that cannot be read (the CI
  overview says so): a task that brings a readable one arms its merge (`dev` does not move
  before the run), and its run shows the task's job.
- **AC-83** With a promotion pull request open and GitHub's checks at its head passed: a
  failed run of Yulius CI at that head refuses the promotion (409, "fail…"), nothing is
  merged; a newer passed run lets it merge.
- **AC-84** With a fast-forward profile: a failed run of Yulius CI at the integration head
  refuses the promotion and the production branch does not move; a newer passed run lets
  it fast-forward.
- **AC-85** In a project without CI: a pull request without jobs has no verdict of Yulius
  CI; one that brings a job is pending before the scheduler's round and passed after its
  run.
- **AC-87** With automatic promotion, required checks and Yulius CI as the project's CI: a
  merge is refused while the pull request's head has no run, goes once its run passed, and
  the production branch is fast-forwarded to the merge's commit with it, although that
  commit has no run of its own.
- **AC-88** With automatic promotion, required checks, a manual integration environment and
  a failed run of Yulius CI at the integration head: confirming the environment promotes
  nothing, the answer says "Automatic promotion did not happen", and the production branch
  stays; once a newer run passed, a person's promotion fast-forwards it.
- **AC-86** The existing tests of a task's delivery, the Git page, the remote control's
  merge and the promotions pass unchanged (pending, unreadable and passed checks of
  GitHub's for a project without Yulius CI among them).
- **AC-89** The states that keep an armed merge and those that do not, as a table
  (ARM-001).
- **AC-90** With a merge armed and its checks pending: the task in `WaitingCapacity`, then
  `Switching`, then `Queued`, stays armed after a pass, and the saved merge is kept by a new
  server's restore. With the armed pull request merged by a person and the task closed
  afterwards, the pass's outcome says "merged" and not "dropped". Closed: the next pass ends it ("dropped", "run ended (closed)"), the saved
  merge is gone, and with the checks passed neither a pass nor a new server's start merges
  the pull request. A merge saved for a task that was closed while the server was down is
  not merged at the start and is forgotten.
- **AC-91** The existing tests of armed merges, merge trains and the playground's delivery
  pass with their tasks running (a live run in a fixture is a running task now).
- **AC-92** `status_for` gives each kind of application error the status of STS-001.
- **AC-93** No line of a file under `crates/yulius-server/src/routes` is a `match` arm from
  an `AppError` kind to a `StatusCode` (the two tables that had such arms are gone).
- **AC-94** The existing tests of the hooks, the Git credential helper, `yulius wait` and
  the remote MCP endpoint pass unchanged; `yulius wait` against a listener that answers
  500 ends after a few tries with "HTTP 500" (as before: now what Yulius answers for a
  failure of its own).
- **AC-95** With a merge armed, a commit past the armed head and the task closed: the pass
  says the run ended, not that the branch moved. With the armed pull request merged by a
  person, a commit past the armed head and the task closed: the pass says "merged".
- **AC-96** A goal task on a subscription at its threshold, a second one with room, and a
  look at the background work that gives no answer: at the turn's end the task has one run
  and is done. Once the look answers, the next turn's end moves the session to the second
  subscription.
- **AC-97** A switch of a task whose session does not hear the request to end and whose
  host cannot be terminated ends with "the session could not be ended, so it was not
  switched": the task has its one run, live; it is `running` and reads Needs your action
  with "Yulius could not end this session" and "Stop the task to try again."; the switch
  is recorded `failed`, its terminal
  says "The session was not switched: it could not be ended.", and switches cool down. A
  session that leaves by itself afterwards ends as `exited` and closes the task (no intent
  of the switch was left for it).
- **AC-98** A switch of a task whose session does not hear the request to end and whose
  host can be terminated ends with "the session did not end in time during the switch": the task
  has its one run, lost, and is `stopped_switch_timeout`; the switch is recorded `failed`.
- **AC-99** The same outcome when the run's reader records the loss before the switch
  does (the host is killed after the switch gave the run up, before it terminates it), and
  when the run exits by itself at that moment (its end is recorded `switch_timeout`), also
  when the host then cannot be terminated: nothing says a session could not be ended.
- **AC-100** A run whose end is being recorded when the switch's wait runs out (held
  after its recorder took the switch's intent) is switched: the old run ended `switched`,
  the new run is live, the task runs.
- **AC-101** A session that leaves by itself after its task became `switching`, before
  the switch claims the run's end: the task is `closed` with its one run `exited`, the
  switch ends with "the task was closed during the switch", and nothing says a session
  did not end in time.
- **AC-102** A run whose agent died and whose end is being recorded (its recorder holds
  the given-up intent) when the kill is refused and the task is put back to `running`:
  the switch waits for the record; the task is `stopped_switch_timeout`, its run ended
  `switch_timeout`, and nothing says a session could not be ended.
- **AC-103** A start that finds a `switching` task whose run ended while Yulius was down
  rests it as `stopped_server_lost` (a running one whose session left is `closed`, as
  before); with the mark of a host move the task is queued again.
- **AC-104** A subscription that signed in, was refused by Claude (`relogin` with
  Claude's words) and whose next sign-in fails: its status is `relogin` and its error
  Claude's words; the ended sign-in's `exit` frame is `error`, the exit code and "the
  sign-in ended without producing a token (exit code 2)" as `cause`.
- **AC-105** A subscription with a credential that reads `error`, and one that reads
  `relogin`: a background usage round stores nothing for it; a reading an administrator
  asks for answers 200 with the status unchanged, stores the usage and keeps the reason.
- **AC-106** With one subscription that works and has no address yet, one in `error` and
  one in `relogin`, a background round stores the first one's address and asks Claude
  nothing about the two others.
- **AC-107** On the Subscriptions page with every subscription but one in `error`,
  "Refresh all" says "Usage read again for 1 subscription." and one request was made.
- **AC-108** On the Subscriptions page a sign-in of the refused subscription that fails
  says "Sign-in failed" with "The sign-in ended without producing a token" and without
  Claude's refusal; the card then still shows the refusal and "Sign in again", and the
  subscription reads `relogin`.
- **AC-109** With one session slot: a task paused at a usage limit without a process
  becomes resumable while another task holds the slot; the pool's round queues it. While
  it waits the subscription the pool had chosen fills up and another frees. When the slot
  frees, the task's new run is a native resume without a prompt that continues the
  interrupted turn, no transcript still holds the limit's record, and it runs on the
  subscription that is free then.
- **AC-110** With one session slot held and the pool preferring another subscription, a
  stopped task continued on a chosen subscription is queued; Yulius restarts (the
  holder's session lives on); when the slot frees, the task runs on the chosen one.
- **AC-111** In the store: a task that no longer reads the status it was read in is not
  queued and nothing is kept; a kept start is read back as written (a prompt with a line
  break, quotes and the word null, a subscription id); one this build cannot read (another
  kind, a subscription id of another shape, no JSON) or of an earlier wait gives none;
  after the task left the queue the column is empty, and a task queued again has the
  start of that wait. A database of schema version 56 opens at 57.
- **AC-112** With the slot held, a stopped task continued on a chosen subscription that
  then fills to 100 % while the task waits: when the slot frees the task runs on the
  other subscription and its terminal says that the chosen one could not take it; it is
  not failed.
- **AC-113** With one slot held and a queue of one place taken, a task paused at a usage
  limit without a process that becomes resumable keeps its `limit` pause through the
  pool's round and stays `waiting_capacity`; once the queue has room the next round
  queues it and its pause ends; when its turn comes it resumes past the limit.
- **AC-114** A task's run holds its owner's personal MCP server, which is then turned
  off (so only the run's own record says it holds it); Yulius restarts and reattaches the
  run. Another person's message to the task is refused for the MCP servers, and when
  control passes to that person the run is resumed.
- **AC-115** Yulius restarts while a continued goal session is in its opening turn of
  12 s (the run directory holds `typed-goal-pending`), with a watch of 3 s: after that
  turn's Stop was answered the new server flags the task with "Yulius restarted before it
  set this task's goal", and the mark is removed.
- **AC-117** A reattached run whose `personal-mcp-of` names another person than the
  task's owner is restarted by the new server (a second run, a resume).
- **AC-118** With an agent whose prompts reach Yulius's hooks, a person types a prompt at
  the task's terminal and sends it, and the full context refuses it: the session is
  compacted.
- **AC-119** A message for the task that comes while `/compact` is in the prompt box and
  its Enter not sent is refused with "is being typed", nothing of it reaches the session,
  and the session is compacted. The same while a resumed session's `/goal` is in the box:
  the message is refused and the goal is set. The same after the Enter of a goal whose
  submit the TUI dropped.
- **AC-120** The same person types on while that prompt's turn runs and sends nothing;
  the refusal comes: the task reads Needs your action with "The session's context is
  full, and Yulius does not compact it yet: …" and "Send what you typed in the terminal,
  or clear it and send any prompt: …", the terminal says that Yulius holds the
  compaction, and no compaction is typed. They type on: the terminal says the session is
  left to them, the task reads Your turn and still nothing is typed. They send it: the
  session is compacted.
- **AC-121** With a hold of 3 s and a key that left no text, the task is flagged, nothing
  is typed at first, then the session is compacted, the flag is gone and the session is
  told to carry on. With real text in the box, the task gets a fresh session (a second
  run, new) within the test's minute, and the terminal says "Compacting did not help (the
  command was sent with text left in the prompt box)".
- **AC-125** A compaction is held and flagged; Yulius restarts; the new server (hold 3 s)
  takes the recovery up again and the task goes on (a second run).
- **AC-126** A compaction is held for a key that left no text; a prompt hook for a
  command reaches Yulius (the box is emptied, no turn starts): Yulius types its command,
  and at that moment the task no longer reads Needs your action; the session is
  compacted.
- **AC-122** A reattached run that holds another person's servers and sits at a usage
  limit is restarted past the limit (a resume that continues the interrupted turn, no
  limit record left in a transcript).
- **AC-123** A key of the task's person at the moment Yulius is about to type `/compact`
  (quiet terminal, nobody typed, the typing mark not taken yet) keeps the command out:
  the terminal says the session is left to them and the task reads Your turn. The same
  key before a resumed session's `/goal` keeps the goal out, and the task reads Needs
  your action with "Someone typed in the terminal before Yulius set the task's goal".
- **AC-124** A key typed right after the Enter of Yulius's `/compact`: the session is
  compacted, the terminal says "Yulius did not tell the session to carry on: …", the task
  reads Your turn, and the line that wakes a compacted session is not typed.
- **AC-116** A typed goal that Claude Code took removes the mark; with the mark put back
  by hand and a restart, the new server removes it and flags nothing.
- **AC-127** A move whose report is 60 s old reads running and systemd is not asked; at
  60 001 ms, with the helper's service still running or systemd silent, it reads running,
  the card offers only Abort, a move is refused as running, and Update now and Restart are
  refused with "is being moved to another host"; once systemd says the service ended it
  reads lost, Plan, Move and Abort are offered and an update starts. A helper that writes
  its last report (ended well) and ends just as systemd is asked reads ended, and Move is
  not offered.
- **AC-128** Systemd's `ActiveState` of the helper's service: `activating`, `active`,
  `deactivating` and `reloading` are running, `inactive` and `failed` ended, anything else
  or no answer unknown; outside a unit there is no helper to run.
- **AC-129** A move's mark of bytes that are not text: the next start queues the lost task
  and removes the mark, and the report says 2 queued.
- **AC-130** A running task closed at 200, closed again at 300, continued at 400, queued
  at 450 and closed at 500 has the close times 200, 200, none, none, 500; a version-57
  database's running and chained tasks lose their close time, a closed one keeps it.
- **AC-131** A running task with a permission prompt counts 1 needing you; paused by its
  owner it counts 1 working and none needing you; resumed, 1 needing you again. In the
  sidebar's rule a paused task with a permission prompt or a held prompt reads Paused,
  counted working.
- **AC-132** A `pending` sandbox with its onboarding task waiting for Start onboarding and a
  task chained after it counts 2 in line, none working, none needing you; the sidebar's
  counter says the same of the onboarding task.
- **AC-133** An armed, green pull request whose task's goal is then achieved: the chained
  task stays chained, the task's view says `merge_armed: true`, the project counts it
  working, archiving it answers 409 and it stays running with the merge armed. The sidebar
  reads "Waiting for checks or merge" with and without the pull request; an achieved goal
  without an armed merge is still "Done · PR #7". A supertask created then is held by it.
- **AC-134** A live document whose edit is refused with 403 has the server's text back, is
  read-only, saved and not busy; typing more sends nothing; close ends within a second; a new
  join clears the read-only state. A refusal as `csrf_invalid` sends the edit again and the
  document stays writable. When the catch-up is refused too, the text is the document's, the
  document is read-only and not busy, and after 1.5 s nothing more was asked (one join, one
  catch-up). In the Editor the status says "Read-only: you can no longer change this folder",
  the editor is read-only, the text is the file's and the tab reads `notes.txt×` (not marked
  modified).
- **AC-135** A proxied document arrives byte for byte with its length; the app's
  `X-Frame-Options: DENY` and its CSP (with `frame-ancestors 'none'`, the WebSocket source
  mapped) are kept beside `frame-ancestors 'self'`; `/__yulius/bridge.js` answers 404; a
  gzip document keeps `Content-Encoding: gzip` and its length, and the app saw the
  browser's `Accept-Encoding: gzip, br`. The proxy's refusal page (the landing's) has the
  CSP `default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'self'` and no
  script.
- **AC-137** With slice 25's deletions, `scripts/dev ci` (build, lint, cargo-shear, rustdoc and
  every test) and the whole e2e suite pass; `TaskStatus` still names every status a run can
  end in; a manual move names its account.
- **AC-136** A catch-up that says the document ended and a rejoin refused as `forbidden`: the
  document is read-only and not busy, the text is the document's, and after 2.5 s it asked
  for no second rejoin. The same when the edit itself meets an ended document (409), its
  catch-up too, and the rejoin is refused: read-only, saved, not busy, the document's text,
  one edit, two joins, and a close ends within a second.

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

- Slice 4: `remote_mcp::a_read_token_reads_nothing_above_view_access` (AC-17), `yulius-store`
  `writes_that_change_access_grow_the_access_epoch` (AC-18),
  `http_auth::a_github_sign_in_completes_only_in_the_browser_that_started_it` (AC-19),
  `http_auth::github_sign_in_starts_are_limited_per_client` and `yulius-store`
  `oauth_state_single_use_and_expiry` (AC-20),
  `ui_contract::embedded_assets_serve_with_content_types_and_every_import_resolves` (AC-21),
  `delivery::a_pull_request_pointed_at_another_branch_is_never_merged` and the domain's merge
  refusal table (AC-22), the delivery test of the `rollout` command (AC-23); the e2e suite
  (the web UI changed).

- Slice 5: `secrets::a_use_by_use_secret_goes_somewhere_new_only_after_two_factor` (AC-24),
  `secrets::agents_request_hosts_and_people_allow_them` and
  `secrets::agents_bind_the_hosts_of_a_secret_they_set_up` (the token and the locked session
  on a host request and on `auto_allow`), `remote_mcp` (AC-25), e2e `secrets.e2e.ts` "a person
  allows the hosts an agent asked for", "a person allows an agent to copy a secret into .env,
  then revokes it" and "a new agent secret lets agents set it up unless turned off" (AC-26),
  `yulius-app` `vault::terminal::tests::output_is_withheld_while_agent_secrets_cannot_be_read`
  (AC-27), `yulius-egress` `proxy::a_request_whose_answer_cannot_be_checked_for_secrets_is_not_sent`
  (AC-28), `yulius-store` `an_edit_read_before_a_rotation_does_not_undo_it` and `yulius-app`
  `vault::team::tests::an_edit_overtaken_by_another_is_told_to_reload` (AC-29),
  `cockpit::a_superagent_screen_shows_no_output_while_a_secret_cannot_be_opened` (AC-27),
  `cd::production_gets_a_secret_agents_set_up` and `cd::production_secrets_need_their_own_allowance`
  (AC-30), `yulius-store` `env_copies_change_only_from_what_was_seen` and its host-request test
  (AC-31), `yulius-app` `session_docs::tests::side_files_are_copied_without_following_a_link`
  (AC-32), `secrets::the_proxy_gets_nothing_to_redact_with_while_a_secret_cannot_be_opened`
  (AC-33), `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task` (AC-34).

- Slice 6: `yulius` `host_setup::tests::the_browser_policy_is_written_where_it_is_missing_or_differs`
  (AC-35), `browser::viewer_clicks_and_keys_wait_for_the_hosts_browser_policy` (AC-36),
  `yulius-browser` `real_chromium::a_viewers_click_on_a_file_input_opens_no_file_chooser`
  (AC-37, opt-in: `YULIUS_TEST_CHROMIUM`), `policy::tests`, `browser::chromes_own_pages_are_no_tabs`,
  `browser::a_page_cannot_hide_a_window_behind_the_probes_address`,
  `browser::tabs_made_while_the_probe_waits_are_listed_and_a_tab_at_chromes_page_stays`,
  `host_setup::tests::a_helper_from_before_the_browser_policy_gets_its_drop_in_anew` (AC-38),
  `doctor::tests::the_browser_policy_line_warns_until_host_setup_wrote_it`, e2e
  `browser.e2e.ts` "a sandbox browser without the host policy says once that clicks and keys
  are off"; `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once` (the
  nit left by slice 5's review).

- Slice 7: `yulius-browser` `instance::a_secret_is_typed_only_into_a_page_it_may_go_to` and
  `browser::agents_type_secrets_only_into_pages_of_their_hosts` (AC-39),
  `instance::a_secret_typed_while_the_page_moves_on_is_reported` (AC-40),
  `instance::a_navigation_waits_for_a_secret_being_typed` (AC-41);
  `browser::tabs_made_while_the_probe_waits_are_listed_and_a_tab_at_chromes_page_stays` and
  `browser::a_tab_made_and_closed_while_the_probe_waits_is_no_tab` (the nits left by slice 6's
  review).
- Slice 8: `yulius-browser` `instance::a_secret_is_not_typed_into_a_frame_of_a_site_it_is_not_for`
  (AC-42), `instance::a_secret_is_not_typed_into_a_tab_leaving_for_another_site` (AC-43),
  `instance::a_secret_is_not_typed_into_a_page_that_changed_since_the_look` (AC-45), the unit
  tests `instance::tests::an_announced_navigation_stands_until_it_settled_or_is_old`,
  `instance::tests::announcements_are_bounded_and_losing_them_is_not_knowing` and
  `instance::tests::an_address_too_long_to_keep_is_no_address` (AC-46),
  `browser::a_secret_is_typed_neither_into_another_sites_frame_nor_a_tab_that_is_leaving`
  (AC-42, AC-43, AC-45 and AC-47 through the MCP tool); opt-in, on a real Chrome:
  `real_chromium::a_secret_is_not_typed_into_another_sites_frame_in_a_real_chromium` and
  `real_chromium::a_secret_is_not_typed_into_a_tab_that_is_leaving_in_a_real_chromium` (AC-44).
- Slice 9: `yulius-egress` `browser::tests::a_browser_reaches_its_sandboxes_ports_and_no_other_local_address`
  `browser::tests::a_name_that_resolved_to_a_local_address_is_refused`,
  `browser::tests::of_yulius_a_browser_gets_its_pages_only` and
  `browser::tests::a_refusal_names_the_sandboxes_ports` (AC-48); `tests/browser.rs`
  `a_browser_loads_its_sandboxes_ports_and_no_other_local_address`,
  `a_name_that_resolves_to_the_loopback_reaches_nothing_there` and
  `of_yulius_the_browser_gets_its_welcome_page_only` (AC-49),
  `nothing_is_added_to_or_taken_from_a_browsers_requests`,
  `a_browsers_tunnels_count_toward_its_sandboxes_share` and
  `a_browsers_upload_is_not_cut_and_a_silent_server_is_given_up` (AC-50); `yulius-browser`
  `manager::tests::chromium_args_send_everything_through_the_browsers_proxy` and
  `manager::a_browser_runs_behind_its_own_proxy_or_not_at_all` (AC-51); `yulius`
  `browser_network::each_browsers_proxy_reaches_its_own_sandboxes_ports` and
  `serve::tests::every_sandbox_browser_is_configured_behind_its_proxy` (AC-52) and, opt-in,
  `browser_network::a_real_chromium_reaches_its_sandboxes_ports_and_no_other_local_address`
  (AC-53). The nits left by slice 8's review:
  `instance::tests::an_address_too_long_to_keep_is_kept_by_its_site`.
- Slice 10: `yulius` `git_confined::a_sandboxes_everyday_git_works_confined` (AC-54),
  `git_confined::nothing_the_clones_configuration_names_is_run` (AC-55),
  `git_confined::a_link_in_the_clone_leads_git_nowhere_outside` (AC-56),
  `git_confined::only_the_sandboxes_own_origin_is_taken_from_its_configuration` (AC-57),
  `git_confined::the_launcher_starts_git_which_executes_gits_programs_only` (AC-58);
  `yulius-git` `tests::a_command_reaches_its_own_sandboxes_folders` and
  `tests::only_a_sandboxes_own_origin_is_a_local_one` (AC-59); every test of `yulius` that
  uses the real adapter (`support::confined_git`, AC-60). The nits left by slice 9's review:
  `yulius-egress` `nothing_is_added_to_or_taken_from_a_browsers_requests` (a request without
  a body). A flaky test of slice 5, fixed with what it found:
  `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once`.
- Slice 11: `yulius` `git_confined::a_transfer_with_the_token_is_not_shaped_by_the_clone`
  (AC-61), `git_confined::a_fetch_and_a_push_through_the_servers_own_repository_arrive` and
  `git_confined::a_data_folder_behind_a_link_transfers_as_any_other` (AC-62),
  `git_confined::the_loader_is_never_started_with_a_file_of_the_clone` (AC-64),
  `git_confined::a_missing_object_is_not_fetched_with_the_clones_command` (AC-65),
  `git_confined::a_push_goes_into_no_submodule` (AC-68),
  `git_confined::a_filter_added_to_a_new_clone_is_not_run` (AC-69),
  `git_confined::a_clones_transfer_settings_reach_no_transfer` (AC-70),
  `git_confined::a_shallow_origin_makes_a_whole_shallow_clone` and
  `playground::a_shallow_repository_keeps_its_branches` (AC-71),
  `projects::a_refused_first_commit_leaves_onboarding_pending` (AC-66),
  `git_confined::the_doctor_reports_the_confinement` and
  `doctor::tests::a_git_that_fetches_missing_objects_fails` (AC-67);
  `yulius-git` `runner::tests::a_confined_command_is_the_launcher_with_its_sandboxes_folders`,
  `runner::tests::nothing_is_started_with_an_argument_of_the_clones`,
  `runner::tests::a_local_transfer_gets_the_shell_and_no_credential` and
  `runner::tests::a_command_reads_what_it_is_given` (AC-63, TKR-008),
  `tests::a_command_reaches_its_own_sandboxes_folders` (TKR-005),
  `courier::tests::a_list_of_refs_is_read_line_by_line`,
  `courier::tests::the_clones_refs_follow_the_couriers` and
  `courier::tests::the_remotes_head_names_its_default_branch` (TKR-002). Every test of
  `yulius` that uses the real adapter with a playground or the stand-in for GitHub now
  makes its transfers through a courier (AC-60).
- Slice 12: `yulius-app` `ports::egress::tests::a_production_secret_applies_to_no_client`
  (AC-72); `yulius-store` `tests::a_production_secrets_old_hosts_are_removed` (AC-73);
  `yulius` tests with `support::production_secret` (`browser`, `cockpit`, `secrets`,
  `local_ci`: AC-74, the helper asserts the refusal); `e2e/tests/secrets.e2e.ts` "a
  production secret’s Edit dialog offers no hosts and says why" (AC-75); `yulius-git`
  `runner::tests::a_local_transfer_gets_the_shell_and_no_credential` and `yulius`
  `git_confined::a_folder_is_written_only_by_a_push` (AC-76).
- Slice 13: `yulius` `local_ci::a_run_again_tests_a_pull_request_with_its_own_jobs` (AC-77);
  `local_ci`, `merge_train`, `playground`, `remote_mcp` and `ci_setup` as they are (AC-78);
  `local_ci::nothing_is_tested_while_the_integration_branchs_configuration_is_broken`
  (AC-79) and `local_ci::a_branch_behind_is_tested_with_the_integration_branchs_jobs_of_now`
  (AC-80): two parts of the rule that only a unit test of its comparison, or nothing, held;
  `playground::a_task_that_changes_the_jobs_is_tested_with_its_own` (AC-81) and
  `playground::a_task_that_repairs_a_broken_configuration_is_tested_with_its_own` (AC-82).
- Slice 14: `yulius` `delivery::standard_flow_merges_on_green_checks_and_promotes_by_pr`
  (AC-83), `delivery::product_flow_deploys_on_confirmation_then_fast_forwards_and_rolls_out`
  (AC-84), `local_ci::a_pull_request_that_brings_ci_waits_for_its_first_run` (AC-85),
  `delivery::an_automatic_promotion_goes_on_the_checks_of_its_merge` (AC-87),
  `delivery::an_automatic_promotion_stops_at_a_failed_head` (AC-88);
  `delivery`, `remote_mcp`, `merge_train` and `playground` as they are (AC-86).
- Slice 15: `yulius-domain` `task::tests::an_armed_merge_lives_with_the_run_that_armed_it`
  (AC-89); `yulius` `delivery::an_armed_merge_ends_with_its_tasks_run` (AC-90); `delivery`,
  `merge_train` and `playground` (AC-91).
- Slice 16: `yulius-server` `error::tests::every_application_error_has_one_status` (AC-92)
  and `error::tests::no_route_maps_an_application_error_to_a_status_itself` (AC-93);
  `yulius` `restart::a_server_failing_every_request_ends_the_wait_after_a_few_tries` and
  the suites of `agent_hooks`, `remote_mcp`, `delivery` (AC-94);
  `delivery::an_armed_merge_ends_with_its_tasks_run` (AC-95, slice 15's rule).
- Slice 17: `yulius` `task_done::a_switch_waits_while_the_background_work_cannot_be_told`
  (AC-96), `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run` (AC-97),
  `task_done::a_switch_whose_old_run_had_to_be_killed_starts_no_new_run` (AC-98),
  `task_done::a_run_its_switch_gave_up_is_that_switchs_loss_whoever_sees_it_first`,
  `task_done::a_run_that_exits_as_its_switch_gives_it_up_rests_the_task` and
  `task_done::a_run_that_exits_before_a_refused_kill_rests_the_task` (AC-99),
  `task_done::a_run_whose_end_is_being_recorded_as_the_wait_runs_out_is_switched` (AC-100),
  `task_done::a_run_that_ends_before_its_switch_claims_it_closes_the_task` (AC-101),
  `task_done::a_run_whose_exit_is_being_recorded_at_a_refused_kill_rests_the_task`
  (AC-102), on the real session host behind a test host that can drop requests to end,
  refuse to terminate and leave the look unanswered; the orders are made with pause
  points (`SwitchDecided`, and three new ones: `RunEndTaken`, `SwitchWaitOver`,
  `SwitchEndsRun`);
  `host_move::a_task_left_switching_whose_run_ended_is_lost_to_the_restart` (AC-103), on a
  host that finds every run ended.
- Slice 18: `yulius` `accounts::a_failed_sign_in_leaves_a_refused_subscription_needing_one`
  (AC-104, through the sign-in's own terminal with a scripted host) and
  `account_usage::a_usage_reading_leaves_a_broken_subscription_broken` (AC-105);
  `account_email::the_round_asks_no_address_of_a_subscription_that_needs_a_sign_in`
  (AC-106); e2e `subscriptions.e2e.ts` "a subscription in error is asked for no usage
  reading" (AC-107) and "a failed sign-in of a refused subscription says its own reason
  and leaves the refusal on the card" (AC-108); the existing
  `accounts::failed_logins_record_errors_and_relogin_failures_keep_working_accounts`
  keeps the two other cases of SGN-001.
- Slice 19: `yulius` `pool_supervision::a_resume_past_a_limit_that_waits_in_the_queue_is_still_one`
  (AC-109) and `pool_supervision::a_continue_on_a_chosen_subscription_keeps_it_through_the_queue`
  (AC-110), with the real session host and one slot; `yulius-store`
  `tests::a_queued_task_keeps_its_start_while_it_waits` (AC-111); `yulius-app`
  `supervisor::tests::a_kept_start_comes_back_as_it_was` (every kind, with and without a
  prompt and a subscription);
  `pool_supervision::a_chosen_subscription_that_filled_up_while_waiting_is_given_up`
  (AC-112) and `pool_supervision::a_resume_that_finds_the_queue_full_keeps_its_pause`
  (AC-113).
- Slice 20: `yulius`
  `pool_supervision::a_hand_over_after_a_restart_still_restarts_a_run_with_personal_servers`
  (AC-114), `first_message::a_goal_still_to_be_typed_at_a_restart_is_flagged` (AC-115),
  `first_message::a_typed_goal_that_was_taken_leaves_no_marker` and
  `first_message::a_typed_goal_recorded_before_a_restart_is_not_flagged` (AC-116),
  `pool_supervision::a_reattached_run_with_another_persons_servers_is_restarted`
  (AC-117); each restart is a second server on the same data that reattaches the first
  one's run.
- Slice 21: `yulius` `context_full::a_persons_own_refused_prompt_is_compacted` (AC-118),
  `context_full::a_possible_draft_holds_the_compaction_and_its_person_is_told` (AC-120)
  and `context_full::a_held_compaction_is_typed_once_its_hold_is_over` (AC-121), with a
  hooked agent and keys typed through the task's terminal;
  `context_full::nothing_is_typed_between_a_line_of_yulius_and_its_enter`,
  `first_message::nothing_is_typed_between_the_goal_and_its_enter` and
  `task_done::nothing_is_typed_while_a_typed_goal_is_not_taken` (AC-119, with pause
  points after the line and after its Enter, `LineTyped` and `LineEntered`);
  `pool_supervision::a_reattached_run_at_a_limit_is_restarted_past_it` (AC-122);
  `context_full::a_key_just_before_the_compaction_keeps_it_out` and
  `first_message::a_key_just_before_the_goal_keeps_it_out_and_is_said` (AC-123, with a
  pause point before the typing mark, `LineReady`);
  `context_full::a_draft_typed_during_the_compaction_keeps_the_wake_up_out` (AC-124, the
  key at the pause point after the Enter, `LineEntered`);
  `context_full::a_command_sent_behind_real_text_continues_in_a_fresh_session` (AC-121)
  and `context_full::a_compaction_held_at_a_restart_is_taken_up_again` (AC-125);
  `context_full::a_hold_whose_box_was_emptied_without_a_key_ends_its_flag` (AC-126, the
  hook sent by the test with the run's own token).
- Slice 22: `yulius` `migration::a_silent_helper_is_lost_only_once_its_service_ended`
  (AC-127, the fake helper says what systemd says and counts the questions),
  `migration::an_unanswered_request_and_a_silent_helper_are_said` (lost as before when the
  service ended), `wiring::migrate_files::tests::the_helper_runs_while_systemd_says_its_service_does`
  (AC-128, a fake `systemctl`), `host_move::a_mark_that_is_not_text_still_brings_the_tasks_back_and_goes`
  (AC-129); `yulius-store` `tasks::tests::a_close_time_is_the_close_the_task_is_in` and
  `tests::a_task_that_is_not_closed_loses_its_old_close_time` (AC-130).
- Slice 23: `yulius-domain` `task::tests::group_follows_the_sidebar_rule` (GRP-001 to
  GRP-003, every activity and pull request case); `yulius`
  `task_pause::a_paused_task_is_counted_working_whatever_it_waited_for` (AC-131),
  `import_server` (the onboarding test's counts, AC-132),
  `merge_train::an_achieved_goal_with_an_armed_merge_is_not_done` (AC-133); e2e
  `task-done.e2e.ts` "a paused task, a waiting onboarding task and an armed merge are counted
  as the server counts them" (the sidebar's rule in the page), `live-editor.e2e.ts` "an edit
  refused for lack of access gives the text back and leaves nothing waiting" (the document
  against a scripted server) and "… makes the file read-only in the Editor and says so" (the
  mock refuses edits on a switch).
- Slice 24: `yulius`
  `preview::responses_reach_the_browser_as_the_app_sent_them_at_the_preview_origin` (AC-135,
  a compressed document added to the fixture app; the test was named for the frames it no
  longer allows), the handshake test's landing page (AC-135, the proxy's own pages) and the
  preview proxy's unit test of the CSP rewrite; e2e `live-editor.e2e.ts` (AC-136, the `ended`
  and `gone` cases); the
  configurations of the test fixtures that still name `webhook_secret` (GHA-001: they
  parse); the gate's lint and e2e for the deleted web code.
- Slice 25: no new test (DED-001: nothing behaves differently). The tests that only exercised
  a deleted item went with it or now exercise what production calls
  (`current_used_percent_for`, `available_at_for`, `slug_or`, `set_account_state`,
  `task_messages`, value equality of a move's values); the gate and the whole e2e suite
  (AC-137).

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
- [ADR-0316](../../docs/decisions/0316-access-rules-hold-whatever-the-way-in.md): the scope
  in every access check; the sign-in bound to its browser by a cookie holding the state; the
  lab not shipped; the base as a fact of the merge rule; production-stage commands under
  `cd::may_deploy`. The response headers' missing script and style policy is finding W1b.
- [ADR-0317](../../docs/decisions/0317-a-guarded-secret-goes-somewhere-new-only-after-two-factor.md):
  the unlock before a secret allowed use by use goes somewhere new, not for one agents set up;
  tokens refused; redaction that cannot load withholds, the proxy before it sends; an edit
  compares the sealed value.
- From the review of slice 4 (its two minor findings and three of five nits are in slice 5):
  `form_train` does not skip an armed pull request that was pointed at another branch. It is
  left so: skipping needs a GitHub read per candidate on every look, and such a train is
  refused when it lands (`WrongBase`) and the armed merge dropped, so the cost is one train.
- [ADR-0318](../../docs/decisions/0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md):
  Chrome's own windows are switched off by a managed policy host setup writes; the browser is
  asked whether it loaded it; no policy, no clicks and keys. A private file system view for
  Chrome comes with its network (E2).
- From the review of slice 5, round 4 (two nits): a message refused for a withheld screen now
  flags the task once (slice 6). The launch goal's watch is left: with a withheld screen an
  untaken launch goal is flagged as "Claude Code did not take the task's goal" while the
  terminal says its screen is withheld; nothing is typed either way, and telling the two
  apart needs a program that draws without answering, which no test here has.
- [ADR-0319](../../docs/decisions/0319-a-secret-is-typed-into-the-page-the-browser-shows-when-the-typing-begins.md):
  the page rule is applied in the browser with the input lock held; Yulius's navigations wait
  for the lock; a page's own move is reported.
- [ADR-0320](../../docs/decisions/0320-a-secrets-keys-reach-only-sites-it-is-for.md): unless
  the focus is in a text field of the page, every frame of another site the page holds must
  be one the secret is for; a page with a frame on its way to a page the secret is not for
  gets nothing typed. The second is asked although Chrome 157 holds its answers during a
  navigation (measured), because nothing promises that. The secret's user name and one-time
  code follow the same rules (E10 is fixed in this slice: it is the same typing path and the
  same rule, and leaving it open would leave the slice's claim false for two of three fields).
- [ADR-0321](../../docs/decisions/0321-a-sandboxes-browser-goes-out-through-a-proxy-of-its-own.md):
  each browser behind a proxy of its own, served by the egress proxy with the sandbox's rule;
  no browser without it. Chrome inside the sandbox's namespaces stays the firmer fix and is
  not attempted here: it cannot be proven inside a sandbox.
- [ADR-0322](../../docs/decisions/0322-the-hosts-own-git-runs-confined.md): the host's own
  Git is confined by Landlock to Git's programs and its sandbox's folders, through a
  launcher that is the Yulius binary itself (the workspace forbids `unsafe`, so the
  restriction is applied by a process to itself before it becomes Git, not in a `pre_exec`).
  The shell only for transfers Yulius names; a remote's name stands for a folder only when
  it is the sandbox's own origin. No Landlock, no Git.
- [ADR-0323](../../docs/decisions/0323-a-transfer-with-the-servers-token-is-made-in-a-repository-of-the-servers-own.md):
  transfers with the token in a courier of the server's own; no adapter configuration for a
  command in a sandbox's folder; nothing started with an argument of the clone's. The
  remote's address stays the clone's for now (ADR-0323 says why).
- Slice 25 (no decision record): a dead item goes rather than being wired in where the
  auditors offered both. The task events could have been fired from the supervisor so that
  `apply` would be the one table of every change; that moves no behaviour and the supervisor's
  compare-and-set writes are what the slices 17 to 21 tests pin, so the table stays the
  table of a waiting task's start changes, and the module says so. A seam only tests used
  goes when a production method sets the same state (`set_account_error` →
  `set_account_state`); one that is the only way to make the state the export reads stays
  (`insert_message`).
- Slice 24 (no decision record: ADR-15 decided that nothing frames a preview; this slice
  removes what that left). A preview keeps `frame-ancestors 'self'` of Yulius's own rather
  than only the app's policy: a dev server rarely sets one, and a page that holds a person's
  preview grant should not be framed by another site; the app's stricter policy and its
  `X-Frame-Options` are kept, so a stricter app stays stricter. `webhook_secret` is removed
  rather than kept for a future webhook: a webhook route would need its own secret, set up
  with it. The deleted web code has no history to keep (it is in Git).
- Slice 23 (no decision record: SPEC-191, SPEC-153 and SPEC-487 already decided each case;
  the slice makes the second rule say what the first said). The pause wins over everything
  but an end, as the sidebar always had it: a paused run cannot answer a prompt or a person
  until it is resumed, so "Needs you" asked for something that could not be done. The
  onboarding task is counted in line rather than working, as the sidebar lists it beside the
  prepared tasks; the sidebar's counter changed, not its group. An armed merge is its own
  fact (`merge_armed`) rather than "news due" (`yulius_wait`) applied to achieved goals:
  CHN-003 keeps an achieved goal Done once nothing but news is left, and only the armed merge
  is work still to come. Archiving such a task is refused by the existing rule for live tasks
  ("stop the task before archiving it") instead of a new message. A refused edit gives the
  text back rather than keeping it for a later retry: the refusal is about access, which does
  not come back by retrying, and the person still sees their file; a new join asks again.
  The typing is given up in the tab itself (the text it had from the document), not by a
  catch-up: a person who lost all access to the project gets no catch-up either, and a
  rejoin loop would make the file writable again for each try (review of slice 23). The
  awaited onboarding task is now also one of the tasks "after all current tasks" names
  (`manage.rs`); it counts as done once onboarding stops holding tasks, which they wait for
  anyway.
- Slice 22 (no decision record: each fix makes a reading agree with what the code
  already said). A silent report is lost on systemd's word, not on a longer timeout: the
  copy may block the report's write for any time, and a longer wait would only delay a
  person whose helper really died. Systemd's silence counts as running: a move that is
  held as in flight for nothing costs an update's wait, an update that stops a move
  costs the move (MVB-023). Asked each time and not kept: only a silent report asks, and
  its answer must change as soon as the service ends. `FileMoveHelper` takes the server's
  unit and names both helper units itself (`units::migrate_path_unit`, `migrate_unit`),
  rather than deriving one name from the other. The stale close times are cleared by a
  migration rather than left: no reader wants a close time of a task that is not closed.
- Slice 21 (no decision record: SPEC-480's care for a person's draft, brought to the
  typists it did not reach). `/compact` typed behind a person's unsent text sent that text
  as part of a command; that is what happened. The first version of this slice left such
  a session to its person, as one they type into after the refusal. The review showed
  what that costs: "a draft" is any key since the last prompt sent from the box (an arrow,
  Esc, a dialog's answer), a goal task goes on by its Stop hook and never sends a prompt
  from the box, so one key hours ago would stop every later compaction, with one line in
  a terminal nobody watches. So the compaction is held, not given up: the task is flagged
  with the reason and what to do, as a held message's task is (SPEC-480 DRF-006), and
  after ten minutes the command is typed all the same. With an empty box (the key left
  nothing: the usual case of a task nobody watches) that compacts the session and keeps
  its conversation. With real text there the command is no command: it goes out behind
  the text as a prompt, the full context refuses it, and the task continues in a fresh
  session, which is where it would have ended anyway; the refusal counts as the
  compaction's failure at once (the review's second round: left to the compaction's own
  ten minutes, the texts promised a compaction that could not come). The person was told
  all of this for ten minutes, in the flag and in the terminal. The other ends were
  weighed and are worse: waiting for good stops unattended work for a key that left
  nothing, going to a fresh session without trying loses the conversation also when the
  box was empty, and clearing the box first (Ctrl-U) clears one line of it. The flag is
  the hold's: it ends with it on every way out, and a restart of Yulius does not turn a
  hold into "gave up". Yulius's own Enter empties the box in what Yulius knows, since a
  hook may not say so for a command; without that the very key the hold was for kept the
  wake-up out after the compaction. The review went to a fourth look at one change,
  against the rule of three rounds, as slice 17's did: the third round's only finding
  above a nit was a regression of this slice's own second-round fix, with its fix named
  by the reviewer in two lines; a split would have delivered the regression or held back
  the rest. The rule is not the message's rule (`box_held`: a person's keys, and a
  minute after their last one): a line for a full session also stays behind a message of
  Yulius's own whose Enter failed, and does not wait for a person who is only near.
  The goal's typist keeps its own question (did a person type since the run was
  attached): a new process starts with an empty box, so it is the same question there.
  The one typing mark is waited for by Yulius's own lines and never refuses them: a
  recovery must not depend on a mark that someone forgot to clear; what was asked before
  the wait is asked again once it is held. The goal keeps the mark until it is taken: the
  review found that between its tries the line may sit in the box (from before this
  slice), where a message would be sent with it or erased by the retype. The three waits
  for a quiet terminal (the goal, the first message, the full session) stay three loops:
  each asks other things while it waits, and one helper would take five parameters.
- Slice 20 (no decision record: two more marks of the kind the run directory already
  holds, `launch-goal-pending`, `session-started`, `person-typed`). The facts belong to
  one run and are needed only while it lives, so they are files beside it, not columns.
  For the goal the auditor named two ways: flag at once, or start the typist again. The
  typist is not started again: a restart cannot tell whether the goal's line was already
  half typed, and typing `/goal` over it would submit both. Nor is the task flagged at
  once: the goal may have been taken just before the restart. The new server does what it
  does for a launch goal (the same watch, with its own mark and its own sentence): the
  transcript's replay says whether a goal was recorded, and only a task left without one
  is flagged. The typist became a function that says whether it ended with an outcome,
  so that the mark is removed in one place; a run that ended, or could not be typed into,
  keeps it, and has no later server to read it. A launch that cannot write the personal
  servers' mark fails rather than starts a run the next server knows nothing of; the
  goal's mark is only logged when it cannot be written, as the launch goal's is.
  The first version watched from the reattach on, and its flag could come inside the
  session's opening turn, whose end then set the task back to "your turn" (the review,
  finding 1). The mark now says whether the session opens with a turn, and a restarted
  server that finds no answered Stop of the run gives the run that turn back: the same
  two facts the old server had, read from the run directory, not guessed from the screen.
  A Stop that was answered by a hold (no capacity) counts as answered; such a run's
  opening turn then ends through the gates, as every reattached run's did before, and a
  flag raised before that end can still be taken away by it. The other way round, an
  opening turn that ended while Yulius was down left no answered Stop (the hook's client
  got no answer): the new server gives the run a turn that is over, so the flag comes
  after the opening turn's bound (five minutes) and the bound of the watch, and the
  session's next Stop ends as an opening turn's does, once (the review's second round, a
  nit). Telling these apart needs the transcript's replay, which the watch does not wait
  for.
- Slice 19 (no decision record: a column and the one function every start goes through).
  The auditor proposed to leave a paused task paused when it cannot start, so that the
  pause keeps the start. Not taken: a paused task would then wait beside the queue, not
  in it, and lose its place to every task queued later; and Continue on a chosen
  subscription has no pause to keep anything. The queue itself keeps the start instead,
  in the task's row, read in `start_run_if` under the launch lock: whoever starts a queued
  task (the queue's pass, with its plain start) gets the kept one, so no caller can forget
  to ask. It is written with the status in one transaction (`queue_task`) and
  removed by the status change that takes the task out of the queue, so a held prompt
  stays in the database no longer than a pause's continuation does (the review, findings
  1 and 2: the first version only made it unreadable by the task's `queued_at`, and wrote
  it in a second step). The `queued_at` check stays as a guard. A subscription is kept
  only when a person chose it (the start of a resting task); the pool's choice for a
  paused task is a fact of the moment it was made, and keeping it could send the task to
  a subscription that filled up while it waited. A person's choice can go stale the same
  way; then the task is not failed, as an immediate Continue on that subscription is
  refused to the person's face: nobody is there, so the pool places it and the terminal
  says so (MOV-002 forbids landing elsewhere unsaid, not landing elsewhere). One case
  keeps no choice: a person's move of a running task whose old session reports a limit as
  it ends goes on from `waiting_capacity`, and if every slot is taken then, the task
  queues with the pool's choice (the status read, not the caller, says who chose).
  The pause of a paused task used to be ended before its start was tried, and a start the
  full queue refused left the task waiting for capacity with no pause: nothing started it
  again (the review, from before this slice). The start ends the pause now, where it is
  made or queued.
- Slice 18 (no decision record: the comment at the usage write already stated the rule,
  "a reading never makes a subscription that needs a new sign-in usable again"; the code
  now has it once). `error` is kept as a status: it is what a first sign-in that failed
  reads, with no credential. Rows that the old code left `error` with a credential stay
  so until a sign-in succeeds; no migration turns them into `relogin`: they are out of
  the pool either way and the page offers the sign-in for both, so it would change a word
  and nothing else. A failed sign-in of a refused subscription keeps
  Claude's reason and does not add its own: the card must go on saying why the
  subscription is out. The sign-in's own reason goes where it belongs, to the sign-in:
  its `exit` frame carries it (the frame's `cause`, which a task's terminal uses for the
  limit a run ended at), and the dialog reads it there. The first version of this slice
  left the dialog reading the subscription's error, which would have named Claude's
  refusal as the reason the sign-in failed (the review, finding 1).
- Slice 17 (no decision record: END-001 and SPEC-002's `stopped(switch_timeout)` applied
  where they were not). After a switch's wait the run that had to be killed ends the
  switch, and no new session starts. The other way was weighed: go on with the switch once
  the host is proven gone, which keeps an autonomous task working. It was what the code
  tried, and three recorders see that run's end (the switch, the run's reader, the run's
  own exit): going on needs all three to agree that the next run starts, resting needs
  only that each of them rests the task. A session that ignores its end for a minute after
  the host's own escalation is broken, and the state machine already had the status for
  it. The first version of this slice cleared the switch's intent before the kill and so
  left two orders in which the task stayed `switching` with no run (the review, finding
  1): the intent slot now says "given up" instead of nothing, each recorder reads it, and
  a run whose end was already taken is waited for instead of killed. The second round
  found the order before all of that: a run that ends by itself between the task's change
  to `switching` and the switch's claim of its end met a status with no way out (before
  this slice the switch recovered it by starting the next run a minute later). Such an end
  now ends the task as from `running`, which is what "the task follows its session" says,
  and the switch finds a resting task. The orders are tested with pause points, as the
  stop's and the close's are (SPEC-250). The wait is a setting (`switch_end_ms`) only so
  that tests need not wait a minute. The status's text in the web UI says "Restarting the
  session took too long": a restart on a newer Claude Code ends there too.
  The third round found where that new way out reached too far: a start that finds a
  `switching` task with an ended run records that end through the same recorder, and the
  task closed as if its session had left. The start rests such a task itself first, as it
  rests one with no run. The review went to a fourth look at that one change, against the
  rule of three rounds: the finding was a regression of this slice with its fix named by
  the reviewer, and a split would have delivered the regression or held back the rest.
  Left as it is, a nit by the review's word (the mark it called harmless): when a run
  ends in the window before the switch claims it and the switch reads the status before
  that end has settled, the switch says "did not end in time" for a task that closed, and
  leaves its given-up mark in the slot until the task's next launch clears it.
  A finished (`Done`) task whose idle session is moved and has to be given up rests as
  stopped, not closed: RBT-001 closes only a running task, and its domain test says why
  for this very case ("a switch or a stop in flight was cut"); the review asked for
  `Closed`, which would change that rule for every loss during a switch, not only this
  one. (A task's activity is cleared when it becomes `switching`, so the question does
  not even arise in the store: a mutation that asked RBT-001's rule at the exit of a
  given-up run changed no outcome.)
  The auditor's third point, the end of a run recorded twice (`finalize_attempt` takes the
  task's intent before it knows it is the first to record), is not changed: no path
  records a run's end while its reader still serves it and then starts another run.
  The test starter takes its two waits as a pair, not a struct: it has two callers.
- Slice 16 (no decision record): the loopback listener takes the public table as it is,
  422 for an invalid request included; nothing in the repository reads its 400 (the hook
  client takes any 5xx for "try again while held" and any other error for none, the
  credential helper anything but 200 for no credential, `yulius wait` is described above).
  `yulius wait` keeps waiting out 502 to 504: with one table a 502 from Yulius means that
  what Yulius asked (GitHub) could not be reached, which passes by itself as an outage
  does; only the words it prints then ("Yulius cannot be reached") are less exact, and
  stay. The source test (AC-93) is the guard against a fourth table; a test through the
  listener would need a store that fails on demand, which the harness does not have.
- Slice 15 (no decision record: DLV-205's sentence "put back at startup for tasks still
  running", made the rule of every pass too): an armed merge lives with the run that armed
  it. The other way was weighed: the order stands whatever becomes of the task, so a
  closed task's green pull request merges. It was not taken, for three reasons. Everything
  that moves an armed merge forward already needs a running, idle agent (its branch
  brought up to date, a place in a merge candidate, a refusal answered), so the order could
  only be kept for the one case that needs nothing more. A person who stops or closes a
  task has ended what Yulius does for it, and a merge afterwards is a surprise. And a
  restart already decided this way. "Held" states keep the merge (the run is there and
  comes back by itself), and so does `Queued`: a held run goes through the queue when its
  capacity returns while every slot is taken, and a task revived after a host move starts
  from it (the review of this slice; a task queued for its first run has armed nothing).
  `Stopping` does not (the run is being ended). That the pull request was merged already
  is asked before the run's end is said: a person's merge does not clear the armed slot.
  The agent's own message says what to do; the pull request is not touched.
- Slice 14 (no decision record: DLV-006's rule, "Yulius CI is a project's only CI", given
  one place): the answer is `dlv_checks_at`, and a promotion asks it for the integration
  head, which is the promotion pull request's head too. A fast-forward promotion is held
  to the same checks as one by pull request (it had none): what reaches production is the
  same commit either way, and `require_checks` is the profile's word for both. One
  promotion does not wait for a verdict of its own: the automatic one (`auto_promote`),
  wherever it starts (after a merge, or after the last integration environment of a merged
  change). What it promotes went in on green checks of a current head, the commit the merge
  made may have no run yet, and nothing would promote later if this were refused as
  pending; a verdict that is already there (failed, unreadable) stops it. Narrowing the
  exception to the merge's own commit was weighed (review of this slice) and not taken: a
  promotion after an environment that finishes before the head's run would then never
  happen, where before this slice a fast-forward read no checks at all. The
  "pending before the first run" case moved from the remote control into `ci_gate_result`,
  so that a task's `delivery_merge` waits for it too; it costs one read of the commit's
  `yulius.toml` per question, only in a project without CI and only for a commit without
  a run. The remote control keeps its two refusals with their own words (a broken
  configuration on the integration branch, an unreadable one in the pull request).
- Slice 13 (no decision record: an existing rule, CI-110 and SPEC-217, given one place):
  the rule lives in `ci_gate_for_head` and takes the commit and what it is (a proposal or
  the branch with a change merged); what the commit branched off is the merge base with the
  integration branch for every caller. For a train's merge commit that is the integration
  head it was built on, as its own copy had it. For a playground's it is that head too,
  where its copy compared with the task's fork point: the answers differ only when `dev`
  changed its jobs since the task began, the task did not, and the playground's profile
  has a `gate` command (the copy then ran `dev`'s project jobs as the task's own instead of
  the profile's gate); the new answer, the gate, is the rule's, since the task changed
  nothing. A playground task that changes its
  jobs now keeps the trusted security job: the copy that lacked it was the odd one. A
  playground task's merge is still tested with its own jobs while `dev`'s configuration is
  broken (the review of this slice: without it the repairing task would land untested, and
  a playground has no other checks); a pull request beside a broken branch is not, as
  before. A run again asks the
  rule anew instead of storing the first run's jobs: the answer for a commit changes only
  when the integration branch's CI does, and then the scheduler would give the new answer
  too.
- [ADR-0324](../../docs/decisions/0324-a-production-secret-reaches-no-run-and-has-no-hosts.md):
  one rule for a production secret at the egress proxy (no client gets it), the hosts old
  rows kept removed by a migration with an audit line, the pair refused at insert; the
  dialog's choice for such rows removed with them. Chosen over keeping the hosts as text
  and over leaving the rule to the rows alone.
- From the review of slice 11, round 1 (two major, five minor, seven nits; all answered in
  the slice): a remote without an address is refused instead of named to Git, and no
  program is asked for a password; a new clone and the first commit of an empty repository
  are made outside the sandbox's folder (chosen over a courier that only fetches: a real
  `git clone` twice keeps what a clone is to Git itself); what the clone has is written into
  the courier as data, so that no process of the clone's works there before the token's
  last step; the note of the remote's default branch is asked from the remote, not left to
  the Git version at hand. Found on the way and fixed with it: G3, by `GIT_NO_LAZY_FETCH`
  (chosen over transfers without the shell, which would need a second process per transfer
  and the launcher to pass descriptors; Ubuntu 24.04's Git honours the variable, checked in
  a container, and the doctor asks every host's Git). G2 stays open where Git is not
  confined (macOS, a developer's machine): ADR-0323 says so.
- From the review of slice 11, round 2 (two major): no command goes into a submodule (both
  keys pinned, for every command, rather than an option on each push: the same rule in one
  place); a clone into a sandbox's folder checks nothing out, and its files are checked out
  without the shell (chosen over making the whole clone elsewhere and moving it in: a move
  between folders with different quota projects is refused by the file system, and a copy
  by the server itself would follow links that Git, confined, cannot). Both are again cases
  of one class, which this slice closes case by case and names as G5; the next slice takes
  the shell from every command that reads a clone's configuration.
- From the review of slice 11, round 3 (one major: a third case of the class, in this
  slice's own step): the list is given up for the rule, here and not in a later slice. The
  delivery rule says to split a change after three rounds; what round 3 found cannot be
  split off, because the step it is in (the clone's fetch from a courier) is what makes
  the slice work, and merging the slice without it would add a command that may start the
  shell to every GitHub sandbox's clone. So the slice is finished by the design the three
  rounds point to, which also makes it smaller in what it claims (a rule in two places of
  the runner instead of a list), and is reviewed anew by a fresh reviewer who has not seen
  the earlier rounds. The design: a courier for every transfer of a clone, a playground's
  too; the courier's commands work on the clone's objects (`GIT_OBJECT_DIRECTORY`, chosen
  over an alternate plus a transfer into the clone, which is the command that had to go,
  and over the server copying packs itself, which would write in a sandbox's folder
  outside Git's confinement), with maintenance off for them (the courier knows only the
  remote's branches, and would take the clone's own commits for unreachable); refs as data
  through `update-ref --stdin` (the runner learns to give a command input); the clone's
  `shallow` file through a link in the courier, which Git reads and replaces in place (a
  playground made of a shallow upload); a new clone as `init`, a fetch and a `checkout`.
  This replaces what rounds 1 and 2 chose for the same steps (the clone's fetch from the
  courier, a new clone copied from a courier and then copied without its files, the
  objects lent through an alternates file and its one-line rule). The earlier pins stay as
  depth.
- From the fresh review of the slice as redesigned, round 1 (two major, three minor, five
  nits; the rules on the token and the shell held): the clone's refs are set in two steps
  with the values the clone had (as Git's own fetch prunes first and checks what it
  replaces); a fetch and a listing reach the folder they take from read-only (chosen over
  copying the `shallow` file in and out by the server: with the origin read-only the link
  can lead Git only where the clone's agents write anyway, and the object folders linked
  into the origin, which slice 10's fetch could be made to write through, are closed by the
  same rule); the courier reaches the clone's sandbox by the caller's path; a failed clone
  removes its folder and a clone needs an empty one, as `git clone`; a new clone asks for
  every tag; the doctor's `PATH` and the bound on a clone's list of refs are said in
  ADR-0323.
- From the review of slice 10 (four nits, answered in slice 11): the loader (what the claim
  rests on is in ADR-0322 and enforced in the runner); a sandbox being made gets its own
  folder; what `/etc` and `/usr` readable means is in ADR-0322 already; the safety of a
  transfer between folders rests on the other folder being one Yulius names (ADR-0322).
- From the review of slice 9, round 2 (two nits, answered in slice 10): a request without a
  body goes on without one (a wrapper of the body that keeps its end and length); ADR-0321
  says that a name can still be answered with an address of the host's private network.
- Slice 5's `a_message_kept_out_of_a_withheld_screen_flags_the_task_once` failed about one
  run in five: it typed a prompt, and the fake agent's turn (working, then waiting) replaced
  the flag before or after the test's own. The agent's states replacing a flag is how flags
  work (`set_task_activity`); what was wrong in slice 5's code is that the withheld screen
  was flagged once per run, so a flag replaced that way never came back. It is raised with
  every message kept out now (a standing flag stays; the terminal is told once), and the
  test types a key that starts no turn.
- From the review of slice 8, round 3 (two nits, answered in slice 9): of an announced address
  too long to keep, its site is kept, so a sign-in redirect with a long query to a site the
  secret is for no longer refuses a fill; the report after the keys says what is known ("the
  focus is not in a field of this page").
- From the review of slice 8, round 1, not changed and why: the wiring of two lines has no
  test of its own (the record's `session_ended` on a detached session, and `lose` on skipped
  events): a detached session's id is never asked again, so the first only frees memory, and
  a lag of the event channel cannot be made from a test without a hook in the product code;
  both functions are unit-tested. Frames nested inside an out-of-process frame, in that
  frame's process, are not asked (ADR-0320 says why that is the allowed site's own page).
- Slice 8's first gate failed in slice 5's own test
  `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task`: the task was
  flagged before "Yulius is typing the first message" was cleared, so a message sent on seeing
  the flag was refused as "still typing" instead of with the withheld screen. The typing now
  ends first (`supervisor.rs`); nothing was typed either way. Fixed here, not filed: it is
  this task's code.
- From the review of slice 6, round 3 (three nits, all answered in slice 7): a tab's later
  announcement held with its first is replayed too; a tab made and closed during the wait is
  no tab; a probe whose tab comes after it gave up closes it.
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
| G1 | git | The host's own Git works by path in the agent-writable clone: its `git config` writes and index writes follow links, and a repository's configuration can make it run commands (ADR-0114's remaining hole, #54 part B, #486) | critical | fixed (slice 10): what Git executes and the files it touches; the token's connection is G2 |
| G2 | git | A key for a URL in the clone's configuration (`http.https://github.com/.proxy`, `.sslVerify`) beats the server's own: the connection of a fetch or push that carries the server's token can go through a proxy in the sandbox without a certificate check (found in slice 10) | high | fixed (slice 11) |
| G3 | git | A clone can name a "promisor" remote whose command for the other side the shell of a transfer between two folders runs when an object is missing (found in slice 11) | high | fixed (slice 11) |
| G4 | git | A new clone with the token, and the first commit of an empty repository, are made in the sandbox's folder, where its agents can add to the configuration or the commit (review of slice 11) | medium | fixed (slice 11) |
| G5 | git | A command that reads a clone's configuration may start the shell in a transfer between folders, and the shell runs what that configuration names for a transfer (a promisor remote's command, a submodule's remote's, a credential helper for a bundle address, a new clone's filter) (review of slice 11) | high | fixed (slice 11) |
| C2 | containment | `upper/etc/passwd` and `group` are read through links, unbounded, into people's `/etc/passwd` | high | fixed (slice 3) |
| C3 | host reads | Transcripts in the sandbox home are opened by path (FIFO blocks threads, links followed) | medium | fixed (slice 3) |
| C5 | host reads | `usage::measure` walks by path | low | fixed (slice 3) |
| I4 | host reads | `record_number` lists a directory through a link | low | fixed (slice 3) |
| A1 | access | A `read` API token reads the working tree (ignored `.env` included) over REST GET | medium | fixed (slice 4) |
| A2 | access | GitHub sign-in is not bound to the browser that started it | low | fixed (slice 4) |
| A3 | access | Revoking an API token leaves its open terminal sockets typing for up to 30 s | low | fixed (slice 4) |
| A4 | access | `GET /api/auth/github/start` stores states without a limit | low | fixed (slice 4) |
| W1 | web UI | The brand lab ships in the binary: `?lab=` injects CSS, which makes a device approval one blind click | high | fixed (slice 4) |
| D3 | delivery | The merge gate never pins a task pull request's base to the integration branch | high | fixed (slice 4) |
| P8 | delivery | A production-stage environment run starts for a writer with `confirm: true` (ADR-0110: an administrator typing the slug) | medium | fixed (slice 4) |
| W1b | web UI | The response headers carry no script or style policy (`Content-Security-Policy` is `frame-ancestors 'none'` only) | low | slice 18 |
| E1 | browser | A viewer's mouse and keys drive a Chrome on the host: file chooser, context menu and DevTools reach host files | critical | fixed (slice 6) |
| E2 | browser | Browser tools reach other sandboxes' ports, host loopback services and link-local addresses | high | fixed (slice 9) |
| E3 | browser | `browser_fill_secret` checks a cached URL, then types after waiting for the input lock | high | fixed (slice 7) |
| E8 | browser | A secret's keys go to the focused element, which can be a frame of another site inside a page of the secret's host (a selector naming the frame, or a click into it before); answered `filled` (found by the review of slice 7; on `dev` before it) | high | fixed (slice 8) |
| E9 | browser | A navigation under way when a secret's typing begins (a clicked link, an Enter) can land between the look at the page and the keys; reported afterwards, not prevented | medium | fixed (slice 8) |
| E10 | browser | `browser_fill_secret` types the user name and the TOTP code through `tool_type`, with none of the page, frame or navigation checks: a live code can go to another site's frame or a tab that moved on (found by the review of slice 8; on `dev` before it) | high | fixed (slice 8) |
| V3 | vault | An edit re-seals the value it read: a concurrent rotation is undone | medium | fixed (slice 5) |
| V4 | vault | Redaction that cannot load the secrets passes text through in the terminal and the proxy, withholds elsewhere | medium | fixed (slice 5) |
| V5 | vault | Reading a value needs two-factor; sending it to a new host does not | medium | fixed (slice 5) |
| V6 | vault | "May this production secret reach this run" is decided four ways | low | fixed (slice 12) |
| D4 | CI | `ci_rerun` picks other jobs than the run it repeats | medium | fixed (slice 13) |
| D5 | delivery | "Checks green at this head" has five copies and three rules; a promotion never asks Yulius CI | medium | fixed (slice 14) |
| D6 | delivery | An armed merge outlives its task's run, and a restart decides it the other way | medium | fixed (slice 15) |
| D8 | lifecycle | A task whose agent armed a merge and went idle is grouped Done (the grouping does not know the armed merge); archiving it from there closes it, which ends the armed merge with a reason typed to nobody (review of slice 15) | medium | fixed (slice 23; the case left after SPEC-487 CHN-001: an achieved goal) |
| QB1 | interface | The internal listener maps errors to statuses by a second table; `yulius wait` reads Yulius's own 502 as an outage | medium | fixed (slice 16) |
| QA1 | lifecycle | A start that has to queue forgets how it was to start | medium | fixed (slice 19) |
| QA2 | lifecycle | A failed re-login turns `relogin` into `error`, and the next usage reading into `ok` | medium | fixed (slice 18) |
| QA3 | lifecycle | After a restart a hand-over no longer restarts a run holding the previous owner's personal MCP servers | medium | fixed (slice 20) |
| QA4 | lifecycle | The `/goal` still to be typed into a resumed session is lost at a restart | medium | fixed (slice 20) |
| QA5 | lifecycle | A background probe without an answer means "busy" everywhere but at a subscription switch | medium | fixed (slice 17) |
| QA6 | lifecycle | "May Yulius type now" has three rules; `/compact` is typed behind a person's draft | medium | fixed (slice 21) |
| QA7 | lifecycle | A switch that times out records the old run lost without proof | medium | fixed (slice 17) |
| E6 | move | A move's report state is a string with three readings of "lost" | medium | fixed (slice 22) |
| E7 | move | An unreadable move mark does nothing and stays | low | fixed (slice 22) |
| QC8 | store | `tasks.closed_at` is never cleared | low | fixed (slice 22) |
| W2 | web UI | Sidebar groups (client) and counts (server) decide a paused task differently | medium | fixed (slice 23) |
| W3 | web UI | A 403 on a live edit leaves the document unsaved for good | medium | fixed (slice 23) |
| P1-P7, P9-P14 | documents | Contradictions between documents, shipped texts and code (promotion, hand merges, spec template, docs and chore workflows, UI copy, API table, guard text, briefing, stale engineering documents) | medium | slice 14 |
| QB9 | documents | Undocumented options and routes, stale architecture sentences | low | slice 14 |
| P15 | documents | `docs/api.md` lists the error code `unavailable` (503); no error kind and no route produces it (review of slice 16) | low | slice 14 |
| W7 | web UI | Dead exports and CSS rules | low | fixed (slice 24) |
| D7 | GitHub | `webhook_secret` is stored and never read | low | fixed (slice 24) |
| E5 | previews | The preview bridge has no listener since ADR-15 | low | fixed (slice 24) |
| W8 | web UI | A live document's rejoin refused for lack of access is tried again every second while the file is open (review of slice 23) | low | fixed (slice 24) |
| QA9, QC2, QC5, QC7, QB4, QB6 | dead code | Store functions, task events and reasons, contract items, a one-shot workflow shim, Cargo features nothing uses | low | slice 25 |
| W4, W5, W6 | web UI | The unsaved-changes guard five times, the pull request chip three times, byte and commit formatters | low | slice 16 |
| QB2, QB3, QB7, QB8 | duplication | Three loopback HTTP clients, the device contract twice, the Superagent's tools dispatched twice, five bearer parsers | low | slice 16 |
| QC3, QC4 | duplication | RFC 3339 parsing four times with different inputs; the GitHub-token scrub twice | low | slice 16 |
| QA8, QC1, QC6, QB5 | efficiency | Store round trips per task, unindexed capacity scans, the watched transcript re-read every 1.5 s | medium | slice 17 |
| C4 | containment | The engine leaf's limits are writable from a nested cgroup namespace | low | slice 18 |

## Evidence

The evidence of slices 1 to 20, all merged, is in
[evidence/SPEC-483-slices-1-20.md](evidence/SPEC-483-slices-1-20.md), word for word as it
stood here (this file is bounded at 256 KiB); the later slices follow.

### Slice 21 (2026-11-14)

- **Tests failing before the fix** (`target/audit/slice21/before.log`): with a person's
  keystroke from before the refusal the terminal never said that the session was left to
  them (the compaction was typed), and no message could be tried between `/compact` and
  its Enter (nothing marked that moment).
- **Tests failing without their fix** (`target/audit/slice21/mutate.py`, `mutations.log`, 4
  cases, each failed its test): a draft from before the refusal not keeping `/compact`
  out; `/compact` not holding the typing mark; the goal's typist not holding it; a
  message not asking for it.
- **Gate.** `scripts/dev ci` at `3d4c7877d`: exit 0;
  `4337 tests run: 4337 passed (1 slow), 17 skipped` (`target/audit/slice21/gate.log`);
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real Claude Code prompt box with a real draft (the test records a
  keystroke; the fake agent has no box); the line that wakes a compacted session with a
  draft in the box (the same function and rule as `/compact`); a typing mark that stays
  set for more than 5 s (Yulius's line is then typed all the same).
- **Review, round 1** (a fresh session, read-only, head `114041dfa`): request changes. Two
  major findings in this slice's code. The gate counted any key since the last prompt
  sent from the box, so "a person who once answered an AskUserQuestion dialog or pressed
  Ctrl-O to read the transcript makes every later full context stop the task", with one
  line in the terminal and no flag: the compaction is held now, flagged, and typed when
  the hold ends (TYP-001 as it reads above; the paragraphs before this one describe the
  first version). And "The only test of the new gate runs the fake agent without hooks",
  where a prompt never empties the box: the tests type through the task's terminal with
  an agent whose prompts reach the hooks, and the usual case (a person's own prompt is
  refused) is shown to compact (AC-118, AC-120, AC-121). Two minor: nothing was asked
  again after the wait for the typing mark (asked again with the mark held, AC-123); the
  restart that waits for the transcript's replay had no test (AC-122). One from before:
  between the goal's tries the mark was free while the line could sit in the box (the
  goal keeps it until it is taken, AC-119). Nits: the documents say which rule each
  typist asks and which typists hold the mark; the comments say what they cover.
- **Tests failing before those fixes** (`target/audit/slice21/before2.log`): with the hooks
  on, the person's own refused prompt was compacted already (the first version was right
  there, and untested), and the two tests of the hold failed for want of a flag.
- **Tests failing without their fix, after the review** (`mutations2.log`, 14 cases, each
  failed its test): a possible draft not holding the compaction; its person not told;
  the flag staying when they type; the hold lasting for good; the flag of a hold that is
  over staying; the wake-up typed behind a draft; a key before the typing mark not asked
  for again, for `/compact` and for the goal; `/compact`, a message and the goal each
  without the mark; the goal giving the mark up after its Enter; a goal kept out at its
  first try saying nothing; the restart of a reattached run not waiting for the replay.
- **The suites after it:** `2771 tests run: 2771 passed (1 slow), 17 skipped`
  (`target/audit/slice21/suite2.log`), before the last three tests were added; the gate
  ran after them.
- **Gate after it.** `scripts/dev ci` at `3cc050727`: exit 0;
  `4344 tests run: 4344 passed (1 slow), 17 skipped` (`target/audit/slice21/gate2.log`).
- **Not tested, after it.** A real Claude Code prompt box (keys are typed through the
  task's terminal into a fake agent that has none); what Claude Code does with `/compact`
  typed behind a real draft once the hold is over; the ten minutes themselves (the hold
  is 3 s in the test); a typing mark that stays set for more than 5 s.
- **Review, round 2** (the same session, head `99aed011e`): request changes; the findings
  of round 1 but the first were fixed, and the hold that answered the first brought new
  ones. One major: after the hold the command goes out behind real text as a prompt and
  is refused, so the flag's and the notice's promise of a compaction was untrue where it
  mattered (the texts say what happens now, and that refusal fails the compaction at
  once: AC-121). Three minor: "The hold's flag outlives the hold" on every way out but
  two, and a restart of Yulius took a held compaction for one that gave up (the flag ends
  with the hold, and the new server takes the hold up again: AC-125); the wake-up was
  kept out behind a draft without a word, after the terminal had said that the task goes
  on (the terminal says that the session was not told to carry on; the first line no
  longer promises: AC-124); the test of the hold read its notice in a race and did not
  assert one it waited for (both asserted through one helper; the time the tests assume
  is named). A nit: the hold's end is logged once. Found on the way: the key the hold was
  for kept the wake-up out after the compaction it had waited for, since nothing empties
  the box for a command; Yulius's own Enter does now.
- **Tests failing without their fix, after round 2** (`mutations3.log`, 19 cases, each
  failed its test): the fourteen above as they are now, and: a key from before Yulius's
  own Enter still counting as a draft; a command refused behind a draft waiting for the
  compaction's time; the episode not told that the command went behind a draft; a hold
  cut by a restart counted as given up; a wake-up kept out saying nothing.
- **Gate after it.** `scripts/dev ci` at `13f445e3c`: exit 0;
  `4346 tests run: 4346 passed (1 slow), 17 skipped` (`target/audit/slice21/gate3.log`).
- **Not tested, after it.** A real Claude Code (what it does with a command typed behind
  real text is taken from how it reads slash commands, at the start of the input only);
  the hold's ten minutes; a run that ends, or a terminal that never goes quiet, during a
  hold (the flag's end there is the same line as when its person types).
- **Review, round 3** (the same session, head `f687673ac`): request changes. Every finding
  of round 2 fixed; one minor, "a regression of the R2-2 fix": the flag of a hold was
  ended before the command only when the hold had run out, not when the box was emptied
  without a key. It ends whenever the hold's flag is up now (AC-126). The reviewer's own
  test for it (the text sent by an Enter nobody typed) could not fail: that prompt starts
  a turn, which ends every flag. The test empties the box by a command's prompt hook
  instead, which starts none, and the mutation fails it. Nits: the comment and TYP-001
  say that Claude Code may run no prompt hook for a command (2.1.292 ran none for
  `/compact`); the comment names the Enter a TUI may drop.
- **Tests failing without their fix, after round 3** (`mutations4.log`, 20 cases, each
  failed its test): the nineteen above, and a hold whose box was emptied without a key
  keeping its flag while Yulius compacts.
- **Gate after it.** `scripts/dev ci` at `26c79dcba`: exit 0;
  `4347 tests run: 4347 passed (1 slow), 17 skipped` (`target/audit/slice21/gate4.log`).
- **Review, fourth and fifth look** (the same session): approve at `b6f1b8d77`
  ("Nothing of severity minor or above in own code remains"), and at `ee6bc62aa` after the
  evidence of slices 1 to 20 moved to its own file (Yulius refused a spec above 256 KiB).
- **Delivery.** Pull request #1276 merged into `dev` at `6c8663268` (the approved head
  with `dev` merged into it by Yulius for its merge train), whose own Yulius CI run
  passed.

### Slice 22 (2026-11-14)

- **Tests failing before the fix** (the five new tests and the changed `installed` test,
  run against a stub that answered
  nothing for the helper's service): a silent helper whose service still ran read
  `lost`; `FileMoveHelper` asked nothing of systemd and could not name the service; a
  mark of bytes that are not text left the task `stopped (server lost)`; a second close
  moved the close time from 200 to 300; a version-57 database stayed at 57.
- **Tests failing without their fix** (`target/audit/slice22/mutate.py`,
  `mutations-r1.log`, 12 cases, each failed its test): a silent report lost whatever
  systemd says; systemd's silence taken for an ended service; a silent report never
  lost; systemd asked of every report; a lost move that cannot be continued; an
  `activating` oneshot taken for ended; a `failed` one for unknown; the path unit asked
  instead of the service; a report that ended between the two reads read lost; a mark
  that is not text taken for no mark; a continued task that keeps its close time; a
  repeated close that moves it.
- **Gate.** `scripts/dev ci` at `3f22df324`: exit 0; `4369 tests run: 4369 passed (1
  slow), 17 skipped` (`target/audit/slice22/gate.log`); doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
- **Review, round 1** (a fresh session, head `3f22df324`): approve, with four nits in own
  code, answered in `0011416f5`: a test's doc comment had moved above the new test (back
  in place); "the report is read before systemd is asked", so a helper that writes its
  last report and ends between the two reads read lost for one read (read again once
  systemd says ended; the test's new case fails without it); a line of
  `docs/operations.md` over the wrap and its logs paragraph still saying "after a
  minute" (both said as MOV-001 says). The fourth, "the create-time branch … has no
  test", is left: no caller creates a task chained or prepared without a start.
- **Gate after it.** `scripts/dev ci` at `0011416f5`: exit 0; `4369 tests run: 4369
  passed (1 slow), 17 skipped` (`target/audit/slice22/gate2.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real systemd (the tests answer as `systemctl show` does, with a fake
  `systemctl`); a real copy that holds the helper's report back for a minute.
- **Review, round 2** (the same session, head `e417e1c6c`): approve; two nits in the spec
  ("the diff adds five tests", and a line of MOV-001 over the wrap), answered in slice 23.
- **Delivery.** Pull request #1277 merged into `dev` at `fb9f12a60`, the approved head
  `e417e1c6c` squashed, whose own Yulius CI run passed.

### Slice 23 (2026-11-14)

- **Tests failing before the fix**: the domain rule found a paused task with a permission
  prompt NeedsYou; the sidebar's counter counted the awaited onboarding task working and
  showed an armed achieved goal "Done"; with the server's group fixed, a task chained after
  an armed achieved goal was still started (`task_is_done`, a second copy of the Done rule);
  the live document after a 403 kept "abcd", busy and unsaved, a close waited, and a new
  join stayed read-only; the Editor never said the refusal.
- **Tests failing without their fix** (`target/audit/slice23/mutate.py`; `mutations.log`,
  `mutations-local.log`, after the review `mutations-r1.log` and `mutations-r1-join.log`;
  each case failed its test): the pause, the awaited onboarding task and the armed merge
  taken out of the rule and, each, out of what the server reads; the armed merge out of
  both Done functions (the local one is what a supertask's wait asks: its test was added
  when this case survived) and out of the task's view; the sidebar's three changes; and in
  the live document, the refusal keeping the typing, any 403 taken for lost access, the
  tab's text left as typed, the tab strip not told, typing after a refusal kept, a join not
  clearing the refusal, a join clearing it before it is answered (its test was added when
  this case survived), the Editor not saying it.
- **Gate.** `scripts/dev ci` at `969a33a91`: one failure,
  `remote_mcp::a_persons_agent_edits_orders_and_stops_tasks_remotely` (a queued task the
  queue started before the test's bulk stop, under load; 6 of 6 alone; filed as #1278: a
  race in that test, which this slice does not touch); rerun at the same head: exit 0,
  `4371 tests run: 4371 passed (1 slow), 17 skipped` (`target/audit/slice23/gate2.log`).
- **Review, round 1** (a fresh session, head `969a33a91`): request changes. One major in own
  code: "After a refused edit the tab strip keeps the unsaved mark", and the Editor test's
  "status does not contain Unsaved" could not fail (the status says read-only instead).
  Two minor: "Any 403 is taken as lost write access, but the server also answers 403 for
  csrf_invalid"; and a refused catch-up after the refusal rejoins every second, with
  `readOnly` false during each attempt. All three answered: the typing is given up in the tab
  (its text the document's as it had it, the tab strip told), only `forbidden` is a refusal
  of access, a refused catch-up asks no more, and only a successful join clears read-only
  (LVE-001, AC-134; the Editor test asserts the tab `notes.txt×`). Nits answered: the mock's
  summary carries `merge_armed`; the counts read the pause without building its view
  (`App::is_paused`); `docs/api.md` says a run paused by a person or an update never needs
  you; doc lines rewrapped; GRP-003 names how it narrows SPEC-487 CHN-003; the observation
  that "after all current tasks" now names the awaited onboarding task is in the Decisions.
- **Gate after it.** `scripts/dev ci` at `d72d28945`: exit 0; `4371 tests run: 4371 passed
  (1 slow), 17 skipped` (`target/audit/slice23/gate3.log`). e2e `live-editor.e2e.ts`,
  `editor.e2e.ts` and `task-done.e2e.ts` at the same head: 71 passed
  (`target/audit/slice23/e2e.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real server refusing an edit after a worktree's owner changed (the mock
  refuses on a switch; the server's 403 for that case is `forbidden`, as the mock's); the
  counts with a real update's hold (the domain test and a person's pause cover the fact).
- **Review, round 2** (the same session, head `f9c68ec17`): approve; two nits, answered in
  slice 24: a doc line of the group rule over the wrap, and a refused rejoin retried every
  second (pre-existing, W8).
- **Delivery.** Pull request #1279 merged into `dev` at `d6fe5d400`, the approved head
  `f9c68ec17` squashed, whose own Yulius CI run passed.

### Slice 24 (2026-11-14)

- **Tests failing before the fix**: the proxied document came back without the app's
  `X-Frame-Options`, with the bridge script appended and no length, and the app saw no
  `Accept-Encoding` for a document; a refused rejoin was asked again every second (more than the
  two joins expected in 2.5 s; the count itself is not in the log). After the review: a 409 edit whose rejoin was refused left the
  tab busy, unsaved, with "abcd" and a close that waited.
- **Tests failing without their fix** (`target/audit/slice24/mutate.py`, `mutations.log`, then
  `mutations-r1.log`; each case failed its test): documents fetched uncompressed again; the
  app's `X-Frame-Options` removed again; no `frame-ancestors` of Yulius's own; the app's
  `frame-ancestors` dropped again; a refused rejoin retried every second; after the review,
  a script or the Yulius origin back on the proxy's own pages, and a forbidden rejoin that
  only marks the document read-only.
- **Gate.** `scripts/dev ci` at `91f197b4e`: exit 0; `4371 tests run: 4371 passed (1 slow),
  17 skipped` (`target/audit/slice24/gate.log`). The whole e2e suite at the same head: `84
  passed | 1 skipped` files, `1026 passed | 1 skipped` tests (`target/audit/slice24/e2e-all.log`;
  it covers the deleted web code: nothing imported it).
- **Review, round 1** (a fresh session, head `91f197b4e`): request changes. One major in own
  code: a rejoin refused as `forbidden` "sets only `readOnly = true; onState()`, leaving this
  tab's unconfirmed typing (`outstanding`/`buffer`), its timers and its text in place", the
  W3 state again behind a read-only banner, which the `ended` case could not reach. Two
  minor: PRV-001's "the proxy's own pages carry no script and the same framing rule" had no
  test; SPEC-002 (ADR-14's framing and bridge paragraphs) and SPEC-001 AC-BRW-7 still
  described the old proxy. Nits: `docs/operations.md` justified same-site preview origins by
  "the panel's frames" and the web README suggested `frame-src` for previews (pre-existing);
  a line of `docs/api.md` over the wrap and a short line left in the group rule's doc; the
  preview test's name. All answered in `92c51654d`: the
  forbidden rejoin calls `refused()` (the `gone` case), the landing's page is asserted, the
  two specs, operations and the README say what the proxy does now, the lines are rewrapped
  and the test renamed.
- **Gate after it.** `scripts/dev ci` at `92c51654d`: exit 0; `4371 tests run: 4371 passed
  (1 slow), 17 skipped` (`target/audit/slice24/gate2.log`); e2e `live-editor.e2e.ts` and
  `editor.e2e.ts` at the same head: 49 passed (`target/audit/slice24/e2e-r1.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real browser on a real preview (the proxy is tested over HTTP with a
  fixture app; framing and compression are header and body checks); a real GitHub manifest
  answer (the fake answers the field, as GitHub does, and it parses).
- **Review, round 2** (the same session, head `3eddcf9eb`): approve; four nits, answered in
  slice 25: two sentences of this Evidence (a join count no log records, and "two lines over
  the wrap"), the proxy page's criterion in AC-136 instead of AC-135, and SPEC-002's "Cookies
  in iframes" still arguing from the frames (pre-existing).
- **Delivery.** Pull request #1281 merged into `dev` at `3864dc11b`, the approved head
  `3eddcf9eb` squashed, whose own Yulius CI run passed.
