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
    // Its hooks learn the read-only paths, to say why a write there fails (MEM-006).
    assert_eq!(
        readonly_paths(&plan),
        [
            at(&memory),
            at(&config.join("CLAUDE.md")),
            at(&config.join("rules"))
        ]
    );
    Ok(())
}

/// The run's `YULIUS_READONLY_PATHS`, one path per line.
fn readonly_paths(plan: &LaunchPlan) -> Vec<String> {
    plan.env
        .iter()
        .find(|(k, _)| k == "YULIUS_READONLY_PATHS")
        .map(|(_, v)| v.lines().map(str::to_owned).collect())
        .unwrap_or_default()
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
    assert_eq!(
        readonly_paths(&plan),
        [at("AGENTS.md"), at("AGENTS.override.md"), at("memories")]
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

/// Whether bubblewrap can make namespaces here (not in a task's sandbox, nor where unprivileged
/// user namespaces are off); `Err` is why not.
fn bwrap_usable() -> Result<(), String> {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
        ])
        .args(["--", "/bin/true"])
        .output()
        .map_err(|e| format!("bwrap not runnable: {e}"))?;
    if probe.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&probe.stderr).trim().to_owned())
    }
}

/// What a refused attempt's error says: a read-only mount, a mount point, a cross-mount link, or
/// a mount call refused in a nested namespace (`umount` words `EINVAL` on a locked mount as "not
/// mounted"; a guard that is missing shows as writes that went through).
const REFUSALS: [&str; 7] = [
    "Read-only file system",
    "Device or resource busy",
    "Operation not permitted",
    "Invalid cross-device link",
    "ermission denied",
    "Invalid argument",
    "not mounted",
];

/// The shell script a guarded run executes: every attempt prints `<name>=wrote` or
/// `<name>=refused:<its error, on one line>` into `$RESULTS`, each read `<name>=<what it read>`, and
/// a missing tool `tools=missing <tool>`.
const ATTEMPTS: &str = r#"
r="$RESULTS"
for tool in python3 unshare mount umount ln; do command -v "$tool" >/dev/null || echo "tools=missing $tool" >>"$r"; done
try() { name=$1; shift; if out=$("$@" 2>&1 >/dev/null); then echo "$name=wrote" >>"$r"; else echo "$name=refused:$(printf '%s' "$out" | tr '\n' ' ')" >>"$r"; fi; }
py() { try "$1" python3 -c "$2" "$3"; }
for f in "$CLAUDE_MEMORY/MEMORY.md" "$CLAUDE_CONFIG/CLAUDE.md" "$CODEX_HOME/AGENTS.md" "$GEMINI/config/AGENTS.md"; do
  n=$(echo "$f" | tr -c 'a-zA-Z0-9\n' _)
  try "append$n" sh -c 'echo note >>"$1"' _ "$f"
  try "replace$n" sh -c 'printf note >"$1.new" && mv -f "$1.new" "$1"' _ "$f"
  try "truncate$n" sh -c ': >"$1"' _ "$f"
  try "delete$n" rm -f "$f"
  try "link$n" ln -f "$f" "$HOME/hardlink"
  py "pyappend$n" 'import sys; open(sys.argv[1], "a").write("note")' "$f"
  py "pyunlink$n" 'import os, sys; os.unlink(sys.argv[1])' "$f"
done
for d in "$CLAUDE_MEMORY" "$CLAUDE_CONFIG/rules" "$CODEX_HOME/memories" "$GEMINI/config/rules"; do
  n=$(echo "$d" | tr -c 'a-zA-Z0-9\n' _)
  try "create$n" sh -c 'echo note >"$1/new.md"' _ "$d"
  try "mkdir$n" mkdir "$d/sub"
  py "pycreate$n" 'import sys; open(sys.argv[1] + "/py.md", "w").write("note")' "$d"
  try "rmdir$n" rm -rf "$d"
done
try rename_memory mv "$CLAUDE_MEMORY" "$CLAUDE_MEMORY.away"
try rename_key mv "$(dirname "$CLAUDE_MEMORY")" "$(dirname "$CLAUDE_MEMORY").away"
try rename_projects mv "$CLAUDE_CONFIG/projects" "$CLAUDE_CONFIG/projects.away"
try rename_config mv "$CLAUDE_CONFIG" "$CLAUDE_CONFIG.away"
try remount_rw unshare -Urm sh -c 'mount -o remount,bind,rw "$1" && echo note >>"$1/MEMORY.md"' _ "$CLAUDE_MEMORY"
try umount_memory unshare -Urm sh -c 'umount "$1" && echo note >>"$1/MEMORY.md"' _ "$CLAUDE_MEMORY"
echo "read_memory=$(cat "$CLAUDE_MEMORY/MEMORY.md")" >>"$r"
echo "read_claude=$(cat "$CLAUDE_CONFIG/CLAUDE.md")" >>"$r"
echo "read_codex=$(cat "$CODEX_HOME/AGENTS.md")" >>"$r"
echo "read_agy=$(cat "$GEMINI/config/AGENTS.md")" >>"$r"
echo "read_link=$(cat "$CODEX_HOME/AGENTS.override.md")" >>"$r"
echo "read_rules=$(ls -A "$CLAUDE_CONFIG/rules")" >>"$r"
echo "fds=$(for fd in /proc/self/fd/*; do readlink "$fd"; done | grep -v -e '^/dev/' -e '^pipe:' -e "^/proc/" | tr '\n' ' ')" >>"$r"
try worktree_still_writable sh -c 'echo ok >"$1/work.txt"' _ "$PWD"
echo done >>"$r"
"#;

/// SPEC-652 AC-2, AC-3, AC-6 in a real namespace, the whole path a session takes (`yulius
/// ptyhost` → `yulius sandbox-exec --guard` → bwrap): with the layouts of a Claude Code, a Codex
/// and an Antigravity session at once, every append, replace, truncate, create, hard link and
/// delete of a memory or instruction file by the shell and by Python fails, so do renaming the
/// memory folder or its parents and remounting it; the notes still read; a link shows an empty
/// placeholder, never its target; no descriptor of the guard reaches the run; the worktree stays
/// writable; and the sandbox with its notes is removed afterwards. Skipped where bubblewrap cannot
/// make namespaces (a task's sandbox); run on a guest for SPEC-652's Evidence.
#[tokio::test]
async fn a_guarded_session_cannot_write_its_memory_but_reads_it() -> TestResult {
    use yulius_app::ports::session::{Bind, SessionHost as _};
    if let Err(reason) = bwrap_usable() {
        eprintln!("SKIPPED a_guarded_session_cannot_write_its_memory_but_reads_it: {reason}");
        return Ok(());
    }
    let dir = tempfile::Builder::new().prefix("yg").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let root = data.join("sandboxes/s1");
    let home = root.join("home");
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git"))?;
    let config = home.join(".claude");
    let key = yulius_app::memory_guard::claude_key(&repo);
    let memory = config.join("projects").join(&key).join("memory");
    std::fs::create_dir_all(&memory)?;
    std::fs::write(memory.join("MEMORY.md"), "existing note")?;
    std::fs::write(config.join("CLAUDE.md"), "user rule")?;
    let codex = root.join("codex/a1");
    std::fs::create_dir_all(&codex)?;
    std::fs::write(codex.join("AGENTS.md"), "codex rule")?;
    let secret = data.join("secret.txt");
    std::fs::write(&secret, "HOST SECRET")?;
    std::os::unix::fs::symlink(&secret, codex.join("AGENTS.override.md"))?;
    std::os::unix::fs::symlink(data.join("runs"), config.join("rules"))?;
    let run_home = data.join("runs/r1/antigravity-home");
    std::fs::create_dir_all(run_home.join(".gemini/config"))?;
    std::fs::write(run_home.join(".gemini/config/AGENTS.md"), "agy rule")?;
    let mut guard = yulius_app::memory_guard::claude_guard(&root, &config, &config, &repo)?;
    guard.extend(yulius_app::memory_guard::codex_guard(&root, &codex)?);
    guard.extend(yulius_app::memory_guard::antigravity_guard(
        &run_home,
        &home.join(".gemini"),
    )?);
    let results = root.join("results.txt");
    let script = data.join("attempts.sh");
    std::fs::write(&script, ATTEMPTS)?;
    let at = |p: &Path| p.to_string_lossy().into_owned();
    let plan = LaunchPlan {
        argv: vec!["/bin/sh".into(), at(&script)],
        cwd: repo.clone(),
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), at(&home)),
            ("RESULTS".into(), at(&results)),
            ("CLAUDE_CONFIG".into(), at(&config)),
            ("CLAUDE_MEMORY".into(), at(&memory)),
            ("CODEX_HOME".into(), at(&codex)),
            ("GEMINI".into(), at(&home.join(".gemini"))),
        ],
        secret_env: Vec::new(),
        files: Vec::new(),
        containment: Containment::Bubblewrap {
            sandbox_root: root.clone(),
            home: home.clone(),
            writable: Vec::new(),
            readonly: vec![script.clone()],
            binds: vec![Bind {
                source: run_home.join(".gemini"),
                dest: home.join(".gemini"),
                writable: true,
            }],
            identity: None,
            join: None,
            guard,
        },
    };
    let host =
        yulius::wiring::session::PtySessionHost::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius")));
    host.spawn(
        &data.join("runs/run"),
        "att-guard",
        "task-guard",
        &plan,
        &[5; 32],
        (80, 24),
    )
    .await?;
    let finished = wait_for("the attempts", Duration::from_secs(60), || async {
        std::fs::read_to_string(&results).is_ok_and(|text| text.ends_with("done\n"))
    })
    .await;
    if let Err(err) = finished {
        let run = data.join("runs/run");
        let mut why = String::new();
        for name in ["exit.json", "host.log", "status.json"] {
            why.push_str(&format!(
                "{name}: {}\n",
                std::fs::read_to_string(run.join(name)).unwrap_or_default()
            ));
        }
        let partial = std::fs::read_to_string(&results).unwrap_or_default();
        return Err(format!("{err}\n{why}results so far:\n{partial}").into());
    }
    let text = std::fs::read_to_string(&results)?;
    eprintln!("{text}");
    let mut wrote = Vec::new();
    let mut unexplained = Vec::new();
    for line in text.lines() {
        let (name, value) = line.split_once('=').unwrap_or((line, ""));
        match name {
            "read_memory" => assert_eq!(value, "existing note"),
            "read_claude" => assert_eq!(value, "user rule"),
            "read_codex" => assert_eq!(value, "codex rule"),
            "read_agy" => assert_eq!(value, "agy rule"),
            "read_link" | "read_rules" => assert_eq!(value, "", "{name}: a link's target"),
            "fds" => assert_eq!(value.trim(), "", "descriptors reached the run: {value}"),
            "worktree_still_writable" => assert_eq!(value, "wrote"),
            "done" => {}
            "tools" => return Err(format!("the attempts need {value}").into()),
            _ => match value.strip_prefix("refused:") {
                // Refused by the namespace, never for another reason (a missing tool, a typo).
                Some(why) if REFUSALS.iter().any(|r| why.contains(r)) => {}
                Some(why) => unexplained.push(format!("{name}: {why}")),
                None => wrote.push(line.to_owned()),
            },
        }
    }
    assert!(
        wrote.is_empty(),
        "writes that went through: {wrote:?}\n{text}"
    );
    assert!(
        unexplained.is_empty(),
        "refused for another reason: {unexplained:?}"
    );
    assert!(text.contains("rename_config=refused"), "{text}");
    // Nothing changed on the host, and the sandbox with its notes is removed.
    assert_eq!(
        std::fs::read_to_string(memory.join("MEMORY.md"))?,
        "existing note"
    );
    assert_eq!(std::fs::read_dir(&memory)?.count(), 1);
    assert_eq!(
        std::fs::read_to_string(config.join("CLAUDE.md"))?,
        "user rule"
    );
    assert_eq!(std::fs::read_to_string(&secret)?, "HOST SECRET");
    assert!(!codex.join("memories/new.md").exists());
    std::fs::remove_dir_all(&root)?;
    assert!(!root.exists());
    Ok(())
}
