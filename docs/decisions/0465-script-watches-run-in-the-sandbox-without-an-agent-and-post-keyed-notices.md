# 0465. Script watches run in the sandbox without an agent and post keyed notices

## Status

Accepted (2026-11-23). Specified in
[SPEC-630](../../.ai/specs/SPEC-630-script-watches-scheduled-scripts-that-no.md); task #614. The
owner approved it on 22 Nov 2026 after a review of iTermX's `itermx-watch`, a separate tool. Its
design was read as a reference, and none of its code is used. Builds on
[ADR-0462](0462-notices-carry-a-key-and-an-expiry-and-the-newest-per-key-wins.md) (keyed notices),
[ADR-0003](0003-sandbox-isolation.md) (the sandbox's containment) and
[ADR-0015](0015-yulius-ci.md) (CI jobs through the process host). Changes none of them.

## Context

- Every automation creates a task, and every Superagent check types a prompt into the Superagent.
  Both spend an agent turn even when nothing changed. Watching a URL, a release, a CI number or a
  date costs tokens for nothing. For example, the owner's daily check "#479 merge-queue targets"
  answers "not due" every day until 2026-11-24.
- Yulius already runs commands without an agent, in the sandbox's containment, with time limits
  and a named secret grant through the egress proxy: CI jobs and delivery environment runs.
- Since ADR-0462, Yulius has keyed notices. A newer notice with the same project, recipient and
  key replaces the older one, and it reaches the person's pages, digest and Telegram.

## Decision

- **A watch is a new record** (`watches`, migration 0093) with a name, a bash script, a schedule
  (an interval of 1 minute to 7 days, or a cron expression with a UTC offset), an optional notice
  expiry, a run timeout, named agent secrets and an owner. It is not a kind of automation, whose
  contract is the task it creates, nor a kind of Superagent check, whose contract is the prompt box.
- **Yulius runs it like a CI job.** The script runs through the process host in the sandbox's
  containment: bubblewrap on the sandbox root, joined to the sandbox's namespace holder, in CI's
  resource group. It gets no agent, no subscription, no task token, no GitHub token and no hooks.
  `HOME` is the project's watch home, and each watch has a state directory. Both are kept between
  runs under `<data>/watches/<sandbox>/`, and they and the run's own directory are the only
  writable paths outside the sandbox root.
- **Bounds.** A fixed bash wrapper sets `ulimit -t` (CPU time), `-d` (data segment) and `-f`
  (file size) as soft and hard limits, and the process host enforces the timeout. The values
  travel as arguments and the script as its own `bash -c` argument, so nothing is spliced into
  shell text. At most 4 runs go at once on an instance, and 1 per watch.
- **Secrets as a CI job gets them.** A watch names agent secrets of its project that reach runs of
  no person. Its run gets their placeholders and an egress credential covering only those names
  (`EgressCaller::Ci`, run `watch:<name>`), closed when the run ends.
- **iTermX's exit codes.**
  - Exit 0 with output posts it; exit 0 with no output posts nothing.
  - Exit 3 posts and ends the watch.
  - Anything else is a failure, and the third failure in a row posts one warning.
- **Posts are keyed notices for the owner.** A report is keyed `watch:<name>`, so the newest
  replaces the previous one; the warning is keyed `watch:<name>:failing`. A successful run removes
  the warning.
- **An outbox.** A run's posts are written as pending in the same transaction as its state, then
  posted. A post leaves the outbox only once its notice is stored, newest per key. Each scheduler
  tick (15 s) retries pending posts first.
- **Restarts.** Watches and their state are rows. A run that a restart interrupted is recorded as
  lost (a failure), its process host is ended, and the watch is due again.

## Alternatives

- **A script variant of automations or of `superagent_check_set`.** Each record would gain a
  second meaning that every reader of it must tell apart, and neither has a process or a notice
  in its contract.
- **Run the script as its owner's identity** (their Git account and personal secrets, as a person's
  shell does). Anyone who could later change the script would act as that person. Automations
  hold no person's credential for the same reason.
- **A cgroup per run for CPU and memory.** The isolation makes per-job groups only for CI jobs,
  and they do not exist without isolation, where tests run. `ulimit`s hold everywhere.
- **`RLIMIT_AS` for memory.** Runtimes such as Node reserve large unwritable address ranges that
  are not memory, so address-space limits break them.
- **A new egress caller kind.** It would repeat CI's rule (a job of no person with a named grant)
  in every match of the proxy.

## Consequences

- A watch costs no tokens and no subscription. A check that a script can decide belongs in a
  watch, and an agent turn is spent only when the script says something.
- Scripts run with what the sandbox has installed and its network, and with only the secrets
  they name.
- A watch's posts reach the person wherever notices reach them. In the bell, that is once
  SPEC-624's second pull request makes it show server notices.
- The bounds are instance defaults: 120 s CPU, 1 GiB data and 1 MiB per file.
