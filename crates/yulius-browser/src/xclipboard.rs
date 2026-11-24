//! CLIPBOARD transfers on a sandbox's private display, both ways.
//!
//! Reading ([`Clipboard`]): each click gets an unmapped requestor window so late X events cannot
//! complete a newer viewer's request.
//!
//! Writing ([`Owner`], SPEC-639): a viewer's paste makes its text the CLIPBOARD, owned by an
//! unmapped window of the display's thread, which then presses Shift+Insert so the page gets a real
//! paste. The owner answers `TARGETS`, `UTF8_STRING` and `text/plain;charset=utf-8` (32 KiB at
//! most, never incremental), says whether a client fetched the text within [`PASTE_WINDOW`], waits for the reads of one
//! paste to end before a newer one replaces it ([`PASTE_QUIET`]), and lets the
//! selection go after [`PASTE_HOLD`] or when another client takes it, erasing the text.

use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, PropMode,
    Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use yulius_contracts::browser::CLIPBOARD_MAX_BYTES;
use zeroize::Zeroizing;

use crate::display::PasteOutcome;

const CLICK_WINDOW: Duration = Duration::from_secs(3);
const OFFER_WINDOW: Duration = Duration::from_secs(15);
/// How long a client (Chrome) has to ask for a paste's text after the paste key.
pub(crate) const PASTE_WINDOW: Duration = Duration::from_secs(3);
/// How long the display's CLIPBOARD holds a pasted text.
const PASTE_HOLD: Duration = Duration::from_secs(30);
/// A newer paste replaces the CLIPBOARD only once no client asked for the held text for this
/// long: Chrome reads it several times for one paste (`TARGETS`, `UTF8_STRING`, `TARGETS`
/// again), and a CLIPBOARD that changes in between, or a read left unanswered, gives the page
/// an empty paste after Chrome's one-second timeout (traced with Chrome 158).
pub(crate) const PASTE_QUIET: Duration = Duration::from_millis(100);
/// The longest a newer paste waits for that quiet.
pub(crate) const PASTE_QUIET_MAX: Duration = Duration::from_secs(1);

/// The display thread's CLIPBOARD for viewers' pastes.
pub(super) struct Owner {
    window: Window,
    selection: Atom,
    targets: Atom,
    utf8: Atom,
    plain: Atom,
    held: Option<Held>,
    /// When a client last asked for the held text (any target).
    asked: Option<Instant>,
}

struct Held {
    text: Zeroizing<Vec<u8>>,
    until: Instant,
    /// The paste waiting for a client to ask, and until when.
    waiting: Option<(oneshot::Sender<PasteOutcome>, Instant)>,
}

impl Owner {
    pub(super) fn new(conn: &RustConnection, root: Window) -> Option<Self> {
        let atom = |name| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let window = conn.generate_id().ok()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )
        .ok()?
        .check()
        .ok()?;
        Some(Self {
            window,
            selection: atom(b"CLIPBOARD")?,
            targets: atom(b"TARGETS")?,
            utf8: atom(b"UTF8_STRING")?,
            plain: atom(b"text/plain;charset=utf-8")?,
            held: None,
            asked: None,
        })
    }

    /// Whether a client may still read the held text: its paste waits for the text to be
    /// fetched, or a client asked for it less than [`PASTE_QUIET`] ago. A newer paste waits
    /// (answering) until it is not, so it neither takes another viewer's paste away before the
    /// page read it nor changes the CLIPBOARD between Chrome's reads.
    pub(super) fn reading(&self) -> bool {
        self.held.as_ref().is_some_and(|held| {
            held.waiting.is_some() || self.asked.is_some_and(|at| at.elapsed() < PASTE_QUIET)
        })
    }

    /// Makes `text` the CLIPBOARD; `outcome` hears whether a client asked for it. Gives
    /// `outcome` back when the server did not hand the selection to this owner.
    pub(super) fn take(
        &mut self,
        conn: &RustConnection,
        text: &str,
        outcome: oneshot::Sender<PasteOutcome>,
    ) -> Result<(), oneshot::Sender<PasteOutcome>> {
        self.drop_held();
        let owned = conn
            .set_selection_owner(self.window, self.selection, x11rb::CURRENT_TIME)
            .is_ok()
            && self.owns(conn);
        if !owned {
            return Err(outcome);
        }
        let now = Instant::now();
        self.held = Some(Held {
            text: Zeroizing::new(text.as_bytes().to_vec()),
            until: now + PASTE_HOLD,
            waiting: Some((outcome, now + PASTE_WINDOW)),
        });
        Ok(())
    }

    /// Ends a wait whose window passed (unread) and lets go of a text held long enough.
    pub(super) fn expire(&mut self, conn: &RustConnection) {
        let now = Instant::now();
        let Some(held) = &mut self.held else {
            return;
        };
        if held
            .waiting
            .as_ref()
            .is_some_and(|(outcome, until)| now >= *until || outcome.is_closed())
            && let Some((outcome, _)) = held.waiting.take()
        {
            let _ = outcome.send(PasteOutcome::Unread);
        }
        if now >= held.until {
            self.drop_held();
            // Only this owner's selection is cleared, never another client's.
            if self.owns(conn) {
                let _ = conn.set_selection_owner(x11rb::NONE, self.selection, x11rb::CURRENT_TIME);
            }
        }
    }

    /// Whether the server names this owner's window as the CLIPBOARD's owner now.
    fn owns(&self, conn: &RustConnection) -> bool {
        conn.get_selection_owner(self.selection)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.owner == self.window)
    }

    fn drop_held(&mut self) {
        if let Some(Held {
            waiting: Some((outcome, _)),
            ..
        }) = self.held.take()
        {
            let _ = outcome.send(PasteOutcome::Unread);
        }
    }

    pub(super) fn on_event(&mut self, conn: &RustConnection, event: &Event) {
        match event {
            Event::SelectionRequest(request)
                if request.owner == self.window && request.selection == self.selection =>
            {
                self.answer(conn, request);
            }
            Event::SelectionClear(clear)
                if clear.owner == self.window && clear.selection == self.selection =>
            {
                // Another client (the page copying) took the CLIPBOARD, unless this clear is of
                // an earlier ownership (it was taken again with the current time since).
                if !self.owns(conn) {
                    self.drop_held();
                }
            }
            _ => {}
        }
    }

    /// Answers a client's request for the held text: its targets or its UTF-8 text; anything
    /// else, or nothing held, is refused. Only the text answered tells the waiting paste it was
    /// read (the targets alone give the page nothing); any answer is a read for [`Self::reading`].
    fn answer(&mut self, conn: &RustConnection, request: &SelectionRequestEvent) {
        // Obsolete clients name no property: the target is the property then (ICCCM 2.2).
        let property = if request.property == x11rb::NONE {
            request.target
        } else {
            request.property
        };
        let text = request.target == self.utf8 || request.target == self.plain;
        let written = self.held.as_ref().is_some_and(|held| {
            let written = if request.target == self.targets {
                conn.change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    AtomEnum::ATOM,
                    &[self.targets, self.utf8, self.plain],
                )
            } else if text {
                conn.change_property8(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    request.target,
                    &held.text,
                )
            } else {
                return false;
            };
            written.is_ok()
        });
        if written {
            self.asked = Some(Instant::now());
        }
        if written
            && text
            && let Some(held) = &mut self.held
            && let Some((outcome, _)) = held.waiting.take()
        {
            let _ = outcome.send(PasteOutcome::Read);
        }
        let _ = conn.send_event(
            false,
            request.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property: if written { property } else { x11rb::NONE },
            },
        );
    }
}

pub(super) struct Clipboard {
    selection: Atom,
    utf8: Atom,
    incr: Atom,
    request: Option<Request>,
}

struct Request {
    window: Window,
    expires: Instant,
    phase: Phase,
}

enum Phase {
    Watching(oneshot::Sender<u32>),
    Offered(u32),
    Reading(oneshot::Sender<String>, Transfer),
}

enum Transfer {
    Converting,
    Chunks(Vec<u8>),
}

impl Clipboard {
    pub(super) fn new(conn: &RustConnection) -> Option<Self> {
        conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
        let atom = |name| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        Some(Self {
            selection: atom(b"CLIPBOARD")?,
            utf8: atom(b"UTF8_STRING")?,
            incr: atom(b"INCR")?,
            request: None,
        })
    }

    pub(super) fn start(
        &mut self,
        conn: &RustConnection,
        root: Window,
        offer: oneshot::Sender<u32>,
    ) {
        self.cancel(conn);
        let Ok(window) = conn.generate_id() else {
            return;
        };
        if conn
            .create_window(
                0,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .is_err()
        {
            return;
        }
        self.request = Some(Request {
            window,
            phase: Phase::Watching(offer),
            expires: Instant::now() + CLICK_WINDOW,
        });
        if conn
            .xfixes_select_selection_input(
                window,
                self.selection,
                SelectionEventMask::SET_SELECTION_OWNER,
            )
            .is_err()
        {
            self.cancel(conn);
        }
    }

    pub(super) fn expire(&mut self, conn: &RustConnection) {
        if self.request.as_ref().is_some_and(|r| {
            Instant::now() >= r.expires
                || match &r.phase {
                    Phase::Watching(offer) => offer.is_closed(),
                    Phase::Reading(result, _) => result.is_closed(),
                    Phase::Offered(_) => false,
                }
        }) {
            self.cancel(conn);
        }
    }

    pub(super) fn cancel(&mut self, conn: &RustConnection) {
        if let Some(request) = self.request.take() {
            let _ = conn.destroy_window(request.window);
        }
    }

    /// Only this explicit acceptance may ask the current selection owner for its text.
    pub(super) fn accept(
        &mut self,
        conn: &RustConnection,
        offer: u32,
        result: oneshot::Sender<String>,
    ) {
        self.expire(conn);
        let Some(request) = self.request.as_mut().filter(|r| r.window == offer) else {
            return;
        };
        let Phase::Offered(timestamp) = request.phase else {
            return;
        };
        if conn
            .convert_selection(
                request.window,
                self.selection,
                self.utf8,
                self.utf8,
                timestamp,
            )
            .is_err()
        {
            self.cancel(conn);
        } else {
            request.expires = Instant::now() + CLICK_WINDOW;
            request.phase = Phase::Reading(result, Transfer::Converting);
        }
    }

    pub(super) fn on_event(&mut self, conn: &RustConnection, event: &Event) {
        self.expire(conn);
        let Some(request) = self.request.as_mut() else {
            return;
        };
        match event {
            Event::XfixesSelectionNotify(changed) if changed.window == request.window => {
                if let Phase::Watching(offer) =
                    std::mem::replace(&mut request.phase, Phase::Offered(changed.timestamp))
                    && changed.owner != x11rb::NONE
                    && offer.send(request.window).is_ok()
                {
                    request.expires = Instant::now() + OFFER_WINDOW;
                } else {
                    // The offered selection was replaced before the viewer accepted it.
                    self.cancel(conn);
                }
            }
            Event::SelectionNotify(answer) if answer.requestor == request.window => {
                if answer.property == self.utf8
                    && answer.target == self.utf8
                    && answer.selection == self.selection
                {
                    self.read(conn);
                } else {
                    self.cancel(conn);
                }
            }
            Event::PropertyNotify(chunk)
                if chunk.window == request.window
                    && chunk.atom == self.utf8
                    && chunk.state == Property::NEW_VALUE
                    && matches!(request.phase, Phase::Reading(_, Transfer::Chunks(_))) =>
            {
                self.read(conn)
            }
            _ => {}
        }
    }

    fn read(&mut self, conn: &RustConnection) {
        let Some(request) = self.request.as_mut() else {
            return;
        };
        let Phase::Reading(_, state) = &mut request.phase else {
            return;
        };
        let limit = u32::try_from(CLIPBOARD_MAX_BYTES.div_ceil(4)).unwrap_or(u32::MAX);
        let Some(property) = conn
            .get_property(true, request.window, self.utf8, AtomEnum::ANY, 0, limit)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            self.cancel(conn);
            return;
        };
        let Some(result) = state.receive(property, self.utf8, self.incr) else {
            return;
        };
        if let Some(request) = self.request.take() {
            let _ = conn.destroy_window(request.window);
            if let Phase::Reading(reply, _) = request.phase
                && let Ok(text) = result
            {
                let _ = reply.send(text);
            }
        }
    }
}

impl Transfer {
    /// None awaits the next INCR chunk. A terminal result closes this requestor.
    fn receive(
        &mut self,
        property: GetPropertyReply,
        utf8: Atom,
        incr: Atom,
    ) -> Option<Result<String, ()>> {
        if property.bytes_after != 0 {
            return Some(Err(()));
        }
        if property.type_ == incr && matches!(self, Self::Converting) && property.format == 32 {
            // Deleting the INCR property tells its owner to send the first chunk.
            *self = Self::Chunks(Vec::new());
            return None;
        }
        if property.type_ != utf8 || property.format != 8 {
            return Some(Err(()));
        }
        let bytes = match self {
            Self::Converting => property.value,
            Self::Chunks(bytes) if property.value.is_empty() => std::mem::take(bytes),
            Self::Chunks(bytes) => {
                if property.value.len() > CLIPBOARD_MAX_BYTES - bytes.len() {
                    return Some(Err(()));
                }
                bytes.extend(property.value);
                return None;
            }
        };
        if bytes.is_empty() {
            return Some(Err(()));
        }
        Some(String::from_utf8(bytes).map_err(|_| ()))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    mod native;

    use super::*;

    const UTF8: Atom = 9;
    const INCR: Atom = 10;

    fn property(type_: Atom, format: u8, value: Vec<u8>) -> GetPropertyReply {
        GetPropertyReply {
            type_,
            format,
            value,
            ..GetPropertyReply::default()
        }
    }

    #[test]
    fn only_complete_utf8_text_is_forwarded() {
        let mut copy = Transfer::Converting;
        assert_eq!(
            copy.receive(property(UTF8, 8, b"hi".to_vec()), UTF8, INCR),
            Some(Ok("hi".to_owned()))
        );
        for invalid in [
            property(UTF8, 8, vec![]),
            property(UTF8, 8, vec![255]),
            property(UTF8, 16, vec![42]),
            property(42, 8, vec![42]),
            property(INCR, 8, vec![42]),
            GetPropertyReply {
                bytes_after: 1,
                ..property(UTF8, 8, vec![42])
            },
        ] {
            assert_eq!(copy.receive(invalid, UTF8, INCR), Some(Err(())));
        }
    }

    #[test]
    fn incremental_text_reassembles_split_characters_and_stops_at_the_byte_limit() {
        let mut copy = Transfer::Converting;
        assert_eq!(
            copy.receive(property(INCR, 32, vec![0; 4]), UTF8, INCR),
            None
        );
        for chunk in [[0xf0, 0x9f], [0x98, 0x80]] {
            assert_eq!(
                copy.receive(property(UTF8, 8, chunk.to_vec()), UTF8, INCR),
                None
            );
        }
        assert_eq!(
            copy.receive(property(UTF8, 8, vec![]), UTF8, INCR),
            Some(Ok("😀".to_owned()))
        );
        let mut copy = Transfer::Chunks(vec![b'a'; CLIPBOARD_MAX_BYTES - 1]);
        assert_eq!(
            copy.receive(property(UTF8, 8, vec![b'b']), UTF8, INCR),
            None
        );
        assert_eq!(
            copy.receive(property(UTF8, 8, vec![b'c']), UTF8, INCR),
            Some(Err(()))
        );
    }
}
