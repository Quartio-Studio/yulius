---
yulius-task: 5b06b876-20e3-4a88-b4c2-2c7c3667ff86
status: in-progress
---

# SPEC-595 Shared compilation for Yulius Rust builds

## Goal

Reuse compiled Rust on the instance across Yulius task worktrees and CI lanes. Each checkout keeps its own Cargo outputs and can finish a correct build if caching is unavailable.

## Context

On November 21 the operator measured about 195 seconds compiling and 151 seconds testing in one shard, 614 seconds compiling in quality, and 18–22 GB per CI target. Agent and CI homes differ. CI jobs run in individually budgeted groups. [SPEC-569](SPEC-569-shorten-yulius-mutation-verification-whi.md) measured reuse inside mutation campaigns. This task measures ordinary builds.

## Requirements

- **RCC-001** Enable local disk reuse by default for this repository's Rust builds in tasks and CI lanes while preserving concurrent independent builds.
- **RCC-002** Keep the store private to one sandbox, cap its size and document disk upkeep. The compiler stays within the caller's resource and cancellation boundary.
- **RCC-003** A changed source, compiler input or observable checkout path must produce the right output. A missing tool, unavailable store or damaged entry must permit ordinary compilation.
- **RCC-004** Compare sccache, a shared dependency target and Cargo's own pinned-toolchain facilities. Measure cold and warm commands in different fresh worktrees, cache hits across CI lanes and before/after real CI durations.

## Plan

- [x] Specify cache ownership and capture the uncached CI baseline
- [x] Enable a bounded local compiler cache across project builds
- [x] Verify concurrent reuse, path correctness and recovery
- [x] Measure fresh-worktree test and quality builds and document the choice
- [x] Approve the cache implementation for delivery
- [x] Build and test the idle-store upkeep candidate
- [x] Record the observed CI lane reuse and before/after run times
- [x] Verify candidate idle-store recovery and reconcile the shipped launcher CI measurements
- [x] Restore task admission after compiler-store eviction with multiply linked files
- [ ] Verify the completed quota and disk-upkeep integration for delivery

## Acceptance criteria

- **AC-1** With empty targets in two worktrees, the second build serves hits from the first. Two jobs building different revisions simultaneously produce their own expected values.
- **AC-2** The sandbox's existing mount boundary encloses all shared artifacts. The documented owner evicts entries under the cap. With broken storage or missing tooling the expected output still builds.
- **AC-3** The decision records elapsed times for both requested commands, cacheable requests and hits with denominators, paths, versions and CI conditions. A reviewed head passes the gate and lands on dev.

## Test plan

Exercise the Cargo entry point with real small crates, including an embedded checkout path, changed inputs, concurrent builds and cache loss. Run the benchmark with an empty target per invocation. Compare CI job execution and queue times separately. Verify repository contracts and delivery on the exact head.

## Decisions

Deliver the repository launcher and its 16 GiB collection first, then the measured scheduled CI evidence. Deliver host-side eviction with quota remeasurement and bounded task admission retry after all cache links are removed. Use mbx 1.30.0 with private Cargo targets and a sandbox-private disk budget. [ADR-0434](../../docs/decisions/0434-keep-a-local-compiler-cache-in-each-project-sandbox.md)

Adopt the policy in Yulius's repository. To make it the default emitted by Rust project bootstrap, the onboarding template would install a digest-pinned tool for supported hosts, generate the launcher, wire task and CI commands to it, and honor existing wrappers and a project-level opt-out. Disk upkeep would need a budget and ownership stamp for each enrolled sandbox. Validate the macro and source-path policy, parallel builds, cache loss and cold/warm timings on representative quartio-v2 and autoresearch checkouts before updating that template. Measure reused targets as well as cold CI lanes and require evidence that the cache improves the intended workload. Existing projects would adopt it through their own reviewed onboarding changes.

## Evidence

Before the change, Yulius CI run `cf1c465e-1711-4e70-9b2e-929e1dab55c4` (PR #1608, `19d368f`) passed in 604 s. Quality ran for 552 s and the two test shards for 547 s and 392 s. Their admission waits were 16 s, 23 s and 1 s. E2E ran for 572 s. The project had Vidoc disabled. The physical merge-train run that verified the initial base `6810a3c` was `7f9305c6-32a9-46d7-9a80-ab26b132afb6` at `a155ebe`. It passed in 738 s. Quality and the two shards ran for 682 s, 479 s and 411 s after admission waits of 55 s, 212 s and 26 s. The dev run reused that verified tree.

The release-integrity and launcher suite (`python3 -m unittest discover -s scripts -p test_compile_cache.py`) passed 15 tests with pinned mbx at `52b59ecb3` and 17 after the foreground-terminal and jobserver regressions were added. The real compiler fixture built a path-independent library and a path-sensitive executable in task and CI layouts with distinct homes. The second checkout served a library hit and printed its own manifest directory. A same-length damaged cached executable rebuilt correctly. Simultaneous builds of different source revisions printed their own values. The launcher preserved an explicit Cargo failure, recovered from cache startup failure, and used plain Cargo during eviction. A fresh task found the pinned binary installed by another task.

The pressure integration test first failed at `assert!(!cache.exists())` with the original upkeep. Three existing xtask command-order, failure and concurrency tests passed with the original explicit Cargo override. A trial that wrapped the override produced extra discovery calls. The launcher now routes only the pinned Cargo from scripts/dev through the cache.

The corrected CI lane-layout replay at `52b59ecb3` used two real Git worktrees under an isolated `ci/jobs/test.0` and `ci/jobs/test.0@1`, the same CI home, separate empty targets and a local ext4 store with no other writers. Both ran `scripts/cargo check --locked --offline -p yulius-domain --lib` with two Cargo jobs. The first lane took 9.49 s and populated 12 initially unknown compilations. The second took 4.58 s and served 12 of 12 lookups (100%). Each lane additionally reported seven mbx bypasses and three compiler invocations that Yulius ran directly. Ten bypasses bring the warm share across all reported invocations to 12/22 (54.5%). No other writer could supply these hits.

In the preliminary instance-upkeep slice, the original Rust upkeep scenarios passed with nextest, including the original failing assertion. The tests cover an active reader, a pressured idle store, an unowned directory, a linked store and the healthy-disk size boundary. The release-integrity suite and the cross-checkout fixture passed with mbx installed. `scripts/dev quick` and `spec_check` passed. `scripts/dev security` reported Vidoc disabled.

Independent review of `050869aaa` reproduced three defects. A warm procedural-macro expansion embedded the previous worktree's manifest directory, PID-directed cancellation left a child running with the sandbox lease, and a truncated ownership stamp was accepted by builds. New regressions reproduced all three. The corrected launcher keeps uncontrolled procedural-macro expansion out of the cache, closes the lease at Cargo dispatch, forwards cancellation to the process group and publishes a complete stamp atomically. The macro fixture includes a re-export through a separate library. A first-start interruption leaves the cache available on the next build.

A second independent review of `7a4d5af07` found that a restored registry crate's `file!()` could name the previous Cargo home and a debug binary's DWARF compilation directory could name the previous worktree. A requested `CARGO_INCREMENTAL=1` also lost its private incremental state. Regression fixtures reproduced all three against plain Cargo. The source-path identity is now part of the key whenever an absolute Rust source or debug working directory can be recorded. An explicit incremental request runs with its original Cargo settings. A fresh target using the same registry source root served a cached result, while different Cargo homes printed their own file paths. Both debug binaries named their own checkout in DWARF. Bootstrap continued after a simulated interrupted HTTP download.

The third review of `229ff6323` reproduced two caller-contract failures. A foreground test stopped while reading its terminal. GNU Make's inherited pipe jobserver descriptors did not reach Cargo. Separate fixtures compare a foreground read and a live jobserver token with the caller's expected result. This delivery retains plain Cargo in those contexts. Redirected standard input also preserves an accessible foreground `/dev/tty`. Unattended Yulius jobs identify their run and redirect all three standard streams. They use the cache in the PTY runner’s original session and job cgroup. A fixture runs both configured CI commands on a PTY and verifies that Cargo and the cache retain the original session ID.

The independent repository-launcher review approved `8b5554106` against `6810a3c65`. All 19 launcher and release fixtures passed independently under a PTY with inherited Make jobserver flags. Both CI commands used the cache and retained the original runner session. The launcher-only `scripts/dev mutants` reported no mutant in the changed Rust lines. The host-upkeep campaign belongs to its separate delivery.

On the instance, the task's mount table showed a writable mount for its own sandbox root. Exactly one sandbox directory was visible under the parent and the compiler store mode was 0700. The layout fixture selected distinct stores for two project roots. Nested bubblewrap namespace creation was refused in the agent container, so the existing contained-job test's nested launch branch could not be exercised there.

The cold test worktree took 1,160.11 s. Nextest ran all 5,754 selected tests in 908.38 s. One pre-existing terminal test observed an empty attention flag after Ctrl+U. The cold build served zero cache hits and the terminal code and test matched dev. This stopped the command before doctests. The remaining doctest stage passed separately in that cold target with pinned ordinary Cargo in 142.25 s. That duration is separate from the failed command's wall time. [Issue #1615](https://github.com/Quartio-Studio/yulius/issues/1615)

The supplemental harness exposed a caller that selected the project launcher itself through `CARGO`. A four-second regression timed out on `8b5554106`. On the corrected head `7fe27a3d6`, selecting the launcher by absolute path or first in `PATH` dispatched once to the real Cargo executable. Independent review approved that exact head, reproduced the previous timeout and passed all 20 launcher and release tests under a PTY with inherited Make flags. An explicitly selected backend preserved its exit status 43. The local suite and `scripts/dev quick` also passed.

In different empty-target worktrees, both `scripts/dev checks` runs passed in 754.29 s cold and 807.27 s warm. The warm check reused 403/424 eligible requests (95.0%) and 403/777 of all reported requests (51.9%). The macOS step fell from 269 s to 192 s. The whole warm command took 52.98 s longer during overlap with other host work. [ADR-0434 measurements](../../docs/decisions/0434-keep-a-local-compiler-cache-in-each-project-sandbox.md#measurements)

The warm `scripts/dev test` passed in 1,003.30 s, including all 5,754 selected tests and doctests. The nextest run took 665.75 s. The warm worktree restored 398.5 MiB and reused 222/248 eligible compilations (89.5%), or 222/445 of all reported invocations (49.9%). The cold and warm test builds took 246 s and 305 s. Both command families used 19 Cargo jobs and 19 nextest workers under concurrent host load. Only the same command in the preceding worktree populated each store.

Immediately before launcher delivery, dev advanced to `e19ea9f`. Its physical uncached merge-train run `8a5d7592-6a05-42fe-8182-a6ef8a3ca7a8` at `87463f1` passed in 1,077 s. Quality and the two shards ran for 899 s, 689 s and 492 s after admission waits of 51 s, 56 s and 448 s. Dev run `78961328-710a-4c94-8f17-0d78865e9e92` reused the verified tree.


### Scheduled CI

The launcher landed in [PR #1617](https://github.com/Quartio-Studio/yulius/pull/1617). Its first PR head `211a53a3e` ran the full gate. The merge train physically tested `e630657a2`. The final PR head `77838af2e` passed in run `9fae846e-97d1-45ef-9fb4-574ed26f16a1` through verified-tree reuse. Dev `b00cc76a4` passed in run `059c84af-f5de-46e0-af5c-598798ff99aa` using that same tree. The following timings come from physical runs. Job cells show execution / admission wait in seconds. Gate wall includes admission, E2E and job overlap. [Dev verification record](https://yulius.quartio.ai/api/tasks/5b06b876-20e3-4a88-b4c2-2c7c3667ff86/delivery/evidence/b00cc76a430a258f63297434405a6c34da400941)

| Physical run | Run ID | Gate wall | Quality | Test 1 | Test 2 |
| --- | --- | ---: | ---: | ---: | ---: |
| Before, PR #1608 | `cf1c465e-1711-4e70-9b2e-929e1dab55c4` | 604 | 552 / 16 | 547 / 23 | 392 / 1 |
| After, PR #1617 | `39e25b2a-2f3b-410a-b489-38e222205675` | 1,269 | 677 / 591 | 445 / 503 | 366 / 432 |
| Immediately preceding dev train, #1607 + #1613 | `613bed2a-9758-4868-b02b-01103dde29e8` | 866 | 819 / 47 | 712 / 83 | 579 / 20 |
| Launcher dev train, #1617 | `e802f510-91e7-43b6-b83d-6747cd2ee676` | 812 | 802 / 10 | 446 / 77 | 360 / 89 |

The checkout logs identified `quality@1`, `test.1@1`, `test.2@1` in the PR and `quality`, `test.1`, `test.2` in the train. Each physical job UUID was matched to its run using `ci_status`. Both sets of retained private targets served **0/27 eligible shared-cache requests**. Quality had nine misses. The first shard had four misses and 14 initially unknown keys. The second shard issued no eligible lookup. The PR also recorded 94 mbx bypasses and 290 direct compiler calls, the train 94 and 302. Different source trees, admission waits, persistent Cargo targets and host load affect these wall times. No shared-cache hits contributed to either scheduled run. The isolated clean-target lane replay above establishes reuse when an equivalent compilation is actually requested.

A subsequent full physical merge train for [PR #1618](https://github.com/Quartio-Studio/yulius/pull/1618), run `01163ca6-3baf-4227-947a-e952b0710956` at `ed55e2c17`, passed in 477 s. The scheduler matched job IDs `57f73ae1-c891-4140-a010-69d45b8ddde3`, `9ee50e2e-4513-4158-a582-8421291d3ccb` and `03761ac8-aedf-481a-9884-102613152f38` to quality and the two shards. Their logs identified separate lane checkouts. Counters include all Cargo steps in each job.

| Lane | Execution / wait (s) | Hits | Misses | Initially unknown | mbx bypass | Direct compiler |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `quality@3` | 469 / 7 | 15 | 7 | 5 | 25 | 193 |
| `test.1@3` | 391 / 67 | 9 | 5 | 14 | 66 | 65 |
| `test.2@3` | 344 / 67 | 9 | 1 | 0 | 3 | 65 |

These scheduled lanes served **33/65 eligible requests (50.8%)** and **33/482 of all reported compiler invocations (6.8%)**. The logs count hits without identifying which checkout populated each artifact. The separate empty-target replay establishes where its 12 hits came from. Gate wall also depends on source changes, admission and host load.

The sequential test pair at the approved launcher `7fe27a3d6` used another initially empty store, two separate empty targets and 19 workers. Cold took 854.69 s and warm took 593.92 s. Cargo's test build took 337 s and 196 s. Each ran 5,752 tests with 5,751 passing. Stopped-run attention failed cold and loopback port release failed warm. Each command exited before doctests. Warm served 222/248 eligible requests and 222/431 of all reported compiler invocations. These additional elapsed times are failed command observations. [Issue #1611](https://github.com/Quartio-Studio/yulius/issues/1611) and [issue #1620](https://github.com/Quartio-Studio/yulius/issues/1620).

### Instance disk upkeep

The first mutation campaign listed 25 changes, caught 14, missed seven and found four unviable in 1,185 s. Tests now pin the 20 GiB boundary, count reclaimed stores and cover missing roots and linked paths. The two open-flag mutations ceased to apply when the code expressed the flag union explicitly. The changed-lines campaign caught both listed mutants in 829 s. The targeted replay caught four of the five remaining original mutants. After adding the missing-sandbox case, the remaining guard mutant was caught in a single-mutant run in 53 s. The tests use real filesystem locks and marked disposable stores.

Independent review of the first upkeep head `8237d0b5d` found three failures in eviction. Three regressions failed on that head. A swapped-in unmarked directory lost its sentinel, the interrupted-removal fixture could not keep a reusable store, and 641 names for one 32 MiB inode triggered the 20 GiB limit. The next revision held the stamped directory open for removal and used the sandbox quota walker's hard-link accounting. All three regressions passed on that revision.

Review of `226f660fa` found that a checkout from PR #1617 still recognized the ready stamp during interrupted removal. It also showed that counting a removed artifact's blocks as freed could stop pressure relief when another directory retained a hard link. Upkeep now moves the ownership stamp to its pending name and syncs the store before removing entries. The existing PR #1617 launcher uses ordinary Cargo until upkeep restores the ready name. Store size counts each inode once. Reclamation credit excludes multiply linked files, including links that might all be inside the store.

Both Rust regressions failed on `226f660fa`. The interrupted cache still exposed its ready stamp. Upkeep credited 262,144 bytes for an artifact whose other hard link remained readable. The launcher and release suite passed all 21 tests with the launcher from PR #1617, including plain Cargo during eviction and cache reuse after the ready stamp returned. `scripts/dev quick` passed. Two attempts to run mutation testing stopped before testing any mutant because an unrelated session-socket fixture exceeded its path limit. The second attempt used a short symlink, which the fixture resolved back to its long target.

At `c7a697cd5`, all nine focused Rust tests passed across `yulius`, `yulius-app` and `yulius-sandbox`, including the two regressions above. The interruption test recovered on a healthy disk. The allocation test also cleared a store reporting zero freed artifact blocks and skipped the subsequent empty round. The release suite exercised the unchanged PR #1617 launcher. Vidoc reported disabled. The task’s delivery verification record supplies the exact landed SHA, CI run and individual job verdicts after the upkeep pull request merges.

With a short, disk-backed temporary directory, the full upkeep change at `c7a697cd5` passed its unmutated baseline of 3,386 tests. `scripts/dev mutants --base b00cc76a4 --minutes 45` reported **38 listed, 34 caught, 0 missed, 0 timed out, 4 unviable, 0 not reached** in 743 seconds. The four unviable mutations proposed invalid constructors for the disk report result. After integrating dev `00ff06cbf`, the nine focused tests also passed at `d322a8a7b`. Clippy passed with warnings denied on all targets of the three changed crates. `scripts/dev quick` and `spec_check` passed.

The scheduled evidence landed in [PR #1622](https://github.com/Quartio-Studio/yulius/pull/1622) at dev `a6314e1fa`. Its PR head `443977f21` passed in `7052f856-f4e9-4cdb-8f23-0fbaa62faaa1` through the verified train `aaf4a11f-6bf5-4b8f-b4d2-7cd364b5b876`. Full dev run `77e6c468-4dbe-4c2b-a306-19a9b38168d3` passed both shards and E2E. Revision `dc08c1a` superseded it while quality was running, so the overall dev verdict was cancelled. [Dev verification record](https://yulius.quartio.ai/api/tasks/5b06b876-20e3-4a88-b4c2-2c7c3667ff86/delivery/evidence/a6314e1fa472bbd762c96b03b7b77cae1f2aca9a)

Review of `8812780fb` found that removing the last two hard links to one 256 KiB artifact can release all its blocks even though pressure accounting correctly grants zero bytes. Before the admission fix, quota refresh and retry depended on the byte estimate or worktree deletion. The quota regression exercises task admission with both links in the store and with a surviving link outside it.

The task-admission test first failed because upkeep made zero fresh quota measurements after removing the artifact. After the fix it measured once and retried once. With both links in the cache the task started. With an external link the sandbox stayed over quota and the retry preserved the refusal. Fourteen focused Rust tests passed, including ordinary task and queued CI admission. The 21 launcher tests passed.

A CI dispatcher regression showed two full quota measurements after upkeep cleared a store with zero credited bytes. With an empty round it measured once. The dispatcher now reuses the reading upkeep took after a removal.
