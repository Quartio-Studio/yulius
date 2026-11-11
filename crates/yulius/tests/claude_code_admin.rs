//! Settings → Instance, "Claude Code" over the API (SPEC-461 CCM-008..CCM-010): the superadmin
//! sees which Claude Code sandboxes get and what it lacks, changes the release channel (stored,
//! audited, applied to the adapter at once) and starts a check; nobody else does.

use crate::support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness,
    setup_superadmin_creds,
};
use yulius_app::claude_code::CLAUDE_CODE_TARGET;
use yulius_app::ports::agent_cli::{AgentCli, AgentCliStatus, CheckResult, CliSource};
use yulius_app::ports::store::AuditStore as _;
use yulius_app::ports::store_ext::UpdateStore as _;
use yulius_domain::agent_cli::{Channel, HOST_INSTALL, Version};
use yulius_domain::harness::DEFAULT_CLAUDE_MODEL;
use yulius_domain::time::Timestamp;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
}

/// An adapter that shows what it is told and records what it is asked.
struct FakeCli {
    status: Mutex<AgentCliStatus>,
    checks: AtomicUsize,
    channels: Mutex<Vec<(Channel, bool)>>,
}

impl FakeCli {
    fn new(status: AgentCliStatus) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(status),
            checks: AtomicUsize::new(0),
            channels: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<(Channel, bool)> {
        self.channels.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl AgentCli for FakeCli {
    async fn status(&self) -> AgentCliStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    fn check_now(&self) {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut status) = self.status.lock() {
            status.checking = true;
        }
    }

    fn set_channel(&self, channel: Channel, chosen: bool) {
        if let Ok(mut channels) = self.channels.lock() {
            channels.push((channel, chosen));
        }
        if let Ok(mut status) = self.status.lock() {
            status.channel = channel;
        }
    }

    fn version_for_new_runs(&self) -> Option<Version> {
        self.status.lock().ok().and_then(|s| s.version)
    }
}

/// A minute before the harness's clock starts.
const READ_AT: i64 = 1_791_331_200_000 - 60_000;

/// Runs get Yulius's 2.1.322, which knows every offered model; npm was read a minute ago.
fn healthy() -> AgentCliStatus {
    AgentCliStatus {
        unmanaged: None,
        channel: Channel::Latest,
        version: Some(v("2.1.322")),
        source: Some(CliSource::Yulius),
        host_version: Some(v("2.1.311")),
        missing: Some(Vec::new()),
        latest: Some(v("2.1.322")),
        stable: Some(v("2.1.311")),
        checked_at: Some(Timestamp(READ_AT)),
        checking: false,
        result: Some(CheckResult::Installed(v("2.1.322"))),
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    alice: Creds,
}

async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    Ok(Instance { h, admin, alice })
}

const VIEW: &str = "/api/admin/claude-code";
const CHECK: &str = "/api/admin/claude-code/check";
const CHANNEL: &str = "/api/admin/claude-code/channel";

/// CCM-008: a server that does not isolate sandboxes keeps nothing current: the card says why,
/// and there is nothing to check or change.
#[tokio::test]
async fn without_isolation_the_card_says_why_and_nothing_changes() -> TestResult {
    let i = instance().await?;
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["managed"], false);
    assert!(
        view.body["unmanaged"]
            .as_str()
            .is_some_and(|why| why.contains("sandboxes are not isolated on this server")),
        "{}",
        view.body
    );
    assert_eq!(view.body["default_model"], DEFAULT_CLAUDE_MODEL);
    assert_eq!(view.body["channel"], "latest");
    let check = call(&i.h.router, Method::POST, CHECK, None, &i.admin).await?;
    assert_eq!(check.status, StatusCode::CONFLICT, "{}", check.body);
    let put = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "stable"})),
        &i.admin,
    )
    .await?;
    assert_eq!(put.status, StatusCode::CONFLICT, "{}", put.body);
    assert!(i.h.store.claude_channel().await?.is_none());
    assert_eq!(i.h.app.claude_code_for_new_runs(), None);
    Ok(())
}

/// AC6 (CCM-009): the superadmin's channel is stored, audited and followed at once; `null` goes
/// back to the server's configuration; an unknown channel and everyone else are refused.
#[tokio::test]
async fn the_superadmin_changes_the_channel_stored_audited_and_applied() -> TestResult {
    let i = instance().await?;
    let cli = FakeCli::new(healthy());
    assert!(
        i.h.app
            .set_agent_cli(cli.clone() as Arc<_>, Channel::Latest)
    );
    assert_eq!(
        i.h.app.claude_channel_at_start(Channel::Latest).await,
        (Channel::Latest, false),
        "nothing chosen: the configuration"
    );
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["managed"], true);
    assert_eq!(view.body["channel"], "latest");
    assert_eq!(view.body["channel_source"], "config");
    assert_eq!(view.body["config_channel"], "latest");
    assert_eq!(view.body["version"], "2.1.322");
    assert_eq!(view.body["source"], "yulius");
    assert_eq!(view.body["host_version"], "2.1.311");
    assert_eq!(view.body["latest"], "2.1.322");
    assert_eq!(view.body["stable"], "2.1.311");
    assert_eq!(view.body["result"], "installed");
    assert_eq!(
        view.body["result_text"],
        "Installed 2.1.322: new runs use it."
    );
    assert_eq!(view.body["models_read"], true);
    assert_eq!(view.body["unknown_models"], json!([]));
    assert!(view.body["problem"].is_null() && view.body["fix"].is_null());
    assert!(view.body["checked_at"].is_string());

    let put = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "stable"})),
        &i.admin,
    )
    .await?;
    assert_eq!(put.status, StatusCode::OK, "{}", put.body);
    assert_eq!(put.body["channel"], "stable");
    assert_eq!(put.body["channel_source"], "settings");
    assert_eq!(put.body["channel_set_by"], "superadmin");
    assert!(put.body["channel_set_at"].is_string());
    assert_eq!(put.body["config_channel"], "latest");
    assert_eq!(cli.asked(), [(Channel::Stable, true)]);
    let stored =
        i.h.store
            .claude_channel()
            .await?
            .ok_or("a stored channel")?;
    assert_eq!(stored.channel, "stable");
    assert_eq!(stored.set_by.as_deref(), Some("superadmin"));
    // The next start follows it, whatever the unit says.
    assert_eq!(
        i.h.app.claude_channel_at_start(Channel::Latest).await,
        (Channel::Stable, true)
    );
    // The same choice again changes and audits nothing.
    call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "stable"})),
        &i.admin,
    )
    .await?;
    assert_eq!(cli.asked().len(), 1);

    // Off, then back to the server's configuration.
    let off = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "off"})),
        &i.admin,
    )
    .await?;
    assert_eq!(off.body["channel"], "off");
    let back = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": null})),
        &i.admin,
    )
    .await?;
    assert_eq!(back.status, StatusCode::OK, "{}", back.body);
    assert_eq!(back.body["channel"], "latest");
    assert_eq!(back.body["channel_source"], "config");
    assert!(back.body["channel_set_by"].is_null());
    assert_eq!(
        cli.asked(),
        [
            (Channel::Stable, true),
            (Channel::Off, true),
            (Channel::Latest, false)
        ]
    );
    assert!(i.h.store.claude_channel().await?.is_none());
    let audited =
        i.h.store
            .audit_details(CLAUDE_CODE_TARGET, "claude_code.channel", 10)
            .await?;
    assert_eq!(audited, ["configuration", "off", "stable"]);

    // A body without the channel (a misspelt field) is refused: it is no "back to the
    // configuration".
    call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "stable"})),
        &i.admin,
    )
    .await?;
    let misspelt = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"chanel": "off"})),
        &i.admin,
    )
    .await?;
    assert_eq!(
        misspelt.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        misspelt.body
    );
    assert_eq!(
        i.h.store
            .claude_channel()
            .await?
            .map(|c| c.channel)
            .as_deref(),
        Some("stable")
    );
    let asked = cli.asked().len();

    // An unknown channel is refused and changes nothing.
    let bad = call(
        &i.h.router,
        Method::PUT,
        CHANNEL,
        Some(json!({"channel": "nightly"})),
        &i.admin,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", bad.body);
    assert_eq!(cli.asked().len(), asked);

    // Only the superadmin: an instance administrator and a visitor are refused everywhere.
    for (method, path, body) in [
        (Method::GET, VIEW, None),
        (Method::POST, CHECK, None),
        (Method::PUT, CHANNEL, Some(json!({"channel": "off"}))),
    ] {
        let member = call(&i.h.router, method.clone(), path, body.clone(), &i.alice).await?;
        assert_eq!(
            member.status,
            StatusCode::FORBIDDEN,
            "{path}: {}",
            member.body
        );
        let visitor = call(&i.h.router, method, path, body, &Creds::default()).await?;
        assert_eq!(visitor.status, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(cli.asked().len(), asked);
    assert_eq!(cli.checks.load(Ordering::SeqCst), 0);
    Ok(())
}

/// AC6 (CCM-010) and CCM-008: "Check now" answers 202 with the view saying it runs, and the view
/// names what is wrong, with the fix, while runs get a Claude Code without the default model.
#[tokio::test]
async fn check_now_starts_a_check_and_the_view_says_what_is_wrong() -> TestResult {
    let i = instance().await?;
    // The incident: runs get the host's 2.1.311; the newest release was refused too.
    let cli = FakeCli::new(AgentCliStatus {
        version: Some(v("2.1.311")),
        source: Some(CliSource::Host),
        missing: Some(vec![
            DEFAULT_CLAUDE_MODEL.to_owned(),
            "claude-haiku-4-5".to_owned(),
        ]),
        result: Some(CheckResult::Refused(vec![(
            v("2.1.322"),
            vec![DEFAULT_CLAUDE_MODEL.to_owned()],
        )])),
        ..healthy()
    });
    i.h.app
        .set_agent_cli(cli.clone() as Arc<_>, Channel::Latest);
    assert_eq!(i.h.app.claude_code_for_new_runs(), Some(v("2.1.311")));
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    assert_eq!(view.body["source"], "host");
    assert_eq!(
        view.body["unknown_models"],
        json!([DEFAULT_CLAUDE_MODEL, "claude-haiku-4-5"])
    );
    let problem = view.body["problem"].as_str().ok_or("a problem")?;
    assert!(
        problem.starts_with("Claude Code 2.1.311 does not know claude-fable-5-2"),
        "{problem}"
    );
    // The newest release was refused for it: installing on the host would not help.
    let fix = view.body["fix"].as_str().ok_or("a fix")?;
    assert!(fix.contains("2.1.322, the newest, does not"), "{fix}");
    assert_eq!(view.body["result"], "refused");
    assert_eq!(
        view.body["result_text"],
        "Refused, nothing was installed: 2.1.322 does not know claude-fable-5-2 (Yulius's \
         default model)."
    );
    assert_eq!(
        view.body["refused"],
        json!([{"version": "2.1.322", "missing": [DEFAULT_CLAUDE_MODEL]}])
    );

    // Nothing refused yet (a failed check): the fix is the next check, or the host's command.
    if let Ok(mut status) = cli.status.lock() {
        status.result = Some(CheckResult::Failed(
            "registry.npmjs.org is unreachable".into(),
        ));
    }
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    assert_eq!(view.body["result"], "failed");
    assert_eq!(
        view.body["result_text"],
        "The check failed: registry.npmjs.org is unreachable."
    );
    let fix = view.body["fix"].as_str().ok_or("a fix")?;
    assert!(
        fix.contains("at its next check") && fix.contains(HOST_INSTALL),
        "{fix}"
    );
    assert_eq!(view.body["refused"], json!([]));

    // Runs get npm's newest release and it does not know the model: a check or the host's
    // install would not help, and the fix says so.
    if let Ok(mut status) = cli.status.lock() {
        status.result = Some(CheckResult::UpToDate(v("2.1.322")));
        status.version = Some(v("2.1.322"));
    }
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    let fix = view.body["fix"].as_str().ok_or("a fix")?;
    assert!(fix.contains("2.1.322, the newest, does not"), "{fix}");
    assert!(!fix.contains(HOST_INSTALL), "{fix}");

    // Review round 2 (major 1): that is said only from a fresh read of npm on a channel that
    // reads it. With updates off nobody reads it, whatever the last check on another channel
    // found a minute ago.
    if let Ok(mut status) = cli.status.lock() {
        status.channel = Channel::Off;
    }
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    let fix = view.body["fix"].as_str().ok_or("a fix")?;
    assert!(
        fix.starts_with("Claude Code updates are off: choose Latest") && fix.contains(HOST_INSTALL),
        "{fix}"
    );
    // A read of three hours ago is no news either: releases come more often than that.
    if let Ok(mut status) = cli.status.lock() {
        status.channel = Channel::Latest;
        status.result = Some(CheckResult::UpToDate(v("2.1.322")));
    }
    i.h.clock.0.fetch_add(3 * 60 * 60 * 1000, Ordering::SeqCst);
    let view = call(&i.h.router, Method::GET, VIEW, None, &i.admin).await?;
    let fix = view.body["fix"].as_str().ok_or("a fix")?;
    assert!(
        fix.contains("at its next check") && fix.contains(HOST_INSTALL),
        "{fix}"
    );

    let check = call(&i.h.router, Method::POST, CHECK, None, &i.admin).await?;
    assert_eq!(check.status, StatusCode::ACCEPTED, "{}", check.body);
    assert_eq!(check.body["checking"], true);
    assert_eq!(cli.checks.load(Ordering::SeqCst), 1);
    Ok(())
}
