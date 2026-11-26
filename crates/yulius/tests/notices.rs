//! Notices for people: a key per notice (newest wins per project, recipient and key),
//! an expiry (hidden everywhere once it passes, then pruned), a decision that never expires, and
//! the reads: `GET`/`DELETE /api/me/notices` and the person tools over the remote MCP.

use crate::support;

use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::World;
use support::{Creds, TestResult, call};
use yulius_app::model::UserRecord;
use yulius_app::notices::NewNotice;
use yulius_app::ports::store_ext::NoticeStore as _;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::SandboxId;
use yulius_domain::notice::{Expiry, NoticeKey, NoticeTone};
use yulius_domain::notify::NoticeKind;

type Error = Box<dyn std::error::Error>;
const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

/// Alice writes in `shop`, bob reads it, carol sees nothing; dave is an instance administrator.
struct People {
    w: World,
    shop: SandboxId,
    alice: (UserRecord, Creds, Creds),
    bob: (UserRecord, Creds, Creds),
    carol: (UserRecord, Creds, Creds),
    dave: (UserRecord, Creds, Creds),
}

async fn person(w: &World, login: &str, admin: bool) -> Result<(UserRecord, Creds, Creds), Error> {
    let (user, _) = w.user(login, admin).await?;
    let browser = w.browser(&user).await?;
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "laptop", "scope": "operate"})),
        &browser,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let bearer = Creds {
        bearer: made.body["token"].as_str().map(str::to_owned),
        ..Creds::default()
    };
    Ok((user, browser, bearer))
}

async fn people() -> Result<People, Error> {
    let w = World::new("org").await?;
    let shop = support::seed_sandbox(&w.h, "s1", "shop", "u-alice")
        .await?
        .id;
    let alice = person(&w, "alice", false).await?;
    let bob = person(&w, "bob", false).await?;
    let carol = person(&w, "carol", false).await?;
    let dave = person(&w, "dave", true).await?;
    w.grant("alice", "acme/shop", "push");
    w.grant("bob", "acme/shop", "pull");
    Ok(People {
        w,
        shop,
        alice,
        bob,
        carol,
        dave,
    })
}

fn gate(shop: &SandboxId, test: &str, title: &str) -> Result<NewNotice, Error> {
    Ok(NewNotice {
        link: Some("/s/shop/ci".into()),
        ..NewNotice::new(
            Some(shop.clone()),
            NoticeKey::parse(&format!("gate:{test}"))?,
            NoticeKind::Failed,
            NoticeTone::Error,
            title,
        )
    })
}

/// `GET /api/me/notices` as `(key, title)` pairs, newest first.
async fn listed(p: &People, browser: &Creds) -> Result<Vec<(String, String)>, Error> {
    let r = call(&p.w.h.router, Method::GET, "/api/me/notices", None, browser).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body
        .as_array()
        .ok_or("a list")?
        .iter()
        .map(|n| {
            (
                n["key"].as_str().unwrap_or_default().to_owned(),
                n["title"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect())
}

/// A remote tool's `(is_error, text, record)`.
async fn remote(
    p: &People,
    creds: &Creds,
    name: &str,
    args: Value,
) -> Result<(bool, String, Value), Error> {
    let r = call(
        &p.w.h.router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args, "_meta": {"yulius/structured": true}}})),
        creds,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{name}: {}", r.body);
    let result = &r.body["result"];
    Ok((
        result["isError"] == true,
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        result["structuredContent"].clone(),
    ))
}

fn pair(key: &str, title: &str) -> (String, String) {
    (key.to_owned(), title.to_owned())
}

/// A newer notice with the same key replaces the older one for
/// everyone who sees the project, with a new id, back for whoever dismissed the older one; it
/// reaches only their event streams; another key stays beside it.
#[tokio::test]
async fn the_same_key_replaces_the_previous_notice() -> TestResult {
    let p = people().await?;
    let mut events = p.w.h.app.events.subscribe();
    let first =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "quick::a", "quick::a fails at abc123")?)
            .await?;
    let posted = events.recv().await?;
    assert_eq!(posted.sandbox.as_ref(), Some(&p.shop));
    assert!(matches!(&posted.event, UiEvent::NoticePosted { notice } if notice.id == first.id));
    for (who, sees) in [(&p.alice, true), (&p.bob, true), (&p.carol, false)] {
        assert_eq!(
            p.w.h
                .app
                .envelope_reaches(&who.0, &posted, &mut yulius_app::events::Reach::default())
                .await,
            sees,
            "{}",
            who.0.id
        );
    }
    let dismissed = call(
        &p.w.h.router,
        Method::DELETE,
        &format!("/api/me/notices/{}", first.id),
        None,
        &p.bob.1,
    )
    .await?;
    assert_eq!(
        dismissed.status,
        StatusCode::NO_CONTENT,
        "{}",
        dismissed.body
    );
    assert!(listed(&p, &p.bob.1).await?.is_empty());

    p.w.h.clock.0.fetch_add(MINUTE_MS, Ordering::SeqCst);
    let second =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "quick::a", "quick::a fails again at def456")?)
            .await?;
    assert_ne!(first.id, second.id);
    p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
    p.w.h
        .app
        .post_notice(gate(&p.shop, "quick::b", "quick::b fails")?)
        .await?;
    let want = vec![
        pair("gate:quick::b", "quick::b fails"),
        pair("gate:quick::a", "quick::a fails again at def456"),
    ];
    assert_eq!(listed(&p, &p.alice.1).await?, want);
    assert_eq!(
        listed(&p, &p.bob.1).await?,
        want,
        "the replacement is unread again"
    );
    assert!(listed(&p, &p.carol.1).await?.is_empty());
    assert_eq!(p.w.h.store.notice(&first.id).await?, None);

    // A notice for one person, and one for administrators, under the same key.
    p.w.h
        .app
        .post_notice(NewNotice {
            to: Some(p.alice.0.id.clone()),
            ..gate(&p.shop, "quick::a", "Yours")?
        })
        .await?;
    p.w.h
        .app
        .post_notice(NewNotice {
            admin_only: true,
            ..NewNotice::new(
                None,
                NoticeKey::parse("account:acc1")?,
                NoticeKind::Account,
                NoticeTone::Warn,
                "A subscription reached its limit",
            )
        })
        .await?;
    assert_eq!(listed(&p, &p.alice.1).await?.len(), 3);
    assert_eq!(listed(&p, &p.bob.1).await?, want);
    assert_eq!(
        listed(&p, &p.dave.1).await?,
        vec![pair("account:acc1", "A subscription reached its limit")]
    );
    Ok(())
}

/// An expired notice is left out of the HTTP read and the person tools at once,
/// and the tick prunes it; one that never expires stays.
#[tokio::test]
async fn an_expired_notice_is_hidden_and_pruned() -> TestResult {
    let p = people().await?;
    let soon =
        p.w.h
            .app
            .post_notice(NewNotice {
                expiry: Some(Expiry::parse("30m")?),
                ..gate(&p.shop, "soon", "Soon gone")?
            })
            .await?;
    p.w.h
        .app
        .post_notice(NewNotice {
            expiry: Some(Expiry::Never),
            ..gate(&p.shop, "stays", "Stays")?
        })
        .await?;
    let (error, said, before) = remote(
        &p,
        &p.bob.2,
        "notices_list",
        json!({"project": "acme/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(
        before["notices"].as_array().map(Vec::len),
        Some(2),
        "{before}"
    );

    p.w.h.clock.0.fetch_add(30 * MINUTE_MS, Ordering::SeqCst);
    assert_eq!(
        listed(&p, &p.bob.1).await?,
        vec![pair("gate:stays", "Stays")]
    );
    let (_, _, after) = remote(
        &p,
        &p.bob.2,
        "notices_list",
        json!({"project": "acme/shop"}),
    )
    .await?;
    assert_eq!(
        after["notices"].as_array().map(Vec::len),
        Some(1),
        "{after}"
    );
    assert_eq!(after["notices"][0]["key"], "gate:stays");
    assert!(
        p.w.h.store.notice(&soon.id).await?.is_some(),
        "hidden before the prune"
    );
    p.w.h.app.prune_notices_now().await;
    assert_eq!(p.w.h.store.notice(&soon.id).await?, None);
    Ok(())
}

/// A decision posted with an expiry is refused; without one it never expires.
#[tokio::test]
async fn a_decision_never_expires_silently() -> TestResult {
    let p = people().await?;
    let decision = |expiry: Option<Expiry>| -> Result<NewNotice, Error> {
        Ok(NewNotice {
            decision: true,
            expiry,
            ..NewNotice::new(
                Some(p.shop.clone()),
                NoticeKey::parse("watch:approve-release")?,
                NoticeKind::NeedsYou,
                NoticeTone::Warn,
                "Approve the release?",
            )
        })
    };
    let refused =
        p.w.h
            .app
            .post_notice(decision(Some(Expiry::parse("30m")?))?)
            .await;
    assert!(
        matches!(&refused, Err(yulius_app::error::AppError::Invalid(said)) if said.contains("decision")),
        "{refused:?}"
    );
    let kept = p.w.h.app.post_notice(decision(None)?).await?;
    assert_eq!(kept.expires_at, None);
    p.w.h.clock.0.fetch_add(90 * DAY_MS, Ordering::SeqCst);
    p.w.h.app.prune_notices_now().await;
    // Ninety days on, the browser's session is gone: the read it serves is asked directly.
    let still = p.w.h.app.notices_for(&p.alice.0, None).await?;
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].id, kept.id);
    Ok(())
}

/// A producer removes a notice and pages hear it; a person dismisses one through their
/// agent for themselves only; a notice they do not see cannot be dismissed.
#[tokio::test]
async fn notices_are_removed_and_dismissed() -> TestResult {
    let p = people().await?;
    let posted =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "quick::a", "Red")?)
            .await?;
    let (error, said, _) = remote(
        &p,
        &p.alice.2,
        "notice_dismiss",
        json!({"project": "acme/shop", "id": posted.id}),
    )
    .await?;
    assert!(!error, "{said}");
    assert!(listed(&p, &p.alice.1).await?.is_empty());
    assert_eq!(
        listed(&p, &p.bob.1).await?.len(),
        1,
        "dismissed for alice only"
    );
    let hidden = call(
        &p.w.h.router,
        Method::DELETE,
        &format!("/api/me/notices/{}", posted.id),
        None,
        &p.carol.1,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);

    let mut events = p.w.h.app.events.subscribe();
    assert!(
        p.w.h
            .app
            .remove_notice(Some(&p.shop), None, &NoticeKey::parse("gate:quick::a")?)
            .await?
    );
    let removed = events.recv().await?;
    assert!(
        matches!(&removed.event, UiEvent::NoticeRemoved { id, key, .. } if *id == posted.id && key == "gate:quick::a")
    );
    assert!(listed(&p, &p.bob.1).await?.is_empty());
    assert!(
        !p.w.h
            .app
            .remove_notice(Some(&p.shop), None, &NoticeKey::parse("gate:quick::a")?)
            .await?
    );
    Ok(())
}

/// A notice with a link outside Yulius or an expiry in the past is
/// refused; a long title is cut.
#[tokio::test]
async fn a_bad_notice_is_refused() -> TestResult {
    let p = people().await?;
    let outside =
        p.w.h
            .app
            .post_notice(NewNotice {
                link: Some("https://elsewhere.example".into()),
                ..gate(&p.shop, "x", "X")?
            })
            .await;
    assert!(matches!(
        outside,
        Err(yulius_app::error::AppError::Invalid(_))
    ));
    let past =
        p.w.h
            .app
            .post_notice(NewNotice {
                expiry: Some(Expiry::parse("2000-01-01T00:00:00Z")?),
                ..gate(&p.shop, "x", "X")?
            })
            .await;
    assert!(matches!(past, Err(yulius_app::error::AppError::Invalid(_))));
    let long =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "x", &"t".repeat(1000))?)
            .await?;
    assert_eq!(long.title.chars().count(), yulius_app::notices::TITLE_MAX);
    Ok(())
}

/// The digest lists a person's live notices with their keys, and leaves out the
/// expired one and the one they dismissed; someone who sees none of them gets no digest.
#[tokio::test]
async fn the_digest_lists_live_notices_only() -> TestResult {
    let p = people().await?;
    p.w.h
        .app
        .post_notice(gate(&p.shop, "live", "Still red")?)
        .await?;
    p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
    p.w.h
        .app
        .post_notice(NewNotice {
            expiry: Some(Expiry::parse("30m")?),
            ..gate(&p.shop, "soon", "Soon gone")?
        })
        .await?;
    p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
    let dismissed =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "seen", "Seen already")?)
            .await?;
    p.w.h.app.dismiss_notice(&p.bob.0, &dismissed.id).await?;
    p.w.h.clock.0.fetch_add(31 * MINUTE_MS, Ordering::SeqCst);

    p.w.h.app.send_digests_now(None).await;
    let digest = call(&p.w.h.router, Method::GET, "/api/me/digest", None, &p.bob.1).await?;
    assert_eq!(digest.status, StatusCode::OK, "{}", digest.body);
    let keys: Vec<&str> = digest.body["notices"]
        .as_array()
        .ok_or("notices")?
        .iter()
        .filter_map(|n| n["key"].as_str())
        .collect();
    assert_eq!(keys, ["gate:live"], "{}", digest.body);
    let alice = call(
        &p.w.h.router,
        Method::GET,
        "/api/me/digest",
        None,
        &p.alice.1,
    )
    .await?;
    let alice_keys: Vec<&str> = alice.body["notices"]
        .as_array()
        .ok_or("notices")?
        .iter()
        .filter_map(|n| n["key"].as_str())
        .collect();
    assert_eq!(alice_keys, ["gate:seen", "gate:live"]);
    let carol = call(
        &p.w.h.router,
        Method::GET,
        "/api/me/digest",
        None,
        &p.carol.1,
    )
    .await?;
    assert_eq!(carol.body, Value::Null, "carol sees no notice and no task");
    Ok(())
}

/// Review round 1: a reader's notices are found behind many newer ones of a project they cannot
/// see; the read's limit applies to what they may see.
#[tokio::test]
async fn hidden_notices_never_crowd_out_a_readers_own() -> TestResult {
    let p = people().await?;
    let other = support::seed_sandbox(&p.w.h, "s2", "other", "u-alice")
        .await?
        .id;
    p.w.h
        .app
        .post_notice(gate(&p.shop, "mine", "Bob's project is red")?)
        .await?;
    for n in 0..yulius_app::notices::NOTICES_LISTED {
        p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
        p.w.h
            .app
            .post_notice(gate(&other, &format!("t{n}"), "Elsewhere")?)
            .await?;
    }
    assert_eq!(
        listed(&p, &p.bob.1).await?,
        vec![pair("gate:mine", "Bob's project is red")]
    );
    let (_, _, tool) = remote(
        &p,
        &p.bob.2,
        "notices_list",
        json!({"project": "acme/shop"}),
    )
    .await?;
    assert_eq!(tool["notices"].as_array().map(Vec::len), Some(1), "{tool}");
    Ok(())
}

/// Review round 1: dismissing changes the person's own state: a read-only token may not, and an
/// operate token's call is audited; someone else's personal notice cannot be dismissed.
#[tokio::test]
async fn a_dismissal_is_a_change_of_ones_own() -> TestResult {
    use yulius_app::ports::store_ext::AutomationStore as _;
    let p = people().await?;
    let posted =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "quick::a", "Red")?)
            .await?;
    let made = call(
        &p.w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "reader", "scope": "read"})),
        &p.bob.1,
    )
    .await?;
    let read = Creds {
        bearer: made.body["token"].as_str().map(str::to_owned),
        ..Creds::default()
    };
    let args = json!({"project": "acme/shop", "id": posted.id});
    let (error, said, _) = remote(&p, &read, "notice_dismiss", args.clone()).await?;
    assert!(error && said.contains("read-only"), "{said}");
    assert_eq!(listed(&p, &p.bob.1).await?.len(), 1);
    let (error, said, _) = remote(&p, &p.bob.2, "notice_dismiss", args).await?;
    assert!(!error, "{said}");
    let audited: Vec<String> =
        p.w.h
            .store
            .automation_audit(p.shop.as_str(), 50)
            .await?
            .into_iter()
            .filter(|e| e.action == "remote.tool")
            .filter_map(|e| e.detail)
            .filter(|d| d.contains("notice_dismiss"))
            .collect();
    assert_eq!(audited.len(), 1, "the operate call: {audited:?}");

    let alices =
        p.w.h
            .app
            .post_notice(NewNotice {
                to: Some(p.alice.0.id.clone()),
                ..gate(&p.shop, "yours", "Only for alice")?
            })
            .await?;
    let refused = p.w.h.app.dismiss_notice(&p.bob.0, &alices.id).await;
    assert!(
        matches!(refused, Err(yulius_app::error::AppError::NotFound)),
        "{refused:?}"
    );
    assert_eq!(listed(&p, &p.alice.1).await?.len(), 2);
    Ok(())
}

/// Review round 1: a secret is redacted before a long text is cut, so no prefix of it is kept;
/// a link a browser reads as another host and a key Yulius keeps for its own notices are
/// refused.
#[tokio::test]
async fn texts_are_redacted_whole_and_links_and_keys_checked() -> TestResult {
    const SECRET: &str = "s3cret-notice-value-0a1b2c3d4e5f";
    let p = people().await?;
    let created = call(
        &p.w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "PAYMENT_KEY", "value": SECRET})),
        &p.alice.1,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let straddling = format!(
        "{}{SECRET}",
        "x".repeat(yulius_app::notices::TITLE_MAX - 10)
    );
    let posted =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "long", &straddling)?)
            .await?;
    assert!(!posted.title.contains(&SECRET[..8]), "{}", posted.title);
    for link in [
        "/\\elsewhere.example",
        "/s/shop\n/ci",
        "https://elsewhere.example",
    ] {
        let refused =
            p.w.h
                .app
                .post_notice(NewNotice {
                    link: Some(link.into()),
                    ..gate(&p.shop, "x", "X")?
                })
                .await;
        assert!(
            matches!(refused, Err(yulius_app::error::AppError::Invalid(_))),
            "{link:?}"
        );
    }
    for key in ["task:t1", "control:t1", "needs-you:docker", "digest"] {
        let refused =
            p.w.h
                .app
                .post_notice(NewNotice::new(
                    Some(p.shop.clone()),
                    NoticeKey::parse(key)?,
                    NoticeKind::NeedsYou,
                    NoticeTone::Info,
                    "Mine",
                ))
                .await;
        assert!(
            matches!(refused, Err(yulius_app::error::AppError::Invalid(_))),
            "{key}"
        );
    }
    Ok(())
}

/// Review round 2: pages hear of each notice the cap drops; a notice dropped at once (its scope
/// is full of decisions) is refused, not published; a replacement that narrows who sees the
/// notice tells the others it is gone; a title is one line.
#[tokio::test]
async fn the_cap_and_a_narrowed_replacement_are_heard() -> TestResult {
    use yulius_app::notices::NOTICES_KEPT;
    let p = people().await?;
    for n in 0..NOTICES_KEPT {
        p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
        p.w.h
            .app
            .post_notice(gate(&p.shop, &format!("t{n}"), "Red")?)
            .await?;
    }
    let mut events = p.w.h.app.events.subscribe();
    p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
    p.w.h
        .app
        .post_notice(gate(&p.shop, "newest", "Newest")?)
        .await?;
    let removed = events.recv().await?;
    assert!(
        matches!(&removed.event, UiEvent::NoticeRemoved { key, .. } if key == "gate:t0"),
        "{:?}",
        removed.event
    );
    assert!(matches!(
        events.recv().await?.event,
        UiEvent::NoticePosted { .. }
    ));

    // A replacement for administrators only: the project's other people drop the old one.
    let narrowed = NewNotice {
        admin_only: true,
        ..gate(&p.shop, "newest", "For administrators")?
    };
    p.w.h.app.post_notice(narrowed).await?;
    let gone = events.recv().await?;
    assert!(
        matches!(&gone.event, UiEvent::NoticeRemoved { key, .. } if key == "gate:newest"),
        "{:?}",
        gone.event
    );
    assert!(!gone.admin_only, "the old notice's people hear it");
    assert!(matches!(
        events.recv().await?.event,
        UiEvent::NoticePosted { .. }
    ));

    // A title is one line.
    let lines =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "lines", "first\nsecond\tthird")?)
            .await?;
    assert_eq!(lines.title, "first second third");
    Ok(())
}

/// Review round 2: a scope full of decisions refuses a notice that would be dropped at once.
#[tokio::test]
async fn a_scope_full_of_decisions_refuses_a_report() -> TestResult {
    use yulius_app::notices::NOTICES_KEPT;
    let p = people().await?;
    for n in 0..NOTICES_KEPT {
        p.w.h.clock.0.fetch_add(1, Ordering::SeqCst);
        p.w.h
            .app
            .post_notice(NewNotice {
                decision: true,
                ..gate(&p.shop, &format!("d{n}"), "Decide")?
            })
            .await?;
    }
    let mut events = p.w.h.app.events.subscribe();
    let refused =
        p.w.h
            .app
            .post_notice(gate(&p.shop, "report", "Report")?)
            .await;
    assert!(
        matches!(&refused, Err(yulius_app::error::AppError::Capacity(_))),
        "{refused:?}"
    );
    assert!(events.try_recv().is_err(), "nothing was published");
    Ok(())
}

/// Review round 2: `notice_dismiss` takes only a notice of its project or an instance-wide one,
/// and the Cockpit audits it as a change.
#[tokio::test]
async fn a_dismissal_names_its_own_project() -> TestResult {
    let p = people().await?;
    let other = support::seed_sandbox(&p.w.h, "s2", "other", "u-alice")
        .await?
        .id;
    p.w.grant("alice", "acme/other", "push");
    let elsewhere =
        p.w.h
            .app
            .post_notice(gate(&other, "x", "Elsewhere")?)
            .await?;
    let (error, said, _) = remote(
        &p,
        &p.alice.2,
        "notice_dismiss",
        json!({"project": "acme/shop", "id": elsewhere.id}),
    )
    .await?;
    assert!(error, "a notice of another project: {said}");
    assert_eq!(listed(&p, &p.alice.1).await?.len(), 1);
    let (error, said, _) = remote(
        &p,
        &p.alice.2,
        "notice_dismiss",
        json!({"project": "acme/other", "id": elsewhere.id}),
    )
    .await?;
    assert!(!error, "{said}");
    Ok(())
}

/// Waits (10 s at most) for an event on `events` that `wanted` picks, skipping the others.
async fn next_event<T>(
    events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    wanted: impl Fn(&UiEvent) -> Option<T>,
) -> Result<T, Error> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let envelope = tokio::time::timeout_at(deadline, events.recv())
            .await
            .map_err(|_| "no such event came")??;
        if let Some(found) = wanted(&envelope.event) {
            return Ok(found);
        }
    }
}

fn subscription(
    status: &str,
    resets_at: Option<&str>,
) -> yulius_contracts::api::admin::AccountView {
    use yulius_contracts::api::admin::{AccountUsageView, AccountView, UsageWindowView};
    AccountView {
        id: "acc1".into(),
        provider: "anthropic".into(),
        label: "Team Max".into(),
        status: status.into(),
        usage: AccountUsageView {
            primary: resets_at.map(|at| UsageWindowView {
                used_percent: Some(100.0),
                resets_at: Some(at.into()),
                window_minutes: Some(300),
            }),
            ..AccountUsageView::default()
        },
        last_error: None,
        email: None,
        email_typed: false,
        in_use_by: 0,
        tokens_24h: None,
        tokens_7d: None,
        billing: None,
        usage_read: None,
    }
}

fn publish_subscription(p: &People, view: yulius_contracts::api::admin::AccountView) {
    p.w.h.app.events.publish(
        None,
        true,
        UiEvent::AccountUpdated {
            account: Box::new(view),
        },
    );
}

/// A subscription at its limit is one notice `account:<id>` for administrators
/// that expires at its reset; the same state again posts nothing; another reset replaces it; a
/// usable subscription removes it; one that must sign in again never expires.
#[tokio::test]
async fn a_subscription_problem_is_one_keyed_notice() -> TestResult {
    let p = people().await?;
    let mut events = p.w.h.app.events.subscribe();
    p.w.h.app.start_notice_producers();
    let reset = "2026-01-01T05:00:00Z";
    p.w.h.clock.0.store(
        yulius_domain::time::parse_rfc3339("2026-01-01T01:00:00Z")
            .ok_or("time")?
            .0,
        Ordering::SeqCst,
    );
    publish_subscription(&p, subscription("limited", Some(reset)));
    let posted = next_event(&mut events, |e| match e {
        UiEvent::NoticePosted { notice } if notice.key == "account:acc1" => Some(notice.clone()),
        _ => None,
    })
    .await?;
    assert_eq!(
        (
            posted.key.as_str(),
            posted.title.as_str(),
            posted.expires_at.as_deref()
        ),
        (
            "account:acc1",
            "Team Max reached its usage limit",
            Some("2026-01-01T05:00:00Z")
        )
    );
    // People of the project cannot see it; the administrator can, through the session-bound
    // read (the browser's session is younger than the clock this test set).
    assert!(p.w.h.app.notices_for(&p.bob.0, None).await?.is_empty());
    let ours = |list: Vec<yulius_app::ports::store_ext::NoticeRecord>| {
        list.into_iter()
            .filter(|n| n.key.as_str() == "account:acc1")
            .count()
    };
    assert_eq!(ours(p.w.h.app.notices_for(&p.dave.0, None).await?), 1);

    // The same state again is no news; a later reset replaces the notice.
    publish_subscription(&p, subscription("limited", Some(reset)));
    publish_subscription(&p, subscription("limited", Some("2026-01-01T06:00:00Z")));
    let replaced = next_event(&mut events, |e| match e {
        UiEvent::NoticePosted { notice } if notice.key == "account:acc1" => Some(notice.clone()),
        _ => None,
    })
    .await?;
    assert_eq!(replaced.expires_at.as_deref(), Some("2026-01-01T06:00:00Z"));
    assert_eq!(
        ours(p.w.h.app.notices_for(&p.dave.0, None).await?),
        1,
        "one notice per subscription"
    );

    // Usable again: removed.
    publish_subscription(&p, subscription("ok", None));
    let removed = next_event(&mut events, |e| match e {
        UiEvent::NoticeRemoved { key, .. } if key == "account:acc1" => Some(key.clone()),
        _ => None,
    })
    .await?;
    assert_eq!(removed, "account:acc1");
    assert!(
        !p.w.h
            .app
            .notices_for(&p.dave.0, None)
            .await?
            .iter()
            .any(|n| n.key.as_str() == "account:acc1")
    );

    // Signing in again waits for a person: a decision without an expiry.
    publish_subscription(&p, subscription("relogin", None));
    let relogin = next_event(&mut events, |e| match e {
        UiEvent::NoticePosted { notice } if notice.key == "account:acc1" => Some(notice.clone()),
        _ => None,
    })
    .await?;
    assert!(
        relogin.decision && relogin.expires_at.is_none(),
        "{relogin:?}"
    );
    Ok(())
}

/// Review of #1711: a subscription's status written without an `account.updated` (a usage
/// reading, a refused sign-in) is reconciled by the tick; a removed subscription's notice goes;
/// a state whose texts posting normalises (a redacted error, repeated spaces) is posted once.
#[tokio::test]
async fn subscription_notices_follow_the_stored_state() -> TestResult {
    use yulius_app::model::{AccountRecord, AccountStatus};
    use yulius_app::ports::store::AccountStore as _;
    use yulius_domain::ids::AccountId;
    let p = people().await?;
    let id = AccountId::parse("acc2")?;
    let secret = concat!("ghp_", "Zx9Notice00Account1Fixture2Body34567");
    p.w.h
        .store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "Team  Two".into(),
            credential: None,
            status: AccountStatus::Error,
            usage_json: None,
            last_error: Some(format!("token {secret} refused")),
            created_at: p.w.h.app.now(),
            updated_at: p.w.h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let ours = |list: Vec<yulius_app::ports::store_ext::NoticeRecord>| {
        list.into_iter()
            .filter(|n| n.key.as_str() == "account:acc2")
            .map(|n| (n.id, n.title, n.detail))
            .collect::<Vec<_>>()
    };
    p.w.h.app.sync_all_account_notices().await;
    let first = ours(p.w.h.app.notices_for(&p.dave.0, None).await?);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].1, "Team Two reports an error");
    assert!(
        !first[0].2.clone().unwrap_or_default().contains("Zx9Notice"),
        "{first:?}"
    );
    p.w.h.app.sync_all_account_notices().await;
    assert_eq!(
        ours(p.w.h.app.notices_for(&p.dave.0, None).await?),
        first,
        "the same state, normalised by posting, is no news"
    );

    // Usable again, written without an event: the tick's reconcile removes the notice.
    p.w.h
        .store
        .set_account_state(&id, AccountStatus::Ok, None, None, p.w.h.app.now())
        .await?;
    p.w.h.app.sync_all_account_notices().await;
    assert!(ours(p.w.h.app.notices_for(&p.dave.0, None).await?).is_empty());

    // A refused sign-in, then the subscription removed: its decision does not stay for good.
    p.w.h
        .store
        .set_account_state(&id, AccountStatus::Relogin, None, None, p.w.h.app.now())
        .await?;
    p.w.h.app.sync_all_account_notices().await;
    assert_eq!(ours(p.w.h.app.notices_for(&p.dave.0, None).await?).len(), 1);
    p.w.h.store.delete_account(&id).await?;
    p.w.h.app.sync_all_account_notices().await;
    assert!(ours(p.w.h.app.notices_for(&p.dave.0, None).await?).is_empty());
    Ok(())
}

/// Review round 2 of #1711 (the reviewer's probe): a limit's notice ends at its reset and is not
/// posted again by the tick while the stored status still reads limited (no usage reading yet);
/// the next state of it is told as it comes.
#[tokio::test]
async fn a_limit_past_its_reset_is_not_posted_again() -> TestResult {
    use yulius_app::model::{AccountRecord, AccountStatus};
    use yulius_app::ports::store::AccountStore as _;
    use yulius_domain::ids::AccountId;
    let p = people().await?;
    let start = yulius_domain::time::parse_rfc3339("2026-01-01T01:00:00Z").ok_or("time")?;
    let reset = yulius_domain::time::parse_rfc3339("2026-01-01T05:00:00Z").ok_or("time")?;
    p.w.h.clock.0.store(start.0, Ordering::SeqCst);
    let id = AccountId::parse("acc3")?;
    let usage = format!(
        "{{\"primary\":{{\"used_percent\":100.0,\"window_minutes\":300,\"resets_at\":{}}},\"limit_reached\":false,\"observed_at\":{}}}",
        reset.0, start.0
    );
    p.w.h
        .store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "Team Three".into(),
            credential: None,
            status: AccountStatus::Limited,
            usage_json: Some(usage),
            last_error: None,
            created_at: p.w.h.app.now(),
            updated_at: p.w.h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let ours = |list: Vec<yulius_app::ports::store_ext::NoticeRecord>| {
        list.into_iter()
            .filter(|n| n.key.as_str() == "account:acc3")
            .map(|n| (n.id, n.title, n.expires_at.map(|t| t.0)))
            .collect::<Vec<_>>()
    };
    p.w.h.app.sync_all_account_notices().await;
    let first = ours(p.w.h.app.notices_for(&p.dave.0, None).await?);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].2, Some(reset.0));
    // Thirty seconds past the reset, no usage reading yet: the next tick.
    p.w.h.clock.0.store(reset.0 + 30_000, Ordering::SeqCst);
    p.w.h.app.sync_all_account_notices().await;
    let after = ours(p.w.h.app.notices_for(&p.dave.0, None).await?);
    assert!(
        after.is_empty(),
        "a limit notice was posted after its reset: {after:?}"
    );
    Ok(())
}

/// Review round 2 of #1711: an error, or a limit whose reset is unknown, holds until the
/// subscription is usable: one notice, not one a day.
#[tokio::test]
async fn a_lasting_subscription_problem_is_told_once() -> TestResult {
    use yulius_app::model::{AccountRecord, AccountStatus};
    use yulius_app::ports::store::AccountStore as _;
    use yulius_domain::ids::AccountId;
    let p = people().await?;
    let id = AccountId::parse("acc4")?;
    p.w.h
        .store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "Team Four".into(),
            credential: None,
            status: AccountStatus::Error,
            usage_json: None,
            last_error: Some("refused".into()),
            created_at: p.w.h.app.now(),
            updated_at: p.w.h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let ids = || async {
        Ok::<_, Error>(
            p.w.h
                .app
                .notices_for(&p.dave.0, None)
                .await?
                .into_iter()
                .filter(|n| n.key.as_str() == "account:acc4")
                .map(|n| n.id)
                .collect::<Vec<_>>(),
        )
    };
    p.w.h.app.sync_all_account_notices().await;
    let first = ids().await?;
    assert_eq!(first.len(), 1);
    p.w.h.clock.0.fetch_add(2 * DAY_MS, Ordering::SeqCst);
    p.w.h.app.prune_notices_now().await;
    p.w.h.app.sync_all_account_notices().await;
    assert_eq!(ids().await?, first, "still the one notice two days on");
    Ok(())
}
