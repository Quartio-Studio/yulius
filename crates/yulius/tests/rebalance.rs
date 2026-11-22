//! Moving idle tasks off a nearly used-up subscription before it runs out (SPEC-629): placement
//! past a harness's rebalance threshold, idle moves, pins and "nothing has room", with real
//! processes as in `subscription_moves.rs`.

use crate::pool_supervision::{
    HOUR, ScriptedUsage, build_with, new_task, refresh, runtime, seed, stop, temp, usage,
};
use crate::subscription_moves::{ready, run_accounts};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;

use std::sync::Arc;
use std::time::Duration;

use yulius_ptyhost::test_support::locate_fake_agent;

fn agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    }
}

/// RBL-002, AC-1: a new task goes to the subscription below 95 %, although the one at 96 %,
/// whose week resets in an hour, has the better use-it-or-lose-it score.
#[test]
fn a_new_task_keeps_off_a_subscription_past_the_threshold() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe.clone(), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-early", "b-roomier"]).await?;
        probe.set("a-early", usage(10.0, 2 * HOUR, 96.0, 1));
        refresh(&app, &user, "a-early").await?;
        probe.set("b-roomier", usage(10.0, 2 * HOUR, 94.0, 100));
        refresh(&app, &user, "b-roomier").await?;
        let id = new_task(&app, &sandbox, &user, "Placed").await?;
        ready(&app, &id).await?;
        assert_eq!(
            run_accounts(&app, &id).await,
            [Some("b-roomier".to_owned())]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
