//! The production GitHub adapter's checks over the in-process fake GitHub (#111, `F 2026-10-10`):
//! an App without one of the check permissions still reads what it may, and the port says which
//! sources it could not read. No real GitHub call.

use yulius::wiring::github_client::GitHubAdapter;
use yulius_app::model::Secret;
use yulius_app::ports::github::{ChecksSource, ChecksState, GitHubPort};
use yulius_github::GitHubPermissions;
use yulius_github::fake::{FakeCheckRun, FakeGitHub, FakeStatus};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn the_adapter_reads_what_the_app_may_and_names_the_rest() -> TestResult {
    let fake = FakeGitHub::start().await?;
    let adapter = GitHubAdapter::new(fake.config())?;
    let root = fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    let writer = GitHubPermissions {
        push: true,
        pull: true,
        ..GitHubPermissions::default()
    };
    fake.set_permission("acme", "app", "dev", writer);
    let token = Secret(fake.issue_user_token("dev").expose().to_owned());
    let head = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &head,
        vec![FakeCheckRun::new("test", "completed", Some("success"))],
    );
    fake.set_statuses(
        "acme",
        "app",
        &head,
        vec![FakeStatus::new("ci/build", "success")],
    );

    // Everything readable.
    let report = adapter.checks(&token, "acme", "app", &head).await?;
    assert_eq!(report.state, ChecksState::Success);
    assert!(report.unreadable.is_empty());

    // 403 on check runs, 200 on statuses (and on Actions runs, which report nothing here).
    fake.withhold_permission("checks");
    let report = adapter.checks(&token, "acme", "app", &head).await?;
    assert_eq!(report.state, ChecksState::Success);
    assert_eq!(report.unreadable, vec![ChecksSource::CheckRuns]);

    // Nothing readable: unknown, never green.
    fake.withhold_permission("statuses");
    fake.withhold_permission("actions");
    let report = adapter.checks(&token, "acme", "app", &head).await?;
    assert_eq!(report.state, ChecksState::Unknown);
    assert_eq!(
        report.unreadable,
        vec![
            ChecksSource::CheckRuns,
            ChecksSource::CommitStatuses,
            ChecksSource::WorkflowRuns
        ]
    );
    Ok(())
}
