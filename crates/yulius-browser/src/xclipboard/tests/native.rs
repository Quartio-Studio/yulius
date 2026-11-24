//! Event ordering on a private test display. Delayed events from the preceding request and
//! X's property-deletion notifications must never complete the current clipboard transfer.

use super::super::*;
use crate::{DisplayConfig, DisplayMode, StopTimeouts, VirtualDisplay, XAUTHORITY_FILE, xserver};
use tokio::sync::oneshot::error::TryRecvError;
use x11rb::protocol::{xfixes, xproto};
use yulius_domain::ids::SandboxId;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn watch(clip: &mut Clipboard, conn: &RustConnection) -> Result<(Window, oneshot::Receiver<u32>)> {
    let (send, pending) = oneshot::channel();
    clip.start(conn, conn.setup().roots[0].root, send);
    Ok((
        clip.request.as_ref().ok_or("no clipboard request")?.window,
        pending,
    ))
}

fn begin_conversion(
    clip: &mut Clipboard,
    conn: &RustConnection,
) -> Result<(Window, oneshot::Receiver<String>)> {
    let (window, _offer) = watch(clip, conn)?;
    let (send, text) = oneshot::channel();
    clip.request.as_mut().ok_or("no clipboard request")?.phase =
        Phase::Reading(send, Transfer::Converting);
    Ok((window, text))
}

fn property_change(window: Window, atom: Atom, state: Property) -> Event {
    Event::PropertyNotify(xproto::PropertyNotifyEvent {
        window,
        atom,
        state,
        ..Default::default()
    })
}

fn check_events(conn: &RustConnection) -> Result<()> {
    let mut clip = Clipboard::new(conn).ok_or("XFIXES unavailable")?;
    let root = conn.setup().roots[0].root;
    let (window, mut pending) = watch(&mut clip, conn)?;
    clip.on_event(
        conn,
        &Event::XfixesSelectionNotify(xfixes::SelectionNotifyEvent {
            window: root,
            owner: root,
            selection: clip.selection,
            ..Default::default()
        }),
    );
    assert_eq!(pending.try_recv(), Err(TryRecvError::Empty));
    clip.on_event(
        conn,
        &Event::SelectionNotify(xproto::SelectionNotifyEvent {
            requestor: root,
            ..Default::default()
        }),
    );
    assert_eq!(clip.request.as_ref().map(|r| r.window), Some(window));
    clip.request
        .as_mut()
        .ok_or("clipboard watch ended")?
        .expires = Instant::now();
    clip.expire(conn);
    assert_eq!(pending.try_recv(), Err(TryRecvError::Closed));
    let (_, pending) = watch(&mut clip, conn)?;
    drop(pending);
    clip.expire(conn);
    assert!(clip.request.is_none());

    let answer = |requestor, property, target, selection| {
        Event::SelectionNotify(xproto::SelectionNotifyEvent {
            requestor,
            property,
            target,
            selection,
            ..Default::default()
        })
    };
    let utf8 = clip.utf8;
    let selection = clip.selection;
    for (property, target, atom) in [(0, utf8, selection), (utf8, 0, selection), (utf8, utf8, 0)] {
        let (window, mut text) = begin_conversion(&mut clip, conn)?;
        conn.change_property8(xproto::PropMode::REPLACE, window, utf8, utf8, b"fixture")?
            .check()?;
        clip.on_event(conn, &answer(window, property, target, atom));
        assert_eq!(text.try_recv(), Err(TryRecvError::Closed));
    }

    let (window, mut text) = begin_conversion(&mut clip, conn)?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        utf8,
        utf8,
        b"before notify",
    )?
    .check()?;
    clip.on_event(conn, &property_change(window, utf8, Property::NEW_VALUE));
    assert_eq!(text.try_recv(), Err(TryRecvError::Empty));
    conn.change_property32(xproto::PropMode::REPLACE, window, utf8, clip.incr, &[7])?
        .check()?;
    clip.on_event(conn, &answer(window, utf8, utf8, selection));
    conn.change_property8(xproto::PropMode::REPLACE, window, utf8, utf8, b"chunked")?
        .check()?;
    for event in [
        property_change(root, utf8, Property::NEW_VALUE),
        property_change(window, selection, Property::NEW_VALUE),
        property_change(window, utf8, Property::DELETE),
    ] {
        clip.on_event(conn, &event);
        assert!(matches!(clip.request.as_ref().map(|r| &r.phase),
            Some(Phase::Reading(_, Transfer::Chunks(bytes))) if bytes.is_empty()));
    }
    clip.on_event(conn, &property_change(window, utf8, Property::NEW_VALUE));
    conn.change_property8(xproto::PropMode::REPLACE, window, utf8, utf8, b"")?
        .check()?;
    clip.on_event(conn, &property_change(window, utf8, Property::NEW_VALUE));
    assert_eq!(text.try_recv()?.as_str(), "chunked");
    Ok(())
}

#[tokio::test]
async fn expiry_and_late_x_events_preserve_the_current_transfer() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let sandbox = SandboxId::parse("clipboard-events")?;
    let mut config = DisplayConfig::new(dir.path().to_owned());
    if crate::resolve_executable(&config.xvfb).is_err() {
        eprintln!("Xvfb unavailable; clipboard event ordering check did not run");
        return Ok(());
    }
    config.max_size = (320, 240);
    let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
        .await?
        .ok_or("no test display")?;
    let auth = std::fs::read(dir.path().join(sandbox.as_str()).join(XAUTHORITY_FILE))?;
    let number = display.name().trim_start_matches(':').parse()?;
    let result = check_events(&xserver::connect(number, &auth[auth.len() - 16..])?);
    display.stop(StopTimeouts::default()).await?;
    result
}

/// SPEC-639 PST-004, PST-007: the paste owner refuses what it does not hold, hears when another
/// client takes the CLIPBOARD, and lets go of its text after the hold, never of another's.
fn check_owner(conn: &RustConnection, other: &RustConnection) -> Result<()> {
    let root = conn.setup().roots[0].root;
    let mut owner = Owner::new(conn, root).ok_or("no paste owner")?;
    let (window, selection, utf8, plain) = (owner.window, owner.selection, owner.utf8, owner.plain);
    let owned_by = |by: Window| -> Result<bool> {
        Ok(other.get_selection_owner(selection)?.reply()?.owner == by)
    };
    let request = |target| {
        Event::SelectionRequest(xproto::SelectionRequestEvent {
            owner: window,
            requestor: root,
            selection,
            target,
            property: utf8,
            ..Default::default()
        })
    };
    let (send, mut outcome) = oneshot::channel();
    owner
        .take(conn, "pasted", send)
        .map_err(|_| "the CLIPBOARD was not taken")?;
    assert!(owned_by(window)?);
    // A target it does not offer is refused and does not count as read, and neither do its
    // targets: the text is what the page gets.
    let html = conn.intern_atom(false, b"text/html")?.reply()?.atom;
    let targets = conn.intern_atom(false, b"TARGETS")?.reply()?.atom;
    for target in [html, targets] {
        owner.on_event(conn, &request(target));
        assert_eq!(outcome.try_recv(), Err(TryRecvError::Empty));
    }
    owner.on_event(conn, &request(plain));
    assert_eq!(outcome.try_recv(), Ok(PasteOutcome::Read));
    // A newer paste waits while the client still reads (it asked for the text or its targets
    // less than the quiet ago), and no longer.
    assert!(owner.reading());
    owner.asked = Instant::now().checked_sub(PASTE_QUIET);
    assert!(!owner.reading());
    let value = conn
        .get_property(true, root, utf8, AtomEnum::ANY, 0, 64)?
        .reply()?;
    assert_eq!(
        (value.type_, value.value.as_slice()),
        (plain, &b"pasted"[..])
    );
    owner.on_event(conn, &request(targets));
    assert!(owner.reading(), "the targets are a read too");

    // Another client (the page copying) takes the CLIPBOARD: the text is let go, and the wait
    // ends unread.
    let (send, mut outcome) = oneshot::channel();
    owner
        .take(conn, "second", send)
        .map_err(|_| "the CLIPBOARD was not taken")?;
    let copier = other.generate_id()?;
    other
        .create_window(
            0,
            copier,
            root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new(),
        )?
        .check()?;
    other
        .set_selection_owner(copier, selection, x11rb::CURRENT_TIME)?
        .check()?;
    owner.on_event(
        conn,
        &Event::SelectionClear(xproto::SelectionClearEvent {
            owner: window,
            selection,
            ..Default::default()
        }),
    );
    assert_eq!(outcome.try_recv(), Ok(PasteOutcome::Unread));
    assert!(owner.held.is_none());
    owner.on_event(conn, &request(utf8));
    // A held text past its hold is let go; another client's CLIPBOARD is left alone.
    let (send, _outcome) = oneshot::channel();
    owner
        .take(conn, "third", send)
        .map_err(|_| "the CLIPBOARD was not taken")?;
    other
        .set_selection_owner(copier, selection, x11rb::CURRENT_TIME)?
        .check()?;
    owner.held.as_mut().ok_or("nothing held")?.until = Instant::now();
    owner.expire(conn);
    assert!(owner.held.is_none());
    assert!(owned_by(copier)?, "another client's CLIPBOARD was cleared");
    let (send, mut outcome) = oneshot::channel();
    owner
        .take(conn, "fourth", send)
        .map_err(|_| "the CLIPBOARD was not taken")?;
    let held = owner.held.as_mut().ok_or("nothing held")?;
    held.until = Instant::now();
    held.waiting.as_mut().ok_or("no wait")?.1 = Instant::now();
    owner.expire(conn);
    conn.flush()?;
    assert_eq!(outcome.try_recv(), Ok(PasteOutcome::Unread));
    assert!(owned_by(x11rb::NONE)?, "the held paste was not let go");
    Ok(())
}

#[tokio::test]
async fn the_paste_owner_answers_only_its_text_and_lets_it_go() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let sandbox = SandboxId::parse("paste-owner")?;
    let mut config = DisplayConfig::new(dir.path().to_owned());
    if crate::resolve_executable(&config.xvfb).is_err() {
        eprintln!("Xvfb unavailable; paste owner check did not run");
        return Ok(());
    }
    config.max_size = (320, 240);
    let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
        .await?
        .ok_or("no test display")?;
    let auth = std::fs::read(dir.path().join(sandbox.as_str()).join(XAUTHORITY_FILE))?;
    let number = display.name().trim_start_matches(':').parse()?;
    let cookie = &auth[auth.len() - 16..];
    let result = check_owner(
        &xserver::connect(number, cookie)?,
        &xserver::connect(number, cookie)?,
    );
    display.stop(StopTimeouts::default()).await?;
    result
}
