//! Auto-saved notes (SPEC-041 NOT-003..007): one notepad per sandbox and one for the team, stored
//! with revisions; a stale revision is refused, readers read and writers edit sandbox notes, every
//! signed-in person edits team notes, and each stored edit is announced to the people who may read it.
//! Personal notes (SPEC-103 NOT-011..013) are each person's own, in every sandbox, and their edits
//! reach only their owner.
//! Presence (SPEC-112 NOT-018..019): who has a shared notepad open and where their selection is,
//! relayed to its readers and stored nowhere, at most 20 updates a second per person and notepad.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{TestResult, call, harness, perms, seed_github_user, seed_sandbox};
use yulius_contracts::api::notes::NoteSelection;
use yulius_contracts::events::UiEvent;

#[tokio::test]
async fn sandbox_notes_follow_github_access() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("alice", "acme/other", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));

    // Never written: empty at revision 0.
    let empty = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/notes",
        None,
        &alice,
    )
    .await?;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.body);
    assert_eq!(empty.body["scope"], "sandbox");
    assert_eq!(empty.body["body"], "");
    assert_eq!(empty.body["revision"], 0);
    assert!(empty.body["updated_by"].is_null());

    // A writer saves; the note comes back at revision 1 with who saved it.
    let saved = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes",
        Some(json!({"body": "Deploy key rotates on Friday.", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(saved.body["revision"], 1);
    assert_eq!(saved.body["body"], "Deploy key rotates on Friday.");
    assert_eq!(saved.body["updated_by"], "alice");
    assert!(saved.body["updated_at"].as_str().is_some());

    // A reader reads it but may not edit it.
    let read = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/notes",
        None,
        &bob,
    )
    .await?;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.body["body"], "Deploy key rotates on Friday.");
    let refused = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes",
        Some(json!({"body": "mine now", "revision": 1})),
        &bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);

    // Without access the sandbox does not exist, for reading and for saving; nor does a bad id.
    let hidden = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/notes",
        None,
        &carol,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let hidden = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes",
        Some(json!({"body": "x", "revision": 1})),
        &carol,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    for path in ["/api/sandboxes/no%20such/notes", "/api/sandboxes/s9/notes"] {
        let missing = call(&h.router, Method::GET, path, None, &alice).await?;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{path}");
    }

    // Each sandbox has its own notepad.
    let other = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s2/notes",
        None,
        &alice,
    )
    .await?;
    assert_eq!(other.body["body"], "");
    assert_eq!(other.body["revision"], 0);
    Ok(())
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_stores_nothing() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let path = "/api/sandboxes/s1/notes";

    let first = call(
        &h.router,
        Method::PUT,
        path,
        Some(json!({"body": "one", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(first.body["revision"], 1);
    let second = call(
        &h.router,
        Method::PUT,
        path,
        Some(json!({"body": "two", "revision": 1})),
        &alice,
    )
    .await?;
    assert_eq!(second.body["revision"], 2);

    // Edited from revision 1 while the note is at 2 (someone else saved meanwhile).
    for stale in [0, 1, 3] {
        let conflict = call(
            &h.router,
            Method::PUT,
            path,
            Some(json!({"body": "late", "revision": stale})),
            &alice,
        )
        .await?;
        assert_eq!(
            conflict.status,
            StatusCode::CONFLICT,
            "revision {stale}: {:?}",
            conflict.body
        );
        assert_eq!(conflict.body["error"]["code"], "conflict");
    }
    let current = call(&h.router, Method::GET, path, None, &alice).await?;
    assert_eq!(current.body["body"], "two");
    assert_eq!(current.body["revision"], 2);
    Ok(())
}

#[tokio::test]
async fn team_notes_are_shared_by_everyone_signed_in() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    // Bob can see no sandbox at all and still shares the team's notes.
    let bob = seed_github_user(&h, "u-bob", "bob").await?;

    let empty = call(&h.router, Method::GET, "/api/notes", None, &bob).await?;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.body);
    assert_eq!(empty.body["scope"], "team");
    assert_eq!(empty.body["revision"], 0);

    let saved = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": "Standup at 10.", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    let edited = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": "Standup at 10:30.", "revision": 1})),
        &bob,
    )
    .await?;
    assert_eq!(edited.status, StatusCode::OK, "{:?}", edited.body);
    assert_eq!(edited.body["updated_by"], "bob");

    let read = call(&h.router, Method::GET, "/api/notes", None, &alice).await?;
    assert_eq!(read.body["body"], "Standup at 10:30.");
    assert_eq!(read.body["revision"], 2);

    // Signed out: no notes.
    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/notes",
        None,
        &support::Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_note_is_bounded() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let limit = 256 * 1024;
    let fits = "é".repeat(limit / 2);
    let saved = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": fits, "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body["error"]);

    let too_big = format!("{fits}x");
    let refused = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": too_big, "revision": 1})),
        &alice,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        refused.body["error"]
    );
    assert_eq!(refused.body["error"]["code"], "invalid");
    let current = call(&h.router, Method::GET, "/api/notes", None, &alice).await?;
    assert_eq!(current.body["revision"], 1);

    // A request body past the route's limit (2 MiB) is refused before it is read.
    let huge = "x".repeat(3 << 20);
    let refused = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": huge, "revision": 1})),
        &alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);

    // Unknown fields are refused rather than ignored.
    let odd = call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": "x", "revision": 1, "title": "t"})),
        &alice,
    )
    .await?;
    assert!(odd.status.is_client_error(), "{:?}", odd.status);
    Ok(())
}

#[tokio::test]
async fn an_edit_is_announced_to_viewers() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let mut events = h.app.events.subscribe();

    call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes",
        Some(json!({"body": "a", "revision": 0})),
        &alice,
    )
    .await?;
    let envelope = events.try_recv()?;
    assert_eq!(
        envelope
            .sandbox
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("s1")
    );
    assert!(!envelope.admin_only);
    assert_eq!(
        envelope.event,
        UiEvent::NoteUpdated {
            scope: "sandbox".into(),
            sandbox_id: Some("s1".into()),
            revision: 1,
            updated_by: "alice".into(),
        }
    );

    call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": "b", "revision": 0})),
        &alice,
    )
    .await?;
    let envelope = events.try_recv()?;
    assert!(
        envelope.sandbox.is_none(),
        "team notes reach everyone signed in"
    );
    assert_eq!(
        envelope.event,
        UiEvent::NoteUpdated {
            scope: "team".into(),
            sandbox_id: None,
            revision: 1,
            updated_by: "alice".into()
        }
    );

    assert!(
        envelope.user.is_none(),
        "team notes are not addressed to one person"
    );

    call(
        &h.router,
        Method::PUT,
        "/api/me/notes",
        Some(json!({"body": "mine", "revision": 0})),
        &alice,
    )
    .await?;
    let envelope = events.try_recv()?;
    assert!(
        envelope.sandbox.is_none(),
        "personal notes are in no sandbox"
    );
    assert_eq!(
        envelope.user.as_ref().map(ToString::to_string).as_deref(),
        Some("u-alice"),
        "a personal note's event is addressed to its owner"
    );
    assert_eq!(
        envelope.event,
        UiEvent::NoteUpdated {
            scope: "me".into(),
            sandbox_id: None,
            revision: 1,
            updated_by: "alice".into()
        }
    );

    // A refused save announces nothing.
    call(
        &h.router,
        Method::PUT,
        "/api/notes",
        Some(json!({"body": "c", "revision": 0})),
        &alice,
    )
    .await?;
    assert!(events.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn a_personal_edit_reaches_only_its_owner() -> TestResult {
    use futures::StreamExt as _;
    use support::{serve, setup_superadmin_creds, superadmin_bearer, ws_connect};
    use tokio_tungstenite::tungstenite::Message;
    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    // The superadmin sees everything else, and still not someone's own notes.
    setup_superadmin_creds(&h).await?;
    let admin = superadmin_bearer(&h).await?;
    let served = serve(h.router.clone()).await?;
    let origin = h.app.config.public_url.clone();
    let mut alices = ws_connect(served.addr, "/api/events", &alice, &origin).await?;
    let mut bobs = ws_connect(served.addr, "/api/events", &bob, &origin).await?;
    let mut admins = ws_connect(served.addr, "/api/events", &admin, &origin).await?;
    let mark = |task: &str| {
        h.app.events.publish(
            None,
            false,
            UiEvent::TaskRemoved {
                task_id: task.into(),
            },
        );
    };
    // The text frames up to and including the one naming `marker`.
    async fn until(socket: &mut Socket, marker: &str) -> Result<Vec<String>, String> {
        let mut frames = Vec::new();
        loop {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
                .await
                .map_err(|_| format!("no {marker} within 10 s: {frames:?}"))?;
            match next {
                Some(Ok(Message::Text(text))) => {
                    let text = text.to_string();
                    let done = text.contains(marker);
                    frames.push(text);
                    if done {
                        return Ok(frames);
                    }
                }
                Some(Ok(_)) => {}
                other => return Err(format!("stream ended: {other:?}")),
            }
        }
    }

    // The server subscribes once the upgrade completes: mark until every stream has it.
    for (round, socket) in [&mut alices, &mut bobs, &mut admins]
        .into_iter()
        .enumerate()
    {
        let marker = format!("t-ready-{round}");
        let mut ready = false;
        for _ in 0..40 {
            mark(&marker);
            if let Ok(Some(Ok(Message::Text(text)))) =
                tokio::time::timeout(std::time::Duration::from_millis(250), socket.next()).await
                && text.contains(&marker)
            {
                ready = true;
                break;
            }
        }
        assert!(ready, "stream {round} never subscribed");
    }

    let saved = call(
        &h.router,
        Method::PUT,
        "/api/me/notes",
        Some(json!({"body": "mine alone", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    // Events keep their order, so whatever the save announced arrives before this marker.
    mark("t-after");

    let owners = until(&mut alices, "t-after").await?;
    assert!(
        owners
            .iter()
            .any(|f| f.contains(r#""type":"note.updated""#) && f.contains(r#""scope":"me""#)),
        "the owner's stream: {owners:?}"
    );
    for (who, socket) in [("bob", &mut bobs), ("the superadmin", &mut admins)] {
        let others = until(socket, "t-after").await?;
        assert!(
            !others.iter().any(|f| f.contains("note.updated")),
            "{who}'s stream: {others:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn personal_notes_are_each_persons_own() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    // Bob can see no sandbox at all and still has his own notepad.
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));

    let empty = call(&h.router, Method::GET, "/api/me/notes", None, &alice).await?;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.body);
    assert_eq!(empty.body["scope"], "me");
    assert_eq!(empty.body["body"], "");
    assert_eq!(empty.body["revision"], 0);

    let saved = call(
        &h.router,
        Method::PUT,
        "/api/me/notes",
        Some(json!({"body": "grep instead of read?", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(saved.body["scope"], "me");
    assert_eq!(saved.body["revision"], 1);
    assert_eq!(saved.body["updated_by"], "alice");

    // Another person reads only their own notepad and saving it leaves Alice's alone.
    let other = call(&h.router, Method::GET, "/api/me/notes", None, &bob).await?;
    assert_eq!(other.status, StatusCode::OK, "{:?}", other.body);
    assert_eq!(other.body["body"], "");
    assert_eq!(other.body["revision"], 0);
    let bobs = call(
        &h.router,
        Method::PUT,
        "/api/me/notes",
        Some(json!({"body": "Bob's own", "revision": 0})),
        &bob,
    )
    .await?;
    assert_eq!(bobs.status, StatusCode::OK, "{:?}", bobs.body);
    let read = call(&h.router, Method::GET, "/api/me/notes", None, &alice).await?;
    assert_eq!(read.body["body"], "grep instead of read?");
    assert_eq!(read.body["revision"], 1);

    // Not the team's nor a sandbox's notepad.
    let team = call(&h.router, Method::GET, "/api/notes", None, &alice).await?;
    assert_eq!(team.body["revision"], 0);
    let sandbox = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/notes",
        None,
        &alice,
    )
    .await?;
    assert_eq!(sandbox.body["revision"], 0);

    // The same compare-and-set as the shared notepads.
    let stale = call(
        &h.router,
        Method::PUT,
        "/api/me/notes",
        Some(json!({"body": "late", "revision": 0})),
        &alice,
    )
    .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{:?}", stale.body);

    // Signed out: no notes.
    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/me/notes",
        None,
        &support::Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// Drains the events published so far.
fn published(
    events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
) -> Vec<yulius_app::events::Envelope> {
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

#[tokio::test]
async fn presence_is_relayed_to_readers() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let mut events = h.app.events.subscribe();

    // Team: relayed to everyone signed in, naming who it is, stored nowhere.
    let sent = call(
        &h.router,
        Method::PUT,
        "/api/notes/presence",
        Some(json!({"revision": 3, "selection": {"anchor": 5, "head": 2}, "hello": true, "client": "tab1", "seq": 7})),
        &alice,
    )
    .await?;
    assert_eq!(sent.status, StatusCode::NO_CONTENT, "{:?}", sent.body);
    let envelope = events.try_recv()?;
    assert!(envelope.sandbox.is_none() && envelope.user.is_none() && !envelope.admin_only);
    assert_eq!(
        envelope.event,
        UiEvent::NotePresence {
            scope: "team".into(),
            sandbox_id: None,
            user_id: "u-alice".into(),
            client: "tab1".into(),
            name: "alice".into(),
            revision: 3,
            selection: Some(NoteSelection { anchor: 5, head: 2 }),
            hello: true,
            seq: 7,
        }
    );
    let note = call(&h.router, Method::GET, "/api/notes", None, &alice).await?;
    assert_eq!(note.body["revision"], 0, "presence stores nothing");

    // A sandbox's: read access is enough, and only its viewers get it.
    let left = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes/presence",
        Some(json!({"revision": 0, "selection": null})),
        &bob,
    )
    .await?;
    assert_eq!(left.status, StatusCode::NO_CONTENT, "{:?}", left.body);
    let envelope = events.try_recv()?;
    assert_eq!(
        envelope
            .sandbox
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("s1")
    );
    assert_eq!(
        envelope.event,
        UiEvent::NotePresence {
            scope: "sandbox".into(),
            sandbox_id: Some("s1".into()),
            user_id: "u-bob".into(),
            client: String::new(),
            name: "bob".into(),
            revision: 0,
            selection: None,
            hello: false,
            // Without `seq` (an older page): 0, which receivers never drop (SPEC-170 TKS-003).
            seq: 0,
        }
    );

    // Refused, and nothing relayed: no access, unknown fields, offsets out of bounds, signed out,
    // and "Me", which nobody else reads.
    let hidden = call(
        &h.router,
        Method::PUT,
        "/api/sandboxes/s1/notes/presence",
        Some(json!({"revision": 0, "selection": null})),
        &carol,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    for body in [
        json!({"revision": 0, "selection": null, "colour": "red"}),
        json!({"revision": 0, "selection": null, "seq": -1}),
        json!({"revision": 0, "selection": {"anchor": 1, "head": 2, "x": 1}}),
        json!({"revision": 0, "selection": {"anchor": 0, "head": 1_048_577}}),
        json!({"revision": 0, "selection": null, "client": "a".repeat(33)}),
        json!({"revision": 0, "selection": null, "client": "tab one"}),
        json!({"selection": null}),
    ] {
        let refused = call(
            &h.router,
            Method::PUT,
            "/api/notes/presence",
            Some(body.clone()),
            &alice,
        )
        .await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let anonymous = call(
        &h.router,
        Method::PUT,
        "/api/notes/presence",
        Some(json!({"revision": 0, "selection": null})),
        &support::Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let personal = call(
        &h.router,
        Method::PUT,
        "/api/me/notes/presence",
        Some(json!({"revision": 0, "selection": null})),
        &alice,
    )
    .await?;
    assert!(personal.status.is_client_error(), "{:?}", personal.status);
    assert!(published(&mut events).is_empty());
    Ok(())
}

#[tokio::test]
async fn presence_is_rate_limited() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let mut events = h.app.events.subscribe();
    let send = |path: &'static str| {
        call(
            &h.router,
            Method::PUT,
            path,
            Some(json!({"revision": 1, "selection": {"anchor": 0, "head": 0}})),
            &alice,
        )
    };

    for _ in 0..20 {
        assert_eq!(
            send("/api/notes/presence").await?.status,
            StatusCode::NO_CONTENT
        );
    }
    let refused = send("/api/notes/presence").await?;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(published(&mut events).len(), 20, "the 21st is not relayed");

    // Each notepad counts on its own, and the next second starts afresh.
    assert_eq!(
        send("/api/sandboxes/s1/notes/presence").await?.status,
        StatusCode::NO_CONTENT
    );
    h.clock
        .0
        .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        send("/api/notes/presence").await?.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(published(&mut events).len(), 2);
    Ok(())
}
