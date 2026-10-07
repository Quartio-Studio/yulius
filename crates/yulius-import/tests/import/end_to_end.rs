//! `run_import` end to end against a temporary repository, synthetic session stores and the fake
//! server: full import, idempotent re-run (IMP-006, AC-IMP-6), interrupted run resumed
//! (AC-IMP-1), confirmation declined and non-GitHub refusal (AC-IMP-2).

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, Cursor, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use flate2::read::GzDecoder;
use serde_json::json;
use yulius_contracts::import::ImportHarness;
use yulius_import::{
    ApiError, ImportError, ImportIo, ImportOptions, RepoError, RetryPolicy, Secret, SessionSources,
    UploadError, UploadTuning, Url, run_import,
};

use crate::support::server::{FakeServer, TOKEN};
use crate::support::{
    RECENT, RECENT_LATER, RepoFixture, TestResult, codex_user, git, simple_claude_session, since,
    write, write_codex_session,
};

const CHUNK: u64 = 512;

struct TestIo {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    token_calls: usize,
    sources: SessionSources,
}

impl TestIo {
    fn new(input: &str, sources: SessionSources) -> Self {
        Self {
            input: Cursor::new(input.as_bytes().to_vec()),
            output: Vec::new(),
            token_calls: 0,
            sources,
        }
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

impl ImportIo for TestIo {
    fn streams(&mut self) -> (&mut dyn BufRead, &mut dyn Write) {
        (&mut self.input, &mut self.output)
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
    claude_transcript: PathBuf,
}

/// A repository with an unpublished branch, an uncommitted edit, one Claude session at the root
/// and one Codex session in a subdirectory.
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
    let codex = fixture.temp.path().join("codex");
    let claude_transcript = simple_claude_session(
        &claude,
        &root,
        "aaaaaaaa-claude",
        &format!("Implement checkout {}", "details ".repeat(150)),
        RECENT,
        RECENT_LATER,
    )?;
    write_codex_session(
        &codex,
        "bbbbbbbb-codex",
        &root.join("dir"),
        "",
        &[
            codex_user(RECENT, "Speed up the build"),
            json!({"timestamp": RECENT_LATER, "type": "event_msg", "payload": {"type": "token_count"}}),
        ],
    )?;
    Ok(Scenario {
        fixture,
        sources: SessionSources {
            claude_dir: Some(claude),
            codex_home: Some(codex),
        },
        claude_transcript,
    })
}

fn options(
    scenario: &Scenario,
    server: &FakeServer,
    since: Option<&str>,
    assume_yes: bool,
) -> ImportOptions {
    ImportOptions {
        path: scenario.fixture.repo.clone(),
        server: server.url.clone(),
        since: since.map(str::to_owned),
        assume_yes,
    }
}

#[test]
fn imports_everything_then_uploads_nothing_on_rerun() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    let mut io = TestIo::new("", scenario.sources.clone());

    let result = run_import(&options(&scenario, &server, Some("14d"), true), &mut io)?;

    assert_eq!(result.tasks_created, 2);
    assert_eq!(result.sessions_skipped_existing, 0);
    assert_eq!(result.onboarding, "pending");
    let output = io.output();
    assert!(
        output.contains("Repository   Quartio-Studio/demo (main @"),
        "{output}"
    );
    assert!(
        output.contains("Unpublished  1 branch(es): feature"),
        "{output}"
    );
    assert!(
        output.contains("Uploading 4 of 4 files (0 already on the server)."),
        "{output}"
    );
    assert!(
        output.contains("Sandbox: https://yulius.example/s/demo"),
        "{output}"
    );
    assert_eq!(io.token_calls, 1);

    let (manifest, puts_after_first) =
        server.with(|state| (state.manifests[0].clone(), state.puts.len()))?;
    assert_eq!(manifest.repo.owner, "Quartio-Studio");
    assert_eq!(manifest.repo.name, "demo");
    assert_eq!(manifest.repo.head_branch.as_deref(), Some("main"));
    assert_eq!(manifest.since, "2026-09-21T14:13:20.000Z");
    assert!(manifest.bundle.is_some());
    let harnesses: Vec<ImportHarness> = manifest
        .sessions
        .iter()
        .map(|session| session.harness)
        .collect();
    assert_eq!(harnesses, [ImportHarness::Claude, ImportHarness::Codex]);
    assert_eq!(manifest.sessions[1].cwd_relative.as_deref(), Some("dir"));
    assert_eq!(manifest.sessions[1].title, "Speed up the build");
    let Some(snapshot) = manifest.snapshot.clone() else {
        return Err("expected a snapshot".into());
    };

    server.with(|state| -> TestResult {
        assert_eq!(state.stored_bytes, manifest.total_bytes());
        for file in manifest.files() {
            assert!(
                state.content.contains_key(&file.sha256),
                "{} not stored",
                file.name
            );
        }
        // The uploaded transcript is the local session file; the snapshot holds the edit.
        let transcript = &manifest.sessions[0].files[0];
        assert_eq!(
            state.content.get(&transcript.sha256),
            Some(&fs::read(&scenario.claude_transcript)?)
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

    // Same content again: nothing is uploaded and no task is duplicated (IMP-006).
    let mut rerun = TestIo::new("", scenario.sources.clone());
    let again = run_import(&options(&scenario, &server, Some("14d"), true), &mut rerun)?;
    assert_eq!(again.tasks_created, 0);
    assert_eq!(again.sessions_skipped_existing, 2);
    assert!(
        rerun
            .output()
            .contains("Uploading 0 of 4 files (4 already on the server).")
    );
    assert_eq!(server.with(|state| state.puts.len())?, puts_after_first);
    Ok(())
}

#[test]
fn interrupted_import_resumes_on_the_next_run() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    server.with(|state| state.down_after_chunks = Some(3))?;

    let mut io = TestIo::new("", scenario.sources.clone());
    let first = run_import(&options(&scenario, &server, Some("14d"), true), &mut io);
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
    let mut rerun = TestIo::new("", scenario.sources.clone());
    let result = run_import(&options(&scenario, &server, Some("14d"), true), &mut rerun)?;
    assert_eq!(result.tasks_created, 2);
    assert!(rerun.output().contains("resuming at"), "{}", rerun.output());

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
fn declining_the_confirmation_uploads_nothing() -> TestResult {
    let scenario = scenario()?;
    let server = FakeServer::start()?;
    let mut io = TestIo::new("7d\nn\n", scenario.sources.clone());

    let result = run_import(&options(&scenario, &server, None, false), &mut io);

    assert!(matches!(result, Err(ImportError::Cancelled)), "{result:?}");
    let output = io.output();
    assert!(output.contains("[14d]"), "{output}");
    assert!(
        output.contains("since 2026-09-28T14:13:20.000Z"),
        "{output}"
    );
    assert!(
        output.contains("[claude] 2026-10-02 Implement checkout details"),
        "{output}"
    );
    assert!(output.contains("Upload to http://127.0.0.1:"), "{output}");
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
    let mut io = TestIo::new("", scenario.sources.clone());
    let result = run_import(&options(&scenario, &server, Some("14d"), true), &mut io);
    assert!(
        matches!(result, Err(ImportError::Repo(RepoError::NotGitHub { .. }))),
        "{result:?}"
    );
    assert_eq!(io.token_calls, 0);
    assert_eq!(server.with(|state| state.manifests.len())?, 0);
    Ok(())
}
