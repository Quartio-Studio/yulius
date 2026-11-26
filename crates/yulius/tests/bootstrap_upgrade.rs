//! The upgrade path of the Infra bootstrap default: a release that raises the
//! bootstrap revision gives each onboarded sandbox one upgrade task, once.

use crate::support;

use yulius_app::bootstrap_upgrade::{BOOTSTRAP_REVISION, UPGRADE_TITLE};
use yulius_app::ports::store::{AccountStore as _, TaskStore as _};
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An onboarded sandbox gets one Infra bootstrap task for the new revision,
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
    // Without a connected Claude account no task is created; the claim stays, for the next start.
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
    assert_eq!(
        task.origin,
        yulius_app::model::TaskOrigin::Automation,
        "it holds nobody's secrets"
    );
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
    // Once its onboarding (which laid the current default) is done, it still gets none.
    h.store
        .execute_for_tests("UPDATE sandboxes SET onboarding_state = 'ready' WHERE id = 's2'")
        .await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    assert!(
        h.store
            .tasks(&SandboxId::parse("s2")?, false)
            .await?
            .is_empty(),
        "onboarded at the current revision: nothing to upgrade"
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

/// A world with one onboarded sandbox `s1` of alice's and a connected Claude account.
async fn world() -> Result<support::Harness, Box<dyn std::error::Error>> {
    let h = support::harness()?;
    support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    support::seed_github_user(&h, "u-alice", "alice").await?;
    h.github
        .set_perm("alice", "acme/app", support::perms("write"));
    support::seed_sandbox(&h, "s1", "app", "u-alice").await?;
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
    Ok(h)
}

/// The upgrade tasks of `s1`.
async fn upgrades(h: &support::Harness) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(h.store
        .tasks(&SandboxId::parse("s1")?, false)
        .await?
        .into_iter()
        .filter(|t| t.title == UPGRADE_TITLE)
        .map(|t| t.id.as_str().to_owned())
        .collect())
}

/// What `bootstrap_upgrades` records for `s1`: its revision and task.
async fn recorded(
    h: &support::Harness,
) -> Result<Option<(u32, Option<String>)>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store_ext::StagingStore as _;
    Ok(h.store
        .bootstrap_upgrade(&SandboxId::parse("s1")?)
        .await?
        .map(|r| (r.revision, r.task_id.map(|t| t.as_str().to_owned()))))
}

/// SPEC-575 MFS-012 (review round 1 of slice 4): a linked upgrade task a person deletes is not
/// made again, and playgrounds, removed sandboxes and ones whose creator is no longer active get
/// none.
#[tokio::test]
async fn a_deleted_upgrade_task_and_ineligible_sandboxes_get_none() -> TestResult {
    let h = world().await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 1);
    let made = upgrades(&h).await?;
    assert!(
        h.store
            .delete_task(&yulius_domain::ids::TaskId::parse(&made[0])?)
            .await?
    );
    assert_eq!(
        h.app.create_bootstrap_upgrades().await,
        0,
        "a deleted task is not remade"
    );
    assert!(upgrades(&h).await?.is_empty());
    for (id, slug, change) in [
        (
            "s4",
            "gone",
            "UPDATE sandboxes SET removed_at = 9 WHERE id = 's4'",
        ),
        (
            "s5",
            "play",
            "UPDATE sandboxes SET repo_owner = '' WHERE id = 's5'",
        ),
    ] {
        support::seed_sandbox(&h, id, slug, "u-alice").await?;
        h.store.execute_for_tests(change).await?;
    }
    support::seed_github_user(&h, "u-bob", "bob").await?;
    support::seed_sandbox(&h, "s6", "bobs", "u-bob").await?;
    h.store
        .execute_for_tests("UPDATE users SET disabled_at = 9 WHERE id = 'u-bob'")
        .await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    for id in ["s4", "s5", "s6"] {
        assert!(
            h.store
                .tasks(&SandboxId::parse(id)?, false)
                .await?
                .is_empty(),
            "{id} gets no upgrade task"
        );
    }
    Ok(())
}

/// SPEC-575 MFS-012 (rollout blocker found on PR #1541): a server that stopped after claiming
/// the revision but before creating the task leaves a claim with no task. The next start
/// finishes it: one task, linked.
#[tokio::test]
async fn a_claim_left_without_its_task_is_finished_at_the_next_start() -> TestResult {
    let h = world().await?;
    // The state a stop right after the claim leaves.
    h.store
        .execute_for_tests(&format!(
            "INSERT INTO bootstrap_upgrades (sandbox_id, revision, state, task_id, updated_at) \
             VALUES ('s1', {BOOTSTRAP_REVISION}, 'claimed', NULL, 5)"
        ))
        .await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 1);
    let made = upgrades(&h).await?;
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(
        recorded(&h).await?,
        Some((BOOTSTRAP_REVISION, Some(made[0].clone())))
    );
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    assert_eq!(upgrades(&h).await?.len(), 1);
    Ok(())
}

/// SPEC-575 MFS-012 (rollout blocker found on PR #1541): a server that stopped after creating
/// the task but before linking it leaves the task and a claim with no task. The next start
/// adopts that task, exactly once, and creates no second one.
#[tokio::test]
async fn a_task_created_but_not_linked_is_adopted_once() -> TestResult {
    let h = world().await?;
    assert_eq!(h.app.create_bootstrap_upgrades().await, 1);
    let made = upgrades(&h).await?;
    assert_eq!(made.len(), 1);
    // The state a stop between the creation and the link leaves.
    h.store
        .execute_for_tests(
            "UPDATE bootstrap_upgrades SET state = 'claimed', task_id = NULL WHERE sandbox_id = 's1'",
        )
        .await?;
    assert_eq!(
        h.app.create_bootstrap_upgrades().await,
        0,
        "the existing task is adopted, none is created"
    );
    assert_eq!(upgrades(&h).await?, made);
    assert_eq!(
        recorded(&h).await?,
        Some((BOOTSTRAP_REVISION, Some(made[0].clone())))
    );
    assert_eq!(h.app.create_bootstrap_upgrades().await, 0);
    assert_eq!(upgrades(&h).await?, made);
    Ok(())
}
