//! Fresh CLIPBOARD transfers on a sandbox's private display. Each click gets an unmapped
//! requestor window so late X events cannot complete a newer viewer's request.

use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, Property,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use yulius_contracts::browser::CLIPBOARD_MAX_BYTES;

const CLICK_WINDOW: Duration = Duration::from_secs(3);
const OFFER_WINDOW: Duration = Duration::from_secs(15);

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
