# Changes

What changed for the people and agents using Yulius in the last four weeks, newest first, one line
each. A change nobody using Yulius would notice (a refactor, a test fix, this repository's own process)
gets no line; the merged pull request is its record. Older entries: [archive](archive/changes-until-2026-11-26.md).

## 2026-11-26

- Whether a person administers the instance and whether a token's scope allows an action are each decided by one helper, read wherever a gate or a page flag needs them (audit L4 F5). Behaviour is unchanged. [SPEC-669](../.ai/specs/SPEC-669-cleanup-secrets-vault-auth-helpers-audit.md)
- Yulius CI runs Yulius's own Rust tests in one `test` job with 42 CPUs, instead of two shards that shared one nextest archive (audit L3 F1). The archive handoff (`scripts/ci_test_shard.py`, `scripts/dev test-shard`, xtask's archive and partition modes) is gone, and with it the failure class where each shard unpacked the 2.3 GB archive, up to twice, into a bounded temporary directory (the ENOSPC on `main`'s runs). The doctests run after the tests in the same job. `matrix` stays a CI feature for other projects.
- Cleanup of the update drain, behaviour unchanged (audit L6 F7): its freeze pass reuses its freeze of one run, and its freeze-state lookup and its thaw of runs lose a copy each. The update helper and staging CD keep their own install and rollback, and the drain and the move their own freezes: unifying them would change what an update does at eight steps, listed in [SPEC-671](../.ai/specs/SPEC-671-cleanup-wiring-main-rs-arguments-self-up.md).
- Cleanup of `yulius`'s command line, behaviour unchanged (audit L6 F5): `onboard`, `secrets import`, `instance export|import|migrate`, `leftovers` and `doctor` declare their arguments once, on the option struct they fill, instead of in `main.rs` and again field by field; every `--help` page and every refusal reads as before. [SPEC-671](../.ai/specs/SPEC-671-cleanup-wiring-main-rs-arguments-self-up.md)
- The sandbox manager keeps a live holder's Docker front and its upkeep state in one `FrontState` instead of six loose fields (audit L5, F2 part b). Behaviour unchanged.
- Each delivery and review rule is stated once (audit L10 F2). The bundled skills no longer repeat what Yulius says at run time (the nit bound, resuming a reviewer, its effort and model) and name the one skill that owns each rule: `delivery-pipeline` the local checks, `om-code-review` the review, `test-driven-development` mutation checks, `om-check-and-commit` the quick checks, `git-hygiene` history. Here AGENTS.md shrinks to commands, layout, boundaries and the routing table, and the `yulius-git` repository skill is gone (`git-hygiene` and `delivery-pipeline` cover it). [SPEC-674](../.ai/specs/SPEC-674-cleanup-documents-instructions-spec-adr.md)
- The vault's MCP tools share one lookup of an agent secret by name, one refusal of personal and production secrets, one argument parser and one fingerprint-file planner (audit L4 F2). Refusal texts and behaviour are unchanged. [SPEC-669](../.ai/specs/SPEC-669-cleanup-secrets-vault-auth-helpers-audit.md)
- `crates/yulius-sandbox/src/manager.rs` is split without a change of behaviour: the Docker front's methods and helpers are in `manager/docker.rs`, the base build in `manager/build.rs`, and the system-layer readers in `system_layer.rs` (audit L5, F2 part a).
- Agents are told to write an ADR only for a choice of how the system is built or run that someone may want to reverse, with two rejected alternatives, instead of one for each lasting decision (audit L10 F4): the briefing's rule 5, the AGENTS.md and first ADR of a new project, the spec template, the engineering docs and the infra-bootstrap skill. A review blocks only on a contradicted *architectural* decision record, and updating that record in place answers it. Here the repository check refuses a branch that adds two ADRs, one with fewer than two alternatives as list items, or one beside changes only to the web UI and its e2e tests. [SPEC-674](../.ai/specs/SPEC-674-cleanup-documents-instructions-spec-adr.md)
- Cleanup of the `yulius` wiring, behaviour unchanged (audit L6 F4): the unused GitHub placeholder is gone, the test-only unavailable adapters live in the tests, the outbound HTTP adapters share one client builder and one bounded body read (`wiring/http.rs`), the Claude Code and pinned CLI installers share one tarball download (`wiring/tarball.rs`), the git token header uses the `base64` crate, and the wiring reads the wall clock in one place. [SPEC-671](../.ai/specs/SPEC-671-cleanup-wiring-main-rs-arguments-self-up.md)
- A task whose spec is not registered finds it again in a specs directory of more than 500 files. Discovery read the first 500 directory entries in directory order and sorted only those, so with `.ai/specs` past 600 entries a renamed or unregistered spec could be missed. It now lists every Markdown file (up to 2,000) and reads the file named with the task's branch slug first. [SPEC-667](../.ai/specs/SPEC-667-cleanup-delivery-merge-queue-git-github.md)
- Yulius CI's own job commands are `scripts/dev <gate>`: under Yulius CI `scripts/dev` bootstraps the toolchain and the gate prints its stage line, so `yulius.toml` no longer repeats `python3 scripts/bootstrap.py --install-rustup && echo "::yulius-stage …"` in every job (audit L3 F7, F11). The fallbacks for pull requests older than 2026-11-21 are gone, including the `file-contracts` one that failed on such heads. A pull request whose head lacks this change fails `quality` ("Invalid command") until it merges `dev`. The pull request's `quality` job is the new `scripts/dev quality` (doctor, quick, the checker's tests): it no longer compiles the workspace, which `lint`'s Clippy already does.
- The README says who Yulius is for, what leaves the server (Vidoc and Jev off without a key; Vidoc on for every project once a key is saved) and what is not supported.
- A project's `yulius.toml` with an invalid `[disk]` table is now refused by the whole-file check as the disk upkeep's own reader refuses it, and the Security scan card reads `.config/vidoc.toml` as TOML, as the scanner does: an inline `ignore = []`, a multi-line string or `1_0` no longer reads as unreadable (audit L3 F8).
- Agents write an ADR only for a reversible build or run choice, and note only user-visible changes in a project's changes log.

## 2026-11-25

- Agents can no longer write to their harness's persistent memory or user instructions; existing notes still load, and a refused write says why.
- Merge-first staging moves on by itself and judges the newest waiting revision first; writers can deploy its verified head from the CD page.
- An agent asks for a secret's allowance with a tool that shows an Allow button on the Secrets page, not a Needs you card.
- Each card on the Subscriptions page can say when its subscription next charges, once an administrator sets the date.
- The person who creates an organization's project in Yulius is its admin on GitHub, so they can deploy its production.

## 2026-11-24

- Pastes reach the sandbox browser as real pastes, including long sign-in codes in terminals, and a terminal's right-click menu can paste screenshots.
- A task's terminal that appears late no longer takes the keyboard from a control the person moved to meanwhile.

## 2026-11-23

- Script watches: a project's bash script runs on a schedule in the sandbox, without an agent, listed under More → Watches with Run now, Pause and Edit.
- Brokered commands that need approval wait in Needs you; secret managers approve them on the Secrets page's Commands tab or by replying on Telegram.
- Fork a task from its menu or details page: a dialog sets what the fork does differently, its model and subscription, and both pages link each other.
- A live task whose goal is met but whose PR still has an integration failure reads "Waiting for checks or merge" instead of Done.

## 2026-11-22

- New tasks run Claude Code on Opus 5.5 at High effort when nothing is chosen, from the composer, the CLI, the Superagents and Telegram.
- Administrators define brokered commands next to a project's secrets; agents run them with `command_run` or `yulius run`, with secrets blanked from the output.
- A subscription past 95 % of a usage window gets no new tasks while another has room, and its idle tasks move to the one with the most room.
- `task_get` shows an agent where its project's other tasks stand: owner, PR, delivery stage, claimed paths and what each waits for.
- An agent secret can be limited to HTTP methods and paths on the Secrets page.
- A red integration branch gets one repair owner, and project CI and Status name the failing test and the merge that introduced it.

## 2026-11-21

- Task delivery shows each review round of the landed PR, for Claude Code and Codex reviewers, and whether each round reported.
- Merge-first projects: `delivery_status` and the task show when a staging request is due, where it stands and what its revision needs next.
- Codex tasks run at High effort on every model, prefer the subscription with the most quota, and Continue now gives a stopped Codex task its next turn.
- Goal tasks continue unfinished plan and spec work without asking their owner to type "continue".
- Agents fix review findings and get an independent re-review without asking the task owner for another round.
- An armed PR's owner hears its CI failure once, and a green armed PR stuck outside a merge train gets a check-in, then a Needs you.
- Production CD can release the last verified dev revision while a newer verification runs.
- The new-task composer keeps several unfinished drafts per sandbox, each with its own text, attachments and choices.
- The Browser panel offers Copy to my clipboard after a web page copies text, and a plain right-click in a Codex terminal opens the clipboard menu.
- The seven-day API estimate includes Codex runs and shows what the pool would cost on Ultrafast.

## 2026-11-20

- Every task in Needs you carries a notice saying what happened, what a person must do and where, in the UI, notifications and task tools.
- Merge-first staging: a project can mark its staging target `merge_first`, `cd_deploy` queues the merged revision, and Promote takes only verified revisions.
- Parked sessions show their last screen and a Resume button; Resume, Reply or typing wakes them in the same session, and CI results wake them too.
- Work from Telegram, off by default: a person turns on their chat's intake in Settings → Notifications.
- Tasks can hand work to another sandbox, off by default; an instance administrator sets handoffs to Ask or Automatic.
- Task delivery links the landed commit to its CI and deployment checks, and a red CI notice includes partial test failures.
- A sandbox writer can choose Restart browser when Chrome's input stays locked.

## 2026-11-19

- Parked sessions: a task waiting for a person gives up its idle session after 30 minutes, keeping its place, question and PR.
- Take control of a teammate's task after a confirmation, or Reply to its Needs you without taking control.
- An agent's `needs_you` can name the people besides the owner who should answer.
- A task that finds a bug fixes it as a goal with the Fix workflow, instead of reporting it and stopping.
- The composer lists 11 Codex and eight Gemini models in searchable groups; Codex defaults to GPT-6.2 Astra, with Ultrafast on ChatGPT Pro 500.
- Watchers see other people's Codex and Antigravity sessions, and Antigravity tasks show Working while a background command runs.
- Subscription cards show a Claude, Codex or Antigravity badge, and Dashboard → Harnesses compares tasks, run times and tokens per harness.
- An optional end-of-day digest: an administrator turns it on in Settings with a time and an idle threshold.
- Only an automation's creator can edit it, switch its schedule or Run now.

## 2026-11-18

- Antigravity is a third agent: administrators add a Google subscription by Gemini API key or the Antigravity sign-in, and the UI offers it.

## 2026-11-17

- Codex is back as a second agent, offered in the web UI, with its CLI provided by Yulius.
- Every sandbox gets changed skills at a server start, before any task resumes.
- A bad `yulius.toml` never blocks CI: the integration branch's jobs always run, and an invalid file fails the run with the parser's message.
- A CI job removes the Docker containers and networks it created when it ends.
- A merge candidate that passes merges at once, and members whose runs errored stay in the candidate.
- A test failing in the same job on two PRs within 48 hours is quarantined with a flaky-test issue.
- `[merge_queue] apart` keeps PRs that touch the same derived files out of one merge candidate.

## 2026-11-16

- Notifications can reach a person's own Telegram chat.
- A Claude subscription can be connected by pasting a token.
- CI takes jobs out of one host budget and gives each sandbox's Docker 4,096 networks.
- Overlapping `scope_claim`s name the holding task, both tasks show "overlaps #N", and `task_create` can queue a task after the holder.
- A shared Docker problem is one Needs you card for all tasks of a sandbox, and Yulius restarts a jammed sandbox Docker itself.
- A review survives a usage limit: the session moves to a subscription that runs the reviewer's model and resumes the reviewer.

## 2026-11-15

- Yulius picks each review round's model: Fable 5.2 for the first two rounds, then Opus 5.5 (also when no subscription can run Fable).
- Delivery records who wrote and reviewed each PR, and the Dashboard's Pipeline tab shows review statistics per model.
- An approval carries across a follow-up of at most 20 product lines.
- Yulius CI tests a head while its review runs, and an armed PR whose base moved waits about one CI run, not two.
- The Superagent shows a task's screen, token usage and checkout lag, keeps its jobs across restarts, and takes secret values on its panel.

## 2026-11-14

- The Superagent can do what its person does on project pages, such as continue, pause, edit and reorder tasks, run CI, deploy and merge.
- Control handed over on request can be taken back with one click, and requests for control reach the owner anywhere.
- Anyone who may work in a project can stop, archive, restore and delete its tasks; steering a task stays its owner's.
- The web UI asks about unsaved work on every way out.

## 2026-11-13

- A PR delivers the finished part of a task's plan while later steps stay open, so steps show all the work.
- Sessions no longer show Claude Code's "Language server couldn't start" notice.
- Security fixes: production secrets reach no run, `browser_fill_secret` types only into the shown page, and sandbox browser viewers cannot read host files.

## 2026-11-12

- A superadmin moves the instance to another host from Settings → Instance.
- A Fable task continues on Opus 5.5 while no subscription can run Fable.
- Subscription moves and restarts keep what waits in Claude Code's queue, and Yulius's messages no longer land on a person's unsent draft.
- A task in Needs your action says why, and Reply shows where the answer goes.

## 2026-11-11

- `sudo yulius instance migrate root@new-host` moves the instance, copying while it runs and freezing only for the last changes.
- A subscription out of credits for its model no longer stops tasks while others have capacity.
- A restart on a newer Claude Code keeps a person's goal and prompt-box draft, and sessions survive restarts on systemd 258.
- `browser_screenshot` never shows a secret typed with `browser_fill_secret`.
- Your own Anthropic key can be named `ANTHROPIC_API_KEY_<NAME>`, and `yulius tool` and `yulius remove` work in task sessions.

## 2026-11-10

- Each task shows its subscription, "Run #N on…" moves it to another, and removing a subscription always works.

## 2026-11-09

- A subscription that stops working moves its tasks to a working one.
- Tasks hear what happens to their pull request, and a failed update check says why and retries within minutes.

## 2026-11-08

- A pull request's own CI run always finishes; behind a moved `dev`, a merge queue tests waiting pull requests together and merges them in order.
- Agents can ask for a copy of a production secret, and the Superagent follows up tasks it asked for in other sandboxes.

## 2026-11-07

- Deploy production proves its rollback on dev by itself, and the Superagents can use the project browser.

## 2026-11-06

- A `[[ci.jobs]]` entry can set `exclusive = true` (one run at a time per sandbox) and its own `timeout_minutes`; a job's limit counts only its own run.
- The Superagent panel has This sandbox and All sandboxes tabs; the global one takes typing, clicks and pasted screenshots in its terminal.
- A task session whose Yulius MCP server dropped is reconnected, and a superadmin can restart Yulius.

## 2026-11-05

- A code edit waits until the task has a spec with a Plan, and the refusal says where to write it.
- Commits made after a task's PR merged are opened as a new PR of the same task.
- A sandbox's admins can waive the production ruleset on the Git page when the repository plan cannot have one.
- Claude Code updates itself on an instance.

## 2026-11-04

- Rollback is part of every project's CD, shown as "Not needed" where an environment deploys nothing.

## 2026-11-03

- The Editor is shared live: teammates see each other's keystrokes and carets before anyone saves.
- Teammates can try a playground without signing in with GitHub.
- Deleting a task keeps attachments other tasks still use and holds the rest 7 days in a trash that `attachments_trash` restores.

## 2026-11-02

- The egress proxy stops adding agent secrets the moment a run ends.

## 2026-11-01

- `[docs] changes` may name a folder of change notes, one new file per change, so parallel pull requests no longer collide on one log.
- "Review changes" appears only when the task's worktree exists, and the Changes tile says "No worktree yet" or "Worktree gone".

## 2026-10-31

- Sandbox admins and superadmins can switch Vidoc security scans off and on without removing the key.
- A Yulius restart is invisible to a task's session, and one busy run no longer locks the egress proxy for its sandbox.

## 2026-10-30

- Leftover processes no longer pile up in a sandbox, and the Superagent's commands keep running while Yulius restarts.
- `reference_repo` refuses repositories over 1 GiB instead of filling the server's disk.
