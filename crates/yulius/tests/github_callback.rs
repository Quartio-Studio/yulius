//! Whether GitHub accepts the instance's own sign-in address (SPEC-466, ADR-0302): the kept
//! answer, when GitHub is asked, what Settings → Instance and the sign-in page are told, and the
//! loopback endpoint of root's commands.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yulius_app::host_move::{MOVED, MoveReport};
use yulius_app::ports::store::InstanceStore as _;

use crate::support::{
    Creds, Harness, TestResult, call, configure_github_app, harness, seed_github_user,
    setup_superadmin_creds,
};

const CALLBACK: &str = "https://yulius.test/api/auth/github/callback";
const ADDRESS: &str = "/api/admin/instance/address";
const CHECK: &str = "/api/admin/instance/address/check";

fn register(h: &Harness, urls: &[&str]) {
    if let Ok(mut registered) = h.github.callback_urls.lock() {
        *registered = Some(urls.iter().map(|&u| u.to_owned()).collect());
    }
}

fn asked(h: &Harness) -> usize {
    h.github.callback_asked.lock().map_or(0, |a| a.len())
}

fn later(h: &Harness, ms: i64) {
    h.clock.0.fetch_add(ms, Ordering::SeqCst);
}

async fn github(h: &Harness, admin: &Creds) -> Result<Value, Box<dyn std::error::Error>> {
    let view = call(&h.router, Method::GET, ADDRESS, None, admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{:?}", view.body);
    Ok(view.body["github"].clone())
}

/// Reads the address until GitHub's answer is `state`: a check in the background has ended.
async fn until_state(h: &Harness, admin: &Creds, state: &str) -> TestResult {
    for _ in 0..500 {
        if github(h, admin).await?["state"] == state {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!(
        "the answer never became {state}: {}",
        github(h, admin).await?
    )
    .into())
}

async fn says_missing(h: &Harness) -> Result<bool, Box<dyn std::error::Error>> {
    let status = call(
        &h.router,
        Method::GET,
        "/api/setup/status",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(status.status, StatusCode::OK);
    Ok(status.body["github_callback_missing"] == true)
}

async fn start_sign_in(h: &Harness) -> Result<String, Box<dyn std::error::Error>> {
    let start = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/s/app",
        None,
        &Creds::default(),
    )
    .await?;
    assert!(start.status.is_redirection(), "{}", start.status);
    Ok(start.location.ok_or("no location")?)
}

/// MVC-002: without a GitHub App there is no answer and GitHub is never asked.
#[tokio::test]
async fn without_an_app_github_is_never_asked() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    register(&h, &[]);
    h.app.start_github_callback_check();

    let view = call(&h.router, Method::GET, ADDRESS, None, &admin).await?;
    assert_eq!(view.status, StatusCode::OK);
    assert_eq!(view.body["public_url"], "https://yulius.test");
    assert_eq!(view.body["github"], Value::Null);
    assert_eq!(view.body["moved"], Value::Null);
    let checked = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(checked.status, StatusCode::OK);
    assert_eq!(checked.body["github"], Value::Null);
    assert!(!says_missing(&h).await?);
    assert_eq!(asked(&h), 0);
    Ok(())
}

/// MVC-002, MVC-003, MVC-010, MVC-012: a start finds the callback missing; the superadmin gets the
/// App's link and both values; the answer is re-checked while it is read, until it is registered.
#[tokio::test]
async fn a_missing_callback_is_said_and_rechecked_until_it_is_registered() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &["https://old.example/api/auth/github/callback"]);

    h.app.start_github_callback_check();
    until_state(&h, &admin, "missing").await?;
    let said = github(&h, &admin).await?;
    assert_eq!(said["reason"], Value::Null);
    assert_eq!(said["callback_url"], CALLBACK);
    assert_eq!(said["homepage_url"], "https://yulius.test");
    assert_eq!(
        said["settings_url"],
        "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-test"
    );
    assert!(said["checked_at"].is_string(), "{said}");
    assert!(says_missing(&h).await?);
    assert_eq!(asked(&h), 1);

    // GitHub accepts it now. The answer is 20 s old: reading it asks nothing.
    register(
        &h,
        &["https://old.example/api/auth/github/callback", CALLBACK],
    );
    later(&h, 20_000);
    assert_eq!(github(&h, &admin).await?["state"], "missing");
    assert!(says_missing(&h).await?);
    assert_eq!(asked(&h), 1);

    // Older than 30 s: a read asks again in the background, and nobody restarts anything.
    later(&h, 11_000);
    until_state(&h, &admin, "registered").await?;
    assert!(!says_missing(&h).await?);
    assert_eq!(asked(&h), 2);

    // "Registered" is kept: nothing asks GitHub while nobody demands it.
    later(&h, 3_600_000);
    assert_eq!(github(&h, &admin).await?["state"], "registered");
    assert!(!says_missing(&h).await?);
    assert_eq!(asked(&h), 2);
    Ok(())
}

/// MVC-003: the sign-in page alone re-checks a missing callback (nobody is signed in).
#[tokio::test]
async fn the_sign_in_page_rechecks_a_missing_callback() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[]);
    // No start asked: the first read does.
    assert!(!says_missing(&h).await?);
    for _ in 0..500 {
        if says_missing(&h).await? {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(says_missing(&h).await?);
    assert_eq!(asked(&h), 1);

    register(&h, &[CALLBACK]);
    later(&h, 31_000);
    for _ in 0..500 {
        if !says_missing(&h).await? {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!says_missing(&h).await?);
    assert_eq!(github(&h, &admin).await?["state"], "registered");
    assert_eq!(asked(&h), 2);
    Ok(())
}

/// MVC-010, MVC-011: "Check again" asks at once; GitHub giving no answer is said with the reason
/// and blocks nothing; a person's App has its own settings address; members are refused.
#[tokio::test]
async fn check_again_asks_at_once_and_says_why_github_gave_no_answer() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    if let Ok(mut error) = h.github.callback_error.lock() {
        *error = Some("GitHub: timed out".into());
    }

    let unknown = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(unknown.status, StatusCode::OK, "{:?}", unknown.body);
    assert_eq!(unknown.body["github"]["state"], "unknown");
    assert_eq!(unknown.body["github"]["reason"], "GitHub: timed out");
    assert!(unknown.body["github"]["checked_at"].is_string());
    assert!(!says_missing(&h).await?);
    assert!(
        start_sign_in(&h)
            .await?
            .starts_with("https://github.test/login/oauth/authorize")
    );

    if let Ok(mut error) = h.github.callback_error.lock() {
        *error = None;
    }
    register(&h, &[]);
    let missing = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(missing.body["github"]["state"], "missing");
    register(&h, &[CALLBACK]);
    let registered = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(registered.body["github"]["state"], "registered");
    assert_eq!(registered.body["github"]["reason"], Value::Null);

    // An App of a person.
    let sealed = h
        .store
        .instance()
        .await?
        .and_then(|i| i.github_app)
        .ok_or("no App")?;
    h.store.set_github_app(&sealed, "octocat", "user").await?;
    assert_eq!(
        github(&h, &admin).await?["settings_url"],
        "https://github.com/settings/apps/yulius-test"
    );

    let member = seed_github_user(&h, "u-alice", "alice").await?;
    for (method, body) in [(Method::GET, None), (Method::POST, Some(json!({})))] {
        let path = if body.is_some() { CHECK } else { ADDRESS };
        let refused = call(&h.router, method, path, body, &member).await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// MVC-003: one check runs at a time; a second demand waits for it and takes its answer.
#[tokio::test]
async fn a_second_demand_takes_the_running_check_s_answer() -> TestResult {
    use std::future::Future as _;
    use yulius_app::github_callback::CallbackState;
    let h = harness()?;
    setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[]);
    let hold = Arc::new(tokio::sync::Notify::new());
    if let Ok(mut held) = h.github.callback_hold.lock() {
        *held = Some(Arc::clone(&hold));
    }

    let app = Arc::clone(&h.app);
    let first = tokio::spawn(async move { app.check_github_callback(None, 0).await });
    h.github.callback_entered.notified().await;
    // The second demand begins while GitHub has not answered the first, and waits.
    let mut second = std::pin::pin!(h.app.check_github_callback(None, 0));
    let waits =
        std::future::poll_fn(|cx| std::task::Poll::Ready(second.as_mut().poll(cx).is_pending()))
            .await;
    assert!(waits);
    if let Ok(mut held) = h.github.callback_hold.lock() {
        *held = None;
    }
    hold.notify_one();
    let state = |facts: Option<yulius_app::github_callback::CallbackFacts>| facts.map(|f| f.state);
    assert_eq!(state(first.await??), Some(CallbackState::Missing));
    assert_eq!(state(second.await?), Some(CallbackState::Missing));
    assert_eq!(asked(&h), 1);

    // A demand that begins after the answer asks again: "Check again" never reuses.
    register(&h, &[CALLBACK]);
    let third = h.app.check_github_callback(None, 0).await?;
    assert_eq!(state(third), Some(CallbackState::Registered));
    assert_eq!(asked(&h), 2);
    Ok(())
}

/// MVC-013: a sign-in that starts while the callback is missing asks again first, and returns to
/// the sign-in page instead of GitHub's error page; password sign-in works throughout.
#[tokio::test]
async fn a_sign_in_start_asks_again_before_it_sends_anyone_to_github() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[]);
    let missing = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(missing.body["github"]["state"], "missing");

    // The answer is a moment old: it is reused, GitHub is not asked for every visitor.
    let back = start_sign_in(&h).await?;
    assert_eq!(back, "/signin?error=github_callback&next=/s/app");
    assert_eq!(asked(&h), 1);
    later(&h, 6_000);
    assert_eq!(
        start_sign_in(&h).await?,
        "/signin?error=github_callback&next=/s/app"
    );
    assert_eq!(asked(&h), 2);

    let password = call(
        &h.router,
        Method::POST,
        "/api/auth/password",
        Some(json!({"password": "a very long secret"})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(password.status, StatusCode::OK);

    // Registered at GitHub since: the next start finds out by itself and goes to GitHub.
    register(&h, &[CALLBACK]);
    later(&h, 6_000);
    assert!(
        start_sign_in(&h)
            .await?
            .starts_with("https://github.test/login/oauth/authorize")
    );
    assert_eq!(asked(&h), 3);
    assert!(!says_missing(&h).await?);
    Ok(())
}

/// MVC-010: after a move the address names where the instance came from and what its first start
/// did with the tasks.
#[tokio::test]
async fn the_address_names_the_move() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let report = MoveReport {
        version: 1,
        from: Some("https://old.example".into()),
        to: Some("https://yulius.test".into()),
        queued: 7,
        left: 2,
        failed: 1,
        at: 1_791_331_200_000,
    };
    std::fs::write(
        h.app.config.data_dir.join(MOVED),
        serde_json::to_vec(&report)?,
    )?;
    let view = call(&h.router, Method::GET, ADDRESS, None, &admin).await?;
    assert_eq!(
        view.body["moved"],
        json!({"from": "https://old.example", "to": "https://yulius.test",
            "at": "2026-10-07T00:00:00Z", "queued": 7, "left": 2, "failed": 1})
    );
    // A file that is not a report is no move.
    std::fs::write(h.app.config.data_dir.join(MOVED), "{")?;
    let view = call(&h.router, Method::GET, ADDRESS, None, &admin).await?;
    assert_eq!(view.body["moved"], Value::Null);
    Ok(())
}

/// MVC-020: root's commands ask over the loopback listener with the server's key, about the
/// instance's own URL or the one a move goes to.
#[tokio::test]
async fn the_loopback_endpoint_answers_with_the_server_s_key() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[CALLBACK]);
    h.app.write_leftovers_key()?;
    let key = std::fs::read_to_string(
        h.app
            .config
            .data_dir
            .join(yulius_app::leftovers::CLI_KEY_FILE),
    )?;
    let with_key = Creds {
        bearer: Some(key.trim().to_owned()),
        ..Creds::default()
    };
    let path = "/internal/github-callback";

    for creds in [
        Creds::default(),
        Creds {
            bearer: Some("0".repeat(64)),
            ..Creds::default()
        },
    ] {
        let refused = call(&h.internal, Method::GET, path, None, &creds).await?;
        assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(asked(&h), 0);
    // Only the loopback listener has it: the public one answers with the web UI's page.
    let public = call(&h.router, Method::GET, path, None, &with_key).await?;
    assert_eq!(public.body, Value::Null);
    assert_eq!(asked(&h), 0);

    let own = call(&h.internal, Method::GET, path, None, &with_key).await?;
    assert_eq!(own.status, StatusCode::OK, "{:?}", own.body);
    assert_eq!(
        own.body["github"],
        json!({"state": "registered", "reason": null, "callback_url": CALLBACK,
            "homepage_url": "https://yulius.test",
            "settings_url": "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-test",
            "checked_at": "2026-10-07T00:00:00Z"})
    );
    assert_eq!(github(&h, &admin).await?["state"], "registered");

    // The URL a move goes to: asked about, and the instance's own answer stays.
    let new = call(
        &h.internal,
        Method::GET,
        &format!("{path}?public_url=https://new.example"),
        None,
        &with_key,
    )
    .await?;
    assert_eq!(new.status, StatusCode::OK);
    assert_eq!(new.body["github"]["state"], "missing");
    assert_eq!(
        new.body["github"]["callback_url"],
        "https://new.example/api/auth/github/callback"
    );
    assert_eq!(new.body["github"]["homepage_url"], "https://new.example");
    assert_eq!(github(&h, &admin).await?["state"], "registered");
    assert_eq!(asked(&h), 2);

    for bad in [
        "http://new.example",
        "https://new.example/path",
        "https://user@new.example",
        "https://",
        "new.example",
    ] {
        let refused = call(
            &h.internal,
            Method::GET,
            &format!("{path}?public_url={bad}"),
            None,
            &with_key,
        )
        .await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    assert_eq!(asked(&h), 2);
    Ok(())
}

/// MVC-020: an instance without an App says so to root, and GitHub is not asked.
#[tokio::test]
async fn the_loopback_endpoint_says_when_there_is_no_app() -> TestResult {
    let h = harness()?;
    h.app.write_leftovers_key()?;
    let key = std::fs::read_to_string(
        h.app
            .config
            .data_dir
            .join(yulius_app::leftovers::CLI_KEY_FILE),
    )?;
    let with_key = Creds {
        bearer: Some(key.trim().to_owned()),
        ..Creds::default()
    };
    let none = call(
        &h.internal,
        Method::GET,
        "/internal/github-callback?public_url=https://new.example",
        None,
        &with_key,
    )
    .await?;
    assert_eq!(none.status, StatusCode::OK);
    assert_eq!(none.body, json!({"github": null}));
    assert_eq!(asked(&h), 0);
    Ok(())
}

/// MVC-001 through the production adapter, against the fake GitHub: both answers, and a refused
/// secret as the reason a superadmin reads, without the secret.
#[tokio::test]
async fn the_adapter_asks_the_fake_github() -> TestResult {
    use yulius::wiring::github_client::GitHubAdapter;
    use yulius_app::ports::github::{GitHubAppConfig, GitHubPort as _};
    let fake = yulius_github::fake::FakeGitHub::start().await?;
    let adapter = GitHubAdapter::new(fake.config())?;
    let credentials = fake.app_credentials();
    let app = GitHubAppConfig {
        app_id: credentials.app_id,
        slug: credentials.slug.clone(),
        client_id: credentials.client_id.clone(),
        client_secret: credentials.client_secret.expose().to_owned(),
        private_key_pem: credentials.private_key_pem.expose().to_owned(),
        webhook_secret: None,
    };
    fake.set_callback_urls(&["https://old.example/api/auth/github/callback"]);
    assert!(
        !adapter.callback_registered(&app, CALLBACK).await?,
        "the new callback is not registered yet"
    );
    fake.set_callback_urls(&["https://old.example/api/auth/github/callback", CALLBACK]);
    assert!(adapter.callback_registered(&app, CALLBACK).await?);

    let wrong = GitHubAppConfig {
        app_id: app.app_id + 1,
        client_secret: "not-the-secret-0123456789".into(),
        ..app
    };
    let refused = adapter
        .callback_registered(&wrong, CALLBACK)
        .await
        .err()
        .ok_or("a wrong secret was accepted")?;
    assert_eq!(
        refused.to_string(),
        "GitHub answered `incorrect_client_credentials`: it does not accept the App's client id \
         and secret"
    );
    fake.shutdown().await;
    Ok(())
}

/// MVC-021: the question of `yulius instance migrate` reaches the running server's loopback
/// listener, and its answer makes the plan's line with the App's own link.
#[tokio::test]
async fn the_move_s_question_is_answered_by_the_running_server() -> TestResult {
    use yulius::instance::migrate::github::{asked_instance, line};
    use yulius_contracts::update::HostOutcome;
    let h = harness()?;
    setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[CALLBACK]);
    h.app.write_leftovers_key()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let router = h.internal.clone();
    tokio::spawn(async move { axum::serve(listener, router).await });

    let data = h.app.config.data_dir.clone();
    let (new, own) = tokio::task::spawn_blocking(move || {
        (
            asked_instance(&data, &address, Some("https://new.example")),
            asked_instance(&data, &address, None),
        )
    })
    .await?;
    assert_eq!(own?.ok_or("no App")?.state, "registered");
    let (outcome, detail) = line(new, "https://new.example");
    assert_eq!(outcome, HostOutcome::Manual);
    assert!(
        detail.contains(
            "open https://github.com/organizations/Quartio-Studio/settings/apps/yulius-test and \
             add the callback URL https://new.example/api/auth/github/callback and set the \
             homepage https://new.example"
        ),
        "{detail}"
    );
    assert_eq!(
        h.github
            .callback_asked
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default(),
        ["https://new.example/api/auth/github/callback", CALLBACK]
    );
    Ok(())
}

/// Reads the address until GitHub has been asked `count` times: a background check has ended.
async fn until_asked(h: &Harness, admin: &Creds, count: usize) -> TestResult {
    for _ in 0..500 {
        if asked(h) >= count {
            return Ok(());
        }
        github(h, admin).await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!("GitHub was asked {} times, not {count}", asked(h)).into())
}

/// MVC-003: a missing callback that nobody fixes is asked for ever less often while it is read,
/// and a person's demand makes the next looks quick again.
#[tokio::test]
async fn a_callback_that_stays_missing_is_asked_for_less_often_until_someone_demands() -> TestResult
{
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    register(&h, &[]);
    h.app.start_github_callback_check();
    until_state(&h, &admin, "missing").await?;
    assert_eq!(asked(&h), 1);

    // Someone watches the page: every 30 s for the first eleven looks.
    for look in 0..11 {
        later(&h, 30_000);
        until_asked(&h, &admin, look + 2).await?;
    }
    assert_eq!(asked(&h), 12);
    // From then on the wait doubles: 60 s.
    later(&h, 60_000);
    until_asked(&h, &admin, 13).await?;
    // And again: 120 s.
    later(&h, 120_000);
    until_asked(&h, &admin, 14).await?;
    assert_eq!(asked(&h), 14);

    // "Check again": a person is here, so the next looks are 30 s apart again.
    let checked = call(&h.router, Method::POST, CHECK, Some(json!({})), &admin).await?;
    assert_eq!(checked.body["github"]["state"], "missing");
    assert_eq!(asked(&h), 15);
    later(&h, 30_000);
    until_asked(&h, &admin, 16).await?;
    assert_eq!(asked(&h), 16);

    // An answer that changes is news: it is asked for again quickly, whatever stood before.
    for look in 0..10 {
        later(&h, 30_000);
        until_asked(&h, &admin, look + 17).await?;
    }
    later(&h, 60_000);
    until_asked(&h, &admin, 27).await?;
    if let Ok(mut error) = h.github.callback_error.lock() {
        *error = Some("GitHub: timed out".into());
    }
    later(&h, 120_000);
    until_asked(&h, &admin, 28).await?;
    until_state(&h, &admin, "unknown").await?;
    later(&h, 30_000);
    until_asked(&h, &admin, 29).await?;
    assert_eq!(asked(&h), 29);
    Ok(())
}
