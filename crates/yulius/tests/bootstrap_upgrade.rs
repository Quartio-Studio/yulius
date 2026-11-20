//! The upgrade path of the Infra bootstrap default (SPEC-575 MFS-012): a release that raises the
//! bootstrap revision gives each onboarded sandbox one upgrade task, once.

use crate::support;

use yulius_app::bootstrap_upgrade::{BOOTSTRAP_REVISION, UPGRADE_TITLE};
use yulius_app::ports::store::{AccountStore as _, TaskStore as _};
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// SPEC-575 MFS-012: an onboarded sandbox gets one Infra bootstrap task for the new revision,
/// owned by its creator, following the Infra bootstrap workflow and asking to keep what is the
/// project's own. A second start creates none, and a sandbox still onboarding gets none (its
/// onboarding lays the current default).
#[tokio::test]
async fn an_onboarded_sandbox_gets_one_upgrade_task_per_revision() -> TestResult {
    let h = support::harness()?;
    support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    support::seed_github_user(&h, "u-alice", "alice").await?;
    h.github
        .set_perm("alice", "acme/app", support::perms("write"));
    h.github
        .set_perm("alice", "acme/fresh", support::perms("write"));
    // Without a connected Claude account no task is created, and the claim is given back.
    support::seed_sandbox(&h, "s1", "app", "u-alice").await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    assert!(
        h.store
            .tasks(&SandboxId::parse("s1")?, false)
            .await?
            .is_empty()
    );
    h.store
        .insert_account(&yulius_app::model::AccountRecord {
            id: yulius_domain::ids::AccountId::parse("a-1")?,
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "a-1".into(),
            credential: Some(
                h.app
                    .deps
                    .crypto
                    .seal(b"synthetic-subscription-credential")?,
            ),
            status: yulius_app::model::AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            email_typed: false,
            created_at: yulius_domain::time::Timestamp(1),
            updated_at: yulius_domain::time::Timestamp(1),
        })
        .await?;
    support::seed_sandbox(&h, "s2", "fresh", "u-alice").await?;
    h.store
        .execute_for_tests("UPDATE sandboxes SET onboarding_state = 'pending' WHERE id = 's2'")
        .await?;
    assert_eq!(BOOTSTRAP_REVISION, 1);
    assert_eq!(h.app.create_bootstrap_upgrades().await, 1);
    let tasks = h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let upgrade: Vec<_> = tasks.iter().filter(|t| t.title == UPGRADE_TITLE).collect();
    assert_eq!(upgrade.len(), 1, "{tasks:?}");
    let task = upgrade[0];
    assert_eq!(task.owner.as_str(), "u-alice");
    assert_eq!(task.workflow_id.as_deref(), Some("wf-infra-bootstrap"));
    for asked in [
        "Keep every project-specific command, CI job, CD target, secret name and vault grant",
        "`merge_first = true`",
        "`after_merge = true`",
        "`[local]`",
        "no shared target",
        "Reconcile repository guidance",
        "nothing deploys twice",
    ] {
        assert!(task.prompt.contains(asked), "{asked}: {}", task.prompt);
    }
    assert!(
        h.store
            .tasks(&SandboxId::parse("s2")?, false)
            .await?
            .is_empty(),
        "a sandbox still onboarding gets none"
    );
    // Another start: the revision is recorded, nothing more is created.
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    assert_eq!(
        h.store
            .tasks(&SandboxId::parse("s1")?, false)
            .await?
            .iter()
            .filter(|t| t.title == UPGRADE_TITLE)
            .count(),
        1
    );
    Ok(())
}
