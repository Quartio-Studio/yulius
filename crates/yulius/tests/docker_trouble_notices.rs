//! SPEC-494 DFJ-007: while a sandbox's Docker front is full (a `docker` connection trouble), each
//! live task of the sandbox is told once in its session that nobody needs to be asked, and once
//! the trouble ends, the tasks told hear that Docker works again. A fake agent echoes what it is
//! typed.

use std::time::Duration;

use yulius_app::docker_trouble::{DOCKER_BACK, DOCKER_FULL};
use yulius_domain::connection_health::{Limit, Refusal};

use crate::pool_supervision::{build_tuned, new_task, runtime, screen, seed, stop, temp, wait_for};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Passes until one types something (bounded).
async fn until_typed(
    app: &std::sync::Arc<yulius_app::context::App>,
) -> Result<Vec<(yulius_domain::ids::TaskId, &'static str)>, Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let typed = app.tell_docker_troubles().await;
        if !typed.is_empty() {
            return Ok(typed);
        }
        if std::time::Instant::now() > deadline {
            return Err("no notice was typed".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// AC-5: told once per trouble, then told it works again; another sandbox's trouble says nothing.
#[test]
fn a_sandboxs_tasks_hear_of_its_docker_trouble_once_and_of_its_end() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, &fake, agent, |config| config.goal_ready_ms = 2_000)?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Docker user").await?;
        wait_for("the first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: Docker user: answer with an echo") }
        })
        .await?;
        let health = app.connection_health();
        let full = |sandbox: &str| Refusal {
            limit: Limit::Docker,
            sandbox: Some(sandbox.to_owned()),
            held: 256,
            max: 256,
            holders: Vec::new(),
        };
        // Another sandbox's trouble is not this task's.
        health.refused_times(full("another"), 1, 1);
        assert!(app.tell_docker_troubles().await.is_empty());
        health.refused_times(full(sandbox.as_str()), 3, 2);
        // A session still starting takes no typed text: a later pass tells it.
        let typed = until_typed(&app).await?;
        assert_eq!(typed, vec![(id.clone(), DOCKER_FULL)]);
        wait_for("the notice echoed", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("do not raise a Needs you about it") }
        })
        .await?;
        // Within the same trouble, more refusals and passes tell nothing more.
        health.refused_times(full(sandbox.as_str()), 5, 3);
        assert!(app.tell_docker_troubles().await.is_empty());
        // Room again: the trouble ends, and the task told hears it once.
        health.held(Limit::Docker, Some(sandbox.as_str()), 10);
        let typed = app.tell_docker_troubles().await;
        assert_eq!(typed, vec![(id.clone(), DOCKER_BACK)]);
        assert!(app.tell_docker_troubles().await.is_empty());
        wait_for("the end echoed", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("Docker works again") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok(())
    })
}
