//! Event ordering on a private test display. Delayed events from the preceding request and
//! X's property-deletion notifications must never complete the current clipboard transfer.

use super::super::*;
use crate::{DisplayConfig, DisplayMode, StopTimeouts, VirtualDisplay, XAUTHORITY_FILE, xserver};
use tokio::sync::oneshot::error::TryRecvError;
use x11rb::protocol::{xfixes, xproto};
use x11rb::wrapper::ConnectionExt as _;
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
