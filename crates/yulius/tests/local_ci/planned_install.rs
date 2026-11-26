//! A completed producer and its last running consumer across a planned update.
use super::{BoxResult, Fixture, RunState, TestResult, WAIT, Weighing, fixture_tuned};
use crate::support::updates::{FakeHelper, FakeSource, NEW, OLD, services};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use yulius_app::context::App;
use yulius_app::ports::store_ext::{CiStore as _, UpdateStore as _};
use yulius_app::updates::UpdateRunRecord;
use yulius_contracts::update::HelperAction;
use yulius_domain::update::RunState as UpdateState;

struct Install {
    f: Fixture,
    run: String,
    helper: Arc<FakeHelper>,
    weighing: Arc<Weighing>,
}

impl Install {
    // Only the PR writes these files. CI also runs the integration branch under the same budget.
    async fn ready(target: &str) -> BoxResult<Self> {
        let f = fixture_tuned(false, |dir| format!(r#"[[ci.jobs]]
name = "producer"
cpus = 1
command = 'if test -f feature.txt; then echo "$YULIUS_CI_RUN" >> "{d}/calls"; echo "$YULIUS_CI_RUN" > "{d}/artifact"; fi'

[[ci.jobs]]
name = "consumer"
cpus = 1
command = 'if test -f feature.txt; then sh "{d}/consumer.sh" "{d}"; fi'
"#, d=dir.display()), None, |c| c.ci.job_timeout = WAIT).await?;
        std::fs::write(
            f.data.join("consumer.sh"),
            r#"set -eu
echo "$YULIUS_CI_RUN" >> "$1/consumer-calls"
n=0
while test ! -e "$1/release" || test ! -s "$1/artifact"; do
    n=$((n+1))
    test "$n" -lt 600 || exit 2
    sleep 0.1
done
test "$(cat "$1/artifact")" = "$YULIUS_CI_RUN"
test ! -e "$1/fail"
"#,
        )?;
        let weighing = Arc::new(Weighing::default());
        assert!(f.h.app.set_isolation(weighing.clone()));
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        let run = f.running("pr/1").await?["id"]
            .as_str()
            .ok_or("PR run")?
            .to_owned();
        tokio::time::timeout(WAIT, async {
            loop {
                let jobs = f.h.store.ci_jobs(&run, false).await?;
                if jobs
                    .iter()
                    .any(|j| j.key == "producer" && j.state == RunState::Passed)
                    && jobs
                        .iter()
                        .any(|j| j.key == "consumer" && j.state == RunState::Running)
                    && f.data.join("consumer-calls").is_file()
                {
                    return Ok::<_, yulius_app::error::AppError>(());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        let helper = FakeHelper::installed();
        assert!(
            f.h.app
                .set_updates(services(OLD, &FakeSource::behind(), &helper))
        );
        let now = f.h.app.now();
        assert!(
            f.h.store
                .insert_update_run(&UpdateRunRecord {
                    id: "planned-install".into(),
                    target_commit: target.into(),
                    from_commit: Some(OLD.into()),
                    summary: "Install".into(),
                    state: UpdateState::Draining,
                    outcome: None,
                    error: None,
                    requested_by: "admin".into(),
                    created_at: now,
                    state_since: now,
                    finished_at: None,
                })
                .await?
        );
        f.h.app.update_round().await;
        f.h.app.update_round().await;
        if target == NEW {
            assert!(
                helper.last().is_none(),
                "the ready install interrupted the consumer"
            );
        }
        Ok(Self {
            f,
            run,
            helper,
            weighing,
        })
    }

    async fn assert_gate(&self, expected: RunState, executions: usize) -> TestResult {
        let stored = self.f.h.store.ci_run(&self.run).await?.ok_or("run")?;
        assert_eq!(stored.state, expected);
        assert_eq!(
            self.f
                .h
                .app
                .ci_gate_result(&self.f.sandbox, &stored.sha)
                .await,
            yulius_domain::ci::GateResult::Run(Some(expected))
        );

        for log in ["calls", "consumer-calls"] {
            let calls = std::fs::read_to_string(self.f.data.join(log))?;
            assert_eq!(
                calls.lines().collect::<Vec<_>>(),
                vec![self.run.as_str(); executions],
                "{log}"
            );
        }
        let jobs = self.f.h.store.ci_jobs(&self.run, false).await?;
        assert!(
            jobs.iter().any(|j| j.key == "producer"
                && j.state == RunState::Passed
                && j.exit_code == Some(0))
        );
        assert!(
            jobs.iter()
                .any(|j| j.key == "consumer" && j.state == expected)
        );
        Ok(())
    }
}

#[tokio::test]
async fn a_planned_install_lets_the_gate_finish_with_its_actual_result() -> TestResult {
    for verdict in [RunState::Passed, RunState::Failed] {
        let i = Install::ready(NEW).await?;
        if verdict == RunState::Failed {
            std::fs::write(i.f.data.join("fail"), "")?;
        }
        std::fs::write(i.f.data.join("release"), "")?;
        i.f.settle().await?;
        i.assert_gate(verdict, 1).await?;
        i.f.h.app.update_round().await;
        assert_eq!(
            i.helper.last().ok_or("install")?.action,
            HelperAction::Install
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_same_build_restart_keeps_its_original_timing_with_ci_running() -> TestResult {
    let i = Install::ready(OLD).await?;
    assert_eq!(
        i.helper.last().ok_or("restart")?.action,
        HelperAction::Restart
    );
    std::fs::write(i.f.data.join("release"), "")?;
    i.f.settle().await?;
    Ok(())
}

#[tokio::test]
async fn proceed_installs_while_the_last_ci_job_still_runs() -> TestResult {
    use axum::http::{Method, StatusCode};
    let i = Install::ready(NEW).await?;
    let admin = crate::support::superadmin_bearer(&i.f.h).await?;
    let view =
        crate::support::call(&i.f.h.router, Method::GET, "/api/update", None, &admin).await?;
    assert!(
        view.body["run"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("CI")),
        "{}",
        view.body
    );
    let response = crate::support::call(
        &i.f.h.router,
        Method::POST,
        "/api/admin/update/proceed",
        None,
        &admin,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    i.f.h.app.update_round().await;
    assert_eq!(
        i.helper.last().ok_or("urgent install")?.action,
        HelperAction::Install
    );
    std::fs::write(i.f.data.join("release"), "")?;
    i.f.settle().await?;
    Ok(())
}

#[test]
fn the_deadline_installs_and_recovery_rebuilds_the_missing_output() -> TestResult {
    // Drop only the test daemon's executor. Keep its SQLite store, clone and detached job hosts.
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    };
    let old = runtime()?;
    let i = old.block_on(async {
        let i = Install::ready(NEW).await?;
        i.f.h.clock.0.fetch_add(29_000, Ordering::SeqCst);
        i.f.h.app.update_round().await;
        assert!(i.helper.last().is_none(), "still within the grace period");
        i.f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        i.f.h.app.update_round().await;
        assert_eq!(
            i.helper.last().ok_or("deadline")?.action,
            HelperAction::Install
        );
        Ok::<_, Box<dyn std::error::Error>>(i)
    })?;
    assert!(i.weighing.history().last().is_some_and(|n| *n > 0));
    drop(old);
    assert_eq!(
        i.weighing.history().last(),
        Some(&0),
        "old executor released its grants"
    );
    let artifact = i.f.data.join("artifact");
    assert_eq!(std::fs::read_to_string(&artifact)?.trim(), i.run);
    std::fs::remove_file(artifact)?;
    std::fs::write(i.f.data.join("release"), "")?;
    runtime()?.block_on(async {
        let restarted = App::new(i.f.h.app.config.clone(), i.f.h.app.deps.clone());
        let weighing = Arc::new(Weighing::default());
        assert!(restarted.set_isolation(weighing.clone()));
        assert!(restarted.set_updates(services(NEW, &FakeSource::behind(), &i.helper)));
        restarted.restore_update_hold().await;
        let version = crate::support::call(
            &yulius_server::internal_router(restarted.clone()),
            axum::http::Method::GET,
            "/internal/version",
            None,
            &crate::support::Creds::default(),
        )
        .await?;
        assert_eq!(version.body["commit"], NEW);
        assert!(weighing.history().is_empty());
        assert_eq!(
            i.f.h
                .store
                .ci_run(&i.run)
                .await?
                .ok_or("interrupted run")?
                .state,
            RunState::Running
        );
        let before = i.f.h.store.ci_jobs(&i.run, false).await?;
        restarted.start_ci();
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(run) = i.f.h.store.ci_run(&i.run).await?
                    && !run.state.is_active()
                {
                    return Ok::<_, yulius_app::error::AppError>(());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        assert!(
            weighing.history().iter().any(|n| *n > 0),
            "fresh grants on replay"
        );
        let after = i.f.h.store.ci_jobs(&i.run, false).await?;
        assert!(before.iter().map(|j| &j.id).eq(after.iter().map(|j| &j.id)));
        i.assert_gate(RunState::Passed, 2).await
    })
}
