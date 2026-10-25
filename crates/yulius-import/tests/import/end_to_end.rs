//! `run_import` (`yulius onboard`) end to end against a temporary repository, synthetic session
//! stores and the fake server: discovery and choice (all, recent, pick), redaction shown and
//! applied before upload, idempotent re-runs that send only new or changed sessions (IMP-006,
//! AC-IMP-6), an interrupted run resumed (AC-IMP-1), dry run, status, declined confirmation and
//! refusals (AC-IMP-2, AC-IMP-7..9).

use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use flate2::read::GzDecoder;
use serde_json::json;
use yulius_contracts::import::ImportHarness;
use yulius_import::{
    ApiError, ImportError, ImportIo, ImportOptions, PlainUi, RepoError, RetryPolicy, Secret,
    SessionChoice, SessionSources, Ui, UploadError, UploadTuning, Url, run_import,
};

use crate::support::server::{FakeServer, TOKEN};
use crate::support::{
    RECENT, RECENT_LATER, RepoFixture, TestResult, claude_line, git, simple_claude_session, since,
    write, write_claude_session,
};

const CHUNK: u64 = 512;

struct TestIo {
    ui: PlainUi<Cursor<Vec<u8>>, Vec<u8>>,
    interactive: bool,
    token_calls: usize,
    sources: SessionSources,
}

impl TestIo {
    fn new(input: &str, sources: SessionSources) -> Self {
        Self {
            ui: PlainUi::new(Cursor::new(input.as_bytes().to_vec()), Vec::new()),
            interactive: true,
            token_calls: 0,
            sources,
        }
    }

    fn unattended(sources: SessionSources) -> Self {
        Self {
            interactive: false,
            ..Self::new("", sources)
        }
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(self.ui.output()).into_owned()
    }
}

impl ImportIo for TestIo {
    fn ui(&mut self) -> &mut dyn Ui {
        &mut self.ui
    }

    fn interactive(&self) -> bool {
        self.interactive
    }

    fn token(&mut self, _server: &Url) -> Result<Secret, ImportError> {
        self.token_calls += 1;
        Ok(Secret::new(TOKEN))
    }

    fn session_sources(&self) -> SessionSources {
        self.sources.clone()
    }

    fn now(&self) -> SystemTime {
        since() + Duration::from_secs(14 * 24 * 3600)
    }

    fn upload_tuning(&self) -> UploadTuning {
        UploadTuning {
            chunk_size: CHUNK,
            retry: RetryPolicy::immediate(2),
        }
    }
}

struct Scenario {
    fixture: RepoFixture,
    sources: SessionSources,
    claude: PathBuf,
    checkout_transcript: PathBuf,
}

/// A synthetic GitHub token built at run time (no literal credential in the source).
fn fake_token() -> String {
    format!("ghp_{}", "Wv4Hn".repeat(8))
}

/// A repository with an unpublished branch and an uncommitted edit; one Claude session at the
/// root (2026-10-01..02) and another in a subdirectory (2026-10-02).
fn scenario() -> TestResult<Scenario> {
    let fixture = RepoFixture::new()?;
    git(&fixture.repo, &["checkout", "-q", "-b", "feature"])?;
    // Poorly compressible content so the bundle spans many chunks.
    let mut seed = 0x2545_f491_u64;
    let noise: Vec<u8> = (0..6000)
        .map(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 33) as u8
        })
        .collect();
    fixture.commit_file("feature.txt", &hex::encode(noise), "local feature")?;
    git(&fixture.repo, &["checkout", "-q", "main"])?;
    write(
        &fixture.repo.join("README.md"),
        "# demo, uncommitted edit\n",
    )?;
    let root = fixture.root()?;
    let claude = fixture.temp.path().join("claude");
    let checkout_transcript = simple_claude_session(
        &claude,
        &root,
        "aaaaaaaa-claude",
        &format!("Implement checkout {}", "details ".repeat(150)),
        RECENT,
        RECENT_LATER,
    )?;
    simple_claude_session(
        &claude,
        &root.join("dir"),
        "bbbbbbbb-claude",
        "Speed up the build",
        RECENT_LATER,
        RECENT_LATER,
    )?;
    Ok(Scenario {
        sources: SessionSources {
            claude_dir: Some(claude.clone()),
        },
        fixture,
        claude,
        checkout_transcript,
    })
}

fn options(
    scenario: &Scenario,
    server: &FakeServer,
    sessions: Option<SessionChoice>,
) -> ImportOptions {
    ImportOptions {
        sessions,
        assume_yes: true,
        ..ImportOptions::new(scenario.fixture.repo.clone(), server.url.clone())
    }
}

#[test]
fn onboards_everything_then_sends_nothing_new_on_rerun() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    server.with(|state| state.sandbox_exists = false)?;
    let mut io = TestIo::unattended(scenario.sources.clone());

    let outcome = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut io,
    )?;

    let result = outcome.result.clone().ok_or("applied")?;
    assert_eq!(result.sessions_imported, 2);
    assert_eq!(outcome.sessions_found, 2);
    assert_eq!(outcome.sessions_sent, 2);
    let output = io.output();
    for expected in [
        "[1/4] Repository",
        "GitHub       Quartio-Studio/demo",
        "Unpublished  1 branch: feature",
        "[2/4] Yulius",
        "Signed in    127.0.0.1 as @alice",
        "new: Quartio-Studio/demo is added when you upload",
        "[3/4] Claude Code sessions",
        "Found        2 sessions",
        "Secrets      none found",
        "[4/4] Upload",
        "Will send    2 sessions, unpublished branches, uncommitted changes",
        "✓ Quartio-Studio/demo is on Yulius",
        "2 new · 0 updated · 0 unchanged, attributed to @alice",
        "Sessions     https://yulius.example/s/demo/sessions",
        "Onboarding   ● waiting to start: Start onboarding on the sandbox page, or \
         `yulius project onboard` with an operate token",
        // SPEC-313 NBW-001/NBW-002 (#790): both ways to start it, the operate token's too.
        "· someone starts onboarding: their agent runs `yulius project onboard` \
         (`onboarding_start`) with an operate token, or they click Start onboarding on the \
         sandbox page: https://yulius.example/s/demo",
        "New tasks wait for it and start by themselves once onboarding is ready.",
        // SPEC-141 R5: what people must fix is said too.
        "The production branch `main` is not protected from agents: add a ruleset.",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
    assert_eq!(io.token_calls, 1);

    let (manifest, puts_after_first) =
        server.with(|state| (state.manifests[0].clone(), state.puts.len()))?;
    assert_eq!(manifest.repo.owner, "Quartio-Studio");
    assert_eq!(manifest.repo.head_branch.as_deref(), Some("main"));
    assert!(manifest.bundle.is_some());
    let harnesses: Vec<ImportHarness> = manifest.sessions.iter().map(|s| s.harness).collect();
    assert_eq!(harnesses, [ImportHarness::Claude, ImportHarness::Claude]);
    let build = manifest
        .sessions
        .iter()
        .find(|s| s.title == "Speed up the build")
        .ok_or("build session")?;
    assert_eq!(build.cwd_relative.as_deref(), Some("dir"));
    let Some(snapshot) = manifest.snapshot.clone() else {
        return Err("expected a snapshot".into());
    };
    server.with(|state| -> TestResult {
        assert_eq!(state.stored_bytes, manifest.total_bytes());
        // A clean transcript is uploaded byte for byte.
        let checkout = manifest
            .sessions
            .iter()
            .find(|s| s.session_id == "aaaaaaaa-claude")
            .ok_or("checkout")?;
        assert_eq!(
            state.content.get(&checkout.files[0].sha256),
            Some(&fs::read(&scenario.checkout_transcript)?)
        );
        let archive_bytes = state
            .content
            .get(&snapshot.sha256)
            .cloned()
            .unwrap_or_default();
        let mut archive = tar::Archive::new(GzDecoder::new(&archive_bytes[..]));
        let mut readme = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path()?.to_string_lossy() == "files/README.md" {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                readme = Some(text);
            }
        }
        assert_eq!(readme.as_deref(), Some("# demo, uncommitted edit\n"));
        Ok(())
    })??;

    // Same content again: the sessions are already there, nothing is uploaded (IMP-006).
    let mut rerun = TestIo::unattended(scenario.sources.clone());
    let again = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut rerun,
    )?;
    assert_eq!(again.sessions_sent, 0);
    let output = rerun.output();
    assert!(
        output.contains("On Yulius    0 new · 0 changed · 2 already there"),
        "{output}"
    );
    assert!(output.contains("2 files already on the server"), "{output}");
    assert_eq!(server.with(|state| state.puts.len())?, puts_after_first);
    let second = server.with(|state| state.manifests[1].clone())?;
    assert!(
        second.sessions.is_empty(),
        "unchanged sessions are not offered"
    );

    // Only local sessions: with --sessions-only and nothing new there is nothing to send.
    let mut quiet = TestIo::unattended(scenario.sources.clone());
    let none = run_import(
        &ImportOptions {
            sessions_only: true,
            ..options(&scenario, &server, Some(SessionChoice::All))
        },
        &mut quiet,
    )?;
    assert!(none.result.is_none());
    assert!(
        quiet
            .output()
            .contains("nothing new: everything is already on Yulius"),
        "{}",
        quiet.output()
    );
    assert_eq!(server.with(|state| state.manifests.len())?, 2);
    Ok(())
}

#[test]
fn secrets_are_redacted_before_upload_and_listed() -> TestResult {
    let scenario = scenario()?;
    let root = scenario.fixture.root()?;
    write_claude_session(
        &scenario.claude,
        &root,
        "cccccccc-claude",
        &[
            claude_line(
                "user",
                &root,
                RECENT_LATER,
                json!(format!("Deploy with {} please", fake_token())),
            ),
            claude_line(
                "assistant",
                &root,
                RECENT_LATER,
                json!([{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": format!("{}/.env", root.display())}}]),
            ),
            claude_line(
                "user",
                &root,
                RECENT_LATER,
                json!([{"type": "tool_result", "tool_use_id": "t1", "content": "     1→DATABASE_URL=postgres://app:pw@db/app\n     2→PORT=3000\n"}]),
            ),
        ],
    )?;
    let server = FakeServer::start()?;
    let mut io = TestIo::unattended(scenario.sources.clone());
    let outcome = run_import(
        &ImportOptions {
            sessions_only: true,
            ..options(&scenario, &server, Some(SessionChoice::All))
        },
        &mut io,
    )?;
    assert_eq!(outcome.redactions, 2);
    let output = io.output();
    assert!(
        output.contains("Redacted     2 secrets in 1 session"),
        "{output}"
    );
    assert!(output.contains("github-token ×1: ghp_"), "{output}");
    assert!(output.contains("env-value ×1: DATABASE_URL"), "{output}");
    assert!(
        !output.contains(&fake_token()),
        "the secret is never printed"
    );
    let manifest = server.with(|state| state.manifests[0].clone())?;
    assert!(manifest.bundle.is_none() && manifest.snapshot.is_none());
    let secret = manifest
        .sessions
        .iter()
        .find(|s| s.session_id == "cccccccc-claude")
        .ok_or("session")?;
    assert_eq!(secret.redactions, 2);
    let uploaded = server.with(|state| state.content.get(&secret.files[0].sha256).cloned())?;
    let uploaded = String::from_utf8(uploaded.ok_or("uploaded")?)?;
    assert!(!uploaded.contains(&fake_token()));
    assert!(!uploaded.contains("postgres://app:pw"));
    assert!(uploaded.contains("PORT=3000"));
    for line in uploaded.lines() {
        let _: serde_json::Value = serde_json::from_str(line)?;
    }

    // The same session redacts to the same bytes: a re-run finds it unchanged.
    let mut rerun = TestIo::unattended(scenario.sources.clone());
    let again = run_import(
        &ImportOptions {
            sessions_only: true,
            ..options(&scenario, &server, Some(SessionChoice::All))
        },
        &mut rerun,
    )?;
    assert_eq!(again.sessions_sent, 0);

    // A session that grew is sent again (changed), alone.
    let transcript = scenario
        .claude
        .join("projects")
        .read_dir()?
        .flatten()
        .map(|dir| dir.path().join("bbbbbbbb-claude.jsonl"))
        .find(|path| path.exists())
        .ok_or("build transcript")?;
    let mut text = fs::read_to_string(&transcript)?;
    text.push_str(&claude_line(
        "user",
        &root,
        RECENT_LATER,
        json!("one more thing"),
    ));
    text.push('\n');
    fs::write(&transcript, text)?;
    let mut grown = TestIo::unattended(scenario.sources.clone());
    let grown_outcome = run_import(
        &ImportOptions {
            sessions_only: true,
            ..options(&scenario, &server, Some(SessionChoice::All))
        },
        &mut grown,
    )?;
    assert_eq!(grown_outcome.sessions_sent, 1);
    assert_eq!(
        grown_outcome.result.map(|r| r.sessions_updated),
        Some(1),
        "{}",
        grown.output()
    );
    assert!(
        grown
            .output()
            .contains("0 new · 1 changed · 2 already there")
    );
    Ok(())
}

#[test]
fn people_choose_recent_sessions_or_pick_them() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    server.with(|state| state.sandbox_exists = true)?;
    // Recent with a window asked interactively: "1d" before now (2026-10-05T14:13:20Z) keeps
    // nothing; the default window would keep both.
    let mut recent = TestIo::new("1\n1d\n\n", scenario.sources.clone());
    let none = run_import(
        &ImportOptions {
            assume_yes: false,
            sessions_only: true,
            ..ImportOptions::new(scenario.fixture.repo.clone(), server.url.clone())
        },
        &mut recent,
    );
    let output = recent.output();
    assert!(
        output.contains("Which Claude Code sessions should go to Yulius?"),
        "{output}"
    );
    assert!(
        output.contains("Recent: the last 14d (2 sessions)"),
        "{output}"
    );
    assert!(output.contains("Chosen       0 sessions"), "{output}");
    assert!(none.is_ok(), "{none:?}");
    assert_eq!(
        server.with(|state| state.manifests.len())?,
        0,
        "nothing to send"
    );

    // Pick: the list is newest first; "2" is the older session.
    let mut pick = TestIo::new("3\n2\ny\n", scenario.sources.clone());
    let picked = run_import(
        &ImportOptions {
            assume_yes: false,
            sessions_only: true,
            ..ImportOptions::new(scenario.fixture.repo.clone(), server.url.clone())
        },
        &mut pick,
    )?;
    let output = pick.output();
    assert!(
        output.contains("[x]   1. 2026-10-02  Speed up the build"),
        "{output}"
    );
    assert!(
        output.contains("Upload 1 session to 127.0.0.1?"),
        "{output}"
    );
    assert_eq!(picked.sessions_sent, 1);
    let manifest = server.with(|state| state.manifests[0].clone())?;
    let ids: Vec<&str> = manifest
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["aaaaaaaa-claude"]);
    Ok(())
}

#[test]
fn interrupted_upload_resumes_on_the_next_run() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    server.with(|state| state.down_after_chunks = Some(3))?;

    let mut io = TestIo::unattended(scenario.sources.clone());
    let first = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut io,
    );
    assert!(
        matches!(
            first,
            Err(ImportError::Upload(UploadError::Api(
                ApiError::Unavailable { .. }
            )))
        ),
        "{first:?}"
    );

    server.with(|state| {
        state.down = false;
        state.down_after_chunks = None;
    })?;
    let mut rerun = TestIo::unattended(scenario.sources.clone());
    let outcome = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut rerun,
    )?;
    assert_eq!(outcome.result.map(|r| r.sessions_imported), Some(2));
    assert!(
        rerun.output().contains("resuming after"),
        "{}",
        rerun.output()
    );

    server.with(|state| {
        let manifest = &state.manifests[1];
        let expected_chunks: u64 = manifest.files().map(|file| file.size.div_ceil(CHUNK)).sum();
        let accepted: Vec<(String, u64)> = state
            .puts
            .iter()
            .filter(|put| put.status == 200)
            .map(|put| (put.name.clone(), put.offset))
            .collect();
        let unique: BTreeSet<&(String, u64)> = accepted.iter().collect();
        assert_eq!(unique.len(), accepted.len(), "a chunk was sent twice");
        assert_eq!(accepted.len() as u64, expected_chunks);
        assert_eq!(state.stored_bytes, manifest.total_bytes());
    })?;
    Ok(())
}

#[test]
fn dry_runs_status_and_declined_confirmations_upload_nothing() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    server.with(|state| state.sandbox_exists = true)?;

    let mut dry = TestIo::unattended(scenario.sources.clone());
    let planned = run_import(
        &ImportOptions {
            dry_run: true,
            ..options(&scenario, &server, Some(SessionChoice::All))
        },
        &mut dry,
    )?;
    assert!(planned.result.is_none());
    assert!(dry.output().contains("Dry run: nothing was uploaded."));

    let mut status = TestIo::unattended(scenario.sources.clone());
    run_import(
        &ImportOptions {
            status_only: true,
            ..options(&scenario, &server, None)
        },
        &mut status,
    )?;
    let output = status.output();
    assert!(output.contains("[2/2] Yulius"), "{output}");
    assert!(
        output.contains(
            "Onboarding   ● waiting to start: Start onboarding on the sandbox page, or \
             `yulius project onboard` with an operate token"
        ),
        "{output}"
    );
    assert!(output.contains("Not ready yet: tasks start once onboarding is done."));
    assert!(
        !output.contains("Claude Code sessions"),
        "status reads no sessions"
    );

    let mut declined = TestIo::new("2\nn\n", scenario.sources.clone());
    let result = run_import(
        &ImportOptions {
            assume_yes: false,
            ..ImportOptions::new(scenario.fixture.repo.clone(), server.url.clone())
        },
        &mut declined,
    );
    assert!(matches!(result, Err(ImportError::Cancelled)), "{result:?}");
    assert_eq!(server.with(|state| state.manifests.len())?, 0);

    // A refusal from the server stops before anything is read or sent.
    server.with(|state| {
        state.refusal = Some("uploading needs write access to Quartio-Studio/demo".into());
    })?;
    let mut refused = TestIo::unattended(scenario.sources.clone());
    let result = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut refused,
    );
    assert!(
        matches!(&result, Err(ImportError::Refused(message)) if message.contains("write access")),
        "{result:?}"
    );
    assert_eq!(server.with(|state| state.manifests.len())?, 0);
    Ok(())
}

#[test]
fn non_github_repositories_are_refused_before_authentication() -> TestResult {
    let scenario = scenario()?;
    git(
        &scenario.fixture.repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://gitlab.com/team/demo.git",
        ],
    )?;
    let server = FakeServer::start()?;
    let mut io = TestIo::unattended(scenario.sources.clone());
    let result = run_import(
        &options(&scenario, &server, Some(SessionChoice::All)),
        &mut io,
    );
    assert!(
        matches!(result, Err(ImportError::Repo(RepoError::NotGitHub { .. }))),
        "{result:?}"
    );
    assert_eq!(io.token_calls, 0);
    assert_eq!(server.with(|state| state.manifests.len())?, 0);
    Ok(())
}

/// SPEC-171 PGL-001, PGL-002: `--playground` on a checkout without any remote: the plan and the
/// manifest say playground with an empty owner, the bundle holds every branch, the uncommitted
/// work and the sessions are sent, and the output names the playground, not GitHub.
#[test]
fn a_checkout_without_github_goes_to_a_playground() -> TestResult {
    let scenario = scenario()?;
    git(&scenario.fixture.repo, &["remote", "remove", "origin"])?;
    let server = FakeServer::start()?;
    server.with(|state| state.sandbox_exists = false)?;
    let mut io = TestIo::unattended(scenario.sources.clone());
    let options = ImportOptions {
        playground: true,
        playground_name: Some("shop".to_owned()),
        ..options(&scenario, &server, Some(SessionChoice::All))
    };

    let outcome = run_import(&options, &mut io)?;

    assert!(outcome.result.is_some());
    assert_eq!(outcome.sessions_sent, 2);
    let output = io.output();
    for expected in [
        "Playground   shop (local Git on the Yulius server, nothing pushed)",
        "Branches     2 branches: feature, main",
        "new playground shop: it is made when you upload",
        "✓ shop is on Yulius",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
    assert!(!output.contains("GitHub"), "{output}");
    let (plans, manifest) =
        server.with(|state| (state.plan_requests.clone(), state.manifests[0].clone()))?;
    assert!(
        plans
            .iter()
            .all(|plan| plan.playground && plan.owner.is_empty() && plan.name == "shop")
    );
    assert!(manifest.playground);
    assert_eq!(manifest.repo.owner, "");
    assert_eq!(manifest.repo.remote_url, "");
    assert_eq!(manifest.repo.name, "shop");
    assert_eq!(manifest.repo.head_branch.as_deref(), Some("main"));
    assert!(manifest.snapshot.is_some());
    let bundle = manifest.bundle.clone().ok_or("bundle")?;
    let bytes = server
        .with(|state| state.content.get(&bundle.sha256).cloned())?
        .ok_or("stored")?;
    let path = scenario.fixture.temp.path().join("received.bundle");
    fs::write(&path, bytes)?;
    let heads = git(
        &scenario.fixture.repo,
        &["bundle", "list-heads", &path.to_string_lossy()],
    )?;
    assert!(
        heads.contains("refs/heads/main") && heads.contains("refs/heads/feature"),
        "{heads}"
    );

    // Without --playground the same checkout is refused: it has no GitHub remote.
    let refused = run_import(
        &options_default(&scenario, &server),
        &mut TestIo::unattended(scenario.sources.clone()),
    );
    assert!(
        matches!(refused, Err(ImportError::Repo(RepoError::NoRemote))),
        "{refused:?}"
    );
    Ok(())
}

fn options_default(scenario: &Scenario, server: &FakeServer) -> ImportOptions {
    options(scenario, server, Some(SessionChoice::None))
}
