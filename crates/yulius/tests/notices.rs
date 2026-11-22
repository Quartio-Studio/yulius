//! Notices for people (SPEC-624): a key per notice (newest wins per project, recipient and key),
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

/// NTC-003, NTC-007, AC-1, AC-2: a newer notice with the same key replaces the older one for
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

/// NTC-004, AC-3: an expired notice is left out of the HTTP read and the person tools at once,
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

/// NTC-005, AC-4: a decision posted with an expiry is refused; without one it never expires.
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

/// NTC-006: a producer removes a notice and pages hear it; a person dismisses one through their
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

/// NTC-001, NTC-002, AC-5: a notice with a link outside Yulius or an expiry in the past is
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

/// NTC-009, AC-8: the digest lists a person's live notices with their keys, and leaves out the
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
