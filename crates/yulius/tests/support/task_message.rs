//! The fake agent's screen modes and a Superagent's terminal.

use crate::support::agents::FakeAgentOptions;
use crate::support::eventually;
use crate::support::live::first_message::{hooked, screen, start};
use crate::support::live::pool::ScriptedUsage;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::coordination::{Channel, ProjectCaller};
use yulius_app::terminal::TerminalAttach;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_ptyhost::test_support::locate_fake_agent;

type Error = Box<dyn std::error::Error>;

/// The Superagent of `user` in `sandbox`, as `cockpit_caller` makes it.
pub(crate) fn superagent(sandbox: &SandboxId, user: &UserId, login: &str) -> ProjectCaller {
    ProjectCaller {
        sandbox: sandbox.clone(),
        user: user.clone(),
        task: None,
        channel: Channel::Superagent,
        actor: format!("cockpit:{login}"),
        detail: json!({"via": "cockpit", "shell": "sh-test"}),
    }
}

/// A hooked app (its hook server up, so Claude Code's SessionStart reaches it) whose fake agent
/// runs as `agent`.
pub(crate) async fn app(data: &Path, agent: FakeAgentOptions) -> Result<Arc<App>, Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (app, _) = start(data, &fake, agent, 0, Arc::new(ScriptedUsage::default())).await?;
    Ok(app)
}

pub(crate) fn fake_agent() -> FakeAgentOptions {
    FakeAgentOptions {
        answers_first_message: true,
        ..hooked()
    }
}

/// A writer's terminal on `id` for the person `caller` acts for: what they type goes the way a
/// viewer's keys go.
pub(crate) async fn terminal(
    app: &Arc<App>,
    caller: &ProjectCaller,
    id: &TaskId,
) -> Result<(AuthContext, TerminalAttach), Error> {
    let ctx = AuthContext {
        user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let attach = app
        .terminal_open(&ctx, id.as_str(), Some((120, 40)))
        .await?;
    Ok((ctx, attach))
}

/// A fake agent that draws full screen as Claude Code does: its prompt box (a row `❯` between
/// two rules) with the cursor in it. The PTY echoes what a person types into that row, so a
/// draft shows in the box, and `Ctrl+U` erases it there.
pub(crate) fn full_screen() -> FakeAgentOptions {
    FakeAgentOptions {
        tui: true,
        ..fake_agent()
    }
}

/// Read the fake agent's rendered input independently of the production screen recognizer.
pub(crate) fn fixture_box_empty(app: &App, id: &TaskId) -> bool {
    screen(app, id).lines().any(|row| row.trim_end() == "❯")
}

/// Wait for the fake agent's SIGWINCH redraw before sending PTY-echoed draft text.
pub(crate) async fn full_screen_terminal(
    app: &Arc<App>,
    caller: &ProjectCaller,
    id: &TaskId,
) -> Result<(AuthContext, TerminalAttach), Error> {
    let terminal = terminal(app, caller, id).await?;
    eventually("the resized box", Duration::from_secs(30), || async {
        fixture_box_empty(app, id) && screen(app, id).contains("120×40")
    })
    .await?;
    Ok(terminal)
}
