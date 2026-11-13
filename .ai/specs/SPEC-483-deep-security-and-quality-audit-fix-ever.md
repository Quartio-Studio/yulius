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
  11. production secrets decided once (V6);
  12. delivery and CI decided once (D4-D6, QB1);
  13. lifecycles decided once (QA1-QA7, E6, E7, QC8, W2, W3);
  14. documents and shipped texts against the code (P1-P14, QB9);
  15. dead code (W7, D7, E5, QA9, QB4, QB6, QC2, QC5, QC7);
  16. duplicated code (W4-W6, QB2, QB3, QB7, QB8, QC3, QC4);
  17. efficiency (QA8, QB5, QC1, QC6);
  18. the web UI's script and style policy (W1b), and the engine leaf's limits (C4): two small
      changes of their own, taken out of slice 9, which is one concern;
  19. the connection that carries the server's token in a clone its agents configure (G2,
      found in slice 10): taken next, as slice 11 of the Plan (the earlier number 11, V6,
      follows it).
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

- **TKR-001** A confined fetch, push or listing of a clone in a sandbox's folder with a
  remote whose address is `https:` or `http:` is made in a courier: a bare repository in
  `<data>/git-couriers`, made for it and removed after it, with the clone's objects as its
  alternate. The clone's objects must lie in the sandbox's own folders.
- **TKR-002** A fetch through a courier leaves the clone's `refs/remotes/<remote>/*` as a
  fetch by name with pruning would; a push sends the commit of the given branch and is
  refused when it would rewind the remote; the listing answers the remote's branch.
- **TKR-003** A command whose folder is a sandbox's, and a transfer between folders, gets
  none of the adapter's `GIT_CONFIG_*`; a command in another folder or in none gets it.
- **TKR-004** Every command of the runner carries the constant options that switch off the
  `ssh` and `git` transports, signing, signatures and the alternate-refs command, and
  `GIT_EDITOR=:`; every `diff`, `log`, `show`, `diff-tree`, `diff-index` and `diff-files`
  carries `--no-ext-diff --no-textconv` right after its name, once.
- **TKR-005** A sandbox that is being made gets its own folder for its first command, not
  the folder of all sandboxes.

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
- [ ] Slice 11: a transfer that carries the server's token is made in a repository of the server's own, reviewed

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
  token, the proxy was never connected to, and `<data>/git-couriers` is empty afterwards. A
  worktree whose `commondir` names another sandbox's repository gets no transfer.
- **AC-62** With the tests' stand-in for GitHub (an address rewritten to a folder in the
  server's own configuration only): a clone, a push that arrives, the remote's head, a push
  that would rewind refused, a fetch that brings the remote's new commit and prunes a branch
  the remote dropped; the clone's configuration keeps the address; no courier is left.
- **AC-63** The runner's command: in a sandbox's folder no `GIT_CONFIG_*` of the adapter; in
  a folder of the server's own and in none, all of it; the constant options and
  `GIT_EDITOR=:` on every command; the two diff options right after the name of each command
  that can make a diff, past the command's own `-c` pairs, not twice, and on no other.
- **AC-64** With the clone naming the program loader as external diff, diff driver, text
  conversion, `ssh` command and signing program, and a new file in a folder that is a
  program: the server's status, review, diff, log, commit, fetch and push start nothing.

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
  (AC-61), `git_confined::a_fetch_and_a_push_through_the_servers_own_repository_arrive`
  (AC-62), `git_confined::the_loader_is_never_started_with_a_file_of_the_clone` (AC-64);
  `yulius-git` `runner::tests::a_confined_command_is_the_launcher_with_its_sandboxes_folders`
  and `runner::tests::nothing_is_started_with_an_argument_of_the_clones` (AC-63),
  `tests::a_command_reaches_its_own_sandboxes_folders` (TKR-005).

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
| G2 | git | A key for a URL in the clone's configuration (`http.https://github.com/.proxy`, `.sslVerify`) beats the server's own: the connection of a fetch or push that carries the server's token can go through a proxy in the sandbox without a certificate check (found in slice 10) | high | slice 11 |
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
| V6 | vault | "May this production secret reach this run" is decided four ways | low | slice 11 |
| D4 | CI | `ci_rerun` picks other jobs than the run it repeats | medium | slice 12 |
| D5 | delivery | "Checks green at this head" has five copies and three rules; a promotion never asks Yulius CI | medium | slice 12 |
| D6 | delivery | An armed merge outlives its task's run, and a restart decides it the other way | medium | slice 12 |
| QB1 | interface | The internal listener maps errors to statuses by a second table; `yulius wait` reads Yulius's own 502 as an outage | medium | slice 12 |
| QA1 | lifecycle | A start that has to queue forgets how it was to start | medium | slice 13 |
| QA2 | lifecycle | A failed re-login turns `relogin` into `error`, and the next usage reading into `ok` | medium | slice 13 |
| QA3 | lifecycle | After a restart a hand-over no longer restarts a run holding the previous owner's personal MCP servers | medium | slice 13 |
| QA4 | lifecycle | The `/goal` still to be typed into a resumed session is lost at a restart | medium | slice 13 |
| QA5 | lifecycle | A background probe without an answer means "busy" everywhere but at a subscription switch | medium | slice 13 |
| QA6 | lifecycle | "May Yulius type now" has three rules; `/compact` is typed behind a person's draft | medium | slice 13 |
| QA7 | lifecycle | A switch that times out records the old run lost without proof | medium | slice 13 |
| E6 | move | A move's report state is a string with three readings of "lost" | medium | slice 13 |
| E7 | move | An unreadable move mark does nothing and stays | low | slice 13 |
| QC8 | store | `tasks.closed_at` is never cleared | low | slice 13 |
| W2 | web UI | Sidebar groups (client) and counts (server) decide a paused task differently | medium | slice 13 |
| W3 | web UI | A 403 on a live edit leaves the document unsaved for good | medium | slice 13 |
| P1-P7, P9-P14 | documents | Contradictions between documents, shipped texts and code (promotion, hand merges, spec template, docs and chore workflows, UI copy, API table, guard text, briefing, stale engineering documents) | medium | slice 14 |
| QB9 | documents | Undocumented options and routes, stale architecture sentences | low | slice 14 |
| W7 | web UI | Dead exports and CSS rules | low | slice 15 |
| D7 | GitHub | `webhook_secret` is stored and never read | low | slice 15 |
| E5 | previews | The preview bridge has no listener since ADR-15 | low | slice 15 |
| QA9, QC2, QC5, QC7, QB4, QB6 | dead code | Store functions, task events and reasons, contract items, a one-shot workflow shim, Cargo features nothing uses | low | slice 15 |
| W4, W5, W6 | web UI | The unsaved-changes guard five times, the pull request chip three times, byte and commit formatters | low | slice 16 |
| QB2, QB3, QB7, QB8 | duplication | Three loopback HTTP clients, the device contract twice, the Superagent's tools dispatched twice, five bearer parsers | low | slice 16 |
| QC3, QC4 | duplication | RFC 3339 parsing four times with different inputs; the GitHub-token scrub twice | low | slice 16 |
| QA8, QC1, QC6, QB5 | efficiency | Store round trips per task, unindexed capacity scans, the watched transcript re-read every 1.5 s | medium | slice 17 |
| C4 | containment | The engine leaf's limits are writable from a nested cgroup namespace | low | slice 18 |

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
- **Review of slice 3, round 1** (head cb7da51fc): request changes, two major findings, taken:
  - "the walk can use up the server's file descriptors" (own code): `MAX_DEPTH` 128, deeper
    folders counted and not listed, the measurement marked truncated;
    `usage::tests::folders_nested_deeper_than_the_bound_are_not_listed`;
  - "one transcript reader still goes by path" (there before): `continue_session` opens the
    imported transcript with `transcripts::open` and copies its side files through handles;
    `session_docs::tests::side_files_are_copied_without_following_a_link`;
  - minor: a transcript that is no regular file reads as none (pages, a task's transcript,
    `session_read`), not as a 500; `native.exists()` became a no-follow check; the review's
    anchor (the worktree's folder, above which a link misleads Git first: finding G1) is
    stated in ADR-0315; nits: path parts below the anchor must be plain names, the change
    log's long line is wrapped. Left as they are, with the reviewer's reasons: an opaque
    `upper/etc` without `passwd` shows the base's (a server-built copy); a root is not kept
    in the hard-link set; a FIFO regression would end in the test runner's timeout.
- **Gate after round 1.** `scripts/dev ci` at bb4859b97: exit 0; `4125 tests run: 4125
  passed, 16 skipped`; the other steps passed.

### Slice 4 (2026-11-13)

- **Tests failing before their fix.**
  `delivery::a_pull_request_pointed_at_another_branch_is_never_merged`: with the three checks
  switched off the armed merge merged the pull request that had been pointed at `main`.
  `ui_contract::embedded_assets_serve_with_content_types_and_every_import_resolves`: failed
  while the lab was embedded (`GET /assets/lab.js` answered 200). `yulius-store`
  `writes_that_change_access_grow_the_access_epoch`: failed at "an API token revocation".
  `remote_mcp::a_read_token_reads_nothing_above_view_access`: its last route
  (`superagent/requests`) answered 200 until that function refused a read token; the other
  seven routes were refused by `require_for`, written before the test. The sign-in tests
  (`a_github_sign_in_completes_only_in_the_browser_that_started_it`,
  `github_sign_in_starts_are_limited_per_client`) and the production-stage command's
  refusals were written with their fixes and not run against `dev`.
- **Web UI.** `cd e2e && npx e2e run`: `Test Files 84 passed | 1 skipped (85)`, `Tests 1018
  passed | 1 skipped (1019)`, exit 0, on this slice's tree.
- **Gate.** `scripts/dev ci` on this slice's tree (the commit after this text names it): exit 0;
  `4129 tests run: 4129 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 3, round 2** (head 5810cc78f): approve, no blocker or major; its three
  minor findings and two nits are taken in this slice: the bound on an imported session's side
  files holds across folders (`copy_folder` stops at 0), the depth test no longer assumes a
  file system where folders take no blocks (run with `TMPDIR` on ext4: passed), one file of an
  imported session is copied only up to 1 GiB, the side files' path must be plain names, and a
  folder that cannot be listed is left out.
- **Not tested here.** A real browser against a real server (the cookie on a top-level
  navigation back from github.com): the routes are tested through the router with the cookie
  header set as a browser would.

### Slice 5 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice5/mutate.py`: one fix switched off,
  its tests run, the file restored). The unlock (`fresh_to_widen` answering "not needed"):
  `secrets::a_use_by_use_secret_goes_somewhere_new_only_after_two_factor`,
  `secrets::agents_request_hosts_and_people_allow_them`,
  `secrets::agents_bind_the_hosts_of_a_secret_they_set_up` and
  `remote_mcp::secrets_are_managed_by_name_and_binding_only` failed. The proxy using an empty
  set instead of refusing: `a_request_whose_answer_cannot_be_checked_for_secrets_is_not_sent`
  failed. The terminal not withholding: `output_is_withheld_while_agent_secrets_cannot_be_read`
  failed. `vault_update` without the value in its compare:
  `an_edit_read_before_a_rotation_does_not_undo_it` failed.
- **Slice 4's tests, as its reviewer asked** (the same way): the callback not asking for the
  cookie: `a_github_sign_in_completes_only_in_the_browser_that_started_it` failed; no limit on
  starts: `github_sign_in_starts_are_limited_per_client` failed; the production-stage command
  without `may_deploy`: `delivery::a_production_environment_run_starts_in_production_containment`
  failed.
- **Web UI.** `cd e2e && npx e2e run` at `7f3379e06` (nothing under `web/` or `e2e/` changed
  after it): `Test Files 84 passed | 1 skipped (85)`,
  `Tests 1018 passed | 1 skipped (1019)`, exit 0. (At `78c891dac` one test failed in the whole
  suite and passed alone: `cd.e2e.ts` "a needs_you decision reads Decide, and an action alone
  only To do", the known flake under load of issue #1175, in a page this slice does not touch.)
- **Review of slice 5, round 1** (head 78c891dac): request changes, one major and seven minor
  findings and two nits, all in this slice's code, all answered in it. Major: the copy budget
  took a file's stated size, so a file growing while it was copied was not counted (now what
  is read counts; the test with a file stating 0 failed before). Minor: a secret that never
  opens made every chunk of output read and open all secrets and log (a retry after a second,
  one log line; ADR-0317 no longer says it recovers by itself); allowing production deploys
  was not asked for the code (it is); an answer given without the code because `auto_allow`
  was on could land after it was switched off (the two store writes compare it); an overtaken
  team edit answered 404 (409); the app's own failing paths were untested (a Superagent's real
  terminal and the app's proxy policy with a secret whose key is gone); closing the code
  dialog was untested (e2e, with "nothing before the code" for `.env`, production and the
  switch). Nits: a doc comment on the wrong helper; the mock compared hosts as typed.
- **Review of slice 5, round 2** (head fa200a9ed): request changes, two minor findings and two
  nits; round 1's answers all verified. Minor: with a withheld screen the supervisor's wait
  timed out and typed blind, which a withheld folder-trust dialog would have taken as its
  answer (the wait now types nothing while the screen is withheld and flags the task; ADR-0317
  corrected); "revoking production needs neither" had no test (a token revokes in
  `production_secrets_need_their_own_allowance`). Nits: the race guards are tested in the
  store only (below, "Not tested here"); the copy budget's failing run is not in the mutation
  log (its test was run and seen failing before the fix was written: "Ok(())" where a refusal
  was expected); issue #1175 is the flake's own issue (`gh issue view 1175`).
- **Review of slice 5, round 3** (head f4e864bec): request changes, one minor finding and three
  nits; round 2's answers verified. Minor: `type_into_session` (messages from agents and
  Yulius's own) read the trust dialog off the same withheld screen and typed (it refuses now).
  Nits: a failure at a program's last output stayed "withheld" and flagged a healthy run with
  the wrong remedy, and after a recovery the screen missed what was dropped (one rule now: a
  screen with dropped output is unknown until the next whole screen, the wait flags at once
  and says to stop and continue); ADR-0317 said no run starts meanwhile (one whose own
  secrets are readable does); the test did not show that nothing was typed (it reads the
  transcripts now). The round-2 fix had also flagged a goal given at the launch, which is not
  typed: the launch goal's watch is left as it was.
- **Gate.** `scripts/dev ci` at `5fde92f16` (round 3's answers; `2021b3604` after round 2:
  4138 passed; `7f3379e06` after round 1: 4137 passed; `78c891dac` before the reviews: 4134
  passed): exit 0; `4138 tests run: 4138 passed, 16 skipped`; doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
  `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task` with the wait's
  check switched off: "timed out waiting for the flag"; with the message's check switched off:
  its assertion on the refusal failed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 4** (head f07fde13c): approve, no blocker or major. Its two minor findings
  are fixed here (one byte budget for an imported session's transcript and side files; the
  `busy` text), and of its five nits: tests for the cleared cookie and the plain-HTTP cookie
  name, two consequences added to ADR-0316, the fail-before-fix runs above; `form_train` is
  left as it is (Decisions).
- **Not tested here.** The two races through the app (an `auto_allow` switched off between an
  answer's read and its write; a second edit of one team secret): the store's compare and the
  refusal's wording are tested, the interleaving is not. The proxy process end to end with the app's policy failing (the proxy is
  tested with a policy that has nothing to redact with, the app's policy on its own); a task
  agent's terminal (the Superagent's goes through the same `scrub_output`). The two-factor
  dialog over the open edit form is tested in the mock (e2e), not against a real server.

### Slice 6 (2026-11-13)

- **Reproduced on a real browser** (Google Chrome 157.0.8103.50 on Xvfb, this machine, the
  opt-in test with `YULIUS_TEST_CHROMIUM`): before the fix a click through
  `BrowserInstance::dispatch_mouse` on a page's file input mapped the window `"Open File"`,
  1280x800, over the page (`xwininfo -root -tree`), and the test failed with "a native file
  dialog opened". With the fix and no policy installed: "the host's browser policy is not
  installed: the click was refused", no window, passed. With the policy written to
  `/etc/opt/chrome/policies/managed/yulius.json`: "the host's browser policy is in force: the
  click reached the page" (the page's title turned to "clicked"), no window; then Ctrl+O,
  Ctrl+S, Ctrl+P, Ctrl+U, F12 and Ctrl+Shift+I through `dispatch_key`: no window shown (GTK
  keeps one unmapped), no file dialog, no `devtools:`, `view-source:`, `file:` or print page
  among the targets; and a click on a link to another program's address
  (`yulius-test-scheme://open`): under the first draft of the policy the page stayed and the
  address was handed over; with only web addresses allowed the tab shows it blocked; passed.
- **Probes of the policy on that Chrome.** `Page.navigate` to the marker host:
  `net::ERR_BLOCKED_BY_ADMINISTRATOR`; to `file:///etc/hostname`: the same; to another
  `.invalid` host: `net::ERR_NAME_NOT_RESOLVED`. `showOpenFilePicker()`: `NotAllowedError`.
  With `DeveloperToolsAvailability: 2` in the file `Target.attachToTarget` answered "Not
  allowed" for every target, so it is left out. The existing real smoke test
  (`drives_a_real_chromium_when_configured`: load, snapshot, type, click, screenshots, PDF
  frames) passed with the policy installed.
- **Tests failing without their fix** (`target/audit/slice6/mutate.py`, ten cases; held tabs
  dropped and a tab at Chrome's own page dropped fail
  `browser::tabs_made_while_the_probe_waits_are_listed_and_a_tab_at_chromes_page_stays`): the mouse
  gate off, the key gate off, typed text ungated, the probe's tab listed:
  `browser::viewer_clicks_and_keys_wait_for_the_hosts_browser_policy` failed; Chrome's own
  pages listed: `browser::chromes_own_pages_are_no_tabs` failed; a look-alike hidden by its
  address: `browser::a_page_cannot_hide_a_window_behind_the_probes_address` failed; host setup
  writing nothing:
  `host_setup::tests::the_browser_policy_is_written_where_it_is_missing_or_differs` failed; no
  flag on a refused message: `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once`
  failed.
- **Web UI.** `cd e2e && npx e2e run` at `2c4b50dbd`: `Test Files 84 passed | 1 skipped (85)`,
  `Tests 1019 passed | 1 skipped (1020)`, exit 0 (nothing under `web/` or `e2e/` changed
  after it).
- **Review of slice 6, round 1** (head 53cb2786d): request changes, two major and four minor
  findings and three nits, all in this slice's code, all answered in it. Major: the update
  helper's unit could not write the policy directories, so its re-check could never bring a
  newer policy (the shipped unit lists them, the setup writes a drop-in for a unit from
  before; test); the probe's tab was hidden by its address, which a page can open (it is
  known by Chrome's id now; test). Minor: a tab that went to a page of Chrome's own was
  dropped (only a target created as one is no tab); texts that did not match the code; the
  real-browser proof covered the file input only (accelerators and a link to another program
  added, which changed the policy to web addresses only); typed text and releases untested.
  Nits: a yes stood for the browser's life (asked again after 5 minutes); the probe could
  hold a viewer's connection (3 seconds at most); another policy file can override a setting
  (ADR-0318 says so).
- **Review of slice 6, round 2** (head 0a566a925): request changes, two minor findings and
  three nits; round 1's answers verified. Minor: the probe's tab could still be listed when
  Chrome announces it under another address first, or a resync runs meanwhile (every new
  page no page opened now waits for the id, and the probe's is taken off the list when the
  id is known); the replay of what waited and "a tab at Chrome's own page stays" were
  untested (test with a tab made beside the probe and a tab sent to `chrome://settings/`;
  two more mutations). Nits: the wait for the id survives a dropped probe now (a drop guard)
  and the tab's creation is timed; a replayed announcement no longer overwrites a newer one;
  `ws:` and `wss:` are in the policy's allowed addresses (whether the blocklist reaches
  WebSocket handshakes was not measured).
- **Gate.** `scripts/dev ci` at `d42465f95` (round 2's answers; `84a173793` after round 1:
  4147 passed; `53cb2786d` before the reviews: 4145 passed): exit 0; `4148 tests run: 4148
  passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed. Both real-browser tests passed again with the final policy
  file.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 5, round 4** (head cf2f03a6e): approve, two nits; one is fixed here (the
  flag on a refused message), one is left with its reason (Decisions).
- **Not tested here.** That a task flagged for a withheld screen is flagged again after it was
  stopped and continued (the flag's reset at attach): a start reads the secrets, so a
  continued task does not start while the secret that withheld the screen is unreadable.
  The update helper's unit under systemd (its writable paths are tested against the fake
  host's answer to `systemctl show`). `yulius host setup` on a real host (the item is tested against the fake
  host; the policy file was written by hand for the real-browser runs, with the content
  `managed_policy()` gives). Chromium and a snap-packaged Chromium (only Google Chrome is on
  this machine): the browser is asked itself, so a flavor that reads no policy from these
  directories refuses viewers' input instead of taking it unlocked. The context menu's
  entries (Save as, Inspect) were not clicked on the real browser; their accelerators were
  pressed. That the accelerator check would see a window or page were one to open was not
  shown (without the policy the keys are refused before they reach Chrome).

### Slice 7 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice7/mutate.py`, ten cases; the two
  added after the review: the wrong page asked after the move, no activity entry for a
  secret gone astray): the page
  not asked under the lock: `instance::a_secret_is_typed_only_into_a_page_it_may_go_to` and
  `browser::agents_type_secrets_only_into_pages_of_their_hosts` failed (the secret was typed
  into `evil.example`); the page it moved to not asked:
  `instance::a_secret_typed_while_the_page_moves_on_is_reported` failed; navigate, reload and
  history each without the lock: `instance::a_navigation_waits_for_a_secret_being_typed`
  failed each time; a later held announcement dropped, a closed tab's announcement kept: the
  two probe tests failed.
- **Real browser.** `drives_a_real_chromium_when_configured` (Google Chrome 157, opt-in; it
  types secrets into a real page and takes screenshots): passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed in this slice; the e2e suite last ran at
  slice 6 (1019 passed).
- **Review of slice 7, round 1** (head 48dbe9523): request changes, three minor findings and
  three nits in this slice's code, and one major finding that was on `dev` before it. Minor:
  the astray test could not tell which page the rule was asked about (the fake now moves to
  another site mid-typing and the test asserts both addresses and the host named); the
  application's answer and activity entry for a secret gone astray were untested (they are,
  through the MCP tool); "none of them can put another page under the keys" claimed more
  than a lock on the request gives (ADR-0319 and FSR-002 say what it covers; finding E9).
  Nits: a typing error no longer hides the astray report; one task owns the probe tab's
  creation and closes it unless the probe takes it, also when the probe is dropped; the
  ADR says who waits behind a navigation. The major one, a secret typed into a focused
  frame of another site, is finding E8 with its own slice.
- **Gate.** `scripts/dev ci` at `5a89ab57f` (round 1's answers; `0d924205e` before the review:
  4152 passed): exit 0; `4152 tests run: 4152 passed, 16 skipped`; doctor, repository check,
  lint, the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Slice 6's delivery.** Its first CI run failed in one e2e test of a page it does not touch
  (`notes.e2e.ts` "a failed reload after a teammate's edit backs off": 4 loads counted in a
  fixed 2.5 s window, at most 3 expected), filed as issue #1246; the run was repeated once at
  the same head and passed.
- **Review of slice 6, round 3** (head 70cc7698d): approve, three nits, all answered here.
- **Not tested here.** The race on a real browser (a slow screenshot, a fill and a navigation
  at once): the order is tested on the fake with the lock held by a delayed look at the page.
  A page that redirects by itself while it is typed into, on a real site.

### Slice 8 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice8/mutate.py`, ten cases, each
  switched one check off and ran its test): the frames not asked (browser and through the MCP
  tool), the frames in other processes not asked, frames that cannot be listed typed into:
  `instance::a_secret_is_not_typed_into_a_frame_of_a_site_it_is_not_for` and
  `browser::a_secret_is_typed_neither_into_another_sites_frame_nor_a_tab_that_is_leaving`
  failed; where the tab is going not asked, a navigation given up still standing (each in the
  browser and through the MCP tool), only a link's navigation counting, a navigation inside
  the document counting:
  `instance::a_secret_is_not_typed_into_a_tab_leaving_for_another_site` and the MCP test
  failed.
- **Real browser** (Google Chrome 157, opt-in, `YULIUS_TEST_CHROMIUM`): four tests passed.
  `a_secret_is_not_typed_into_another_sites_frame_in_a_real_chromium`: a page at `127.0.0.2`
  framing `localhost` with the focus in the frame's field got nothing typed and the answer
  named `localhost`; with both allowed it was typed. Before the fix the same steps typed the
  secret into the frame. `a_secret_is_not_typed_into_a_tab_that_is_leaving_in_a_real_chromium`:
  a fill right after a click on a link to `localhost` (answering after 1.5 s) typed nothing.
  Measured there with a probe since removed: `Page.getFrameTree` on a new session answered
  1.52 s after the click with the new address (4.01 s with a 4 s server, also for a link to
  the same site), so on Chrome 157 the answer is `SecretPageRefused` by slice 7's rule and
  `SecretPageLeaving` is not reached; the test accepts either and says so.
- **The gate's first run** at `8d90eacc5` failed in
  `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task` (slice 5's test):
  reproduced with a 300 ms delay before the typing flag was cleared (the same panic), fixed
  by ending the typing before the task is flagged, passed with the delay in place, delay
  removed.
- **Gate.** `scripts/dev ci` at `2f17216be`: exit 0; `4157 tests run: 4157 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run for this slice.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 7, round 2** (head ec5c5c240): approve; merged as pull request #1247.
- **Not tested here.** `SecretPageLeaving` on a real browser: Chrome 157 holds its answer
  during a navigation, so only the fake reaches it. A frame nested in another site's frame
  in a process of its own on a real Chrome (the fake lists such targets; the real test uses
  one frame). Chromium builds other than Google Chrome 157.
- **Review of slice 8, round 1** (head a3320e970): request changes. Own code, all answered
  test-first:
  1. (major) the frame check hung on a focus test that failed open for `<object>`, `<embed>`,
     a frame in a closed shadow root and `<fencedframe>`. Now only a text field of a document
     the probe read is the page's own; anything else asks the frames; a fenced frame is
     refused. On Chrome 157, with round 1's gate, the secret was typed into `localhost` in an
     `object` and in a closed shadow root's frame of a `127.0.0.2` page
     (`target/audit/slice8/real-chrome-mutations.log`); with the fix both are refused.
  2. (minor) nothing was asked again between the look and the keys: the document is read
     again after the focus (a changed one gets nothing typed), where the frames are going is
     asked last, and a frame that has the focus after the keys is reported
     (`SecretFrameAstray`, with an activity entry).
  3. (minor) the record of announced navigations had no bound and only the main frame was
     asked: `Leaving` holds 256 frames, drops a frame's entry when it is detached, counts
     nothing older than 30 s and says "not known" past its bound; every frame of the page's
     tree is asked.
  4. (minor) untested branches: the fake sends any page event (`emit`); the tests cover
     `Page.frameRequestedNavigation` and its disposition, `Page.frameNavigated`,
     `Page.frameDetached`, a frame of the page, an announcement during the look, the 30 s,
     the bound, a session's end, a page that cannot be read, frames that cannot be told, and
     the order of focus and look. The last step of the leaving test now does what it says.
  Nits: skipped events lose the record ("try again" for 30 s); the frame error says what is
  known ("the focus is not in a field of this page itself, and the page holds a frame of
  …"); an address without a host is "an unknown site"; ADR-0320 names the page that cannot be
  read, the address of another program, and the frames inside an out-of-process frame that
  are not asked. Two lines stay without a test of their own (Decisions say why).
  Pre-existing (major), finding E10: fixed here (`tool_type_beside_secret`).
- **Tests failing without their fix, after the review** (`target/audit/slice8/mutate.py`, 28
  cases, `mutations2.log`): each failed its test: the frames not asked (browser, MCP), round
  1's gate, a fenced frame typed into, frames in other processes not asked, frames that
  cannot be listed typed into, a page that cannot be read typed into, the page read before
  the focus, a page that changed since the look typed into, a frame that took the focus not
  reported (browser, MCP), where the tab is going not asked (browser, MCP) or asked before
  the look, only the main frame's way asked, a navigation given up, arrived, or of a detached
  frame still standing, only a link's navigation counting, a same-document one counting,
  another tab's counting, the requested navigation not kept, an old announcement standing
  for ever, nothing lost past the bound, a session's end keeping its frames, lost
  announcements forgotten at once, and the user name typed without the page rule (E10).
- **Real browser after the review** (Chrome 157): the four opt-in tests passed; the frame
  test now also covers an `object`, a closed shadow root's frame that takes the focus by
  itself, the user name's path, and the page's own field beside a frame (typed).
- **The gate after the review.** At `c323db624` it failed in
  `superagent_browser::the_global_superagent_drives_a_named_projects_browser`: the fake's
  new focus kinds called an unnamed focused password field "other", so a screenshot was
  refused (a fault of the fake, fixed there). `scripts/dev ci` at `b71c09db1`: exit 0; `4160
  tests run: 4160 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Not tested here, added by the review's answers.** A fenced frame on a real Chrome (the
  probe names the element; no page with one was built). The loss of announcements on skipped
  events, end to end (the function is unit-tested).
- **Review of slice 8, round 2** (head 2c1af3d04): request changes; "All nine round 1
  findings are answered as the coordinator described, and I checked each in the code." Four
  new ones in the new code, all answered test-first:
  1. (minor) an announced address cut to 2048 bytes could read as a host of a wildcard
     binding (`https://<long>.bank.example@evil.example/`): one too long is kept as no
     address, which no secret is for.
  2. (minor) a frame inside a closed shadow root that took the focus during the keys was not
     reported: after the keys the frames are asked by the rule asked before them (the focus
     not in a field of the page), and screenshots follow the same condition.
  3. (nit) an announcement sent a moment before the keys may not have been read: named in
     ADR-0320 (the report afterwards covers it).
  4. (nit) more than 256 frames on their way in one tab stopped fills in every tab: the bound
     and its loss are per tab; skipped events still lose every tab's.
- **Tests failing without their fix, after round 2** (`mutate.py`, 32 cases,
  `mutations3.log` and `mutations3b.log`): all 32 failed their tests; new: a cut address
  kept, one tab's loss not asked, after the keys only a fenced frame reported.
- **Real browser after round 2** (Chrome 157): the four opt-in tests passed.
- **The gate after round 2.** `scripts/dev ci` at `20404f5ae`: exit 0; `4161 tests run: 4161
  passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.

### Slice 9 (2026-11-13)

- **The hole, on a real browser.** With the proxy's switches taken out (the browser as it
  was), Google Chrome 157 of sandbox `s1` opened `http://localhost:43020/`, a port of sandbox
  `s2`, and showed its page ("other page";
  `target/audit/slice9/real-chrome-mutations.log`). With only
  `--proxy-bypass-list=<-loopback>` taken out the same test failed too: Chrome 157 sends the
  loopback past a proxy unless told otherwise (also seen with a logging proxy,
  `probe_proxy.py`: `localhost` reached the origin directly, and through the proxy with the
  switch).
- **Real browser, with the fix** (Chrome 157, opt-in, `YULIUS_TEST_CHROMIUM`):
  `browser_network::a_real_chromium_reaches_its_sandboxes_ports_and_no_other_local_address`
  passed: its own server by `localhost` and `127.0.0.1` and the welcome page load; another
  sandbox's port (three spellings), a host service, the internal API, `169.254.169.254` and
  two HTTPS addresses show none of those pages; a page's `fetch`, images and a WebSocket to
  them arrived at no server while its own answered. The four tests of `real_chromium.rs`
  passed as before.
- **Tests failing without their fix** (`target/audit/slice9/mutate.py`, 18 cases,
  `mutations.log` and `mutations-b.log`): each failed its tests: every port of the loopback
  reached, local addresses not refused, every path of Yulius's listener, a tunnel to it, a
  listener bound to every address not taken as Yulius's, what a name resolved to not asked,
  the proxy's hop headers sent on; the loopback past the proxy, names resolved by Chrome, UDP
  past the proxy, a browser started without its proxy, the proxy closed while the browser
  runs, the proxy outliving its browser (this one survived at first: the test let go of the
  browser itself; it now holds on to it, as a viewer's session does); every sandbox's ports
  for every browser, a proxy before the egress proxy runs, the server's browsers left to
  connect by themselves; and the two answers to slice 8's nits (a long address kept cut, or
  kept as none).
- **The gate.** Its first run (at `e57456953`) stopped at rustdoc: a doc comment had ended up
  on the wrong function. Its second (at `b814ad528`) failed in this slice's own
  `browser_network::each_browsers_proxy_reaches_its_own_sandboxes_ports`: the test's "service
  of the host" took a port the system gave, 43013, which lies in the fixture sandbox's range
  (43000-43019), so the sandbox's browser rightly reached it. Reproduced by putting that
  server into the range on purpose (the same assertion, `flake-proof.log`); the test now
  takes a port outside both ranges. `scripts/dev ci` at `b9bd0eb68`: exit 0; `4172 tests
  run: 4172 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run for this slice.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 8, round 3** (head 4fb8a2eab): "No critical, major or minor own-code
  finding is open." Approve, two nits, both answered here; merged as pull request #1248 (its
  first armed merge was refused for a title of 112 characters; retitled, merged at the same
  head).
- **Not tested here.** A real server with real sandboxes: the browser of one sandbox against
  the relayed ports of another on a host (the test's servers stand on the loopback where the
  relay would listen). WebRTC and QUIC being off (the switches are asserted, their effect is
  not). A viewer's address bar on a virtual display (it navigates the same Chrome through
  the same proxy). Chromium builds other than Google Chrome 157. macOS, where the same
  switches are passed.
- **Review of slice 9, round 1** (head 9c2af2437): request changes; no critical or major
  finding. Own code, all answered test-first:
  1. (minor) the loopback rule took any name that resolved to the loopback, resolved anew for
     each request: a name's owner could answer a public address for a page and `127.0.0.1`
     for the page's next request, and the page would read the sandbox's own dev server. The
     loopback's ports and the welcome page are now reached only by an address or `localhost`.
  2. (minor) `*.localhost` was asked of the host's resolver, which not every host answers:
     `localhost` and names below it are the loopback without a resolver.
  3. (minor) a plain request's wait covered its upload with one fixed bound: it now restarts
     with every piece of the body sent on (`Progress`, as an agent's).
  4. (minor) ADR-0320, its index line, FSR-005 and AC-46 still said a long address is kept as
     none: amended (its site is kept).
  5. (minor) the share test proved only the browser's own limit: an agent run's tunnel now
     takes the place and the browser is refused, and the other way round.
  Nits: the proxies hold the egress proxy weakly (no cycle through the application); the
  listener backs off on accept errors and closes a connection that sends no request for 60 s;
  the origin's `Host` and the error texts carry no user and password; the public listener is
  never reached at its own address, also with its port in the sandbox's range; tunnel by
  `localhost`, listed and answered hop headers are tested; a refusal with no known ports says
  so. Pre-existing: ranges that are never given again drift into the system's free ports,
  where a browser's proxy can take one: issue #1249.
- **Tests failing without their fix, after the review** (`mutate.py`, 31 cases,
  `mutations2.log` and `mutations2b.log`): all 31 failed their tests. One reported "passed"
  in the first run: the script's filter had left the new integration test out; with the
  filter corrected it failed there too. New: a name that resolved to the loopback reaching
  the sandbox's port or the welcome page, the test map's answer counted as an address,
  `localhost` asked of the resolver, the public listener reached, the bound not restarting
  with the body, a silent server waited for without end, the origin's hop headers coming
  back, a listed header sent on, a share of the browser's own, unknown ports read as a
  range, user and password sent on as the host, the proxies keeping the egress proxy alive,
  the public listener not passed to the proxies.
- **Real browser after the review** (Chrome 157): the opt-in network test passed.
- **The gate after the review.** `scripts/dev ci` at `d2b1afed2`: exit 0; `4176 tests run:
  4176 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Not tested, named by the review.** How Chrome 157 classes a proxied answer's address
  space (the fix does not depend on it); WebTransport under a fixed proxy; a managed proxy
  policy on the host overriding the switches (documented in `docs/operations.md`).

### Slice 10 (2026-11-13)

- **The hole, on the adapter as it was** (`target/audit/slice10/unconfined-proof.log`: the
  `git_confined` tests with the confinement taken out of the tests' adapter): the server's
  Git ran the clone's `filter.mine.process`, `core.sshCommand`, `remote.origin.uploadpack`
  and `remote.origin.receivepack` (four markers); a fetch replaced a file outside the sandbox
  with Git's `FETCH_HEAD` ("…branch 'main' of …/playgrounds/s1"); a push by the remote's name
  arrived at another sandbox's origin. By hand, a link at `.git/shallow` gave "fatal: bad
  shallow line: secret-line-of-the-host".
- **Confined, by hand first** (`yulius git-confined`, Landlock ABI 8 here): `status`, `log`
  and an HTTPS `ls-remote` work; a filter's command fails with "cannot exec 'sh -c …':
  Permission denied"; the linked `FETCH_HEAD` is refused ("cannot open '.git/FETCH_HEAD':
  Permission denied") and the file outside kept; a local fetch needs the shell ("cannot exec
  'git-upload-pack …'") and works with it; with the shell, `core.alternateRefsCommand` and
  `core.sshCommand` through a rewritten address ran, and with the pins they did not
  ("transport 'ssh' not allowed").
- **The binary's whole suite, confined.** Every test that uses the real adapter now uses it
  confined in the server's layout (`support::confined_git`, 45 places). The first such run
  failed 181 tests and showed what a confined command needed named: destinations of `init`
  and `clone`, the other folder of local transfers, bundles, a CD export's index, worktree
  paths, a playground's origin for pushes by the remote's name, import worktrees beside the
  sandbox's folder. After that: 2134 of 2134 passed.
- **Tests failing without their fix** (`target/audit/slice10/mutate.py`, 23 cases,
  `mutations.log`): each failed its tests: Git unconfined, the shell always executable,
  every program executable, writes not confined, reads not confined, a Git not on `PATH`
  started by name, a local transfer without its pins (attack test and unit), with the
  adapter's configuration, without the shell, the shell for every command, what a command
  names not reached, any folder taken as an origin (attack test and unit), a sandbox's
  folders being the whole data folder (attack test and unit), import worktrees no sandbox's,
  a push or a fetch with the own origin by the remote's name, the server's adapter not
  confined, the doctor passing a host that cannot confine; and the two extras: a request
  without a body framed as chunked, a withheld screen flagged once per run.
- **A flaky test of slice 5**, met in this slice's first confined run:
  `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once` failed 5 of 24
  runs alone (the task read `Waiting`, or its flag had no reason): the prompt the test typed
  started a turn of the fake agent, whose states replaced the flag. With a key that starts
  no turn: 30 of 30. The flag is now raised again after such a replacement (test first:
  `Waiting` where `NeedsAttention` was expected).
- **G2, found here** (`urlmatch-proof.log`): with `http.https://github.com/.sslVerify=false`
  and `.proxy` in the clone's configuration, `git config --get-urlmatch` under the server's
  command line (`-c http.sslVerify=true -c http.proxy=`) and environment answered `false`
  and the clone's proxy. Listed, not fixed here.
- **Gate.** `scripts/dev ci` at `f819929cd`: exit 0; `4188 tests run: 4188 passed, 16
  skipped`; doctor (with "git confinement: ok"), repository check, lint, the macOS compile,
  cargo-deny (the new `landlock` crate: MIT OR Apache-2.0), cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 9, round 2** (head 72b4a521f): "No critical, major or minor own-code
  finding is open; two nits." Approve; both nits answered here; merged as pull request #1250.
- **Not tested here.** A kernel without Landlock (the launcher's refusal is code read, not
  run: this machine enforces it). A real server with real sandboxes, a relay and GitHub over
  HTTPS from the confined adapter (HTTPS was run by hand through this sandbox's proxy; the
  suite's "GitHub" is folders, with the shell). Git builds whose helpers live outside their
  `exec-path`. macOS, where Git runs unconfined as before.
