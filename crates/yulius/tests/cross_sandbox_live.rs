//! SPEC-442 end to end (AC-XST-1, AC-XST-2): from the Superagent of one sandbox, a task asked for
//! prepared in another is started, then messaged, each once its person approves the request on
//! the panel, with real processes: supervisor → detached `yulius ptyhost` → fake agent.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::coordination::{Channel, ProjectCaller};
use yulius_app::cross_sandbox::CrossAction;
use yulius_app::ports::store::AuditStore;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;
use yulius_ptyhost::test_support::locate_fake_agent;

use crate::first_message::{data_dir, hooked, runtime, screen, start, stop, wait_for};
use crate::pool_supervision::{ScriptedUsage, seed};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;

type Error = Box<dyn std::error::Error>;

/// Asks for `action` as `caller`, answers the one waiting request as `person` (a browser
/// session, as the panel's click is), and returns the call's outcome.
async fn approved(
    app: &Arc<App>,
    person: &AuthContext,
    caller: &ProjectCaller,
    action: CrossAction,
    args: Value,
) -> Result<Value, Error> {
    let call = {
        let app = app.clone();
        let caller = caller.clone();
        tokio::spawn(async move { app.tool_sandbox_task_request(&caller, action, &args).await })
    };
    let source = caller.sandbox.as_str();
    wait_for("the request on the panel", Duration::from_secs(10), || async {
        app.superagent_requests(person, source).len() == 1
    })
    .await?;
    let listed = app.superagent_requests(person, source);
    let id = listed[0]["id"].as_str().ok_or("id")?;
    app.superagent_request_answer(person, source, id, true)
        .await?;
    Ok(call.await??)
}

#[test]
fn a_prepared_task_of_another_sandbox_is_started_and_messaged_from_a_superagent() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start(&data, &fake, agent, 0, Arc::new(ScriptedUsage::default())).await?;
        // The task runs in `pool`; the person's Superagent is in `home`.
        let (pool, user) = seed(&app, &data, &["a-only"]).await?;
        let mut home = app.deps.store.sandbox(&pool).await?.ok_or("pool")?;
        home.id = SandboxId::parse("s-home")?;
        home.slug = "home".into();
        home.repo_name = "home".into();
        home.github_repo_id = Some(2);
        home.port_base = 41300;
        app.deps.store.insert_sandbox(&home).await?;
        let superagent = ProjectCaller {
            sandbox: home.id.clone(),
            user: user.clone(),
            task: None,
            channel: Channel::Superagent,
            actor: "cockpit:admin".into(),
            detail: json!({"via": "cockpit", "shell": "sh-home"}),
        };
        let person = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: Some("csrf".into()),
            session_hash: None,
            token: None,
        };

        // Asked for prepared, as on 2026-11-07.
        let created = approved(
            &app,
            &person,
            &superagent,
            CrossAction::Create,
            json!({"sandbox": "pool", "prompt": "Budget: answer with an echo", "mode": "prompt", "prepared": true}),
        )
        .await?;
        assert_eq!(created["outcome"], "approved", "{created}");
        let id = TaskId::parse(created["task"]["task_id"].as_str().ok_or("task_id")?)?;
        let number = created["task"]["number"].clone();
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(task.sandbox_id, pool);
        assert_eq!(task.status, TaskStatus::Prepared);

        // It should have been queued: started from home, no delete and no new task.
        let started = approved(
            &app,
            &person,
            &superagent,
            CrossAction::Update,
            json!({"sandbox": "pool", "number": number}),
        )
        .await?;
        assert_eq!(started["outcome"], "approved", "{started}");
        wait_for("the task to run", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Budget: answer with an echo")
        })
        .await?;
        assert_eq!(app.deps.store.tasks(&pool, false).await?.len(), 1);

        // Told something by the person, from home.
        let sent = approved(
            &app,
            &person,
            &superagent,
            CrossAction::Message,
            json!({"sandbox": "pool", "number": number, "text": "You may use the DO budget."}),
        )
        .await?;
        assert_eq!(sent["outcome"], "approved", "{sent}");
        assert_eq!(sent["delivered"], "sent", "{sent}");
        wait_for("the message", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Message from admin (through their Superagent):")
        })
        .await?;
        assert!(
            screen(&app, &id).contains("You may use the DO budget."),
            "{}",
            screen(&app, &id)
        );

        // Each answer is in the audit of both sandboxes.
        for sandbox in [pool.as_str(), home.id.as_str()] {
            let rows = app
                .deps
                .store
                .audit_details(sandbox, "superagent.cross_task", 10)
                .await?;
            assert_eq!(rows.len(), 3, "{sandbox}: {rows:?}");
            assert!(
                rows.iter().all(|r| !r.contains("DO budget")),
                "{sandbox}: {rows:?}"
            );
        }
        stop(&app, &id).await
    })
}
