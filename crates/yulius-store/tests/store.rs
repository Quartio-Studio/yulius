//! Behavioral tests of the SQLite store contracts used for concurrency and recovery.

use yulius_app::error::AppError;
use yulius_app::model::{
    AttemptRecord, DeviceGrantBounds, DeviceGrantInsert, DeviceGrantRecord, LaunchKind,
    MessageRecord, SandboxRecord, StepRecord, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::store::{
    AttemptUpdate, CoordinationStore, InstanceStore, SandboxStore, SessionStore, TaskStore,
    UserStore,
};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, MessageId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_store::SqliteStore;

type R = Result<(), Box<dyn std::error::Error>>;

fn user(
    id: &str,
    login: Option<&str>,
    superadmin: bool,
) -> Result<UserRecord, Box<dyn std::error::Error>> {
    Ok(UserRecord {
        id: UserId::parse(id)?,
        github_login: login.map(str::to_owned),
        github_id: None,
        display_name: id.to_owned(),
        avatar_url: None,
        is_superadmin: superadmin,
        is_instance_admin: false,
        password_hash: superadmin.then(|| "$argon2id$x".to_owned()),
        created_at: Timestamp(1),
        disabled_at: None,
    })
}

async fn seed(store: &SqliteStore) -> Result<(SandboxId, UserId), Box<dyn std::error::Error>> {
    let u = user("u1", Some("alice"), false)?;
    store.insert_user(&u).await?;
    let sid = SandboxId::parse("s1")?;
    store
        .insert_sandbox(&SandboxRecord {
            id: sid.clone(),
            slug: "studio".into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: "studio".into(),
            github_repo_id: Some(5),
            default_branch: "main".into(),
            profile_json: "{}".into(),
            port_base: 20000,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: u.id.clone(),
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    Ok((sid, u.id))
}

fn task(
    id: &str,
    sandbox: &SandboxId,
    by: &UserId,
) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    Ok(TaskRecord {
        id: TaskId::parse(id)?,
        sandbox_id: sandbox.clone(),
        number: 0,
        title: "t".into(),
        prompt: "p".into(),
        harness: Harness::ClaudeCode,
        model: None,
        effort: None,
        workflow_id: None,
        status: TaskStatus::Starting,
        activity: None,
        branch: None,
        worktree_path: None,
        spec_path: None,
        origin: TaskOrigin::Ui,
        automation_run: None,
        created_by: by.clone(),
        owner: by.clone(),
        created_at: Timestamp(10),
        updated_at: Timestamp(10),
        closed_at: None,
        archived_at: None,
        queued_at: None,
        gate_blocks: 0,
        wake_enabled: true,
        mode: yulius_app::model::TaskMode::Prompt,
    })
}

fn attempt(id: &str, task: &TaskId, seq: u32) -> Result<AttemptRecord, Box<dyn std::error::Error>> {
    Ok(AttemptRecord {
        id: AttemptId::parse(id)?,
        task_id: task.clone(),
        seq,
        harness: Harness::ClaudeCode,
        account_id: None,
        provider_session_id: None,
        launch_kind: LaunchKind::New,
        state: AttemptState::Spawning,
        run_dir: format!("/tmp/{id}"),
        token_hash: format!("hash-{id}"),
        host_token: yulius_app::model::Sealed(vec![1, 2, 3]),
        ptyhost_pid: None,
        child_pid: None,
        started_at: Timestamp(20),
        ended_at: None,
        exit_code: None,
        exit_reason: None,
    })
}

#[tokio::test]
async fn setup_creates_exactly_one_superadmin() -> R {
    let store = SqliteStore::open_in_memory()?;
    store.init_instance("https://y.example", "tokhash").await?;
    let again = store.init_instance("https://other", "other").await?;
    assert_eq!(again.public_url, "https://y.example", "init is idempotent");
    store
        .complete_setup(&user("admin", None, true)?, Timestamp(5))
        .await?;
    let second = store
        .complete_setup(&user("admin2", None, true)?, Timestamp(6))
        .await;
    assert!(matches!(second, Err(AppError::Conflict(_))));
    let instance = store.instance().await?.ok_or("instance")?;
    assert_eq!(instance.setup_token_hash, None, "token is cleared");
    assert!(store.superadmin().await?.is_some());
    Ok(())
}

#[tokio::test]
async fn task_numbers_and_status_cas() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let a = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let b = store.insert_task(&task("t2", &sid, &uid)?).await?;
    assert_eq!((a.number, b.number), (1, 2));
    // Two racing transitions from Starting: only the first wins.
    let first = store
        .transition_task(
            &a.id,
            Some(TaskStatus::Starting),
            TaskStatus::Running,
            None,
            Timestamp(30),
        )
        .await?;
    let second = store
        .transition_task(
            &a.id,
            Some(TaskStatus::Starting),
            TaskStatus::Failed(yulius_domain::task::FailReason::StartTimeout),
            None,
            Timestamp(31),
        )
        .await?;
    assert!(first && !second);
    store
        .transition_task(
            &a.id,
            Some(TaskStatus::Running),
            TaskStatus::Closed,
            None,
            Timestamp(40),
        )
        .await?;
    let closed = store.task(&a.id).await?.ok_or("task")?;
    assert_eq!(closed.status, TaskStatus::Closed);
    assert_eq!(closed.closed_at, Some(Timestamp(40)));
    Ok(())
}

/// SPEC-070: one person at a time asks for control, and control passes only from the owner the
/// decision saw to the person whose request still waits.
#[tokio::test]
async fn control_passes_only_on_the_request_the_owner_saw() -> R {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, alice) = seed(&store).await?;
    for (id, login) in [("u2", "bob"), ("u3", "carol")] {
        store.insert_user(&user(id, Some(login), false)?).await?;
    }
    let (bob, carol) = (UserId::parse("u2")?, UserId::parse("u3")?);
    let t = store.insert_task(&task("t1", &sid, &alice)?).await?;
    assert_eq!(store.control_request(&t.id).await?, None);
    // Nobody asked: nothing passes.
    assert!(
        !store
            .pass_control(&t.id, &alice, &bob, Timestamp(20))
            .await?
    );

    assert!(store.request_control(&t.id, &bob, Timestamp(20)).await?);
    // Asking again keeps the first request; another person waits for none.
    assert!(store.request_control(&t.id, &bob, Timestamp(25)).await?);
    assert!(!store.request_control(&t.id, &carol, Timestamp(25)).await?);
    let waiting = store.control_request(&t.id).await?.ok_or("request")?;
    assert_eq!(
        (waiting.by.clone(), waiting.at),
        (bob.clone(), Timestamp(20))
    );
    // Only the requester's request is dropped by their id.
    assert!(
        !store
            .drop_control_request(&t.id, &carol, Timestamp(26))
            .await?
    );
    assert!(
        store
            .drop_control_request(&t.id, &bob, Timestamp(26))
            .await?
    );
    assert_eq!(store.control_request(&t.id).await?, None);

    assert!(store.request_control(&t.id, &carol, Timestamp(30)).await?);
    // A stale decision (another requester, another owner) changes nothing.
    assert!(
        !store
            .pass_control(&t.id, &alice, &bob, Timestamp(31))
            .await?
    );
    assert!(
        !store
            .pass_control(&t.id, &bob, &carol, Timestamp(31))
            .await?
    );
    assert!(
        store
            .pass_control(&t.id, &alice, &carol, Timestamp(32))
            .await?
    );
    let passed = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(passed.owner, carol);
    assert_eq!(passed.created_by, alice);
    assert_eq!(passed.updated_at, Timestamp(32));
    assert_eq!(store.control_request(&t.id).await?, None);
    Ok(())
}

#[tokio::test]
async fn only_one_live_attempt_and_terminal_state_is_final() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    store.insert_attempt(&attempt("a1", &t.id, 1)?).await?;
    let dup = store.insert_attempt(&attempt("a2", &t.id, 2)?).await;
    assert!(
        matches!(dup, Err(AppError::Conflict(_))),
        "second live attempt refused"
    );
    let a1 = AttemptId::parse("a1")?;
    assert!(
        store
            .transition_attempt(
                &a1,
                AttemptState::Live,
                &AttemptUpdate {
                    child_pid: Some(42),
                    ..Default::default()
                }
            )
            .await?
    );
    assert!(
        store
            .transition_attempt(
                &a1,
                AttemptState::Exited,
                &AttemptUpdate {
                    exit_code: Some(0),
                    ended_at: Some(Timestamp(50)),
                    ..Default::default()
                }
            )
            .await?
    );
    // A late "killed" after the exit was recorded is refused (one durable outcome).
    assert!(
        !store
            .transition_attempt(&a1, AttemptState::Killed, &AttemptUpdate::default())
            .await?
    );
    // Nor is the same outcome recorded twice: the second observer of the end is not first
    // (SPEC-065, a lost run seen by its escalation and by its lost connection).
    assert!(
        !store
            .transition_attempt(
                &a1,
                AttemptState::Exited,
                &AttemptUpdate {
                    exit_code: Some(9),
                    ended_at: Some(Timestamp(60)),
                    ..Default::default()
                }
            )
            .await?
    );
    let stored = store.attempt(&a1).await?.ok_or("attempt")?;
    assert_eq!(
        (stored.state, stored.exit_code, stored.child_pid),
        (AttemptState::Exited, Some(0), Some(42))
    );
    // After the first run ended, a new live attempt is allowed.
    store.insert_attempt(&attempt("a2", &t.id, 2)?).await?;
    assert_eq!(store.live_attempts().await?.len(), 1);
    Ok(())
}

/// Bounds small enough to reach in a test.
const BOUNDS: DeviceGrantBounds = DeviceGrantBounds {
    per_client: 2,
    total: 3,
};

/// A pending grant of `client` created at `at`, live for 1000 ms.
fn grant(user_code: &str, hash: &str, client: &str, at: i64) -> DeviceGrantRecord {
    DeviceGrantRecord {
        user_code: user_code.into(),
        device_code_hash: hash.into(),
        status: "pending".into(),
        user_id: None,
        token: None,
        client_key: client.into(),
        created_at: Timestamp(at),
        expires_at: Timestamp(at + 1000),
    }
}

/// #169: live grants are bounded per client and per instance; a refused grant is not stored.
#[tokio::test]
async fn device_grants_are_bounded_per_client_and_instance() -> R {
    let store = SqliteStore::open_in_memory()?;
    let insert = async |code: &str, client: &str| {
        store
            .insert_device_grant(&grant(code, code, client, 1), BOUNDS)
            .await
    };
    assert_eq!(insert("A1", "a").await?, DeviceGrantInsert::Inserted);
    assert_eq!(insert("A2", "a").await?, DeviceGrantInsert::Inserted);
    assert_eq!(insert("A3", "a").await?, DeviceGrantInsert::ClientFull);
    assert_eq!(insert("B1", "b").await?, DeviceGrantInsert::Inserted);
    assert_eq!(insert("C1", "c").await?, DeviceGrantInsert::InstanceFull);
    for refused in ["A3", "C1"] {
        assert!(store.device_grant_by_user_code(refused).await?.is_none());
    }
    Ok(())
}

/// #169: the purge deletes expired grants of any status and consumed ones, keeps a live denied
/// one; a start purges too, so a client whose grants expired starts again.
#[tokio::test]
async fn expired_and_consumed_device_grants_are_purged() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (_, uid) = seed(&store).await?;
    for (code, client) in [("P1", "a"), ("C1", "a"), ("D1", "b")] {
        store
            .insert_device_grant(&grant(code, code, client, 1), BOUNDS)
            .await?;
    }
    let sealed = yulius_app::model::Sealed(vec![1]);
    store
        .resolve_device_grant("C1", "approved", Some(&uid), Some(&sealed))
        .await?;
    assert!(store.consume_device_grant("C1").await?);
    store
        .resolve_device_grant("D1", "denied", Some(&uid), None)
        .await?;

    assert_eq!(store.purge_device_grants(Timestamp(500)).await?, 1);
    assert!(store.device_grant_by_user_code("C1").await?.is_none());
    for kept in ["P1", "D1"] {
        assert!(store.device_grant_by_user_code(kept).await?.is_some());
    }
    assert_eq!(store.purge_device_grants(Timestamp(1001)).await?, 2);
    assert_eq!(store.purge_device_grants(Timestamp(1001)).await?, 0);

    // A start purges: client "a" fills its bound, and starts again once those expired.
    for code in ["A1", "A2"] {
        store
            .insert_device_grant(&grant(code, code, "a", 2000), BOUNDS)
            .await?;
    }
    assert_eq!(
        store
            .insert_device_grant(&grant("A3", "A3", "a", 2500), BOUNDS)
            .await?,
        DeviceGrantInsert::ClientFull
    );
    assert_eq!(
        store
            .insert_device_grant(&grant("A3", "A3", "a", 3000), BOUNDS)
            .await?,
        DeviceGrantInsert::Inserted
    );
    assert!(store.device_grant_by_user_code("A1").await?.is_none());

    // A refused start purges too: "b"'s grant expired while "a" sits at its bound.
    store
        .insert_device_grant(&grant("B1", "B1", "b", 2600), BOUNDS)
        .await?;
    store
        .insert_device_grant(&grant("A4", "A4", "a", 3100), BOUNDS)
        .await?;
    assert_eq!(
        store
            .insert_device_grant(&grant("A5", "A5", "a", 3700), BOUNDS)
            .await?,
        DeviceGrantInsert::ClientFull
    );
    assert!(store.device_grant_by_user_code("B1").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn device_grant_is_consumed_once() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (_, uid) = seed(&store).await?;
    store
        .insert_device_grant(&grant("ABCD-EFGH", "dh", "client", 1), BOUNDS)
        .await?;
    let sealed = yulius_app::model::Sealed(vec![1, 2, 3]);
    assert!(
        store
            .resolve_device_grant("ABCD-EFGH", "approved", Some(&uid), Some(&sealed))
            .await?
    );
    assert!(
        !store
            .resolve_device_grant("ABCD-EFGH", "denied", None, None)
            .await?,
        "already resolved"
    );
    assert!(store.consume_device_grant("dh").await?);
    assert!(!store.consume_device_grant("dh").await?, "single use");
    let grant = store.device_grant_by_device("dh").await?.ok_or("grant")?;
    assert_eq!((grant.status.as_str(), grant.token), ("consumed", None));
    Ok(())
}

#[tokio::test]
async fn messages_inbox_read_tracking_and_steps_history() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let a = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let b = store.insert_task(&task("t2", &sid, &uid)?).await?;
    let msg = MessageRecord {
        id: MessageId::parse("m1")?,
        sandbox_id: sid.clone(),
        from_task: Some(a.id.clone()),
        from_user: None,
        to_task: Some(b.id.clone()),
        kind: "question".into(),
        subject: "scope".into(),
        body: "May I take src/api?".into(),
        created_at: Timestamp(60),
    };
    store
        .insert_message(&msg, std::slice::from_ref(&b.id))
        .await?;
    assert_eq!(store.unread_count(&b.id).await?, 1);
    assert_eq!(store.unread_count(&a.id).await?, 0);
    assert_eq!(store.mark_delivered(&b.id, Timestamp(61)).await?, 1);
    assert_eq!(
        store.inbox(&b.id, true, 10).await?.len(),
        1,
        "delivered is not read"
    );
    assert_eq!(store.mark_read(&b.id, Timestamp(62)).await?, 1);
    assert!(store.inbox(&b.id, true, 10).await?.is_empty());
    assert_eq!(
        store.task_messages(&a.id, 10).await?.len(),
        1,
        "sender sees its message"
    );

    let step = |id: &str,
                ord: u32,
                title: &str,
                status: &str|
     -> Result<StepRecord, Box<dyn std::error::Error>> {
        Ok(StepRecord {
            id: StepId::parse(id)?,
            task_id: a.id.clone(),
            ordinal: ord,
            title: title.into(),
            status: status.into(),
            note: None,
            updated_by: None,
            updated_at: None,
            effort: if ord == 2 { 3 } else { 1 },
        })
    };
    store
        .replace_steps(
            &a.id,
            &[
                step("s1", 1, "Write spec", "pending")?,
                step("s2", 2, "Implement", "pending")?,
            ],
            "task:1",
            Timestamp(70),
        )
        .await?;
    let updated = store
        .update_step(
            &StepId::parse("s1")?,
            "done",
            Some("spec written"),
            "task:1",
            Timestamp(71),
        )
        .await?;
    assert_eq!(updated.status, "done");
    assert_eq!(updated.updated_by.as_deref(), Some("task:1"));
    // Effort is stored; start and finish times come from the step history (STP-006/007).
    let stored = store.steps(&a.id).await?;
    assert_eq!(
        stored.iter().map(|s| s.effort).collect::<Vec<_>>(),
        vec![1, 3]
    );
    store
        .update_step(
            &StepId::parse("s2")?,
            "in_progress",
            None,
            "task:1",
            Timestamp(73),
        )
        .await?;
    store
        .update_step(&StepId::parse("s2")?, "done", None, "task:1", Timestamp(79))
        .await?;
    let mut timings = store.step_timings(&a.id).await?;
    timings.sort_by(|x, y| x.step_id.as_str().cmp(y.step_id.as_str()));
    let summary: Vec<(&str, Option<i64>, Option<i64>)> = timings
        .iter()
        .map(|t| {
            (
                t.step_id.as_str(),
                t.started_at.map(|x| x.0),
                t.finished_at.map(|x| x.0),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![("s1", None, Some(71)), ("s2", Some(73), Some(79))]
    );
    assert!(matches!(
        store
            .update_step(&StepId::parse("nope")?, "done", None, "x", Timestamp(72))
            .await,
        Err(AppError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn oauth_state_single_use_and_expiry() -> R {
    let store = SqliteStore::open_in_memory()?;
    store
        .insert_oauth_state("h1", "/next", Timestamp(1), Timestamp(100))
        .await?;
    store
        .insert_oauth_state("h2", "/late", Timestamp(1), Timestamp(5))
        .await?;
    assert_eq!(
        store.take_oauth_state("h1", Timestamp(50)).await?,
        Some("/next".into())
    );
    assert_eq!(
        store.take_oauth_state("h1", Timestamp(50)).await?,
        None,
        "single use"
    );
    assert_eq!(
        store.take_oauth_state("h2", Timestamp(50)).await?,
        None,
        "expired"
    );
    Ok(())
}

/// SPEC-001 TSK-012/013: chain links and held prompts survive a reopen; a newer held prompt
/// supersedes the pending one; decisions are compare-and-set; "send anyway" is consumed once;
/// deleting a predecessor keeps its successor's link with the number but no id.
#[tokio::test]
async fn chain_links_and_held_prompts_persist_and_resolve_once() -> R {
    use yulius_app::intake::{ChainRecord, HoldRecord, hold_state};
    use yulius_app::ports::store_ext::IntakeStore;
    use yulius_domain::intake::PromptClass;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("y.db");
    let hold = |id: &str, at: i64| -> Result<HoldRecord, Box<dyn std::error::Error>> {
        Ok(HoldRecord {
            id: id.into(),
            task_id: TaskId::parse("t1")?,
            class: PromptClass::NewScope,
            prompt: "add a dark mode".into(),
            prompt_sha256: "ab12".into(),
            percent: 92,
            typed_by: Some(UserId::parse("u1")?),
            state: hold_state::PENDING.into(),
            created_at: Timestamp(at),
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
    };
    {
        let store = SqliteStore::open(&path)?;
        let (sid, uid) = seed(&store).await?;
        for id in ["t1", "t2", "t3"] {
            let mut record = task(id, &sid, &uid)?;
            record.status = TaskStatus::Chained;
            store.insert_task(&record).await?;
        }
        store
            .insert_chain(&ChainRecord {
                task_id: TaskId::parse("t2")?,
                after_task_id: Some(TaskId::parse("t1")?),
                after_number: 1,
                blocked_reason: None,
                created_at: Timestamp(5),
                rule: None,
            })
            .await?;
        // A start rule (SPEC-017) persists in canonical text.
        store
            .insert_chain(&ChainRecord {
                task_id: TaskId::parse("t3")?,
                after_task_id: Some(TaskId::parse("t1")?),
                after_number: 1,
                blocked_reason: None,
                created_at: Timestamp(5),
                rule: Some("#1 done and not #2 ended".into()),
            })
            .await?;
        store.insert_hold(&hold("h1", 6)?).await?;
        store.insert_hold(&hold("h2", 7)?).await?;
    }
    let store = SqliteStore::open(&path)?;
    let t1 = TaskId::parse("t1")?;
    let t2 = TaskId::parse("t2")?;
    assert_eq!(
        store.task(&t2).await?.ok_or("t2")?.status,
        TaskStatus::Chained
    );
    let chain = store.chain(&t2).await?.ok_or("chain")?;
    assert_eq!(chain.after_task_id.as_ref(), Some(&t1));
    assert_eq!(chain.rule, None);
    let ruled = store
        .chain(&TaskId::parse("t3")?)
        .await?
        .ok_or("chain t3")?;
    assert_eq!(ruled.rule.as_deref(), Some("#1 done and not #2 ended"));
    assert_eq!(store.chain_successors(&t1).await?.len(), 2);
    store.set_chain_blocked(&t2, Some("#1 failed")).await?;
    assert_eq!(
        store
            .chain(&t2)
            .await?
            .ok_or("chain")?
            .blocked_reason
            .as_deref(),
        Some("#1 failed")
    );

    assert_eq!(store.pending_hold(&t1).await?.ok_or("pending")?.id, "h2");
    assert_eq!(
        store.hold("h1").await?.ok_or("h1")?.state,
        hold_state::SUPERSEDED
    );
    assert!(
        store
            .resolve_hold(
                "h2",
                hold_state::PENDING,
                hold_state::SENT,
                "user:alice",
                Timestamp(8),
                None
            )
            .await?
    );
    assert!(
        !store
            .resolve_hold(
                "h2",
                hold_state::PENDING,
                hold_state::PARALLEL,
                "user:bob",
                Timestamp(9),
                None
            )
            .await?
    );
    assert!(
        !store.take_sent_hold(&t1, "ab12", Timestamp(9)).await?,
        "too old"
    );
    assert!(store.take_sent_hold(&t1, "ab12", Timestamp(8)).await?);
    assert!(
        !store.take_sent_hold(&t1, "ab12", Timestamp(8)).await?,
        "once"
    );

    assert!(store.delete_task(&t1).await?);
    let chain = store.chain(&t2).await?.ok_or("chain kept")?;
    assert_eq!(chain.after_task_id, None);
    assert_eq!(chain.after_number, 1);
    assert!(store.delete_task(&t2).await?);
    assert!(store.chain(&t2).await?.is_none());
    Ok(())
}

/// SPEC-001 TSK-015: a task's Auto choice survives a reopen, one row per task, and goes with the
/// task when it is deleted.
#[tokio::test]
async fn auto_choices_persist_and_go_with_their_task() -> R {
    use yulius_app::auto_choice::AutoChoiceRecord;
    use yulius_app::ports::store_ext::IntakeStore;
    use yulius_domain::auto_choice::ChosenBy;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("y.db");
    let t1 = TaskId::parse("t1")?;
    let choice = AutoChoiceRecord {
        task_id: t1.clone(),
        mode_by: Some(ChosenBy::Jev),
        workflow_by: Some(ChosenBy::Default),
        workflow_name: Some("Feature".into()),
        created_at: Timestamp(5),
    };
    {
        let store = SqliteStore::open(&path)?;
        let (sid, uid) = seed(&store).await?;
        store.insert_task(&task("t1", &sid, &uid)?).await?;
        store.insert_task(&task("t2", &sid, &uid)?).await?;
        store.insert_auto_choice(&choice).await?;
        assert!(
            store.insert_auto_choice(&choice).await.is_err(),
            "one per task"
        );
        store
            .insert_auto_choice(&AutoChoiceRecord {
                task_id: TaskId::parse("t2")?,
                mode_by: None,
                workflow_by: Some(ChosenBy::Jev),
                workflow_name: None,
                created_at: Timestamp(6),
            })
            .await?;
    }
    let store = SqliteStore::open(&path)?;
    assert_eq!(store.auto_choice(&t1).await?, Some(choice));
    let t2 = store
        .auto_choice(&TaskId::parse("t2")?)
        .await?
        .ok_or("t2")?;
    assert_eq!((t2.mode_by, t2.workflow_name), (None, None));
    assert!(store.delete_task(&t1).await?);
    assert_eq!(store.auto_choice(&t1).await?, None);
    Ok(())
}

/// SPEC-052 QMS-002/003: the append to a task that has not started is one conditional update:
/// it keeps the whole prompt within the bound in bytes, and refuses a task past waiting or with a
/// run, leaving the prompt as it was.
#[tokio::test]
async fn a_prompt_grows_only_while_the_task_has_not_started_and_within_the_bound() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    // `starting` (the record's status) is past waiting.
    assert!(
        !store
            .append_unstarted_prompt(&t.id, "+a", 64, Timestamp(20))
            .await?
    );
    store
        .transition_task(&t.id, None, TaskStatus::Queued, None, Timestamp(21))
        .await?;
    // "p" is 1 byte; "ł" is 2: 1 + 1 + 2 = 4 bytes fits a bound of 4, a fifth byte does not.
    assert!(
        store
            .append_unstarted_prompt(&t.id, "+ł", 4, Timestamp(22))
            .await?
    );
    assert!(
        !store
            .append_unstarted_prompt(&t.id, "+", 4, Timestamp(23))
            .await?
    );
    let grown = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(
        (grown.prompt.as_str(), grown.updated_at),
        ("p+ł", Timestamp(22))
    );
    // A provider session (an imported session waiting for its first run here) makes it started.
    let imported = store.insert_task(&task("t2", &sid, &uid)?).await?;
    store
        .transition_task(&imported.id, None, TaskStatus::Queued, None, Timestamp(21))
        .await?;
    store
        .upsert_provider_session(&yulius_app::model::ProviderSessionRecord {
            id: "ps-1".into(),
            sandbox_id: sid.clone(),
            task_id: imported.id.clone(),
            harness: Harness::ClaudeCode,
            session_id: "session-1".into(),
            transcript_ref: None,
            origin: "imported".into(),
            first_seen: Timestamp(21),
            last_seen: Timestamp(21),
        })
        .await?;
    assert!(
        !store
            .append_unstarted_prompt(&imported.id, "+c", 1024, Timestamp(22))
            .await?
    );
    assert_eq!(store.task(&imported.id).await?.ok_or("task")?.prompt, "p");
    // A run makes it started, whatever its status.
    store.insert_attempt(&attempt("a1", &t.id, 1)?).await?;
    assert!(
        !store
            .append_unstarted_prompt(&t.id, "+b", 1024, Timestamp(24))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p+ł");
    Ok(())
}
