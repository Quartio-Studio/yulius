//! Behavioral tests of the SQLite store contracts used for concurrency and recovery.

use yulius_app::error::AppError;
use yulius_app::model::{
    ApiTokenRecord, AttemptRecord, DeviceGrantBounds, DeviceGrantInsert, DeviceGrantRecord,
    LaunchKind, MessageRecord, SandboxRecord, StepRecord, TaskOrigin, TaskRecord, UserRecord,
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
        resume_attempt_id: None,
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
        supertask: false,
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
        model: None,
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
            TaskStatus::Failed(yulius_domain::task::FailReason::SpawnError),
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

#[tokio::test]
async fn an_idle_switch_claims_the_observed_activity_and_live_attempt() -> R {
    use yulius_domain::task::Activity;

    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let mut run = attempt("a1", &t.id, 1)?;
    run.state = AttemptState::Live;
    store.insert_attempt(&run).await?;
    let idle = (Activity::Done, Timestamp(30));
    assert!(
        store
            .transition_task(
                &t.id,
                Some(TaskStatus::Starting),
                TaskStatus::Running,
                Some(idle.0),
                idle.1
            )
            .await?
    );

    let at = Timestamp(40);
    let wrong = AttemptId::parse("a2")?;
    assert!(!store.claim_idle_switch(&t.id, idle, &wrong, at).await?);
    assert!(
        !store
            .claim_idle_switch(&t.id, (idle.0, Timestamp(29)), &run.id, at)
            .await?
    );
    assert!(
        !store
            .claim_idle_switch(&t.id, (Activity::Waiting, idle.1), &run.id, at)
            .await?
    );
    assert!(store.claim_idle_switch(&t.id, idle, &run.id, at).await?);
    let switching = store.task(&t.id).await?.ok_or("switching task")?;
    assert_eq!(switching.status, TaskStatus::Switching);
    assert_eq!(switching.activity, None);
    Ok(())
}

/// SPEC-153 OLC-001: an onboarding state change lands only on the state it was decided on; a
/// start that read `pending` before an admin's skip never writes `preparing` over `skipped`.
#[tokio::test]
async fn onboarding_transitions_only_from_the_state_read() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let prep = store.insert_task(&task("t1", &sid, &uid)?).await?;
    store
        .set_onboarding(&sid, "pending", Some(&prep.id))
        .await?;
    assert!(
        store
            .transition_onboarding(&sid, "pending", "skipped")
            .await?
    );
    assert!(
        !store
            .transition_onboarding(&sid, "pending", "preparing")
            .await?,
        "the state is no longer pending"
    );
    let record = store.sandbox(&sid).await?.ok_or("sandbox")?;
    assert_eq!(record.onboarding_state, "skipped");
    assert_eq!(
        record.onboarding_task_id.as_ref(),
        Some(&prep.id),
        "the onboarding task is kept"
    );
    Ok(())
}

/// SPEC-087 TIM-001, TIM-002: a task's state time moves only when its status or activity
/// changes, not when the same value is set again (every prompt sets Working) nor when anything
/// else changes. Its working stretch starts when it starts working and goes on through idle
/// turns; a person needed, a queue or an end closes it.
#[tokio::test]
async fn the_state_time_moves_only_when_the_state_changes() -> R {
    use yulius_app::model::TaskTimes;
    use yulius_app::ports::store::TaskFieldUpdate;
    use yulius_domain::task::Activity;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let times = |at: i64, working: Option<i64>| {
        Some(TaskTimes {
            state_at: Timestamp(at),
            working_since: working.map(Timestamp),
        })
    };
    let running = |activity, at| {
        store.transition_task(
            &t.id,
            None,
            TaskStatus::Running,
            Some(activity),
            Timestamp(at),
        )
    };
    assert_eq!(store.task_times(&t.id).await?, times(10, Some(10)));
    running(Activity::Working, 20).await?;
    store
        .set_activity(&t.id, Some(Activity::Working), Timestamp(30))
        .await?;
    running(Activity::Working, 35).await?;
    store
        .update_task(
            &t.id,
            &TaskFieldUpdate {
                title: Some("renamed".into()),
                ..TaskFieldUpdate::default()
            },
            Timestamp(40),
        )
        .await?;
    assert_eq!(store.task_times(&t.id).await?, times(20, Some(10)));
    assert_eq!(
        store.task(&t.id).await?.ok_or("task")?.updated_at,
        Timestamp(40),
        "the last change of anything moves on"
    );
    // An idle turn keeps the stretch (the app ends it once no background work is left).
    store
        .set_activity(&t.id, Some(Activity::Waiting), Timestamp(50))
        .await?;
    store
        .set_activity(&t.id, Some(Activity::Working), Timestamp(55))
        .await?;
    assert_eq!(store.task_times(&t.id).await?, times(55, Some(10)));
    // A person needed ends it; the next work starts another.
    store
        .set_activity(&t.id, Some(Activity::NeedsAttention), Timestamp(60))
        .await?;
    assert_eq!(store.task_times(&t.id).await?, times(60, None));
    // SPEC-213: a conditional change applies only to the state it saw.
    for stale in [
        (Some(Activity::NeedsAttention), Timestamp(55)),
        (Some(Activity::Waiting), Timestamp(60)),
        (None, Timestamp(60)),
    ] {
        assert!(
            !store
                .set_activity_from(&t.id, stale, Some(Activity::Working), Timestamp(65))
                .await?
        );
    }
    assert_eq!(store.task_times(&t.id).await?, times(60, None));
    assert!(
        store
            .set_activity_from(
                &t.id,
                (Some(Activity::NeedsAttention), Timestamp(60)),
                Some(Activity::Working),
                Timestamp(65)
            )
            .await?
    );
    assert_eq!(store.task_times(&t.id).await?, times(65, Some(65)));
    store
        .set_activity(&t.id, Some(Activity::NeedsAttention), Timestamp(66))
        .await?;
    running(Activity::Working, 70).await?;
    assert_eq!(store.task_times(&t.id).await?, times(70, Some(70)));
    store.set_working_since(&t.id, None).await?;
    store
        .transition_task(&t.id, None, TaskStatus::Switching, None, Timestamp(75))
        .await?;
    assert_eq!(store.task_times(&t.id).await?, times(75, Some(75)));
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled),
            None,
            Timestamp(80),
        )
        .await?;
    assert_eq!(store.task_times(&t.id).await?, times(80, None));
    assert_eq!(store.task_times(&TaskId::parse("missing")?).await?, None);
    Ok(())
}

/// SPEC-472 ATN-001, ATN-002: a flag and its reason are one statement, and a reason speaks only
/// for the flag it was written with: not once the task left that flag, whatever statement
/// changed its state, and not for a flag raised later without one.
#[tokio::test]
async fn a_flags_reason_is_read_only_while_the_task_reads_that_flag() -> R {
    use yulius_app::attention::AttentionRecord;
    use yulius_app::model::TaskTimes;
    use yulius_domain::task::Activity;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let gate = AttentionRecord::yulius("The agent stopped 3 times.", "Tell it how to go on.");
    let secret = AttentionRecord::yulius("A secret waits.", "Answer it.")
        .on_page("https://yulius.test/s/app/secrets");
    let state_at = || async {
        Ok::<_, Box<dyn std::error::Error>>(store.task_times(&t.id).await?.map(|t| t.state_at))
    };

    // Not running: nothing is flagged and nothing is kept.
    assert!(
        !store
            .flag_attention(&t.id, None, Some(&gate), Timestamp(15))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.activity, None);
    assert_eq!(store.task_attention(&t.id).await?, None);

    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            Timestamp(20),
        )
        .await?;
    assert_eq!(store.task_attention(&t.id).await?, None);
    assert!(
        store
            .flag_attention(&t.id, None, Some(&gate), Timestamp(30))
            .await?
    );
    let flagged = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(flagged.activity, Some(Activity::NeedsAttention));
    assert_eq!(flagged.updated_at, Timestamp(30));
    assert_eq!(
        store.task_times(&t.id).await?,
        Some(TaskTimes {
            state_at: Timestamp(30),
            working_since: None,
        }),
        "a flagged task works for nobody (SPEC-087)"
    );
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((gate.clone(), Timestamp(30))),
        "the reason, and when its flag was raised"
    );

    // Flagged already: the flag, its time and its reason stay, with or without a new reason.
    assert!(
        !store
            .flag_attention(&t.id, None, Some(&secret), Timestamp(40))
            .await?
    );
    assert!(
        !store
            .flag_attention(&t.id, None, None, Timestamp(41))
            .await?
    );
    assert_eq!(state_at().await?, Some(Timestamp(30)));
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((gate.clone(), Timestamp(30)))
    );

    // The task works again: the reason is not read, though no statement cleared it.
    store
        .set_activity(&t.id, Some(Activity::Working), Timestamp(50))
        .await?;
    assert_eq!(store.task_attention(&t.id).await?, None);
    // Flagged anew by a statement that knows no reason: the old one is not that flag's.
    store
        .set_activity(&t.id, Some(Activity::NeedsAttention), Timestamp(60))
        .await?;
    assert_eq!(store.task_attention(&t.id).await?, None);
    // Flagged anew without a reason by the flag's own statement: none is kept.
    store
        .set_activity(&t.id, Some(Activity::Waiting), Timestamp(70))
        .await?;
    assert!(
        store
            .flag_attention(&t.id, None, None, Timestamp(80))
            .await?
    );
    assert_eq!(state_at().await?, Some(Timestamp(80)));
    assert_eq!(store.task_attention(&t.id).await?, None);
    // And with another reason: that one, with its page.
    store
        .set_activity(&t.id, Some(Activity::Working), Timestamp(90))
        .await?;
    assert!(
        store
            .flag_attention(&t.id, None, Some(&secret), Timestamp(100))
            .await?
    );
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((secret.clone(), Timestamp(100)))
    );

    // A task that is not running reads no reason, also when its state is of the flag's time.
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Stopping,
            Some(Activity::NeedsAttention),
            Timestamp(100),
        )
        .await?;
    assert_eq!(state_at().await?, Some(Timestamp(100)));
    assert_eq!(store.task_attention(&t.id).await?, None);
    assert_eq!(
        store.task_attention(&TaskId::parse("missing")?).await?,
        None
    );
    Ok(())
}

/// SPEC-472 ATN-001 (SPEC-213 SRA-003): a flag raised from a state the task read is not raised
/// once the task left that state.
#[tokio::test]
async fn a_flag_from_a_state_the_task_left_is_not_raised() -> R {
    use yulius_app::attention::AttentionRecord;
    use yulius_domain::task::Activity;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let secret = AttentionRecord::yulius("A secret waits.", "Answer it.");
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            Timestamp(20),
        )
        .await?;
    // The state moved since it was read: its time, then its activity.
    for stale in [
        (Some(Activity::Working), Timestamp(19)),
        (Some(Activity::Waiting), Timestamp(20)),
        (None, Timestamp(20)),
    ] {
        assert!(
            !store
                .flag_attention(&t.id, Some(stale), Some(&secret), Timestamp(30))
                .await?,
            "{stale:?}"
        );
    }
    assert_eq!(
        store.task(&t.id).await?.ok_or("task")?.activity,
        Some(Activity::Working)
    );
    assert_eq!(store.task_attention(&t.id).await?, None);
    // As it was read: flagged, with the reason.
    let read = (Some(Activity::Working), Timestamp(20));
    assert!(
        store
            .flag_attention(&t.id, Some(read), Some(&secret), Timestamp(30))
            .await?
    );
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((secret, Timestamp(30)))
    );
    // A task with no activity yet is flagged from that state too.
    let u = store.insert_task(&task("t2", &sid, &uid)?).await?;
    store
        .transition_task(&u.id, None, TaskStatus::Running, None, Timestamp(20))
        .await?;
    assert!(
        store
            .flag_attention(&u.id, Some((None, Timestamp(20))), None, Timestamp(30))
            .await?
    );

    Ok(())
}

/// SPEC-472 ATN-011: a flag that changes owner gets another reason and stays the same flag: its
/// time and the task's state do not move. Only the flag raised at the time named is changed;
/// the same reason again changes nothing; no reason clears it.
#[tokio::test]
async fn a_flags_reason_is_replaced_only_on_the_flag_it_was_read_from() -> R {
    use yulius_app::attention::AttentionRecord;
    use yulius_app::model::TaskTimes;
    use yulius_domain::task::Activity;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let secret = AttentionRecord::yulius("A secret waits.", "Answer it.")
        .on_page("https://yulius.test/s/app/secrets?section=agents");
    let question = AttentionRecord::terminal(Some("Pick one"));
    let replace = |flagged: i64, reason: Option<AttentionRecord>, at: i64| {
        let (store, id) = (&store, &t.id);
        async move {
            store
                .replace_attention(id, Timestamp(flagged), reason.as_ref(), Timestamp(at))
                .await
        }
    };
    // No flag to speak for: a queued task, then a working one.
    assert!(!replace(10, Some(question.clone()), 15).await?);
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            Timestamp(20),
        )
        .await?;
    assert!(!replace(20, Some(question.clone()), 25).await?);
    assert_eq!(store.task_attention(&t.id).await?, None);

    assert!(
        store
            .flag_attention(&t.id, None, Some(&secret), Timestamp(30))
            .await?
    );
    // Another flag's time: nothing.
    assert!(!replace(29, Some(question.clone()), 40).await?);
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((secret.clone(), Timestamp(30)))
    );
    // This flag's: the reason changes, the flag does not.
    assert!(replace(30, Some(question.clone()), 40).await?);
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((question.clone(), Timestamp(30))),
        "the same flag, raised when it was"
    );
    let now = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(now.activity, Some(Activity::NeedsAttention));
    assert_eq!(now.updated_at, Timestamp(40), "the task changed");
    assert_eq!(
        store.task_times(&t.id).await?,
        Some(TaskTimes {
            state_at: Timestamp(30),
            working_since: None,
        })
    );
    // The reason it has: no change, nothing to announce.
    assert!(!replace(30, Some(question.clone()), 50).await?);
    assert_eq!(
        store.task(&t.id).await?.ok_or("task")?.updated_at,
        Timestamp(40)
    );
    // No reason: the flag says none, once.
    assert!(replace(30, None, 60).await?);
    assert_eq!(store.task_attention(&t.id).await?, None);
    assert!(!replace(30, None, 70).await?);
    // And one again, over none.
    assert!(replace(30, Some(secret.clone()), 80).await?);
    assert_eq!(
        store.task_attention(&t.id).await?,
        Some((secret.clone(), Timestamp(30)))
    );

    // A task that is not running keeps what it has.
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Stopping,
            Some(Activity::NeedsAttention),
            Timestamp(30),
        )
        .await?;
    assert!(!replace(30, Some(question), 90).await?);
    assert!(
        !store
            .replace_attention(
                &TaskId::parse("missing")?,
                Timestamp(30),
                None,
                Timestamp(90)
            )
            .await?
    );
    Ok(())
}

/// SPEC-315 FWS-001: publishing an idle task ends or starts its working stretch only while the
/// task is still in the state that was decided on; a turn that started in between keeps its
/// stretch (issue #895).
#[tokio::test]
async fn a_stretch_write_applies_only_to_the_state_it_read() -> R {
    use yulius_app::model::TaskTimes;
    use yulius_domain::task::Activity;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let times = |at: i64, working: Option<i64>| TaskTimes {
        state_at: Timestamp(at),
        working_since: working.map(Timestamp),
    };
    store
        .transition_task(
            &t.id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            Timestamp(20),
        )
        .await?;
    store
        .set_activity(&t.id, Some(Activity::Waiting), Timestamp(30))
        .await?;
    // The publish reads the idle task and its times …
    let read = store.task_times(&t.id).await?.ok_or("times")?;
    assert_eq!(read, times(30, Some(10)));
    // … the prompt hook makes it Working …
    assert!(
        store
            .set_activity_from(
                &t.id,
                (Some(Activity::Waiting), read.state_at),
                Some(Activity::Working),
                Timestamp(40)
            )
            .await?
    );
    // … and the publish's end of the stretch no longer applies.
    assert!(
        !store
            .set_working_since_from(&t.id, (Some(Activity::Waiting), read), None)
            .await?,
        "the task is Working since the read"
    );
    assert_eq!(
        store.task_times(&t.id).await?,
        Some(times(40, Some(10))),
        "the Working task keeps its stretch"
    );
    store
        .set_activity(&t.id, Some(Activity::Waiting), Timestamp(50))
        .await?;
    for stale in [
        (Some(Activity::Working), times(50, Some(10))),
        (None, times(50, Some(10))),
        (Some(Activity::Waiting), times(40, Some(10))),
        (Some(Activity::Waiting), times(50, None)),
        (Some(Activity::Waiting), times(50, Some(5))),
    ] {
        assert!(
            !store.set_working_since_from(&t.id, stale, None).await?,
            "{stale:?} is not the task's state"
        );
    }
    assert_eq!(store.task_times(&t.id).await?, Some(times(50, Some(10))));
    // The state it read: the stretch ends, and starts again (TIM-002).
    assert!(
        store
            .set_working_since_from(&t.id, (Some(Activity::Waiting), times(50, Some(10))), None)
            .await?
    );
    assert_eq!(store.task_times(&t.id).await?, Some(times(50, None)));
    assert!(
        store
            .set_working_since_from(
                &t.id,
                (Some(Activity::Waiting), times(50, None)),
                Some(Timestamp(55))
            )
            .await?
    );
    assert_eq!(store.task_times(&t.id).await?, Some(times(50, Some(55))));
    // Only a running task's stretch.
    store
        .transition_task(&t.id, None, TaskStatus::Queued, None, Timestamp(60))
        .await?;
    let queued = store.task_times(&t.id).await?.ok_or("times")?;
    assert!(
        !store
            .set_working_since_from(&t.id, (None, queued), Some(Timestamp(65)))
            .await?
    );
    assert_eq!(store.task_times(&t.id).await?, Some(queued));
    assert!(
        !store
            .set_working_since_from(&TaskId::parse("missing")?, (None, queued), None)
            .await?
    );
    Ok(())
}

/// SPEC-490 OWN-009: a hand-over on request records who gave control, who takes the task back
/// only while they are that giver; handing on records the new giver, handing back to the giver
/// and a pass past an absent owner record nobody.
#[tokio::test]
async fn the_giver_takes_a_task_back_until_control_moves_again() -> R {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, alice) = seed(&store).await?;
    for (id, login) in [("u2", "bob"), ("u3", "carol")] {
        store.insert_user(&user(id, Some(login), false)?).await?;
    }
    let (bob, carol) = (UserId::parse("u2")?, UserId::parse("u3")?);
    let t = store.insert_task(&task("t1", &sid, &alice)?).await?;
    assert_eq!(store.control_from(&t.id).await?, None);
    let hand = |from: UserId, to: UserId, at: i64| {
        let (store, id) = (&store, t.id.clone());
        async move {
            assert!(store.request_control(&id, &to, Timestamp(at)).await?);
            store.pass_control(&id, &from, &to, Timestamp(at + 1)).await
        }
    };
    assert!(hand(alice.clone(), bob.clone(), 10).await?);
    assert_eq!(store.control_from(&t.id).await?, Some(alice.clone()));
    // Only the giver, from the owner seen; a request waiting meanwhile goes with it.
    assert!(store.request_control(&t.id, &carol, Timestamp(15)).await?);
    assert!(
        !store
            .take_back_control(&t.id, &bob, &carol, Timestamp(16))
            .await?
    );
    assert!(
        !store
            .take_back_control(&t.id, &carol, &alice, Timestamp(16))
            .await?
    );
    assert!(
        store
            .take_back_control(&t.id, &bob, &alice, Timestamp(17))
            .await?
    );
    let back = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(
        (back.owner.clone(), back.updated_at),
        (alice.clone(), Timestamp(17))
    );
    assert_eq!(store.control_from(&t.id).await?, None);
    assert_eq!(store.control_request(&t.id).await?, None);
    assert!(
        !store
            .take_back_control(&t.id, &alice, &alice, Timestamp(18))
            .await?
    );

    // Handed on: the new giver is the one who handed it on.
    assert!(hand(alice.clone(), bob.clone(), 20).await?);
    assert!(hand(bob.clone(), carol.clone(), 30).await?);
    assert_eq!(store.control_from(&t.id).await?, Some(bob.clone()));
    assert!(
        !store
            .take_back_control(&t.id, &carol, &alice, Timestamp(32))
            .await?
    );
    // Handed back to its giver: nobody can take it back.
    assert!(hand(carol.clone(), bob.clone(), 40).await?);
    assert_eq!(store.control_from(&t.id).await?, None);
    // Past an absent owner: nobody either.
    assert!(hand(bob.clone(), alice.clone(), 50).await?);
    assert_eq!(store.control_from(&t.id).await?, Some(bob.clone()));
    assert!(
        store
            .take_control(&t.id, &alice, &carol, Timestamp(60))
            .await?
    );
    assert_eq!(store.control_from(&t.id).await?, None);
    Ok(())
}

/// SPEC-510 TKC-003: whether teammates take control of a person's tasks or ask first is theirs to
/// choose; nobody has chosen to be asked until they do.
#[tokio::test]
async fn a_person_chooses_whether_teammates_ask_before_taking_control() -> R {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (_, alice) = seed(&store).await?;
    store.insert_user(&user("u2", Some("bob"), false)?).await?;
    let bob = UserId::parse("u2")?;
    assert!(!store.control_ask_first(&alice).await?);
    store.set_control_ask_first(&alice, true).await?;
    assert!(store.control_ask_first(&alice).await?);
    assert!(!store.control_ask_first(&bob).await?, "only alice chose");
    store.set_control_ask_first(&alice, false).await?;
    assert!(!store.control_ask_first(&alice).await?);
    // Nobody who is not a user asks for anything.
    assert!(!store.control_ask_first(&UserId::parse("u9")?).await?);
    Ok(())
}

/// SPEC-510 TKC-001, TKC-002: a take passes control only from the owner seen, drops a waiting
/// request and makes the previous owner the giver who may take it back.
#[tokio::test]
async fn a_take_passes_control_from_the_owner_seen_who_may_take_it_back() -> R {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, alice) = seed(&store).await?;
    for (id, login) in [("u2", "bob"), ("u3", "carol")] {
        store.insert_user(&user(id, Some(login), false)?).await?;
    }
    let (bob, carol) = (UserId::parse("u2")?, UserId::parse("u3")?);
    let t = store.insert_task(&task("t1", &sid, &alice)?).await?;
    assert!(store.request_control(&t.id, &carol, Timestamp(10)).await?);
    // A stale owner changes nothing.
    assert!(
        !store
            .seize_control(&t.id, &carol, &bob, Timestamp(11))
            .await?
    );
    assert!(
        store
            .seize_control(&t.id, &alice, &bob, Timestamp(12))
            .await?
    );
    let taken = store.task(&t.id).await?.ok_or("task")?;
    assert_eq!(
        (taken.owner, taken.updated_at),
        (bob.clone(), Timestamp(12))
    );
    assert_eq!(store.control_request(&t.id).await?, None);
    assert_eq!(store.control_from(&t.id).await?, Some(alice.clone()));
    // Taken back by the one it was taken from: nobody is the giver any more.
    assert!(
        store
            .seize_control(&t.id, &bob, &alice, Timestamp(13))
            .await?
    );
    assert_eq!(store.control_from(&t.id).await?, None);
    // An owner who chose to be asked meanwhile keeps the task: the choice is read in the same
    // statement (review round 1).
    store.set_control_ask_first(&alice, true).await?;
    assert!(
        !store
            .seize_control(&t.id, &alice, &bob, Timestamp(14))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.owner, alice);
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

    // Past an absent owner, control passes whatever request waits, only from the owner seen.
    assert!(store.request_control(&t.id, &bob, Timestamp(40)).await?);
    assert!(
        !store
            .take_control(&t.id, &alice, &alice, Timestamp(41))
            .await?
    );
    assert!(
        store
            .take_control(&t.id, &carol, &alice, Timestamp(42))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.owner, alice);
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

/// A `cli` token `id` of `user`, its hash `hash-{id}`.
fn cli_token(id: &str, user: &UserId) -> ApiTokenRecord {
    ApiTokenRecord {
        id: id.into(),
        user_id: user.clone(),
        token_hash: format!("hash-{id}"),
        label: "yulius CLI".into(),
        scope: yulius_domain::permission::TokenScope::Cli,
        created_at: Timestamp(1),
        last_used_at: None,
        revoked_at: None,
    }
}

/// #724: a revoked token is not found by its hash, revoked alone or with all of its user's
/// credentials (a recovery, a disabled account); `authenticate_bearer` relies on it.
#[tokio::test]
async fn revoked_api_tokens_are_not_found() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (_, uid) = seed(&store).await?;
    for id in ["t1", "t2"] {
        store.insert_api_token(&cli_token(id, &uid)).await?;
        assert!(
            store.api_token(&format!("hash-{id}")).await?.is_some(),
            "{id}"
        );
    }
    assert!(store.revoke_api_token(&uid, "t1", Timestamp(2)).await?);
    assert!(store.api_token("hash-t1").await?.is_none());
    assert!(store.api_token("hash-t2").await?.is_some());
    store.revoke_user_credentials(&uid, Timestamp(3)).await?;
    assert!(store.api_token("hash-t2").await?.is_none());
    assert!(store.api_tokens_of(&uid).await?.is_empty());
    Ok(())
}

/// #725: approving a device grant inserts its token in the same transaction, and only when the
/// grant was still pending.
#[tokio::test]
async fn approving_a_device_grant_stores_its_token_only_when_pending() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (_, uid) = seed(&store).await?;
    let token = |id: &str| cli_token(id, &uid);
    let sealed = yulius_app::model::Sealed(vec![1]);
    for code in ["A1", "D1"] {
        store
            .insert_device_grant(&grant(code, code, "client", 1), BOUNDS)
            .await?;
    }
    assert!(
        store
            .approve_device_grant("A1", &token("t1"), &sealed)
            .await?
    );
    let approved = store.device_grant_by_user_code("A1").await?.ok_or("A1")?;
    assert_eq!(
        (
            approved.status.as_str(),
            approved.user_id.as_ref(),
            approved.token
        ),
        ("approved", Some(&uid), Some(sealed.clone()))
    );
    assert!(store.api_token("hash-t1").await?.is_some());

    store
        .resolve_device_grant("D1", "denied", Some(&uid), None)
        .await?;
    for (code, id) in [("A1", "t2"), ("D1", "t3"), ("nope", "t4")] {
        assert!(
            !store
                .approve_device_grant(code, &token(id), &sealed)
                .await?,
            "{code}"
        );
        assert!(
            store.api_token(&format!("hash-{id}")).await?.is_none(),
            "{id}"
        );
    }
    assert_eq!(store.api_tokens_of(&uid).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn task_messages_and_steps_history() -> R {
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
    assert_eq!(
        store.task_messages(&b.id, 10).await?,
        std::slice::from_ref(&msg),
        "the recipient's export has it"
    );
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

/// SPEC-484 PLN-009: the store keeps how many steps the plan had when its first step was
/// finished, written once: a plan that grows later is compared with it (task #465 read 4/4,
/// then 2/2, 3/3 … 11/11, and nothing said that it had grown).
#[tokio::test]
async fn the_plan_keeps_its_first_size() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let a = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let b = store.insert_task(&task("t2", &sid, &uid)?).await?;
    let plan =
        |task: &TaskId, statuses: &[&str]| -> Result<Vec<StepRecord>, Box<dyn std::error::Error>> {
            statuses
                .iter()
                .enumerate()
                .map(|(i, status)| {
                    Ok(StepRecord {
                        id: StepId::parse(&format!("{}-s{i}", task.as_str()))?,
                        task_id: task.clone(),
                        ordinal: u32::try_from(i + 1)?,
                        title: format!("Slice {i}"),
                        status: (*status).into(),
                        note: None,
                        updated_by: None,
                        updated_at: None,
                        effort: 1,
                    })
                })
                .collect()
        };
    assert_eq!(store.plan_first_steps(&a.id).await?, None, "no plan yet");

    // Planned and started, nothing finished: no first size yet (a seed plan may still be replaced).
    store
        .replace_steps(
            &a.id,
            &plan(&a.id, &["in_progress", "pending", "pending", "pending"])?,
            "task:1",
            Timestamp(70),
        )
        .await?;
    assert_eq!(store.plan_first_steps(&a.id).await?, None);

    // The first step finished by `update_step`: the plan had four steps then.
    store
        .update_step(
            &StepId::parse("t1-s0")?,
            "done",
            None,
            "task:1",
            Timestamp(71),
        )
        .await?;
    assert_eq!(store.plan_first_steps(&a.id).await?, Some(4));

    // It grows to eleven and more steps are finished or skipped: the first size stays.
    let mut grown = vec!["done"; 6];
    grown.extend(["skipped", "in_progress", "pending", "pending", "pending"]);
    store
        .replace_steps(&a.id, &plan(&a.id, &grown)?, "spec", Timestamp(80))
        .await?;
    store
        .update_step(
            &StepId::parse("t1-s7")?,
            "done",
            None,
            "task:1",
            Timestamp(81),
        )
        .await?;
    assert_eq!(store.steps(&a.id).await?.len(), 11);
    assert_eq!(store.plan_first_steps(&a.id).await?, Some(4));
    // Nor does it follow a plan that shrinks.
    store
        .replace_steps(
            &a.id,
            &plan(&a.id, &["done", "done"])?,
            "spec",
            Timestamp(90),
        )
        .await?;
    assert_eq!(store.plan_first_steps(&a.id).await?, Some(4));

    // A plan set with finished steps (the spec's Plan with ticked boxes), a skipped step
    // included, has its first size at once; another task's plan is its own.
    store
        .replace_steps(
            &b.id,
            &plan(&b.id, &["skipped", "pending", "pending"])?,
            "spec",
            Timestamp(95),
        )
        .await?;
    assert_eq!(store.plan_first_steps(&b.id).await?, Some(3));
    assert_eq!(store.plan_first_steps(&a.id).await?, Some(4));
    Ok(())
}

#[tokio::test]
async fn oauth_state_single_use_and_expiry() -> R {
    let store = SqliteStore::open_in_memory()?;
    assert!(
        store
            .insert_oauth_state("h1", "/next", Timestamp(1), Timestamp(100), 2)
            .await?
    );
    assert!(
        store
            .insert_oauth_state("h2", "/late", Timestamp(1), Timestamp(5), 2)
            .await?
    );
    // SPEC-483 A4: a start needs no sign-in, so the table is bounded. Past the bound nothing
    // is stored and nothing kept is evicted; an expired state makes room.
    assert!(
        !store
            .insert_oauth_state("h3", "/more", Timestamp(2), Timestamp(100), 2)
            .await?,
        "two live states are the bound"
    );
    assert_eq!(store.take_oauth_state("h3", Timestamp(3)).await?, None);
    assert!(
        store
            .insert_oauth_state("h4", "/room", Timestamp(6), Timestamp(100), 2)
            .await?,
        "h2 expired"
    );
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

/// SPEC-487 TCT-002: the request of a task that has not started is replaced by one conditional
/// update: only while it is still the text that was read (a message queued meanwhile is not
/// overwritten), and never once the task is past waiting or has a run.
#[tokio::test]
async fn a_prompt_is_replaced_only_while_unstarted_and_as_it_was_read() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    // `starting` (the record's status) is past waiting.
    assert!(
        !store
            .replace_unstarted_prompt(&t.id, "p", "new", Timestamp(20))
            .await?
    );
    for (i, waiting) in [
        TaskStatus::Queued,
        TaskStatus::Chained,
        TaskStatus::Prepared,
        TaskStatus::WaitingCapacity,
    ]
    .into_iter()
    .enumerate()
    {
        store
            .transition_task(&t.id, None, waiting, None, Timestamp(21))
            .await?;
        let (old, new) = (format!("p{i}"), format!("p{}", i + 1));
        let old = if i == 0 { "p".to_owned() } else { old };
        assert!(
            store
                .replace_unstarted_prompt(&t.id, &old, &new, Timestamp(22))
                .await?,
            "{waiting:?}"
        );
    }
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p4");
    // A message joined the request since it was read: nothing is overwritten.
    assert!(
        store
            .append_unstarted_prompt(&t.id, "+m", 64, Timestamp(23))
            .await?
    );
    assert!(
        !store
            .replace_unstarted_prompt(&t.id, "p4", "other", Timestamp(24))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p4+m");
    // A run makes it started, whatever its status.
    store.insert_attempt(&attempt("a1", &t.id, 1)?).await?;
    assert!(
        !store
            .replace_unstarted_prompt(&t.id, "p4+m", "late", Timestamp(25))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p4+m");
    // So does a provider session (an imported session waiting for its first run here).
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
            .replace_unstarted_prompt(&imported.id, "p", "late", Timestamp(26))
            .await?
    );
    assert_eq!(store.task(&imported.id).await?.ok_or("task")?.prompt, "p");
    Ok(())
}

/// SPEC-487 TCT-003 (review round 1, F5 and F10): two named tasks that share a millisecond
/// still get the order asked for, both ways, and a task not named that is queued a
/// millisecond later stays behind both: the queue's order has a second key (the order the
/// tasks were created in), so a place shared after the swap moves nobody else's task forward.
#[tokio::test]
async fn queued_tasks_that_share_a_millisecond_take_the_order_asked_for() -> R {
    use yulius_app::model::{QueuedStart, QueuedStartKind};
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let mut ids = Vec::new();
    // t0 is created first and queued last: its place, not its age, is what counts.
    for (name, at) in [("t0", 101), ("t1", 100), ("t2", 100), ("t3", 101)] {
        let t = store.insert_task(&task(name, &sid, &uid)?).await?;
        let start = QueuedStart {
            kind: QueuedStartKind::Fresh,
            prompt: None,
            account_id: None,
        };
        assert!(
            store
                .queue_task(&t.id, TaskStatus::Starting, &start, Timestamp(at))
                .await?
        );
        ids.push(t.id);
    }
    let queue = |store: &SqliteStore| {
        let store = store.clone();
        async move {
            Ok::<_, AppError>(
                store
                    .tasks_with_status(&[TaskStatus::Queued])
                    .await?
                    .into_iter()
                    .map(|t| t.id.as_str().to_owned())
                    .collect::<Vec<_>>(),
            )
        }
    };
    assert_eq!(queue(&store).await?, ["t1", "t2", "t0", "t3"]);
    assert!(
        store
            .reorder_queued(&[ids[2].clone(), ids[1].clone()], Timestamp(500))
            .await?
    );
    assert_eq!(queue(&store).await?, ["t2", "t1", "t0", "t3"]);
    assert!(
        store
            .reorder_queued(&[ids[1].clone(), ids[2].clone()], Timestamp(501))
            .await?
    );
    assert_eq!(queue(&store).await?, ["t1", "t2", "t0", "t3"]);
    // All four share one place: the order asked for still holds, with the others between
    // as they were.
    for id in &ids {
        store
            .transition_task(
                id,
                Some(TaskStatus::Queued),
                TaskStatus::Queued,
                None,
                Timestamp(900),
            )
            .await?;
    }
    assert_eq!(queue(&store).await?, ["t0", "t1", "t2", "t3"]);
    assert!(
        store
            .reorder_queued(&[ids[3].clone(), ids[0].clone()], Timestamp(901))
            .await?
    );
    assert_eq!(queue(&store).await?, ["t3", "t1", "t2", "t0"]);
    Ok(())
}

/// SPEC-487 TCT-003: queued tasks take, in the order given, the places they hold among
/// themselves; a task not named keeps its place between them, each stored start still names
/// its task's place, and nothing changes when one of them is not queued.
#[tokio::test]
async fn queued_tasks_swap_the_places_they_hold() -> R {
    use yulius_app::model::{QueuedStart, QueuedStartKind};
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let mut ids = Vec::new();
    for (i, name) in ["t1", "t2", "t3", "t4"].into_iter().enumerate() {
        let t = store.insert_task(&task(name, &sid, &uid)?).await?;
        let start = QueuedStart {
            kind: QueuedStartKind::Fresh,
            prompt: Some(format!("start of {name}")),
            account_id: None,
        };
        let at = Timestamp(100 + i64::try_from(i)? * 10);
        assert!(
            store
                .queue_task(&t.id, TaskStatus::Starting, &start, at)
                .await?
        );
        ids.push(t.id);
    }
    let queue = |store: &SqliteStore| {
        let store = store.clone();
        async move {
            Ok::<_, AppError>(
                store
                    .tasks_with_status(&[TaskStatus::Queued])
                    .await?
                    .into_iter()
                    .map(|t| (t.id.as_str().to_owned(), t.queued_at.map(|at| at.0)))
                    .collect::<Vec<_>>(),
            )
        }
    };
    // t4 first, then t1; t2 and t3 are not named and stay where they are.
    assert!(
        store
            .reorder_queued(&[ids[3].clone(), ids[0].clone()], Timestamp(500))
            .await?
    );
    assert_eq!(
        queue(&store).await?,
        vec![
            ("t4".to_owned(), Some(100)),
            ("t2".to_owned(), Some(110)),
            ("t3".to_owned(), Some(120)),
            ("t1".to_owned(), Some(130)),
        ]
    );
    for (id, name) in ids.iter().zip(["t1", "t2", "t3", "t4"]) {
        assert_eq!(
            store.queued_start(id).await?.and_then(|start| start.prompt),
            Some(format!("start of {name}")),
            "{name} keeps the start it was to make"
        );
    }
    // One of them left the queue: nothing moves.
    store
        .transition_task(&ids[1], None, TaskStatus::Starting, None, Timestamp(501))
        .await?;
    assert!(
        !store
            .reorder_queued(
                &[ids[0].clone(), ids[1].clone(), ids[3].clone()],
                Timestamp(502)
            )
            .await?
    );
    assert_eq!(
        queue(&store).await?,
        vec![
            ("t4".to_owned(), Some(100)),
            ("t3".to_owned(), Some(120)),
            ("t1".to_owned(), Some(130)),
        ]
    );
    Ok(())
}

/// SPEC-487 TCT-003 (review round 2, R2-2): an order changes the tasks it names. Tasks that
/// stand before the first named one are not written at all, also when they share a place;
/// one behind that has to move by a millisecond to keep its slot does not read as updated.
#[tokio::test]
async fn an_order_leaves_the_tasks_it_does_not_name_as_they_were() -> R {
    use yulius_app::model::{QueuedStart, QueuedStartKind};
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let mut ids = Vec::new();
    // Two tied at the head, two named ones tied behind them, one more tied with those.
    for (name, at) in [
        ("h1", 50),
        ("h2", 50),
        ("t1", 100),
        ("t2", 100),
        ("t3", 100),
    ] {
        let t = store.insert_task(&task(name, &sid, &uid)?).await?;
        let start = QueuedStart {
            kind: QueuedStartKind::Fresh,
            prompt: Some(format!("start of {name}")),
            account_id: None,
        };
        assert!(
            store
                .queue_task(&t.id, TaskStatus::Starting, &start, Timestamp(at))
                .await?
        );
        ids.push(t.id);
    }
    let read = |store: &SqliteStore| {
        let store = store.clone();
        async move {
            Ok::<_, AppError>(
                store
                    .tasks_with_status(&[TaskStatus::Queued])
                    .await?
                    .into_iter()
                    .map(|t| {
                        (
                            t.id.as_str().to_owned(),
                            t.queued_at.map(|at| at.0),
                            t.updated_at.0,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        }
    };
    assert!(
        store
            .reorder_queued(&[ids[3].clone(), ids[2].clone()], Timestamp(500))
            .await?
    );
    assert_eq!(
        read(&store).await?,
        vec![
            ("h1".to_owned(), Some(50), 50),
            ("h2".to_owned(), Some(50), 50),
            // It holds the place it is to have already: nothing of it is written.
            ("t2".to_owned(), Some(100), 100),
            ("t1".to_owned(), Some(101), 500),
            // Not named: moved by milliseconds behind them, and not updated.
            ("t3".to_owned(), Some(102), 100),
        ]
    );
    for (id, name) in ids.iter().zip(["h1", "h2", "t1", "t2", "t3"]) {
        assert_eq!(
            store.queued_start(id).await?.and_then(|start| start.prompt),
            Some(format!("start of {name}")),
            "{name} keeps the start it was to make"
        );
    }
    Ok(())
}

/// SPEC-256 LOB-005: the hand-over joins the request of a task created resting (`closed`, the
/// onboarding task before Start onboarding) that never ran; a run, a provider session, another
/// status or the bound refuses it and leaves the prompt as it was.
#[tokio::test]
async fn a_resting_task_that_never_ran_takes_an_addition() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    assert!(
        !store
            .append_resting_prompt(&t.id, "+a", 64, Timestamp(20))
            .await?,
        "`starting` is no resting task"
    );
    store
        .transition_task(&t.id, None, TaskStatus::Closed, None, Timestamp(21))
        .await?;
    assert!(
        store
            .append_resting_prompt(&t.id, "+ł", 4, Timestamp(22))
            .await?
    );
    assert!(
        !store
            .append_resting_prompt(&t.id, "+", 4, Timestamp(23))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p+ł");
    // An addition the prompt holds already (a concurrent call added it) is not added again.
    assert!(
        !store
            .append_resting_prompt(&t.id, "+ł", 1024, Timestamp(23))
            .await?
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p+ł");
    store.insert_attempt(&attempt("a1", &t.id, 1)?).await?;
    assert!(
        !store
            .append_resting_prompt(&t.id, "+b", 1024, Timestamp(24))
            .await?,
        "it ran"
    );
    assert_eq!(store.task(&t.id).await?.ok_or("task")?.prompt, "p+ł");
    Ok(())
}

/// SPEC-083 NUM-002: a record number is reserved once per sandbox, directory and prefix; the
/// reservations survive a reopen, oldest first, and go with their task when it is deleted.
#[tokio::test]
async fn record_numbers_are_reserved_once_and_go_with_their_task() -> R {
    use yulius_app::model::RecordReservation;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("y.db");
    let reserve = |task: &str,
                   prefix: &str,
                   number: u64,
                   at: i64|
     -> Result<RecordReservation, Box<dyn std::error::Error>> {
        Ok(RecordReservation {
            sandbox_id: SandboxId::parse("s1")?,
            dir: "docs/decisions".into(),
            prefix: prefix.into(),
            number,
            task_id: TaskId::parse(task)?,
            reserved_at: Timestamp(at),
        })
    };
    {
        let store = SqliteStore::open(&path)?;
        let (sid, uid) = seed(&store).await?;
        store.insert_task(&task("t1", &sid, &uid)?).await?;
        store.insert_task(&task("t2", &sid, &uid)?).await?;
        assert!(
            store
                .reserve_record_number(&reserve("t1", "", 66, 5)?)
                .await?
        );
        assert!(
            !store
                .reserve_record_number(&reserve("t2", "", 66, 6)?)
                .await?,
            "a number is given out once"
        );
        assert!(
            store
                .reserve_record_number(&reserve("t2", "", 67, 6)?)
                .await?
        );
        assert!(
            store
                .reserve_record_number(&reserve("t2", "ADR-", 66, 7)?)
                .await?,
            "another prefix is another series"
        );
    }
    let store = SqliteStore::open(&path)?;
    let sid = SandboxId::parse("s1")?;
    let held: Vec<(String, String, u64)> = store
        .record_reservations(&sid, "docs/decisions")
        .await?
        .into_iter()
        .map(|r| (r.task_id.as_str().to_owned(), r.prefix, r.number))
        .collect();
    assert_eq!(
        held,
        vec![
            ("t1".into(), String::new(), 66),
            ("t2".into(), String::new(), 67),
            ("t2".into(), "ADR-".into(), 66),
        ]
    );
    assert!(store.record_reservations(&sid, "docs").await?.is_empty());
    assert!(store.delete_task(&TaskId::parse("t2")?).await?);
    assert_eq!(
        store
            .record_reservations(&sid, "docs/decisions")
            .await?
            .len(),
        1
    );
    Ok(())
}

/// SPEC-105 PRP-002/PRP-003: a prepared task's start time survives a reopen, can be moved and
/// cleared, and is due only while the task is still prepared; deleting the task drops it.
#[tokio::test]
async fn prepared_task_schedules_persist_and_come_due() -> R {
    use yulius_app::ports::store_ext::IntakeStore;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("y.db");
    let (t1, t2, t3) = (
        TaskId::parse("t1")?,
        TaskId::parse("t2")?,
        TaskId::parse("t3")?,
    );
    {
        let store = SqliteStore::open(&path)?;
        let (sid, uid) = seed(&store).await?;
        for id in ["t1", "t2", "t3"] {
            let mut record = task(id, &sid, &uid)?;
            record.status = TaskStatus::Prepared;
            store.insert_task(&record).await?;
        }
        store
            .set_task_schedule(&t1, Some(Timestamp(1_000)), Timestamp(1))
            .await?;
        store
            .set_task_schedule(&t2, Some(Timestamp(5_000)), Timestamp(1))
            .await?;
        store
            .set_task_schedule(&t3, Some(Timestamp(900)), Timestamp(1))
            .await?;
    }
    let store = SqliteStore::open(&path)?;
    assert_eq!(store.task_schedule(&t1).await?, Some(Timestamp(1_000)));
    // Due: prepared with a time at or before now, earliest first.
    assert_eq!(store.due_prepared(Timestamp(999)).await?, vec![t3.clone()]);
    assert_eq!(
        store.due_prepared(Timestamp(1_000)).await?,
        vec![t3.clone(), t1.clone()]
    );
    // Moved, then cleared ("when I decide"): never due again.
    store
        .set_task_schedule(&t1, Some(Timestamp(7_000)), Timestamp(2))
        .await?;
    assert_eq!(store.task_schedule(&t1).await?, Some(Timestamp(7_000)));
    store.set_task_schedule(&t1, None, Timestamp(3)).await?;
    assert_eq!(store.task_schedule(&t1).await?, None);
    // A task that left `prepared` is not due, whatever its time.
    assert!(
        store
            .transition_task(
                &t2,
                Some(TaskStatus::Prepared),
                TaskStatus::Queued,
                None,
                Timestamp(4)
            )
            .await?
    );
    assert_eq!(
        store.due_prepared(Timestamp(10_000)).await?,
        vec![t3.clone()]
    );
    assert!(store.delete_task(&t3).await?);
    assert_eq!(store.task_schedule(&t3).await?, None);
    assert!(store.due_prepared(Timestamp(10_000)).await?.is_empty());
    Ok(())
}

/// SPEC-105 PRP-003: the tick's release holds only while the task is prepared and its time, as
/// stored now, has come: a time moved later or cleared after the tick read it wins.
#[tokio::test]
async fn a_scheduled_release_checks_the_time_it_releases_on() -> R {
    use yulius_app::ports::store_ext::IntakeStore;
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, uid) = seed(&store).await?;
    let (t1, t2) = (TaskId::parse("t1")?, TaskId::parse("t2")?);
    for id in ["t1", "t2"] {
        let mut record = task(id, &sid, &uid)?;
        record.status = TaskStatus::Prepared;
        store.insert_task(&record).await?;
    }
    store
        .set_task_schedule(&t1, Some(Timestamp(1_000)), Timestamp(1))
        .await?;
    // Moved later after the tick saw it due: not released.
    store
        .set_task_schedule(&t1, Some(Timestamp(9_000)), Timestamp(2))
        .await?;
    assert!(!store.release_scheduled(&t1, Timestamp(1_000)).await?);
    // No time ("when I decide"): never released by the tick.
    assert!(!store.release_scheduled(&t2, Timestamp(50_000)).await?);
    assert_eq!(
        store.task(&t2).await?.ok_or("t2")?.status,
        TaskStatus::Prepared
    );
    // Due: queued once, in line from now.
    assert!(store.release_scheduled(&t1, Timestamp(9_000)).await?);
    let released = store.task(&t1).await?.ok_or("t1")?;
    assert_eq!(released.status, TaskStatus::Queued);
    assert_eq!(released.queued_at, Some(Timestamp(9_000)));
    assert!(!store.release_scheduled(&t1, Timestamp(9_500)).await?);
    Ok(())
}

/// SPEC-126 CHS-002/CHS-003: a waiting task's start is replaced in one step: the status read
/// (compare-and-set), the chain and the start time together; a task that left that status keeps
/// everything.
#[tokio::test]
async fn a_waiting_task_start_is_replaced_in_one_step() -> R {
    use yulius_app::intake::{ChainRecord, StartChange};
    use yulius_app::ports::store_ext::IntakeStore;
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, uid) = seed(&store).await?;
    let (t1, t2) = (TaskId::parse("t1")?, TaskId::parse("t2")?);
    store.insert_task(&task("t1", &sid, &uid)?).await?;
    let mut record = task("t2", &sid, &uid)?;
    record.status = TaskStatus::Prepared;
    store.insert_task(&record).await?;
    store
        .set_task_schedule(&t2, Some(Timestamp(9_000)), Timestamp(1))
        .await?;
    let chain = |rule: Option<&str>, at| ChainRecord {
        task_id: t2.clone(),
        after_task_id: Some(t1.clone()),
        after_number: 1,
        blocked_reason: None,
        created_at: Timestamp(at),
        rule: rule.map(str::to_owned),
    };
    // Prepared (scheduled) → after #1: chained, no time.
    let change = StartChange {
        task_id: t2.clone(),
        from: TaskStatus::Prepared,
        to: TaskStatus::Chained,
        chain: Some(chain(None, 10)),
        start_at: None,
        supertask: false,
        at: Timestamp(10),
    };
    assert!(store.change_start(&change).await?);
    assert_eq!(
        store.task(&t2).await?.ok_or("t2")?.status,
        TaskStatus::Chained
    );
    assert_eq!(store.chain(&t2).await?, Some(chain(None, 10)));
    assert_eq!(store.task_schedule(&t2).await?, None);
    // A change read before that one (still prepared) changes nothing.
    assert!(!store.change_start(&change).await?);
    // Chained → a rule: the chain row is replaced, not added.
    let ruled = StartChange {
        from: TaskStatus::Chained,
        chain: Some(chain(Some("#1 done"), 20)),
        at: Timestamp(20),
        ..change.clone()
    };
    assert!(store.change_start(&ruled).await?);
    assert_eq!(store.chain(&t2).await?, Some(chain(Some("#1 done"), 20)));
    assert!(store.chain_successors(&t1).await?.len() == 1);
    // Chained → prepared with a time: no chain any more.
    assert!(
        store
            .change_start(&StartChange {
                from: TaskStatus::Chained,
                to: TaskStatus::Prepared,
                chain: None,
                start_at: Some(Timestamp(7_000)),
                at: Timestamp(30),
                ..change.clone()
            })
            .await?
    );
    assert_eq!(store.chain(&t2).await?, None);
    assert_eq!(store.task_schedule(&t2).await?, Some(Timestamp(7_000)));
    // Prepared → a supertask now: queued from now, flagged, no time.
    assert!(
        store
            .change_start(&StartChange {
                from: TaskStatus::Prepared,
                to: TaskStatus::Queued,
                chain: None,
                start_at: None,
                supertask: true,
                at: Timestamp(40),
                ..change.clone()
            })
            .await?
    );
    let started = store.task(&t2).await?.ok_or("t2")?;
    assert_eq!(started.status, TaskStatus::Queued);
    assert_eq!(started.queued_at, Some(Timestamp(40)));
    assert!(started.supertask);
    assert_eq!(store.task_schedule(&t2).await?, None);
    Ok(())
}

/// SPEC-126 CHS-005: a release decided on a chain holds only while the stored chain still names
/// the same predecessor and rule: a start changed after the evaluation read it wins.
#[tokio::test]
async fn a_chain_release_checks_the_chain_it_evaluated() -> R {
    use yulius_app::intake::{ChainRecord, StartChange};
    use yulius_app::ports::store_ext::IntakeStore;
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, uid) = seed(&store).await?;
    let (t1, t2, t3) = (
        TaskId::parse("t1")?,
        TaskId::parse("t2")?,
        TaskId::parse("t3")?,
    );
    store.insert_task(&task("t1", &sid, &uid)?).await?;
    store.insert_task(&task("t2", &sid, &uid)?).await?;
    let mut record = task("t3", &sid, &uid)?;
    record.status = TaskStatus::Chained;
    store.insert_task(&record).await?;
    let evaluated = ChainRecord {
        task_id: t3.clone(),
        after_task_id: Some(t1.clone()),
        after_number: 1,
        blocked_reason: None,
        created_at: Timestamp(5),
        rule: None,
    };
    store.insert_chain(&evaluated).await?;
    // The person moved it after #2 once the evaluation of "after #1" had read it.
    assert!(
        store
            .change_start(&StartChange {
                task_id: t3.clone(),
                from: TaskStatus::Chained,
                to: TaskStatus::Chained,
                chain: Some(ChainRecord {
                    after_task_id: Some(t2.clone()),
                    after_number: 2,
                    created_at: Timestamp(6),
                    ..evaluated.clone()
                }),
                start_at: None,
                supertask: false,
                at: Timestamp(6),
            })
            .await?
    );
    assert!(!store.release_chain(&evaluated, Timestamp(7)).await?);
    // Nor does it block the new chain (review of SPEC-126: an old evaluation found its rule false).
    assert!(!store.block_chain(&evaluated, "#1 failed").await?);
    assert_eq!(store.chain(&t3).await?.ok_or("chain")?.blocked_reason, None);
    assert_eq!(
        store.task(&t3).await?.ok_or("t3")?.status,
        TaskStatus::Chained
    );
    // The same predecessor and rule written again later is another chain (CHS-006 judges from its
    // time): an evaluation of the older one changes nothing.
    let rewritten = store.chain(&t3).await?.ok_or("chain")?;
    assert!(
        store
            .change_start(&StartChange {
                task_id: t3.clone(),
                from: TaskStatus::Chained,
                to: TaskStatus::Chained,
                chain: Some(ChainRecord {
                    created_at: Timestamp(7),
                    ..rewritten.clone()
                }),
                start_at: None,
                supertask: false,
                at: Timestamp(7),
            })
            .await?
    );
    assert!(!store.release_chain(&rewritten, Timestamp(7)).await?);
    assert!(!store.block_chain(&rewritten, "stale").await?);
    let current = store.chain(&t3).await?.ok_or("chain")?;
    assert!(store.block_chain(&current, "#2 failed").await?);
    assert_eq!(
        store
            .chain(&t3)
            .await?
            .ok_or("chain")?
            .blocked_reason
            .as_deref(),
        Some("#2 failed")
    );
    store.set_chain_blocked(&t3, None).await?;
    // The current chain releases it, once, in line from then.
    let current = store.chain(&t3).await?.ok_or("chain")?;
    assert!(store.release_chain(&current, Timestamp(8)).await?);
    let released = store.task(&t3).await?.ok_or("t3")?;
    assert_eq!(released.status, TaskStatus::Queued);
    assert_eq!(released.queued_at, Some(Timestamp(8)));
    assert!(!store.release_chain(&current, Timestamp(9)).await?);
    Ok(())
}

/// SPEC-118 KEY-002: every write that can change who may type into a terminal grows the access
/// epoch (an open terminal re-checks its viewer's write access before the next keystroke); a
/// permission refresh that finds the same level does not.
#[tokio::test]
async fn writes_that_change_access_grow_the_access_epoch() -> R {
    use yulius_app::ports::store::PermissionStore;
    use yulius_domain::permission::AccessLevel;
    fn grew(last: &mut u64, what: &str, now: u64) -> R {
        if now <= *last {
            return Err(format!("{what} did not grow the access epoch ({last} → {now})").into());
        }
        *last = now;
        Ok(())
    }
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let (sid, alice) = seed(&store).await?;
    store.insert_user(&user("u2", Some("bob"), false)?).await?;
    let bob = UserId::parse("u2")?;
    let t = store.insert_task(&task("t1", &sid, &alice)?).await?;

    let mut last = store.access_epoch();

    // A first cached level takes nothing away: no grant was confirmed without one.
    store
        .put_access(&alice, &sid, AccessLevel::Write, Timestamp(1))
        .await?;
    assert_eq!(
        store.access_epoch(),
        last,
        "a first cached level changes nothing"
    );
    store
        .put_access(&alice, &sid, AccessLevel::Write, Timestamp(2))
        .await?;
    assert_eq!(
        store.access_epoch(),
        last,
        "the same level again changes nothing"
    );
    store
        .put_access(&alice, &sid, AccessLevel::Read, Timestamp(3))
        .await?;
    grew(&mut last, "a downgrade", store.access_epoch())?;
    store.clear_access(&alice).await?;
    grew(&mut last, "a cleared cache", store.access_epoch())?;

    assert!(store.request_control(&t.id, &bob, Timestamp(20)).await?);
    assert!(
        store
            .pass_control(&t.id, &alice, &bob, Timestamp(21))
            .await?
    );
    grew(&mut last, "a hand-over", store.access_epoch())?;
    assert!(
        store
            .take_control(&t.id, &bob, &alice, Timestamp(22))
            .await?
    );
    grew(&mut last, "a take-over", store.access_epoch())?;

    store
        .update_user_flags(&bob, Some(Some(Timestamp(30))), None)
        .await?;
    grew(&mut last, "disabling a user", store.access_epoch())?;
    store.update_user_flags(&bob, None, Some(true)).await?;
    grew(&mut last, "the instance-admin flag", store.access_epoch())?;
    store.link_github(&bob, 7, "bobby", "Bob", None).await?;
    grew(&mut last, "a GitHub link", store.access_epoch())?;
    store.revoke_web_session("nohash", Timestamp(31)).await?;
    grew(&mut last, "a session revocation", store.access_epoch())?;
    store.revoke_user_credentials(&bob, Timestamp(32)).await?;
    grew(
        &mut last,
        "revoking a user's credentials",
        store.access_epoch(),
    )?;
    // A revoked API token ends typing on the sockets it opened at the next key, as a logout
    // does (SPEC-247 RMT-001 "refused at once"; SPEC-483 A3).
    store
        .revoke_api_token(&bob, "no-such-token", Timestamp(33))
        .await?;
    grew(&mut last, "an API token revocation", store.access_epoch())?;

    assert!(store.delete_task(&t.id).await?);
    grew(&mut last, "deleting a task", store.access_epoch())?;
    store.remove_sandbox(&sid, Timestamp(40)).await?;
    grew(&mut last, "removing a sandbox", store.access_epoch())?;
    Ok(())
}

/// SPEC-176 MPG-003: a playground's record becomes a GitHub repository's only while it is still a
/// playground; a repository another sandbox holds is a conflict; access may change, so the access
/// epoch grows.
#[tokio::test]
async fn a_playground_record_moves_to_github_once() -> R {
    use yulius_app::ports::store::PermissionStore;
    let store = SqliteStore::open_in_memory()?;
    let (studio, alice) = seed(&store).await?;
    let lab = SandboxId::parse("s2")?;
    let mut playground = store.sandbox(&studio).await?.ok_or("seeded")?;
    playground.id = lab.clone();
    playground.slug = "lab".into();
    playground.repo_owner = String::new();
    playground.repo_name = "lab".into();
    playground.github_repo_id = None;
    playground.port_base = 20020;
    playground.created_by = alice;
    store.insert_sandbox(&playground).await?;

    assert!(matches!(
        store
            .move_to_github(&lab, "Quartio-Studio", "studio", Some(6))
            .await,
        Err(AppError::Conflict(_))
    ));
    assert!(store.sandbox(&lab).await?.ok_or("lab")?.is_playground());

    let before = store.access_epoch();
    assert!(
        store
            .move_to_github(&lab, "Quartio-Studio", "lab", Some(7))
            .await?
    );
    assert!(store.access_epoch() > before, "access follows GitHub now");
    let moved = store.sandbox(&lab).await?.ok_or("lab")?;
    assert_eq!(
        (
            moved.repo_owner.as_str(),
            moved.repo_name.as_str(),
            moved.github_repo_id
        ),
        ("Quartio-Studio", "lab", Some(7))
    );
    assert_eq!(moved.slug, "lab", "the slug stays");
    assert!(
        !store
            .move_to_github(&lab, "Quartio-Studio", "other", Some(8))
            .await?,
        "a GitHub sandbox is never moved again"
    );
    assert!(
        !store
            .move_to_github(&studio, "Quartio-Studio", "x", None)
            .await?
    );
    Ok(())
}

/// An account keeps the email of its Claude account and when Claude answered for it; a rename
/// keeps both, and a new credential (another sign-in, maybe of another Claude account) drops
/// both (SPEC-244 SUB-003).
#[tokio::test]
async fn an_account_email_lasts_until_the_next_sign_in() -> R {
    use yulius_app::model::{AccountChange, AccountRecord, AccountStatus, Sealed};
    use yulius_app::ports::store::AccountStore;
    use yulius_domain::harness::Provider;
    use yulius_domain::ids::AccountId;

    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(&dir.path().join("y.db"))?;
    let id = AccountId::parse("acc1")?;
    store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: Provider::Anthropic,
            label: "work Max".into(),
            credential: Some(Sealed(vec![1])),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    let fresh = store.account(&id).await?.ok_or("account")?;
    assert_eq!((fresh.email, fresh.email_checked_at), (None, None));

    // An answer for another credential (an older sign-in) is not stored.
    let stored = store
        .set_account_email(
            &id,
            &Sealed(vec![9]),
            Some("old@example.com"),
            Some(Timestamp(4)),
            Timestamp(4),
        )
        .await?;
    assert!(!stored);
    assert_eq!(store.account(&id).await?.ok_or("account")?.email, None);
    let stored = store
        .set_account_email(
            &id,
            &Sealed(vec![1]),
            Some("pawel@example.com"),
            Some(Timestamp(5)),
            Timestamp(5),
        )
        .await?;
    assert!(stored);
    let rename = AccountChange {
        label: Some("personal".into()),
        email: None,
    };
    store.update_account(&id, &rename, Timestamp(6)).await?;
    let named = store.account(&id).await?.ok_or("account")?;
    assert_eq!(named.label, "personal");
    assert_eq!(named.email.as_deref(), Some("pawel@example.com"));
    assert_eq!(named.email_checked_at, Some(Timestamp(5)));

    store
        .set_account_credential(&id, &Sealed(vec![2]), AccountStatus::Ok, Timestamp(7))
        .await?;
    let signed_in = store.account(&id).await?.ok_or("account")?;
    assert_eq!((signed_in.email, signed_in.email_checked_at), (None, None));
    assert_eq!(signed_in.label, "personal");
    Ok(())
}

/// An email a person typed is theirs, like the name (SPEC-463 EML-004): a new sign-in keeps it
/// and Claude's answer never replaces it, though the answer still counts as given. Removing it
/// clears the email and the check, so Claude is asked again and its answer is stored as before.
#[tokio::test]
async fn a_typed_account_email_stays_until_its_person_removes_it() -> R {
    use yulius_app::model::{AccountChange, AccountRecord, AccountStatus, Sealed};
    use yulius_app::ports::store::AccountStore;
    use yulius_domain::harness::Provider;
    use yulius_domain::ids::AccountId;

    let store = SqliteStore::open_in_memory()?;
    let id = AccountId::parse("acc1")?;
    store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: Provider::Anthropic,
            label: "work Max".into(),
            credential: Some(Sealed(vec![1])),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    let typed = |email: &str| AccountChange {
        label: None,
        email: Some(Some(email.to_owned())),
    };
    let seen = || async {
        let a = store.account(&id).await?.ok_or("account")?;
        Ok::<_, Box<dyn std::error::Error>>((a.label, a.email, a.email_checked_at))
    };

    store
        .update_account(&id, &typed("agent@example.com"), Timestamp(2))
        .await?;
    assert_eq!(
        seen().await?,
        ("work Max".into(), Some("agent@example.com".into()), None),
        "the name is left alone"
    );
    let record = store.account(&id).await?.ok_or("account")?;
    assert_eq!(
        (record.email_typed, record.updated_at),
        (true, Timestamp(2))
    );

    // Claude answers for this sign-in, with another address and then with none: neither
    // replaces what the person typed, and the answer is marked so nobody asks again.
    for (answer, at) in [(Some("claude@example.com"), 3), (None, 4)] {
        let stored = store
            .set_account_email(
                &id,
                &Sealed(vec![1]),
                answer,
                Some(Timestamp(at)),
                Timestamp(at),
            )
            .await?;
        assert!(stored, "the answer is for the current sign-in");
        assert_eq!(
            seen().await?,
            (
                "work Max".into(),
                Some("agent@example.com".into()),
                Some(Timestamp(at))
            )
        );
    }

    // A new sign-in keeps the typed email and opens the question to Claude again.
    store
        .set_account_credential(&id, &Sealed(vec![2]), AccountStatus::Ok, Timestamp(5))
        .await?;
    assert_eq!(
        seen().await?,
        ("work Max".into(), Some("agent@example.com".into()), None)
    );

    // A rename leaves it alone; a name and an email change together.
    let rename = AccountChange {
        label: Some("Agent TEAM".into()),
        email: None,
    };
    store.update_account(&id, &rename, Timestamp(6)).await?;
    assert_eq!(
        seen().await?,
        ("Agent TEAM".into(), Some("agent@example.com".into()), None)
    );
    let both = AccountChange {
        label: Some("team".into()),
        email: Some(Some("team@example.com".into())),
    };
    store.update_account(&id, &both, Timestamp(7)).await?;
    assert_eq!(
        seen().await?,
        ("team".into(), Some("team@example.com".into()), None)
    );

    // Removed: the email and Claude's check are gone, so Claude is asked again, and what it
    // answers is stored as it was before anything was typed.
    store
        .set_account_email(
            &id,
            &Sealed(vec![2]),
            None,
            Some(Timestamp(8)),
            Timestamp(8),
        )
        .await?;
    let removed = AccountChange {
        label: None,
        email: Some(None),
    };
    store.update_account(&id, &removed, Timestamp(9)).await?;
    assert_eq!(seen().await?, ("team".into(), None, None));
    assert!(!store.account(&id).await?.ok_or("account")?.email_typed);
    store
        .set_account_email(
            &id,
            &Sealed(vec![2]),
            Some("claude@example.com"),
            Some(Timestamp(10)),
            Timestamp(10),
        )
        .await?;
    assert_eq!(
        seen().await?,
        (
            "team".into(),
            Some("claude@example.com".into()),
            Some(Timestamp(10))
        )
    );
    assert!(!store.account(&id).await?.ok_or("account")?.email_typed);
    // That one is Claude's again: the next sign-in drops it (SPEC-244 SUB-003).
    store
        .set_account_credential(&id, &Sealed(vec![3]), AccountStatus::Ok, Timestamp(11))
        .await?;
    assert_eq!(seen().await?, ("team".into(), None, None));

    // An empty change writes nothing, not even the time.
    store
        .update_account(&id, &AccountChange::default(), Timestamp(12))
        .await?;
    assert_eq!(
        store.account(&id).await?.ok_or("account")?.updated_at,
        Timestamp(11)
    );
    Ok(())
}

/// SPEC-281 SSN-003: the lineage is listed in the order sessions were adopted. An earlier session
/// adopted again (`/resume` back to it, or the TUI's own session after a nested one) comes last:
/// it is the one watchers read and a Continue resumes.
#[tokio::test]
async fn the_lineage_lists_the_session_adopted_last_last() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t = store.insert_task(&task("t1", &sid, &uid)?).await?;
    let adopt = |session: &str, at: i64| yulius_app::model::ProviderSessionRecord {
        id: format!("ps-{session}-{at}"),
        sandbox_id: sid.clone(),
        task_id: t.id.clone(),
        harness: Harness::ClaudeCode,
        session_id: session.into(),
        transcript_ref: None,
        origin: "native".into(),
        first_seen: Timestamp(at),
        last_seen: Timestamp(at),
    };
    store.upsert_provider_session(&adopt("tui", 10)).await?;
    store.upsert_provider_session(&adopt("nested", 20)).await?;
    let order = |list: Vec<yulius_app::model::ProviderSessionRecord>| {
        list.into_iter().map(|s| s.session_id).collect::<Vec<_>>()
    };
    assert_eq!(
        order(store.provider_sessions(&t.id).await?),
        ["tui", "nested"]
    );
    store.upsert_provider_session(&adopt("tui", 30)).await?;
    let lineage = store.provider_sessions(&t.id).await?;
    assert_eq!(
        lineage
            .iter()
            .map(|s| (s.first_seen, s.last_seen))
            .collect::<Vec<_>>(),
        [
            (Timestamp(20), Timestamp(20)),
            (Timestamp(10), Timestamp(30))
        ],
        "re-adopting keeps when it was first seen"
    );
    assert_eq!(order(lineage), ["nested", "tui"]);
    Ok(())
}

/// SPEC-281, HRN-004: a session is one task's. Upserting another task's session id changes
/// neither that task's lineage (its order is what its watchers and resumes follow) nor adds it to
/// the upserting task's.
#[tokio::test]
async fn a_session_of_another_task_is_not_reordered_or_taken() -> R {
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let a = store.insert_task(&task("ta", &sid, &uid)?).await?;
    let b = store.insert_task(&task("tb", &sid, &uid)?).await?;
    let entry = |t: &TaskId, session: &str, at: i64| yulius_app::model::ProviderSessionRecord {
        id: format!("ps-{}-{session}-{at}", t.as_str()),
        sandbox_id: sid.clone(),
        task_id: t.clone(),
        harness: Harness::ClaudeCode,
        session_id: session.into(),
        transcript_ref: None,
        origin: "native".into(),
        first_seen: Timestamp(at),
        last_seen: Timestamp(at),
    };
    store
        .upsert_provider_session(&entry(&b.id, "b-1", 10))
        .await?;
    store
        .upsert_provider_session(&entry(&b.id, "b-2", 20))
        .await?;
    store
        .upsert_provider_session(&entry(&a.id, "b-1", 30))
        .await?;
    let order = |list: Vec<yulius_app::model::ProviderSessionRecord>| {
        list.into_iter()
            .map(|s| (s.session_id, s.last_seen))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order(store.provider_sessions(&b.id).await?),
        [
            ("b-1".to_owned(), Timestamp(10)),
            ("b-2".to_owned(), Timestamp(20))
        ]
    );
    assert!(store.provider_sessions(&a.id).await?.is_empty());
    Ok(())
}

/// A playground's guest links (SPEC-410): bounded live links and guests per link, a guest only
/// through a live link, a revoke that ends its guests' sessions once, the expiry sweep's list, and
/// whether a playground ever had a guest (no longer once it is a GitHub sandbox).
#[tokio::test]
async fn playground_guest_links_are_bounded_and_end_once() -> R {
    use yulius_app::model::WebSessionRecord;
    use yulius_app::ports::guests::{GuestJoin, PlaygroundGuestStore, PlaygroundInviteRecord};
    let store = SqliteStore::open_in_memory()?;
    let (studio, alice) = seed(&store).await?;
    let lab = SandboxId::parse("s2")?;
    let mut playground = store.sandbox(&studio).await?.ok_or("seeded")?;
    playground.id = lab.clone();
    playground.slug = "lab".into();
    playground.repo_owner = String::new();
    playground.repo_name = "lab".into();
    playground.github_repo_id = None;
    playground.port_base = 20020;
    store.insert_sandbox(&playground).await?;
    let link = |id: &str, expires: i64| PlaygroundInviteRecord {
        id: id.into(),
        sandbox_id: lab.clone(),
        code_hash: format!("hash-{id}"),
        created_by: alice.clone(),
        created_at: Timestamp(10),
        expires_at: Timestamp(expires),
        revoked_at: None,
    };
    assert!(store.insert_playground_invite(&link("i1", 100), 2).await?);
    assert!(store.insert_playground_invite(&link("i2", 50), 2).await?);
    assert!(
        !store.insert_playground_invite(&link("i3", 100), 2).await?,
        "at most two live links"
    );
    assert!(!store.playground_has_guests(&lab).await?);

    let guest = |id: &str| user(id, None, false);
    assert_eq!(
        store
            .insert_playground_guest("i1", &guest("g1")?, Timestamp(20), 1)
            .await?,
        GuestJoin::Joined
    );
    assert_eq!(
        store
            .insert_playground_guest("i1", &guest("g2")?, Timestamp(20), 1)
            .await?,
        GuestJoin::Full
    );
    assert_eq!(
        store
            .insert_playground_guest("i2", &guest("g3")?, Timestamp(60), 1)
            .await?,
        GuestJoin::Dead,
        "an expired link lets nobody in"
    );
    assert!(
        store.user(&UserId::parse("g2")?).await?.is_none(),
        "nothing was written"
    );
    assert!(store.playground_has_guests(&lab).await?);
    assert_eq!(
        store
            .guest_invite_of(&UserId::parse("g1")?)
            .await?
            .map(|i| i.id),
        Some("i1".into())
    );
    assert_eq!(store.invite_guest_ids("i1").await?, [UserId::parse("g1")?]);

    store
        .insert_web_session(&WebSessionRecord {
            id_hash: "session-g1".into(),
            user_id: UserId::parse("g1")?,
            csrf_token: "csrf".into(),
            created_at: Timestamp(20),
            expires_at: Timestamp(100),
            revoked_at: None,
        })
        .await?;
    // The sweep lists the expired link that nothing ended yet, once.
    let expired = store.unended_expired_invites(Timestamp(60), 10).await?;
    assert_eq!(
        expired.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["i2"]
    );
    store.mark_invite_swept("i2", Timestamp(60), false).await?;
    assert_eq!(
        store
            .unended_expired_invites(Timestamp(60), 10)
            .await?
            .len(),
        1,
        "a try that did not end it keeps it listed"
    );
    store.mark_invite_swept("i2", Timestamp(61), true).await?;
    assert!(
        store
            .unended_expired_invites(Timestamp(60), 10)
            .await?
            .is_empty()
    );

    assert!(
        !store
            .revoke_playground_invite(&studio, "i1", Timestamp(30))
            .await?,
        "another sandbox's"
    );
    assert!(
        store
            .revoke_playground_invite(&lab, "i1", Timestamp(30))
            .await?
    );
    assert!(
        !store
            .revoke_playground_invite(&lab, "i1", Timestamp(31))
            .await?,
        "once"
    );
    let session = store.web_session("session-g1").await?.ok_or("session")?;
    assert_eq!(session.revoked_at, Some(Timestamp(30)));
    let listed = store.unended_expired_invites(Timestamp(30), 10).await?;
    assert_eq!(
        listed.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["i1"],
        "a revoked link waits for its guests' work to end"
    );
    store.mark_invite_swept("i1", Timestamp(31), true).await?;
    assert!(
        store
            .unended_expired_invites(Timestamp(200), 10)
            .await?
            .is_empty()
    );
    assert!(
        store
            .live_playground_invites(&lab, Timestamp(60))
            .await?
            .is_empty()
    );

    store
        .move_to_github(&lab, "Quartio-Studio", "lab", Some(7))
        .await?;
    assert!(
        store.playground_has_guests(&lab).await?,
        "what guests left stays after a move to GitHub"
    );
    Ok(())
}

/// The guest-link sweep (SPEC-410 GST-005) takes the least recently swept dead links first, so
/// links whose guests' work will not stop never hold its batch.
#[tokio::test]
async fn the_guest_link_sweep_takes_the_least_recently_swept_first() -> R {
    use yulius_app::ports::guests::{PlaygroundGuestStore, PlaygroundInviteRecord};
    let store = SqliteStore::open_in_memory()?;
    let (studio, alice) = seed(&store).await?;
    for (id, expires) in [("a", 10), ("b", 20), ("c", 30)] {
        let link = PlaygroundInviteRecord {
            id: id.into(),
            sandbox_id: studio.clone(),
            code_hash: format!("hash-{id}"),
            created_by: alice.clone(),
            created_at: Timestamp(1),
            expires_at: Timestamp(expires),
            revoked_at: None,
        };
        assert!(store.insert_playground_invite(&link, 5).await?);
    }
    let first = |list: Vec<PlaygroundInviteRecord>| list.first().map(|i| i.id.clone());
    assert_eq!(
        first(store.unended_expired_invites(Timestamp(40), 1).await?),
        Some("a".into())
    );
    // "a" and "b" will not stop: tried, not ended.
    store.mark_invite_swept("a", Timestamp(41), false).await?;
    store.mark_invite_swept("b", Timestamp(42), false).await?;
    assert_eq!(
        first(store.unended_expired_invites(Timestamp(50), 1).await?),
        Some("c".into())
    );
    store.mark_invite_swept("c", Timestamp(51), true).await?;
    assert_eq!(
        first(store.unended_expired_invites(Timestamp(60), 1).await?),
        Some("a".into())
    );
    assert_eq!(
        store
            .unended_expired_invites(Timestamp(60), 10)
            .await?
            .len(),
        2
    );
    Ok(())
}

/// SPEC-503 CDX-006: a Codex run's refresh replaces only the credential, and only while the
/// account still holds the one it refreshes: a sign-in made meanwhile wins and nothing changes;
/// status, error, usage and email stay either way.
#[tokio::test]
async fn a_refreshed_credential_replaces_only_the_one_it_refreshes() -> R {
    use yulius_app::model::{AccountRecord, AccountStatus, Sealed};
    use yulius_app::ports::store::AccountStore;
    use yulius_domain::harness::Provider;
    use yulius_domain::ids::AccountId;

    let store = SqliteStore::open_in_memory()?;
    let id = AccountId::parse("acc-codex")?;
    store
        .insert_account(&AccountRecord {
            id: id.clone(),
            provider: Provider::OpenAi,
            label: "Codex".into(),
            credential: Some(Sealed(vec![1])),
            status: AccountStatus::Limited,
            usage_json: Some("{}".into()),
            last_error: Some("usage: read later".into()),
            email: Some("dev@example.com".into()),
            email_checked_at: Some(Timestamp(2)),
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    // Signed in again meanwhile: a refresh of the credential before changes nothing.
    let stale = store
        .refresh_account_credential(&id, &Sealed(vec![9]), &Sealed(vec![2]), Timestamp(5))
        .await?;
    assert!(!stale);
    assert_eq!(
        store.account(&id).await?.ok_or("account")?.credential,
        Some(Sealed(vec![1]))
    );
    let kept = store
        .refresh_account_credential(&id, &Sealed(vec![1]), &Sealed(vec![2]), Timestamp(6))
        .await?;
    assert!(kept);
    let after = store.account(&id).await?.ok_or("account")?;
    assert_eq!(after.credential, Some(Sealed(vec![2])));
    assert_eq!(after.status, AccountStatus::Limited);
    assert_eq!(after.last_error.as_deref(), Some("usage: read later"));
    assert_eq!(after.usage_json.as_deref(), Some("{}"));
    assert_eq!(after.email.as_deref(), Some("dev@example.com"));
    assert_eq!(after.email_checked_at, Some(Timestamp(2)));
    Ok(())
}

fn staging_request(
    id: &str,
    sandbox: &SandboxId,
    task: Option<&TaskId>,
    revision: &str,
    state: yulius_domain::staging::StagingState,
) -> yulius_app::ports::store_ext::StagingRequestRecord {
    yulius_app::ports::store_ext::StagingRequestRecord {
        id: id.to_owned(),
        sandbox_id: sandbox.clone(),
        environment: "staging".into(),
        task_id: task.cloned(),
        revision: revision.to_owned(),
        state,
        ci_run_id: None,
        cd_run_id: None,
        attempts: 0,
        note: None,
        requested_by: "agent:t1".into(),
        told: false,
        created_at: Timestamp(100),
        updated_at: Timestamp(100),
    }
}

/// SPEC-575 MFS-005, MFS-008: a repeated request is the same row, a task has one successful
/// staging deployment, one deploy runs at a time, and every move is a compare-and-set from the
/// state it was decided on (a second tick or a restart never moves a request twice).
#[tokio::test]
async fn staging_requests_are_idempotent_and_move_once() -> R {
    use yulius_app::ports::store_ext::{StagingChange, StagingStore};
    use yulius_domain::staging::StagingState::{
        Deploying, GateFailed, Included, Ready, Superseded, Testing, Verified,
    };
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t1 = task("t1", &sid, &uid)?;
    let t2 = task("t2", &sid, &uid)?;
    store.insert_task(&t1).await?;
    store.insert_task(&t2).await?;
    assert!(
        store
            .insert_staging_request(&staging_request("a", &sid, Some(&t1.id), "r1", Testing))
            .await?
    );
    assert!(
        !store
            .insert_staging_request(&staging_request("a2", &sid, Some(&t1.id), "r1", Testing))
            .await?,
        "the same task and revision: one row"
    );
    let of_t1 = store
        .staging_requests_of(&sid, "staging", Some(&t1.id), "")
        .await?;
    assert_eq!(
        of_t1.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["a"]
    );
    // Compare-and-set: the move applies once, from the state it was decided on.
    let change = StagingChange {
        ci_run_id: Some("ci-1".into()),
        ..StagingChange::default()
    };
    assert!(
        store
            .move_staging_request("a", Testing, Ready, &change, Timestamp(110))
            .await?
    );
    assert!(
        !store
            .move_staging_request("a", Testing, GateFailed, &change, Timestamp(111))
            .await?
    );
    let a = store.staging_request("a").await?.ok_or("a")?;
    assert_eq!(
        (a.state, a.ci_run_id.as_deref(), a.updated_at),
        (Ready, Some("ci-1"), Timestamp(110))
    );
    // One deploy at a time per environment.
    assert!(
        store
            .insert_staging_request(&staging_request("b", &sid, Some(&t2.id), "r2", Ready))
            .await?
    );
    assert!(
        store
            .move_staging_request(
                "a",
                Ready,
                Deploying,
                &StagingChange::default(),
                Timestamp(120)
            )
            .await?
    );
    assert!(
        !store
            .move_staging_request(
                "b",
                Ready,
                Deploying,
                &StagingChange::default(),
                Timestamp(121)
            )
            .await?,
        "a second deploy of the environment is refused"
    );
    let pending = store.pending_staging_requests(&sid, "staging").await?;
    assert_eq!(
        pending.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        store.staging_queues(Timestamp(0)).await?,
        [(sid.clone(), "staging".to_owned())]
    );
    let verified = StagingChange {
        cd_run_id: Some("cd-1".into()),
        attempts: Some(1),
        ..StagingChange::default()
    };
    assert!(
        store
            .move_staging_request("a", Deploying, Verified, &verified, Timestamp(130))
            .await?
    );
    // One successful deployment per task: a second row of t1 can never succeed too.
    assert!(
        store
            .insert_staging_request(&staging_request("a3", &sid, Some(&t1.id), "r3", Testing))
            .await?
    );
    assert!(
        !store
            .move_staging_request(
                "a3",
                Testing,
                Included,
                &StagingChange::default(),
                Timestamp(140)
            )
            .await?
    );
    assert!(
        store
            .move_staging_request(
                "a3",
                Testing,
                Superseded,
                &StagingChange::default(),
                Timestamp(141)
            )
            .await?
    );
    // The outcome is told to the task once.
    let untold = store.untold_staging_requests().await?;
    assert_eq!(
        untold.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["a", "a3"],
        "a superseded request is told too"
    );
    assert!(store.mark_staging_told(&untold[0]).await?);
    assert!(!store.mark_staging_told(&untold[0]).await?);
    assert!(store.mark_staging_told(&untold[1]).await?);
    assert!(store.untold_staging_requests().await?.is_empty());
    let recent = store.recent_staging_requests(&sid, "staging", 2).await?;
    assert_eq!(recent.len(), 2);
    Ok(())
}

/// A notification that already wrote to an agent can only acknowledge the outcome it read.
/// Even another failure on the same request and wall-clock tick has its own delivery.
#[tokio::test]
async fn an_old_staging_acknowledgment_leaves_the_retried_verdict_untold() -> R {
    use yulius_app::ports::store_ext::{StagingChange, StagingStore as _};
    use yulius_domain::staging::StagingState::{DeployFailed, Deploying, Ready};
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let task = task("retry", &sid, &uid)?;
    store.insert_task(&task).await?;
    let first = staging_request("retry", &sid, Some(&task.id), "r1", DeployFailed);
    assert!(store.insert_staging_request(&first).await?);
    for (from, to) in [
        (DeployFailed, Ready),
        (Ready, Deploying),
        (Deploying, DeployFailed),
    ] {
        assert!(
            store
                .move_staging_request(
                    &first.id,
                    from,
                    to,
                    &StagingChange::default(),
                    first.updated_at
                )
                .await?
        );
    }
    assert!(
        !store.mark_staging_told(&first).await?,
        "the first notification cannot acknowledge the retried verdict"
    );
    let current = store
        .staging_request(&first.id)
        .await?
        .ok_or("retried request")?;
    assert_eq!(
        store.untold_staging_requests().await?.as_slice(),
        std::slice::from_ref(&current)
    );
    assert!(store.mark_staging_told(&current).await?);
    assert!(store.untold_staging_requests().await?.is_empty());
    Ok(())
}

/// SPEC-575 (review rounds 1 and 2 of slice 2): deleting tasks keeps their staging requests
/// without their task (what staging verified stays known), also two that landed one revision.
#[tokio::test]
async fn deleted_tasks_keep_their_staging_requests() -> R {
    use yulius_app::ports::store_ext::StagingStore;
    use yulius_domain::staging::StagingState::{Included, Verified};
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t1 = task("t1", &sid, &uid)?;
    let t2 = task("t2", &sid, &uid)?;
    store.insert_task(&t1).await?;
    store.insert_task(&t2).await?;
    assert!(
        store
            .insert_staging_request(&staging_request("a", &sid, Some(&t1.id), "r1", Verified))
            .await?
    );
    assert!(
        store
            .insert_staging_request(&staging_request("b", &sid, Some(&t2.id), "r1", Included))
            .await?
    );
    assert!(store.delete_task(&t1.id).await?);
    assert!(store.delete_task(&t2.id).await?);
    let kept = store.recent_staging_requests(&sid, "staging", 10).await?;
    assert_eq!(
        kept.iter()
            .map(|r| (r.id.as_str(), r.task_id.is_none(), r.state))
            .collect::<Vec<_>>(),
        [("b", true, Included), ("a", true, Verified)]
    );
    Ok(())
}

/// SPEC-575 MFS-012 (review round 1 of slice 4): the bootstrap record. A claim is made once per
/// revision and stays until its task is linked, exactly once. A laid revision is never claimed
/// at that revision, and a higher revision claims again, whatever the state.
#[tokio::test]
async fn bootstrap_upgrades_claim_lay_and_link_once_per_revision() -> R {
    use yulius_app::ports::store_ext::StagingStore;
    let store = SqliteStore::open_in_memory()?;
    let (sid, uid) = seed(&store).await?;
    let t1 = task("t1", &sid, &uid)?;
    store.insert_task(&t1).await?;
    assert!(
        store
            .claim_bootstrap_upgrade(&sid, 1, Timestamp(10))
            .await?
    );
    assert!(
        !store
            .claim_bootstrap_upgrade(&sid, 1, Timestamp(11))
            .await?
    );
    let claim = store.bootstrap_upgrade(&sid).await?.ok_or("claim")?;
    assert_eq!(
        (claim.revision, claim.state.as_str(), claim.claimed_at),
        (1, "claimed", Timestamp(10))
    );
    store.lay_bootstrap_revision(&sid, 1, Timestamp(12)).await?;
    assert_eq!(
        store.bootstrap_upgrade(&sid).await?.ok_or("kept")?.state,
        "claimed",
        "a claim at that revision is not laid over"
    );
    assert!(store.link_bootstrap_task(&sid, 1, &t1.id).await?);
    assert!(
        !store.link_bootstrap_task(&sid, 1, &t1.id).await?,
        "linked once"
    );
    // A higher revision claims again.
    assert!(
        store
            .claim_bootstrap_upgrade(&sid, 2, Timestamp(20))
            .await?
    );
    let again = store.bootstrap_upgrade(&sid).await?.ok_or("again")?;
    assert_eq!(
        (again.revision, again.state.as_str(), again.task_id),
        (2, "claimed", None)
    );
    // A laid revision is never claimed at it.
    let other = SandboxId::parse("s2")?;
    let mut second = store.sandbox(&sid).await?.ok_or("s1")?;
    second.id = other.clone();
    second.slug = "studio-2".into();
    second.repo_name = "studio-2".into();
    second.port_base += 100;
    store.insert_sandbox(&second).await?;
    store
        .lay_bootstrap_revision(&other, 2, Timestamp(30))
        .await?;
    assert!(
        !store
            .claim_bootstrap_upgrade(&other, 2, Timestamp(31))
            .await?
    );
    assert_eq!(
        store.bootstrap_upgrade(&other).await?.ok_or("laid")?.state,
        "laid"
    );
    Ok(())
}

/// SPEC-628 FRK-004, FRK-008: a fork's origin persists, a source lists its forks, deleting the
/// source keeps the fork's link with its number and title but no id, and deleting the fork
/// removes its record.
#[tokio::test]
async fn fork_links_persist_and_outlive_a_deleted_source() -> R {
    use yulius_app::intake::ForkRecord;
    use yulius_app::ports::store_ext::IntakeStore;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("y.db");
    let fork = |id: &str, at: i64| -> Result<ForkRecord, Box<dyn std::error::Error>> {
        Ok(ForkRecord {
            task_id: TaskId::parse(id)?,
            source_task_id: Some(TaskId::parse("t1")?),
            source_number: 1,
            source_title: "Source task".into(),
            source_branch: "feat/1-source-task".into(),
            source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            source_session_id: "11111111-2222-4333-8444-555555555555".into(),
            message: (id == "t2").then(|| "try the other parser".into()),
            transcript_ref: Some(format!("/data/transcripts/{id}/s.jsonl")),
            created_at: Timestamp(at),
        })
    };
    {
        let store = SqliteStore::open(&path)?;
        let (sid, uid) = seed(&store).await?;
        for id in ["t1", "t2", "t3"] {
            store.insert_task(&task(id, &sid, &uid)?).await?;
        }
        store.insert_fork(&fork("t2", 5)?).await?;
        store.insert_fork(&fork("t3", 6)?).await?;
    }
    let store = SqliteStore::open(&path)?;
    let (t1, t2, t3) = (
        TaskId::parse("t1")?,
        TaskId::parse("t2")?,
        TaskId::parse("t3")?,
    );
    assert_eq!(store.fork_of(&t2).await?, Some(fork("t2", 5)?));
    assert_eq!(store.fork_of(&t1).await?, None, "the source is no fork");
    let forks: Vec<TaskId> = store
        .forks_from(&t1)
        .await?
        .into_iter()
        .map(|f| f.task_id)
        .collect();
    assert_eq!(forks, vec![t2.clone(), t3.clone()], "oldest first");
    assert!(store.delete_task(&t1).await?);
    let kept = store.fork_of(&t2).await?.ok_or("fork kept")?;
    assert_eq!(kept.source_task_id, None);
    assert_eq!(
        (kept.source_number, kept.source_title.as_str()),
        (1, "Source task")
    );
    assert!(store.delete_task(&t3).await?);
    assert_eq!(store.fork_of(&t3).await?, None);
    Ok(())
}
