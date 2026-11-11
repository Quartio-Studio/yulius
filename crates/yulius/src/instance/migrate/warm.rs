//! The warm copy of a move (SPEC-465, ADR-0303): passes of the export's stream into a staged
//! copy on the new host while the old instance runs, the estimate of the freeze that follows,
//! and the frozen pass that sends only what the receipts do not cover.

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use super::profile::Profile;
use super::target::{Expect, Facts, Request};
use super::{Move, Stop, bytes, failed, shown};
use crate::host_setup::Cmd;
use crate::instance::export::{self, Sent};
use crate::instance::freeze::Freeze;
use crate::instance::inventory::{Counter, IdSpace, Inventory};
use crate::instance::receipts::{self, Era, Outdated, Reconciled, Stage};
use crate::instance::{Manifest, Pass, Progress, walk};

/// The delta passes one run makes before it freezes anyway, or stops when the freeze is bound.
pub const MAX_DELTA_PASSES: u32 = 5;
/// What the estimate allows for what follows the proof: the mark, the last items of
/// `host setup`, the unit's start and its first answer. They took 10 to 21 s between two
/// emulated hosts (SPEC-465's Evidence).
const START_SECS: u64 = 30;
/// Without a bound the move copies again while the changes alone take longer than this to send.
const WORTH_SECS: u64 = 60;

/// What a delta pass measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// The time the old host needed to look at every entry.
    pub walk: f64,
    /// The time it spent sending the entries that had changed.
    pub sending: f64,
    /// The part of `sending` that went to entries that earned no receipt (they changed too
    /// close to the pass or while they were read, or lie on another filesystem): the next
    /// pass sends them again whatever happens.
    pub again: f64,
    /// The time the new host took to finish the pass (its sync).
    pub tail: f64,
    /// The pass's length.
    pub secs: f64,
    /// How long changes had gathered for the pass: the time since the pass before began; 0
    /// when nobody knows.
    pub window: f64,
}

/// The freeze a move expects after a delta pass (SPEC-465 WCP-012), in seconds: in all, and of
/// that for sending what will have changed. The tree is counted four times (planned and
/// streamed on the old host, pruned and proven on the new one, at the old host's rate); the
/// changes are the last pass's, scaled by how long they gathered for it against how long they
/// gather now, except what earned no receipt, which is sent again in full; then the new host's
/// sync and the start.
#[must_use]
pub fn estimate(m: &Measured) -> (u64, u64) {
    let share = if m.window > 0.0 {
        (m.secs / m.window).min(1.0)
    } else {
        1.0
    };
    let again = m.again.clamp(0.0, m.sending);
    let changes = again + (m.sending - again) * share;
    let total = (4.0 * m.walk + m.tail + changes).round();
    (total as u64 + START_SECS, changes.round() as u64)
}

/// How fast a pass looks at entries of `root` once their metadata is cached: what a walk
/// reaches within `cold` is walked a second time, and that second walk is timed. The entries of
/// the sample and the entries a second; `None` when nothing can be read.
#[must_use]
pub fn sampled(root: &Path, cold: Duration) -> Option<(u64, f64)> {
    let walked = |limit: Option<Duration>, most: u64| {
        let started = Instant::now();
        let mut seen = 0u64;
        // Ended by its own answer once it has seen enough; what it saw until then counts.
        let _ = walk::walk_seen(root, true, &walk::copied, &mut |_| {
            seen += 1;
            if seen >= most || limit.is_some_and(|limit| started.elapsed() >= limit) {
                return Err(io::Error::other("enough"));
            }
            Ok(true)
        });
        (seen, started.elapsed())
    };
    let (found, _) = walked(Some(cold), u64::MAX);
    let (again, took) = walked(None, found);
    (again > 0).then(|| (again, again as f64 / took.as_secs_f64().max(1e-6)))
}

/// The time of a pass that went to entries every pass sends again, in seconds: those that
/// earned no receipt, and those the new host could not install (`deferred`, by their number
/// among the entries sent).
fn again(sent: &Sent, deferred: &[u64]) -> f64 {
    (sent.unearned + sent.waiting(deferred)).as_secs_f64()
}

/// The estimated freeze in words: what it is made of.
fn freeze_said(measured: &Measured) -> String {
    let (total, changes) = estimate(measured);
    let again = measured.again.clamp(0.0, measured.sending).round() as u64;
    let every = if again > 0 {
        format!(
            " ({} of it for entries that every pass sends: they change all the time, lie on \
             another filesystem, or wait for the freeze behind a directory)",
            span(again)
        )
    } else {
        String::new()
    };
    format!(
        "about {} frozen: the entries are counted four times ({} each, twice on each host), \
         what changes meanwhile takes about {} to send{every}, the new host's sync {}, and the \
         start about {}",
        span(total),
        span(measured.walk.round() as u64),
        span(changes),
        span(measured.tail.round() as u64),
        span(START_SECS)
    )
}

fn span(secs: u64) -> String {
    if secs < 120 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
    }
}

impl Move<'_> {
    /// What the plan says of the copy and the freeze before anything is measured.
    pub(super) fn plan_downtime(&mut self, p: &Profile, rate: u64) {
        let bound = self.opts.max_downtime.map_or_else(String::new, |max| {
            format!(
                "; with --max-downtime {max} it begins only when that is at most {max} min, and \
                 the move stops without freezing after {MAX_DELTA_PASSES} more passes"
            )
        });
        let copy = if self.stage.whole() {
            format!(
                "the new host holds a staged copy ({} entries acknowledged): the instance runs \
                 while what changed since is sent",
                self.stage.receipts()
            )
        } else {
            format!(
                "the instance runs during the copy (about {} for up to {} at that rate)",
                span(p.used_bytes / rate.max(1)),
                bytes(p.used_bytes)
            )
        };
        // A first estimate, before anything is copied: the passes will measure it again.
        let entries = p.used_inodes;
        let freeze = self
            .world
            .walk_rate(&self.opts.data_dir)
            .filter(|per_sec| *per_sec >= 1.0 && entries > 0)
            .map_or_else(
                || {
                    "it is frozen only for the last changes, the proof and the start. The \
                     estimate is shown before the freeze"
                        .to_owned()
                },
                |per_sec| {
                    let counts = (4.0 * entries as f64 / per_sec).round() as u64;
                    format!(
                        "then it is frozen for about {}: up to {entries} entries are counted \
                         four times (about {} a second here, twice on each host), what changed \
                         during the last pass is sent, and the new instance starts. It is \
                         measured again before the freeze",
                        span(counts + START_SECS),
                        per_sec.round() as u64
                    )
                },
            );
        self.ok("downtime", format!("{copy}; {freeze}{bound}"));
    }

    /// Makes the receipts agree with the new host's marker; `Ok(true)` when the hosts agree on
    /// no staged copy (the receipts are gone).
    pub(super) fn agree(&mut self) -> Result<bool, Stop> {
        let reply = self.call(
            "copy",
            &Request::Stage {
                data_dir: self.opts.data_dir.clone(),
            },
        )?;
        let (had, at) = (self.stage.id().is_some(), self.world.era());
        let found = self
            .stage
            .reconcile(reply.stage.as_ref(), &at)
            .map_err(|why| self.fail("copy", why))?;
        self.say_outdated(&found, true);
        if found.fresh && had {
            self.changed(
                "copy",
                "the staged copy this host knew is not on the new host (removed, or another \
                 one): the copy starts from the beginning",
            );
        }
        Ok(found.fresh)
    }

    /// Says that what this host knew of the staged copy was dropped, and why (SPEC-465
    /// WCP-016). After a frozen pass (`live` is false) nothing is sent again: nothing changed
    /// while the instance was frozen, and the proof that follows counts the whole copy.
    fn say_outdated(&mut self, found: &Reconciled, live: bool) {
        if found.outdated.is_some() && !live {
            self.changed(
                "copy",
                "this host's clock was set back during the frozen pass: what this host knew of \
                 the staged copy is dropped. Nothing changed while the instance was frozen, and \
                 the copy is proven next",
            );
            return;
        }
        let why = match found.outdated {
            Some(Outdated::Restarted) => {
                "this host was restarted since the last pass: after a crash a file may be newer \
                 than its change time says"
            }
            Some(Outdated::ClockBack) => {
                "this host's clock was set back since a pass began: a later change may carry an \
                 earlier time"
            }
            None => return,
        };
        self.changed(
            "copy",
            format!(
                "{why}. Nothing is taken as copied: everything is sent into the staged copy \
                 again"
            ),
        );
    }

    /// The warm passes (SPEC-465 WCP-001, WCP-012, WCP-013): until a pass that found only
    /// changes leaves a freeze short enough, or the bound cannot be met.
    pub(super) fn warm(&mut self, p: &Profile, facts: &Facts) -> Result<(), Stop> {
        self.phase("Copy");
        // Recorded before the first stream: an abort names the copy on the new host also when
        // this host has dropped what it knew of it.
        if !self.state.copied {
            self.state.copied = true;
            self.save_state()?;
        }
        let zstd = self.zstd(facts);
        let mut deltas = 0;
        for _ in 0..MAX_DELTA_PASSES + 2 {
            if self.agree()? {
                // An unfinished copy that is not this move's goes; then a new stage begins.
                self.call(
                    "data directory",
                    &Request::DataDir {
                        setup: self.setup(p),
                        stage: None,
                    },
                )?;
                self.stage
                    .renew(&receipts::token())
                    .map_err(|why| self.fail("copy", why))?;
            }
            // Only a pass that began with a whole staged copy measures the changes.
            let delta = self.stage.whole();
            let measured = self.live_pass(p, zstd)?;
            // Nor does a pass whose receipts were dropped when it ended (WCP-016): the staged
            // copy is filled again before anything freezes.
            if !delta || !self.stage.whole() {
                continue;
            }
            deltas += 1;
            if self.short_enough(&measured, deltas)? {
                return Ok(());
            }
        }
        Err(self.fail(
            "copy",
            "the new host acknowledged no whole pass: nothing is frozen. Run the command again",
        ))
    }

    /// Says the estimated freeze and decides: `true` to freeze now, `false` to copy again.
    fn short_enough(&mut self, measured: &Measured, deltas: u32) -> Result<bool, Stop> {
        let (total, changes) = estimate(measured);
        let said = freeze_said(measured);
        let last = deltas >= MAX_DELTA_PASSES;
        let above = |max: u64| total > max.saturating_mul(60);
        match self.opts.max_downtime {
            Some(max) if above(max) && last => {
                self.fail(
                    "downtime",
                    format!(
                        "{said}: above --max-downtime {max} after {deltas} passes that sent only \
                         changes. Nothing was frozen and the old instance runs; the staged copy \
                         stays on the new host. Run the command again at a quieter time or with \
                         a higher bound"
                    ),
                );
                Err(Stop::Failed)
            }
            Some(max) if above(max) => {
                self.ok(
                    "downtime",
                    format!("{said}: above --max-downtime {max}, so the move copies again"),
                );
                Ok(false)
            }
            None if changes > WORTH_SECS && !last => {
                self.ok(
                    "downtime",
                    format!("{said}: the move copies again to shorten it"),
                );
                Ok(false)
            }
            _ => {
                self.ok("downtime", said);
                Ok(true)
            }
        }
    }

    pub(super) fn zstd(&mut self, facts: &Facts) -> bool {
        facts.zstd
            && self
                .local
                .run(&Cmd::new(&["zstd", "--version"]))
                .is_ok_and(|out| out.ok())
    }

    /// Names the next stream in `manifest` and begins its journal. `began` and `at` are the
    /// clock and its lead when the pass began: what its receipts are measured against.
    fn named(
        &mut self,
        mut manifest: Manifest,
        live: bool,
        began: (i64, i64),
        at: &Era,
    ) -> Result<Manifest, Stop> {
        let nonce = receipts::token();
        let began = u64::try_from(began.0).unwrap_or(0);
        self.stage
            .begin(&nonce, live, began, at)
            .map_err(|why| self.fail("copy", format!("{why}. Run the command again")))?;
        manifest.pass = Some(Pass {
            stage: self.stage.id().unwrap_or_default().to_owned(),
            seq: self.stage.acked(),
            nonce,
            live,
            kept: 0,
        });
        Ok(manifest)
    }

    /// Sends one stream into the new host's staged copy and takes its answer: what the new
    /// host says of it, and the entries it could not install (by their number among those
    /// sent). Its marker, which came with the answer, is what the receipts follow. `live`
    /// says whether the instance runs meanwhile.
    fn stream(
        &mut self,
        zstd: bool,
        live: bool,
        write: &mut dyn FnMut(&mut dyn Write, &mut Stage) -> io::Result<()>,
    ) -> Result<(String, Vec<u64>), Stop> {
        self.call(
            "copy",
            &Request::Expect(Expect {
                data_dir: self.opts.data_dir.clone(),
                zstd,
            }),
        )?;
        let Self { remote, stage, .. } = self;
        let sent = remote.send_copy(zstd, &mut |out| write(out, stage));
        let reply = sent.map_err(|why| {
            self.fail(
                "copy",
                format!(
                    "{why}. Run the command again: it goes on from what the new host \
                     acknowledged"
                ),
            )
        })?;
        if let Some(mark) = &reply.stage {
            let at = self.world.era();
            let found = self
                .stage
                .reconcile(Some(mark), &at)
                .map_err(|why| self.fail("copy", why))?;
            self.say_outdated(&found, live);
        }
        let deferred = reply
            .stage
            .as_ref()
            .and_then(|mark| mark.last.as_ref())
            .map(|last| last.deferred.clone())
            .unwrap_or_default();
        if failed(&reply.lines) {
            for line in reply.lines {
                self.push(shown(line));
            }
            return Err(Stop::Failed);
        }
        let answer = reply
            .lines
            .into_iter()
            .next()
            .map(|line| shown(line).detail)
            .unwrap_or_default();
        Ok((answer, deferred))
    }

    /// One pass while the instance runs.
    fn live_pass(&mut self, p: &Profile, zstd: bool) -> Result<Measured, Stop> {
        let opts = self.opts;
        // The clock first, then the sync: what is written through a mapping after the sync is
        // stamped after `began`, and earns no receipt in this pass. The clock's lead is read
        // with it: a clock set back from here on is found when the pass ends.
        let at = self.world.era();
        let began = self.world.now();
        if let Err(why) = self.world.sync(&opts.data_dir) {
            return Err(self.fail(
                "copy",
                format!("cannot sync {}: {why}", opts.data_dir.display()),
            ));
        }
        let (number, delta) = (self.stage.acked() + 1, self.stage.whole());
        let before = self.stage.began();
        let manifest = export::manifest_of(&opts.data_dir, &opts.service)
            .map_err(|why| self.fail("copy", why))?;
        let manifest = self.named(manifest, true, began, &at)?;
        self.world.show(&format!(
            "Copying while the instance runs (pass {number}{}){}.",
            if delta { ": what changed" } else { "" },
            if zstd { ", compressed with zstd" } else { "" }
        ));
        // Only a first pass knows roughly how much there is.
        let known = if self.stage.receipts() == 0 {
            p.used_bytes
        } else {
            0
        };
        let mut progress = Progress::new("Sent", known);
        let started = Instant::now();
        let mut sent = Sent::default();
        let mut walked = 0.0;
        let answer = self.stream(zstd, true, &mut |out, stage| {
            sent = export::send_live(&opts.data_dir, &manifest, out, stage, began, &mut |done| {
                progress.tick(done)
            })?;
            walked = started.elapsed().as_secs_f64();
            Ok(())
        });
        progress.end();
        let (answer, deferred) = answer?;
        let secs = started.elapsed().as_secs_f64();
        let later = sent.vanished + sent.torn;
        self.changed(
            "copy",
            format!(
                "pass {number}: {} of {} entries sent ({}) in {} while the instance runs{}; the \
                 new host: {answer}",
                sent.sent,
                sent.walked,
                bytes(sent.bytes),
                span(secs.ceil() as u64),
                if later > 0 {
                    format!(" ({later} vanished or changed while read: a later pass has them)")
                } else {
                    String::new()
                }
            ),
        );
        let sending = sent.sending.as_secs_f64();
        Ok(Measured {
            walk: (walked - sending).max(0.0),
            sending,
            again: again(&sent, &deferred),
            tail: (secs - walked).max(0.0),
            secs,
            window: before.map_or(0.0, |before| {
                u64::try_from(began.0).unwrap_or(0).saturating_sub(before) as f64
            }),
        })
    }

    /// The frozen pass (SPEC-465 WCP-007): exact, of what the receipts do not cover, with the
    /// paths the old host holds. Returns the old host's inventory of the whole tree, counted
    /// while the pass was planned, and the manifest with the pass's totals.
    pub(super) fn frozen_pass(
        &mut self,
        facts: &Facts,
        held: &mut Freeze,
    ) -> Result<(Inventory, Manifest), Stop> {
        let opts = self.opts;
        // Before anything is planned on the receipts: a plan made on receipts that are dropped
        // when the stream begins would not be the stream's.
        let at = self.world.era();
        if let Some(why) = self.stage.outdated(&at) {
            return Err(self.fail(
                "copy",
                format!(
                    "{}: nothing is copied while frozen. Run the command again: it sends \
                     everything into the staged copy again while the instance runs",
                    match why {
                        Outdated::Restarted => "this host was restarted since the last pass",
                        Outdated::ClockBack => "this host's clock was set back since the last pass",
                    }
                ),
            ));
        }
        if !self.stage.whole() {
            return Err(self.fail(
                "copy",
                "the staged copy on the new host is not whole: nothing is copied while frozen. \
                 Run the command again",
            ));
        }
        let zstd = self.zstd(facts);
        let began = self.world.now();
        let manifest = export::manifest_of(&opts.data_dir, &opts.service)
            .map_err(|why| self.fail("copy", why))?;
        let mut counter = Counter::new(IdSpace::of(&manifest));
        let plan = export::plan_pass(&opts.data_dir, Some(&self.stage), &mut |e| counter.add(e))
            .map_err(|e| self.fail("copy", format!("planning failed: {e}")))?;
        let old = counter.finish();
        let mut manifest = self.named(manifest, false, began, &at)?;
        self.world.show(&format!(
            "Copying what changed: {} of {} entries, {}{}.",
            plan.entries,
            plan.kept,
            bytes(plan.total),
            if zstd { ", compressed with zstd" } else { "" }
        ));
        let mut progress = Progress::new("Sent", plan.total);
        let started = Instant::now();
        let answer = self.stream(zstd, false, &mut |out, stage| {
            export::send_pass(
                &opts.data_dir,
                &plan,
                &manifest,
                out,
                Some(stage),
                &mut |done| progress.tick(done),
                &mut || held.helper_idle().map_err(io::Error::other),
            )
            .map(|_| ())
        });
        progress.end();
        let (answer, _) = answer?;
        self.changed(
            "copy",
            format!(
                "the changes since the last pass: {} of {} entries sent ({}) in {}; the new \
                 host: {answer}",
                plan.entries,
                plan.kept,
                bytes(plan.total),
                span(started.elapsed().as_secs_f64().ceil() as u64)
            ),
        );
        manifest.total_bytes = plan.total;
        manifest.entries = plan.entries;
        Ok((old, manifest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_is_four_counts_the_scaled_changes_the_sync_and_the_start() {
        // A pass of 100 s: 60 s to look at everything, 30 s sending what an hour had changed,
        // 10 s for the new host to sync.
        let measured = Measured {
            walk: 60.0,
            sending: 30.0,
            again: 0.0,
            tail: 10.0,
            secs: 100.0,
            window: 3600.0,
        };
        // 100 s of changes instead of an hour's: a 36th of the sending.
        assert_eq!(estimate(&measured), (4 * 60 + 10 + 1 + START_SECS, 1));
        // Nobody knows how long the changes gathered: as much again.
        let unknown = Measured {
            window: 0.0,
            ..measured
        };
        assert_eq!(estimate(&unknown), (4 * 60 + 10 + 30 + START_SECS, 30));
        // A pass longer than what it caught up on is not scaled up.
        let slow = Measured {
            window: 50.0,
            ..measured
        };
        assert_eq!(estimate(&slow).1, 30);
        // Of the 30 s, 12 s went to entries that earned no receipt (they change all the time,
        // or lie on another filesystem): the freeze sends them whatever happens, and only the
        // rest is scaled.
        let unsettled = Measured {
            again: again(
                &Sent {
                    sending: Duration::from_secs(30),
                    unearned: Duration::from_secs(2),
                    // The second entry sent took 10 s and earned a receipt; the new host
                    // could not install it (a directory is in its way).
                    took: vec![0, 100_000, 5],
                    ..Sent::default()
                },
                &[1],
            ),
            ..measured
        };
        assert_eq!(estimate(&unsettled), (4 * 60 + 10 + 13 + START_SECS, 13));
        assert!(freeze_said(&unsettled).contains(
            "what changes meanwhile takes about 13 s to send (12 s of it for entries that \
             every pass sends: they change all the time, lie on another filesystem, or wait \
             for the freeze behind a directory)"
        ));
        assert!(
            freeze_said(&measured).contains("what changes meanwhile takes about 1 s to send, the"),
            "{}",
            freeze_said(&measured)
        );
        assert_eq!(
            (span(119), span(120), span(121)),
            ("119 s".into(), "2 min".into(), "3 min".into())
        );
    }

    #[test]
    fn a_sample_of_the_tree_says_how_fast_a_pass_looks_at_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for d in 0..5 {
            std::fs::create_dir_all(dir.path().join(format!("sandboxes/d{d}")))?;
            for f in 0..40 {
                std::fs::write(dir.path().join(format!("sandboxes/d{d}/f{f}")), b"x")?;
            }
        }
        // What a copy leaves out is not sampled.
        std::fs::create_dir_all(dir.path().join("system/base"))?;
        let (entries, rate) = sampled(dir.path(), Duration::from_secs(5)).ok_or("no rate")?;
        // The data directory, `sandboxes`, five directories and their 200 files.
        assert_eq!(entries, 207);
        assert!(rate > 1.0, "{rate}");
        // A sample is cut by the time it may take, and then read again as far as it got.
        let (entries, rate) = sampled(dir.path(), Duration::ZERO).ok_or("no rate")?;
        assert_eq!(entries, 1);
        assert!(rate > 0.0);
        assert_eq!(
            sampled(&dir.path().join("missing"), Duration::from_secs(1)),
            None
        );
        Ok(())
    }
}
