//! SPEC-652 MEM-001, MEM-002: the plan of every contained task run guards its harness's memory and
//! user-level instruction files, and a Claude Code run's settings name the guarded memory folder.
//! Runs everywhere: the session host records the plans and starts nothing; the namespace itself is
//! proven by the opt-in isolation test and on a guest (SPEC-652 Evidence).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::supervision::{runtime, seed_records, seed_repo, wait_for};
use crate::support;
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use yulius::wiring::clock::SystemClock;
use yulius::wiring::harness::HarnessAdapter;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{TaskMode, TaskOrigin};
use yulius_app::ports::session::{Containment, LaunchPlan};
use yulius_contracts::ptyhost::{GuardKind, GuardPath};
use yulius_domain::harness::Harness;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

/// The plans a contained server hands its session host, which starts nothing.
#[derive(Default)]
struct RecordingHost {
    plans: std::sync::Mutex<Vec<LaunchPlan>>,
}

#[async_trait::async_trait]
impl yulius_app::ports::session::SessionHost for RecordingHost {
    async fn spawn(
        &self,
        _run_dir: &Path,
        _attempt_id: &str,
        _task_id: &str,
        plan: &LaunchPlan,
        _token: &[u8; 32],
        _size: (u16, u16),
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::SpawnedRun> {
        if let Ok(mut plans) = self.plans.lock() {
            plans.push(plan.clone());
        }
        Err(yulius_app::error::AppError::Upstream("recorded".into()))
    }
    async fn attach(
        &self,
        _run_dir: &Path,
        _token: &[u8; 32],
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::AttachedRun> {
        Err(yulius_app::error::AppError::NotFound)
    }
    fn inspect(&self, _run_dir: &Path) -> yulius_app::ports::session::RunInspection {
        yulius_app::ports::session::RunInspection::Missing
    }
    fn terminate(&self, _run_dir: &Path) -> yulius_app::error::AppResult<()> {
        Ok(())
    }
}

/// The recorded plan of a new contained task of `harness` in a fresh sandbox.
fn contained_plan(harness: Harness) -> Result<(PathBuf, LaunchPlan), Box<dyn std::error::Error>> {
    let dir = tempfile::Builder::new().prefix("ym").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    let plan = rt.block_on(async {
        let host = Arc::new(RecordingHost::default());
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let mut config = AppConfig::new(
            data.clone(),
            "https://yulius.test",
            "http://127.0.0.1:9",
            exe.clone(),
        );
        config.contained = true;
        let provided = |name: &str| {
            let dir = data.join(format!("{name}-cli"));
            yulius_app::ports::harness::ProvidedCli {
                program: dir.join(name),
                dir,
            }
        };
        let deps = Deps {
            store: Arc::new(SqliteStore::open(&data.join("yulius.db"))?),
            clock: Arc::new(SystemClock),
            crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
            github: Arc::new(support::FakeGitHub::default()),
            session_host: host.clone(),
            harness: Arc::new(support::agents::FakeAgentHarness {
                inner: HarnessAdapter::new(exe.clone(), "http://127.0.0.1:9"),
                fake: exe.clone(),
                options: FakeAgentOptions {
                    provided_codex: Some(provided("codex")),
                    provided_antigravity: Some(provided("antigravity")),
                    ..FakeAgentOptions::default()
                },
                yulius_exe: exe,
            }),
            git: Arc::new(crate::support::confined_git()),
            system: Arc::new(support::FixedSystem),
            browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
            usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
        };
        let app = App::new(config, deps);
        let (sandbox, user) = seed_records(&app).await?;
        match harness {
            Harness::Codex => {
                crate::codex::add_codex_account(
                    &app,
                    "a-codex",
                    r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"h.e30.s","access_token":"SYNTHETIC-access","refresh_token":"SYNTHETIC-refresh","account_id":"acc-synthetic"},"last_refresh":"2026-11-01T10:00:00Z"}"#,
                )
                .await?;
            }
            Harness::Antigravity => {
                crate::antigravity::add_google_account(
                    &app,
                    "a-google",
                    &crate::antigravity::api_key()?,
                )
                .await?;
            }
            Harness::ClaudeCode => {}
        }
        seed_repo(&data, &sandbox)?;
        app.create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: "remember nothing".into(),
            harness,
            title: Some("guarded".into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?;
        wait_for("the recorded plan", Duration::from_secs(30), || async {
            host.plans.lock().is_ok_and(|plans| !plans.is_empty())
        })
        .await?;
        let plan = host
            .plans
            .lock()
            .map_err(|_| "poisoned")?
            .first()
            .cloned()
            .ok_or("no plan")?;
        Ok::<_, Box<dyn std::error::Error>>(plan)
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    // The folder stays for the assertions; the guard keeps it alive until the test ends.
    std::mem::forget(dir);
    Ok((data, plan))
}

fn guard_of(plan: &LaunchPlan) -> Result<(PathBuf, PathBuf, Vec<GuardPath>), String> {
    let Containment::Bubblewrap {
        sandbox_root,
        home,
        guard,
        ..
    } = &plan.containment
    else {
        return Err("not contained".into());
    };
    Ok((sandbox_root.clone(), home.clone(), guard.clone()))
}

fn shown(guard: &[GuardPath]) -> Vec<(String, GuardKind)> {
    guard.iter().map(|p| (p.dest.clone(), p.kind)).collect()
}

/// A Claude Code task run guards its memory folder (keyed by the sandbox's repository, made
/// before the run), `CLAUDE.md` and `rules`, pins the memory folder's parents, and its run
/// settings name that folder as `autoMemoryDirectory`.
#[test]
fn a_contained_claude_run_guards_its_memory() -> TestResult {
    let (_data, plan) = contained_plan(Harness::ClaudeCode)?;
    let (root, home, guard) = guard_of(&plan)?;
    let config = home.join(".claude");
    let key = yulius_app::memory_guard::claude_key(&root.join("repo"));
    let memory = config.join("projects").join(&key).join("memory");
    let at = |p: &Path| p.to_string_lossy().into_owned();
    assert_eq!(
        shown(&guard),
        [
            (at(&config), GuardKind::Pin),
            (at(&config.join("projects")), GuardKind::Pin),
            (at(&config.join("projects").join(&key)), GuardKind::Pin),
            (at(&memory), GuardKind::Dir),
            (at(&config.join("CLAUDE.md")), GuardKind::File),
            (at(&config.join("rules")), GuardKind::Dir),
        ]
    );
    assert!(guard.iter().all(|p| p.anchor == at(&root)));
    assert!(memory.is_dir(), "made before the run");
    let settings = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("claude-settings.json"))
        .ok_or("run settings")?;
    let settings: serde_json::Value = serde_json::from_slice(&settings.contents)?;
    assert_eq!(settings["autoMemoryDirectory"], at(&memory));
    Ok(())
}

/// A Codex task run guards `AGENTS.md`, `AGENTS.override.md` and `memories` in its Codex home.
#[test]
fn a_contained_codex_run_guards_its_instructions() -> TestResult {
    let (_data, plan) = contained_plan(Harness::Codex)?;
    let (root, _home, guard) = guard_of(&plan)?;
    let codex_home = plan
        .env
        .iter()
        .find(|(k, _)| k == "CODEX_HOME")
        .map(|(_, v)| PathBuf::from(v))
        .ok_or("CODEX_HOME")?;
    assert!(codex_home.starts_with(&root));
    let at = |name: &str| codex_home.join(name).to_string_lossy().into_owned();
    assert_eq!(
        shown(&guard),
        [
            (at("AGENTS.md"), GuardKind::File),
            (at("AGENTS.override.md"), GuardKind::File),
            (at("memories"), GuardKind::Dir),
        ]
    );
    Ok(())
}

/// An Antigravity task run guards its rules, opened from the run's own home and bound in the
/// sandbox home's `.gemini`.
#[test]
fn a_contained_antigravity_run_guards_its_rules() -> TestResult {
    let (data, plan) = contained_plan(Harness::Antigravity)?;
    let (_root, home, guard) = guard_of(&plan)?;
    let gemini = home.join(".gemini");
    let at = |rel: &str| gemini.join(rel).to_string_lossy().into_owned();
    assert_eq!(
        shown(&guard),
        [
            (at("GEMINI.md"), GuardKind::File),
            (at("config/GEMINI.md"), GuardKind::File),
            (at("config/AGENTS.md"), GuardKind::File),
            (at("config/rules"), GuardKind::Dir),
        ]
    );
    for path in &guard {
        assert!(
            Path::new(&path.anchor).starts_with(data.join("runs")),
            "{path:?}"
        );
        assert!(path.anchor.ends_with("antigravity-home"), "{path:?}");
    }
    Ok(())
}
