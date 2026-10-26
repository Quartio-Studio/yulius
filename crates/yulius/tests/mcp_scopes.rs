//! MCP servers for every sandbox or one, and for the team or only their owner (SPEC-332): who
//! sees and manages a personal server, whose runs get it, precedence of the four combinations
//! and changing them later.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, add_sandbox, call, harness_configured, perms, user_with_token};
use yulius_app::ports::harness::McpAttachment;
use yulius_app::ports::store::SandboxStore;
use yulius_app::vault::Holder;
use yulius_domain::ids::{SandboxId, UserId};

type Error = Box<dyn std::error::Error>;

struct World {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
    root: Creds,
}

async fn world() -> Result<World, Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    add_sandbox(&h, "s2", "site", 20020).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    let bob = user_with_token(&h, "u-bob", "bob", false).await?;
    let root = user_with_token(&h, "u-root", "root", true).await?;
    for repo in ["Quartio-Studio/studio", "Quartio-Studio/site"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("write"));
        h.github.set_perm("root", repo, perms("write"));
    }
    Ok(World {
        h,
        alice,
        bob,
        root,
    })
}

fn server(name: &str, url: &str, scope: &str, audience: &str) -> Value {
    json!({"name": name, "transport": "http", "url": url, "scope": scope, "audience": audience})
}

async fn create(w: &World, sid: &str, body: Value, who: &Creds) -> Result<Value, Error> {
    let created = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/mcp"),
        Some(body),
        who,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(created.body)
}

async fn names(w: &World, sid: &str, who: &Creds) -> Result<Vec<String>, Error> {
    let list = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sid}/mcp"),
        None,
        who,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    Ok(list
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_owned))
        .collect())
}

/// The URLs a run of `holder` in `sid` gets, by server name.
async fn attached(
    w: &World,
    sid: &str,
    holder: Holder<'_>,
) -> Result<Vec<(String, String)>, Error> {
    let sandbox =
        w.h.store
            .sandbox(&SandboxId::parse(sid)?)
            .await?
            .ok_or("sandbox")?;
    let mut out: Vec<(String, String)> =
        w.h.app
            .mcp_attachments(&sandbox, holder)
            .await?
            .into_iter()
            .map(|a| match a {
                McpAttachment::Http { name, url, .. } => (name, url),
                McpAttachment::Stdio { name, command, .. } => (name, command),
            })
            .collect();
    out.sort();
    Ok(out)
}

fn pair(name: &str, url: &str) -> (String, String) {
    (name.to_owned(), url.to_owned())
}

/// AC-1, AC-2, MSC-003, MSC-004: a personal server exists only for its owner and reaches only its
/// owner's runs, in one sandbox or in every one.
#[tokio::test]
async fn a_personal_server_is_its_owners_alone() -> TestResult {
    let w = world().await?;
    let alice_id = UserId::parse("u-alice")?;
    let bob_id = UserId::parse("u-bob")?;
    let mine = create(
        &w,
        "s1",
        server("admin", "https://admin.test/mcp", "sandbox", "personal"),
        &w.alice,
    )
    .await?;
    assert_eq!(mine["audience"], "personal");
    assert_eq!(mine["scope"], "sandbox");
    let everywhere = create(
        &w,
        "s1",
        server("fleet", "https://fleet.test/mcp", "instance", "personal"),
        &w.alice,
    )
    .await?;
    assert_eq!(everywhere["scope"], "instance");
    create(
        &w,
        "s1",
        server("docs", "https://docs.test/mcp", "sandbox", "team"),
        &w.bob,
    )
    .await?;

    assert_eq!(
        names(&w, "s1", &w.alice).await?,
        ["fleet", "admin", "docs"].map(String::from)
    );
    assert_eq!(
        names(&w, "s2", &w.alice).await?,
        ["fleet"].map(String::from)
    );
    assert_eq!(names(&w, "s1", &w.bob).await?, ["docs"].map(String::from));
    assert_eq!(
        names(&w, "s1", &w.root).await?,
        ["docs"].map(String::from),
        "administrators included"
    );

    let id = mine["id"].as_str().ok_or("id")?;
    let edit = server("admin", "https://evil.test/mcp", "sandbox", "personal");
    for who in [&w.bob, &w.root] {
        for (method, body) in [(Method::PUT, Some(edit.clone())), (Method::DELETE, None)] {
            let refused = call(&w.h.router, method, &format!("/api/mcp/{id}"), body, who).await?;
            assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.body);
        }
        let test = call(
            &w.h.router,
            Method::POST,
            &format!("/api/mcp/{id}/test"),
            None,
            who,
        )
        .await?;
        assert_eq!(test.status, StatusCode::NOT_FOUND);
    }

    assert_eq!(
        attached(&w, "s1", Holder::Person(&alice_id)).await?,
        [
            pair("admin", "https://admin.test/mcp"),
            pair("docs", "https://docs.test/mcp"),
            pair("fleet", "https://fleet.test/mcp")
        ]
    );
    assert_eq!(
        attached(&w, "s2", Holder::Person(&alice_id)).await?,
        [pair("fleet", "https://fleet.test/mcp")]
    );
    assert_eq!(
        attached(&w, "s1", Holder::Person(&bob_id)).await?,
        [pair("docs", "https://docs.test/mcp")]
    );
    assert_eq!(
        attached(&w, "s1", Holder::Nobody).await?,
        [pair("docs", "https://docs.test/mcp")],
        "an automation's task, CI and CD hold nobody"
    );

    let renamed = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server(
            "admin",
            "https://admin.test/v2",
            "sandbox",
            "personal",
        )),
        &w.alice,
    )
    .await?;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);

    Ok(())
}

/// AC-3, MSC-005: this sandbox's personal > its team > every sandbox's personal > its team, and a
/// disabled winner turns the name off for whom it applies.
#[tokio::test]
async fn the_most_specific_server_wins_per_name() -> TestResult {
    let w = world().await?;
    let alice_id = UserId::parse("u-alice")?;
    let bob_id = UserId::parse("u-bob")?;
    let alice = Holder::Person(&alice_id);
    let bob = Holder::Person(&bob_id);
    create(
        &w,
        "s1",
        server("q", "https://global-team.test", "instance", "team"),
        &w.root,
    )
    .await?;
    assert_eq!(
        attached(&w, "s1", alice).await?,
        [pair("q", "https://global-team.test")]
    );
    create(
        &w,
        "s1",
        server("q", "https://global-alice.test", "instance", "personal"),
        &w.alice,
    )
    .await?;
    assert_eq!(
        attached(&w, "s1", alice).await?,
        [pair("q", "https://global-alice.test")]
    );
    assert_eq!(
        attached(&w, "s1", bob).await?,
        [pair("q", "https://global-team.test")]
    );
    create(
        &w,
        "s1",
        server("q", "https://s1-team.test", "sandbox", "team"),
        &w.bob,
    )
    .await?;
    assert_eq!(
        attached(&w, "s1", alice).await?,
        [pair("q", "https://s1-team.test")]
    );
    assert_eq!(
        attached(&w, "s2", alice).await?,
        [pair("q", "https://global-alice.test")]
    );
    let off = create(&w, "s1", json!({"name": "Q", "transport": "http", "url": "https://s1-alice.test", "scope": "sandbox", "audience": "personal", "enabled": false}), &w.alice).await?;
    assert_eq!(
        attached(&w, "s1", alice).await?,
        [],
        "a disabled personal server turns q off for its owner"
    );
    assert_eq!(
        attached(&w, "s1", bob).await?,
        [pair("q", "https://s1-team.test")],
        "and for nobody else"
    );
    let id = off["id"].as_str().ok_or("id")?;
    let on = call(&w.h.router, Method::PUT, &format!("/api/mcp/{id}"), Some(json!({"name": "Q", "transport": "http", "url": "https://s1-alice.test", "enabled": true})), &w.alice).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(
        attached(&w, "s1", alice).await?,
        [pair("Q", "https://s1-alice.test")]
    );
    Ok(())
}

/// AC-4, MSC-003, MSC-006: who may add each combination, and changing one later.
#[tokio::test]
async fn scope_and_audience_follow_permissions_and_can_change() -> TestResult {
    let w = world().await?;
    let alice_id = UserId::parse("u-alice")?;
    let bob_id = UserId::parse("u-bob")?;
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(server("q", "https://q.test", "instance", "team")),
        &w.alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let bad = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(server("q", "https://q.test", "sandbox", "everyone")),
        &w.alice,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let defaulted = create(
        &w,
        "s1",
        json!({"name": "plain", "transport": "http", "url": "https://plain.test"}),
        &w.alice,
    )
    .await?;
    assert_eq!(
        (defaulted["scope"].as_str(), defaulted["audience"].as_str()),
        (Some("sandbox"), Some("team")),
        "the API default is unchanged"
    );

    // A team server made personal belongs to whoever made it so.
    let id = defaulted["id"].as_str().ok_or("id")?.to_owned();
    let to_bob = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server("plain", "https://plain.test", "sandbox", "personal")),
        &w.bob,
    )
    .await?;
    assert_eq!(to_bob.status, StatusCode::OK, "{}", to_bob.body);
    assert_eq!(to_bob.body["audience"], "personal");
    assert_eq!(names(&w, "s1", &w.alice).await?, Vec::<String>::new());
    assert_eq!(
        attached(&w, "s1", Holder::Person(&bob_id)).await?,
        [pair("plain", "https://plain.test")]
    );
    assert_eq!(attached(&w, "s1", Holder::Person(&alice_id)).await?, []);

    // Bob may take it to every sandbox while it is his own, not as a team server.
    let wide = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server(
            "plain",
            "https://plain.test",
            "instance",
            "personal",
        )),
        &w.bob,
    )
    .await?;
    assert_eq!(wide.status, StatusCode::OK, "{}", wide.body);
    assert_eq!(wide.body["scope"], "instance");
    assert_eq!(
        attached(&w, "s2", Holder::Person(&bob_id)).await?,
        [pair("plain", "https://plain.test")]
    );
    let team = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server("plain", "https://plain.test", "instance", "team")),
        &w.bob,
    )
    .await?;
    assert_eq!(team.status, StatusCode::FORBIDDEN, "{}", team.body);

    // Back into one sandbox: the body names which one.
    let unnamed = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server("plain", "https://plain.test", "sandbox", "team")),
        &w.bob,
    )
    .await?;
    assert_eq!(
        unnamed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unnamed.body
    );
    let mut back = server("plain", "https://plain.test", "sandbox", "team");
    back["sandbox"] = json!("s2");
    let back = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(back),
        &w.bob,
    )
    .await?;
    assert_eq!(back.status, StatusCode::OK, "{}", back.body);
    assert_eq!(
        names(&w, "s2", &w.alice).await?,
        ["plain"].map(String::from)
    );
    assert_eq!(names(&w, "s1", &w.alice).await?, Vec::<String>::new());

    // The administrator may make it every sandbox's team server.
    let global = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{id}"),
        Some(server("plain", "https://plain.test", "instance", "team")),
        &w.root,
    )
    .await?;
    assert_eq!(global.status, StatusCode::OK, "{}", global.body);
    assert_eq!(
        attached(&w, "s1", Holder::Nobody).await?,
        [pair("plain", "https://plain.test")]
    );

    // Names are unique per (scope, owner): two people may each have their own.
    create(
        &w,
        "s1",
        server("own", "https://a.test", "instance", "personal"),
        &w.alice,
    )
    .await?;
    create(
        &w,
        "s1",
        server("own", "https://b.test", "instance", "personal"),
        &w.bob,
    )
    .await?;
    let dup = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(server("OWN", "https://c.test", "instance", "personal")),
        &w.alice,
    )
    .await?;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    Ok(())
}

/// Review of SPEC-332: a server goes only where its person may manage it, `sandbox` moves a
/// server only out of every sandbox, and another person's server cannot be converted.
#[tokio::test]
async fn a_server_goes_only_where_its_person_may_manage_it() -> TestResult {
    let w = world().await?;
    add_sandbox(&w.h, "s3", "docs-site", 20040).await?;
    w.h.github
        .set_perm("alice", "Quartio-Studio/docs-site", perms("read"));
    let reader = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s3/mcp",
        Some(server("own", "https://own.test", "instance", "personal")),
        &w.alice,
    )
    .await?;
    assert_eq!(
        reader.status,
        StatusCode::FORBIDDEN,
        "a reader adds no personal server: {}",
        reader.body
    );

    // Naming another sandbox does not move a sandbox's server there.
    let team = create(
        &w,
        "s1",
        server("docs", "https://docs.test", "sandbox", "team"),
        &w.alice,
    )
    .await?;
    let team_id = team["id"].as_str().ok_or("id")?;
    let mut elsewhere = server("docs", "https://docs.test", "sandbox", "team");
    elsewhere["sandbox"] = json!("s2");
    let kept = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{team_id}"),
        Some(elsewhere),
        &w.alice,
    )
    .await?;
    assert_eq!(kept.status, StatusCode::OK, "{}", kept.body);
    assert_eq!(names(&w, "s1", &w.bob).await?, ["docs"].map(String::from));
    assert_eq!(names(&w, "s2", &w.bob).await?, Vec::<String>::new());

    // An owner cannot take their server into a sandbox they may not work in.
    let own = create(
        &w,
        "s1",
        server("own", "https://own.test", "instance", "personal"),
        &w.alice,
    )
    .await?;
    let own_id = own["id"].as_str().ok_or("id")?;
    let mut into_s3 = server("own", "https://own.test", "sandbox", "personal");
    into_s3["sandbox"] = json!("s3");
    let refused = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{own_id}"),
        Some(into_s3),
        &w.alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    // An administrator cannot convert or move another person's server.
    for body in [
        server("own", "https://own.test", "instance", "team"),
        server("own", "https://own.test", "sandbox", "personal"),
    ] {
        let mut body = body;
        body["sandbox"] = json!("s1");
        let hidden = call(
            &w.h.router,
            Method::PUT,
            &format!("/api/mcp/{own_id}"),
            Some(body),
            &w.root,
        )
        .await?;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.body);
    }
    Ok(())
}

/// Review of SPEC-332: the secret values of every MCP server that can reach a sandbox's runs, the
/// team's and each person's, are redacted from the text Yulius shows of it, as agent secrets are;
/// short and ordinary values are not.
#[tokio::test]
async fn mcp_secret_values_are_redacted_like_agent_secrets() -> TestResult {
    const TEAM: &str = "apify-team-secret-0123456789";
    const MINE: &str = "sk-alice-personal-9876543210";
    let w = world().await?;
    let s1 = SandboxId::parse("s1")?;
    create(
        &w,
        "s1",
        json!({"name": "apify", "transport": "stdio", "command": "npx",
        "env": {"APIFY_TOKEN": TEAM, "APIFY_LOG": "info", "APIFY_MODE": "production"}}),
        &w.bob,
    )
    .await?;
    let text = format!("token {TEAM} and {MINE}; log info, mode production");
    let before = w.h.app.redact_for_sandbox(&s1, &text).await;
    assert!(!before.contains(TEAM), "{before}");
    assert!(before.contains(MINE), "not configured yet: {before}");
    create(
        &w,
        "s1",
        json!({"name": "fleet", "transport": "http", "url": "https://fleet.test/mcp",
        "scope": "instance", "audience": "personal",
        "headers": {"Authorization": format!("Bearer {MINE}")}}),
        &w.alice,
    )
    .await?;
    let after = w.h.app.redact_for_sandbox(&s1, &text).await;
    assert!(!after.contains(TEAM) && !after.contains(MINE), "{after}");
    assert!(after.contains("log info, mode production"), "{after}");
    // A run's own fingerprint file holds only its holder's servers' values.
    let bob_id = UserId::parse("u-bob")?;
    let alice_id = UserId::parse("u-alice")?;
    let bobs =
        w.h.app
            .scrub_fingerprints_for(&s1, Holder::Person(&bob_id))
            .await?;
    assert!(
        !bobs.names.iter().any(|n| n == "fleet.Authorization"),
        "{:?}",
        bobs.names
    );
    assert!(
        bobs.names.iter().any(|n| n == "apify.APIFY_TOKEN"),
        "{:?}",
        bobs.names
    );
    let alices =
        w.h.app
            .scrub_fingerprints_for(&s1, Holder::Person(&alice_id))
            .await?;
    assert!(
        alices.names.iter().any(|n| n == "fleet.Authorization"),
        "{:?}",
        alices.names
    );
    Ok(())
}
