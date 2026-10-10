//! Who sets a shared terminal's size (TSK-006, UI-019; F 2026-10-10: "each user should be able
//! to control it on their side").
//!
//! A PTY has one size, and a full-screen program draws for exactly that size. Every writer viewer
//! reports the size its own window fits and whether it is *active*: its page is visible and
//! focused and the user used the terminal recently. The terminal follows one viewer, the
//! controller:
//!
//! - typing, or clicking into the terminal, **claims** control at once;
//! - an active viewer takes control from an idle controller (or when nobody holds it);
//! - when the controller goes idle, the most recently active other viewer takes over; when it
//!   leaves, the most recently seen remaining viewer does;
//! - an idle controller keeps the size while nobody else is active, so a viewer alone always
//!   controls its terminal.
//!
//! Pure bookkeeping: each operation returns a [`Change`] saying whether control moved and which
//! size the terminal must take now; the caller applies it to the PTY. Read-only viewers never
//! join (they cannot control).

use std::collections::BTreeMap;

/// The effect of one operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    /// Control moved to another viewer (or to nobody): every viewer's `controller` flag may have
    /// changed.
    pub moved: bool,
    /// The size (cols, rows) the terminal must take now: the controller's own, when known.
    pub resize: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    size: Option<(u16, u16)>,
    active: bool,
    /// When this viewer was last active (a logical clock).
    seen: u64,
}

/// Writer viewers of one terminal and which of them controls its size (0 = nobody).
#[derive(Debug, Default)]
pub struct SizeControl {
    controller: u64,
    slots: BTreeMap<u64, Slot>,
    clock: u64,
}

impl SizeControl {
    /// The controlling viewer (0 = nobody).
    #[must_use]
    pub fn controller(&self) -> u64 {
        self.controller
    }

    /// Writer viewers attached.
    #[must_use]
    pub fn writers(&self) -> usize {
        self.slots.len()
    }

    /// The size the controller's window fits, when it reported one.
    #[must_use]
    pub fn controller_size(&self) -> Option<(u16, u16)> {
        self.slots.get(&self.controller).and_then(|slot| slot.size)
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Whether nobody holds control or the holder is idle.
    fn holder_idle(&self) -> bool {
        self.slots
            .get(&self.controller)
            .is_none_or(|slot| !slot.active)
    }

    fn give(&mut self, viewer: u64) -> Change {
        if self.controller == viewer {
            return Change::default();
        }
        self.controller = viewer;
        Change {
            moved: true,
            resize: self.slots.get(&viewer).and_then(|slot| slot.size),
        }
    }

    /// Control for `viewer` when nobody holds it or the holder is idle.
    fn offer(&mut self, viewer: u64) -> Change {
        if self.controller != viewer && self.holder_idle() {
            self.give(viewer)
        } else {
            Change::default()
        }
    }

    /// The most recently seen viewer other than `except`, among the active ones only or all.
    fn successor(&self, except: u64, active_only: bool) -> Option<u64> {
        self.slots
            .iter()
            .filter(|(id, slot)| **id != except && (slot.active || !active_only))
            .max_by_key(|(_, slot)| slot.seen)
            .map(|(id, _)| *id)
    }

    /// A writer attached with the size its window fits; it just opened the terminal, so it is
    /// active, and it takes control when nobody holds it or the holder is idle.
    pub fn join(&mut self, viewer: u64, size: Option<(u16, u16)>) -> Change {
        let seen = self.tick();
        self.slots.insert(
            viewer,
            Slot {
                size,
                active: true,
                seen,
            },
        );
        self.offer(viewer)
    }

    /// The size `viewer`'s window fits now (hello, resize): applied when it controls, or when
    /// nobody does (it then controls).
    pub fn report_size(&mut self, viewer: u64, size: (u16, u16)) -> Change {
        let Some(slot) = self.slots.get_mut(&viewer) else {
            return Change::default();
        };
        slot.size = Some(size);
        if self.controller == viewer {
            return Change {
                moved: false,
                resize: Some(size),
            };
        }
        if self.controller == 0 {
            return self.give(viewer);
        }
        Change::default()
    }

    /// `viewer` is active (visible, focused, used recently): it takes control from an idle
    /// holder.
    pub fn active(&mut self, viewer: u64) -> Change {
        let seen = self.tick();
        let Some(slot) = self.slots.get_mut(&viewer) else {
            return Change::default();
        };
        slot.active = true;
        slot.seen = seen;
        self.offer(viewer)
    }

    /// `viewer` typed or clicked into the terminal: it controls now, whoever held it.
    pub fn claim(&mut self, viewer: u64) -> Change {
        let seen = self.tick();
        let Some(slot) = self.slots.get_mut(&viewer) else {
            return Change::default();
        };
        slot.active = true;
        slot.seen = seen;
        self.give(viewer)
    }

    /// `viewer` went idle (hidden, unfocused, unused for a while). When it controls, the most
    /// recently active other viewer takes over; with nobody else active it keeps the size.
    pub fn idle(&mut self, viewer: u64) -> Change {
        let Some(slot) = self.slots.get_mut(&viewer) else {
            return Change::default();
        };
        slot.active = false;
        if self.controller != viewer {
            return Change::default();
        }
        match self.successor(viewer, true) {
            Some(next) => self.give(next),
            None => Change::default(),
        }
    }

    /// `viewer` left (or lost write access). When it controlled, the most recently active
    /// remaining viewer takes over, else the most recently seen one, else nobody.
    pub fn leave(&mut self, viewer: u64) -> Change {
        if self.slots.remove(&viewer).is_none() || self.controller != viewer {
            return Change::default();
        }
        let next = self
            .successor(viewer, true)
            .or_else(|| self.successor(viewer, false));
        match next {
            Some(next) => self.give(next),
            None => {
                self.controller = 0;
                Change {
                    moved: true,
                    resize: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: (u16, u16) = (220, 50);
    const NARROW: (u16, u16) = (90, 40);

    #[test]
    fn a_viewer_alone_always_controls_its_terminal() {
        let mut c = SizeControl::default();
        assert_eq!(
            c.join(1, Some(WIDE)),
            Change {
                moved: true,
                resize: Some(WIDE)
            }
        );
        // Idle (a hidden tab, a minute without input) changes nothing while nobody else is here.
        assert_eq!(c.idle(1), Change::default());
        assert_eq!(c.controller(), 1);
        // Its window changed while idle: the terminal still follows it.
        assert_eq!(
            c.report_size(1, NARROW),
            Change {
                moved: false,
                resize: Some(NARROW)
            }
        );
        assert_eq!(c.active(1), Change::default());
        assert_eq!(c.controller(), 1);
        assert_eq!(c.controller_size(), Some(NARROW));
    }

    #[test]
    fn an_active_viewer_does_not_take_the_size_from_another_active_one_until_it_claims() {
        let mut c = SizeControl::default();
        c.join(1, Some(WIDE));
        // Another viewer opens the task while the first one is active: it watches.
        assert_eq!(c.join(2, Some(NARROW)), Change::default());
        assert_eq!(c.active(2), Change::default());
        assert_eq!(
            c.report_size(2, (100, 40)),
            Change::default(),
            "stored only"
        );
        // It types (or clicks into the terminal): its own window's size, at once.
        assert_eq!(
            c.claim(2),
            Change {
                moved: true,
                resize: Some((100, 40))
            }
        );
        assert_eq!(
            c.claim(2),
            Change::default(),
            "claiming again changes nothing"
        );
        assert_eq!(
            c.claim(1),
            Change {
                moved: true,
                resize: Some(WIDE)
            }
        );
    }

    #[test]
    fn an_idle_controller_hands_the_size_to_the_active_viewer() {
        let mut c = SizeControl::default();
        c.join(1, Some(WIDE));
        c.join(2, Some(NARROW));
        // The controller hides its tab: the active viewer gets the size.
        assert_eq!(
            c.idle(1),
            Change {
                moved: true,
                resize: Some(NARROW)
            }
        );
        assert_eq!(c.controller(), 2);
        // Coming back does not take it back by itself (2 is active)…
        assert_eq!(c.active(1), Change::default());
        // …but once 2 goes idle, 1 (active) gets it.
        assert_eq!(
            c.idle(2),
            Change {
                moved: true,
                resize: Some(WIDE)
            }
        );
    }

    #[test]
    fn a_viewer_who_opens_the_task_takes_the_size_from_an_idle_holder() {
        // Michał's screenshot (F 2026-10-10): Bartek started the task and left his tab; Michał
        // opens it in a wide window and sees it at his own size.
        let mut c = SizeControl::default();
        c.join(1, Some(NARROW));
        c.idle(1);
        assert_eq!(
            c.join(2, Some(WIDE)),
            Change {
                moved: true,
                resize: Some(WIDE)
            }
        );
        // Likewise an idle viewer who becomes active again takes over from an idle holder.
        c.idle(2);
        assert_eq!(
            c.active(1),
            Change {
                moved: true,
                resize: Some(NARROW)
            }
        );
    }

    #[test]
    fn the_controller_leaving_hands_over_to_the_most_recently_active_viewer() {
        let mut c = SizeControl::default();
        c.join(1, Some(WIDE));
        c.join(2, Some(NARROW));
        c.join(3, Some((120, 30)));
        c.idle(3);
        c.active(2);
        assert_eq!(
            c.leave(1),
            Change {
                moved: true,
                resize: Some(NARROW)
            }
        );
        // Nobody active left: the most recently seen viewer gets it (it is the only one there).
        c.idle(2);
        assert_eq!(
            c.leave(2),
            Change {
                moved: true,
                resize: Some((120, 30))
            }
        );
        // A watcher leaving changes nothing; the last writer leaving leaves nobody in control.
        assert_eq!(c.leave(9), Change::default());
        assert_eq!(
            c.leave(3),
            Change {
                moved: true,
                resize: None
            }
        );
        assert_eq!((c.controller(), c.writers()), (0, 0));
        // The next writer to report a size controls again.
        c.join(4, None);
        assert_eq!(c.controller(), 4);
        assert_eq!(
            c.report_size(4, WIDE),
            Change {
                moved: false,
                resize: Some(WIDE)
            }
        );
    }

    #[test]
    fn viewers_that_never_joined_change_nothing() {
        let mut c = SizeControl::default();
        c.join(1, Some(WIDE));
        for change in [
            c.active(7),
            c.claim(7),
            c.idle(7),
            c.report_size(7, NARROW),
            c.leave(7),
        ] {
            assert_eq!(change, Change::default());
        }
        assert_eq!(c.controller(), 1);
    }
}
