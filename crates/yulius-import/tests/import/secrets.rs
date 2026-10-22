//! `yulius secrets import` against the fake server (SPEC-205 AC-2..AC-6): what is created, what is
//! skipped, the confirmation, failures, and that no value is ever said.

use std::collections::VecDeque;
use std::io;

use yulius_import::{
    Secret, SecretsImportError, SecretsImportOptions, SecretsIo, SecretsSummary, SecretsTarget,
    VaultClient, import_secrets, parse_dotenv, prepare_target, prompt_secrets, secret_file_entry,
};

use crate::support::server::{FakeServer, TOKEN};
use crate::support::{RepoFixture, TestResult};

const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";

/// Every value the tests import: none may appear in what the command says.
const VALUES: &[&str] = &[
    "sk_live_new_value_1",
    "existing-value-2",
    "refused-value-3",
    "after-value-4",
    "b3BlbnNzaC1rZXktdjEAAAAA",
    "multi-line-value",
    "tiny-v1",
    "hidden-typed-value-9",
    "acme-bucket-0042",
];

/// A scripted terminal: answers in order, records what was said.
#[derive(Default)]
struct Terminal {
    answers: VecDeque<String>,
    hidden: VecDeque<String>,
    said: String,
    asked: Vec<String>,
    no_terminal: bool,
}

impl Terminal {
    fn answering(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_owned()).collect(),
            ..Self::default()
        }
    }

    fn assert_no_value(&self) {
        let all = format!("{}\n{}", self.said, self.asked.join("\n"));
        for value in VALUES {
            assert!(!all.contains(value), "said {value:?}:\n{all}");
        }
    }
}

impl SecretsIo for Terminal {
    fn say(&mut self, line: &str) {
        self.said.push_str(line);
        self.said.push('\n');
    }

    fn ask(&mut self, prompt: &str) -> io::Result<String> {
        self.asked.push(prompt.to_owned());
        if self.no_terminal {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no /dev/tty"));
        }
        self.answers
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "end of input"))
    }

    fn ask_hidden(&mut self, prompt: &str) -> io::Result<Secret> {
        self.asked.push(prompt.to_owned());
        self.hidden
            .pop_front()
            .map(Secret::new)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "end of input"))
    }
}

/// Team-wide team secrets (`--instance`).
fn team() -> SecretsImportOptions {
    SecretsImportOptions {
        target: SecretsTarget::Team {
            sandbox: None,
            instance: true,
        },
        checkout: std::env::temp_dir(),
        source_file: Some("secrets.env".into()),
    }
}

fn agent(sandbox: &str, instance: bool) -> SecretsImportOptions {
    SecretsImportOptions {
        target: SecretsTarget::Agent {
            sandbox: Some(sandbox.into()),
            instance,
            confirm_hosts: false,
        },
        checkout: std::env::temp_dir(),
        source_file: None,
    }
}

fn run(
    server: &FakeServer,
    file: &str,
    options: &SecretsImportOptions,
    terminal: &mut Terminal,
) -> TestResult<Result<SecretsSummary, SecretsImportError>> {
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = match prepare_target(options, &client) {
        Ok(target) => target,
        Err(err) => return Ok(Err(err)),
    };
    Ok(import_secrets(
        parse_dotenv(file)?,
        &target,
        &client,
        terminal,
    ))
}

fn with_sandbox(server: &FakeServer) -> TestResult {
    server.with(|s| {
        s.sandboxes = vec![
            (
                "sb-1".into(),
                "quartio-yulius".into(),
                "Quartio-Studio".into(),
                "yulius".into(),
            ),
            (
                "sb-2".into(),
                "other".into(),
                "Quartio-Studio".into(),
                "other".into(),
            ),
        ];
    })
}

#[test]
fn team_import_creates_only_new_secrets_after_yes() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|s| s.team_titles = vec!["OLD_KEY".into()])?;
    let file = "# Stripe live key\nNEW_KEY=sk_live_new_value_1\nOLD_KEY=existing-value-2\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &team(), &mut terminal)??;

    assert_eq!(summary.imported, vec!["NEW_KEY"]);
    assert_eq!(summary.existing, vec!["OLD_KEY"]);
    assert!(summary.ok());
    let created = server.with(|s| s.team_created.clone())?;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].title, "NEW_KEY");
    assert_eq!(created[0].value.as_deref(), Some("sk_live_new_value_1"));
    assert_eq!(created[0].notes.as_deref(), Some("Stripe live key"));
    assert!(terminal.said.contains("exists: skipped, never overwritten"));
    assert!(
        terminal.said.contains("NEW_KEY  new, with notes"),
        "the plan says a comment goes along:\n{}",
        terminal.said
    );
    assert!(terminal.said.contains("secrets.env still holds the values"));
    terminal.assert_no_value();
    Ok(())
}

#[test]
fn agent_import_checks_names_and_values_and_sends_keys_with_literal_line_breaks() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    server.with(|s| {
        s.agent_names = vec![
            ("OLD_KEY".into(), "sandbox".into()),
            ("GLOBAL_KEY".into(), "instance".into()),
        ];
    })?;
    let file = format!(
        "# For tests\nNEW_KEY=sk_live_new_value_1\nOLD_KEY=existing-value-2\nGLOBAL_KEY=after-value-4\n\
         lower_key=refused-value-3\nDEPLOY_KEY='{PEM}'\nTRAILING=\"trailing-line-value\n\"\nNOTE=\"multi-line-value\nsecond\"\nTINY=tiny-v1\n"
    );
    let mut terminal = Terminal::answering(&["YES"]);
    let summary = run(
        &server,
        &file,
        &agent("quartio-studio/YULIUS", false),
        &mut terminal,
    )??;

    assert_eq!(
        summary.imported,
        vec!["NEW_KEY", "GLOBAL_KEY", "DEPLOY_KEY", "TRAILING"]
    );
    assert_eq!(summary.existing, vec!["OLD_KEY"]);
    assert_eq!(summary.invalid, vec!["lower_key", "NOTE", "TINY"]);
    assert!(!summary.ok(), "invalid entries make the run not ok");
    let created = server.with(|s| s.agent_created.clone())?;
    assert!(created.iter().all(|(sid, input)| sid == "sb-1"
        && input.scope.as_deref() == Some("sandbox")
        && input.hosts.is_empty()
        && input.auto_allow.is_none()));
    // SPEC-216: the server's default (agents set them up) is said before yes.
    assert!(
        terminal.said.contains("Agents set them up"),
        "{}",
        terminal.said
    );
    let key = created
        .iter()
        .find(|(_, i)| i.name == "DEPLOY_KEY")
        .map(|(_, i)| i.value.clone());
    assert_eq!(key, Some(PEM.replace('\n', "\\n")));
    // SPEC-211 SCF-003: a value whose only line break ends it is stored without it, as a file
    // is (the shared conversion).
    let trailing = created
        .iter()
        .find(|(_, i)| i.name == "TRAILING")
        .map(|(_, i)| i.value.clone());
    assert_eq!(trailing.as_deref(), Some("trailing-line-value"));
    assert!(
        terminal
            .said
            .contains("the agent secrets of Quartio-Studio/yulius (this sandbox only)")
    );
    assert!(terminal.said.contains("cannot span lines"));
    assert!(
        terminal
            .said
            .contains("NEW_KEY     new, with description (agents read it)")
    );
    assert!(terminal.said.contains("GLOBAL_KEY  new\n"));
    terminal.assert_no_value();
    Ok(())
}

/// SPEC-216 SCO-005: team secrets go to the sandbox by default (checked against its own), and
/// team-wide only with `--instance`.
#[test]
fn team_import_goes_to_the_sandbox_unless_team_wide() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    server.with(|s| {
        s.sandbox_team_titles = vec!["OLD_KEY".into()];
        s.team_titles = vec!["WIDE_KEY".into()];
    })?;
    let file = "NEW_KEY=sk_live_new_value_1
OLD_KEY=existing-value-2
WIDE_KEY=wide-value-3
";
    let options = SecretsImportOptions {
        target: SecretsTarget::Team {
            sandbox: Some("sb-2".into()),
            instance: false,
        },
        ..team()
    };
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &options, &mut terminal)??;
    assert_eq!(summary.imported, vec!["NEW_KEY", "WIDE_KEY"]);
    assert_eq!(summary.existing, vec!["OLD_KEY"]);
    let created = server.with(|s| s.sandbox_team_created.clone())?;
    assert!(
        created
            .iter()
            .all(|(sid, input)| sid == "sb-2" && input.scope.as_deref() == Some("sandbox")),
        "{created:?}"
    );
    assert!(server.with(|s| s.team_created.is_empty())?);
    assert!(
        terminal
            .said
            .contains("the team secrets of Quartio-Studio/other (this sandbox only"),
        "{}",
        terminal.said
    );

    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(
        &server,
        "WIDE_KEY=wide-value-3
ALL_KEY=all-value-4
",
        &team(),
        &mut terminal,
    )??;
    assert_eq!(summary.imported, vec!["ALL_KEY"]);
    let created = server.with(|s| s.team_created.clone())?;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].scope, None, "team-wide is that route's default");
    assert!(terminal.said.contains("team-wide"), "{}", terminal.said);
    Ok(())
}

/// SPEC-216 review M1: `--confirm-hosts` turns agents setting them up off, and says so; an
/// absent choice is not sent, so an older server still takes the import.
#[test]
fn confirmed_hosts_are_asked_for_and_nothing_new_is_sent_by_default() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let options = SecretsImportOptions {
        target: SecretsTarget::Agent {
            sandbox: Some("sb-1".into()),
            instance: false,
            confirm_hosts: true,
        },
        ..agent("sb-1", false)
    };
    let mut terminal = Terminal::answering(&["yes"]);
    run(
        &server,
        "NEW_KEY=sk_live_new_value_1\n",
        &options,
        &mut terminal,
    )??;
    let created = server.with(|s| s.agent_created.clone())?;
    assert_eq!(created[0].1.auto_allow, Some(false));
    assert!(
        terminal.said.contains("a person allows each host"),
        "{}",
        terminal.said
    );
    let plain = serde_json::to_value(yulius_contracts::api::secrets::AgentSecretInput::default())?;
    assert!(plain.get("auto_allow").is_none(), "{plain}");
    let team = serde_json::to_value(yulius_contracts::api::secrets::TeamSecretInput::default())?;
    assert!(team.get("scope").is_none(), "{team}");
    Ok(())
}

/// SPEC-216 review round 4: outside a checkout, team secrets point to `--instance` (team-wide,
/// what `--to team` did before) beside `--sandbox`.
#[test]
fn team_import_outside_a_checkout_names_both_ways() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let elsewhere = tempfile::tempdir()?;
    let options = SecretsImportOptions {
        target: SecretsTarget::Team {
            sandbox: None,
            instance: false,
        },
        checkout: elsewhere.path().to_path_buf(),
        source_file: None,
    };
    let mut terminal = Terminal::answering(&["yes"]);
    match run(
        &server,
        "NEW_KEY=sk_live_new_value_1\n",
        &options,
        &mut terminal,
    )? {
        Err(SecretsImportError::Sandbox(message)) => {
            assert!(message.contains("--sandbox"), "{message}");
            assert!(message.contains("--instance"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn instance_scope_is_sent_and_checked_against_instance_secrets() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    server.with(|s| s.agent_names = vec![("GLOBAL_KEY".into(), "instance".into())])?;
    let file = "GLOBAL_KEY=after-value-4\nNEW_KEY=sk_live_new_value_1\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &agent("sb-2", true), &mut terminal)??;

    assert_eq!(summary.imported, vec!["NEW_KEY"]);
    assert_eq!(summary.existing, vec!["GLOBAL_KEY"]);
    let created = server.with(|s| s.agent_created.clone())?;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].0, "sb-2");
    assert_eq!(created[0].1.scope.as_deref(), Some("instance"));
    assert!(
        terminal
            .said
            .contains("global, switched on for this sandbox")
    );
    Ok(())
}

#[test]
fn anything_but_yes_or_no_terminal_sends_nothing() -> TestResult {
    let server = FakeServer::start()?;
    let file = "NEW_KEY=sk_live_new_value_1\n";
    for answers in [&["no"][..], &["y"][..], &[][..]] {
        let mut terminal = Terminal::answering(answers);
        let outcome = run(&server, file, &team(), &mut terminal)?;
        assert!(
            matches!(outcome, Err(SecretsImportError::Cancelled)),
            "{answers:?}"
        );
        terminal.assert_no_value();
    }
    let mut terminal = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    let outcome = run(&server, file, &team(), &mut terminal)?;
    assert!(matches!(outcome, Err(SecretsImportError::Cancelled)));
    assert_eq!(server.with(|s| s.creates)?, 0);
    Ok(())
}

#[test]
fn nothing_new_asks_nothing() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|s| s.team_titles = vec!["OLD_KEY".into()])?;
    let mut terminal = Terminal::default();
    let summary = run(
        &server,
        "OLD_KEY=existing-value-2\n",
        &team(),
        &mut terminal,
    )??;
    assert!(summary.imported.is_empty());
    assert!(terminal.asked.is_empty(), "asked {:?}", terminal.asked);
    assert!(terminal.said.contains("nothing was sent"));
    assert_eq!(server.with(|s| s.creates)?, 0);
    Ok(())
}

#[test]
fn a_refused_secret_is_reported_and_the_rest_go_on() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|s| s.refuse_names = vec!["BAD_KEY".into()])?;
    let file = "BAD_KEY=refused-value-3\nNEXT_KEY=after-value-4\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &team(), &mut terminal)??;
    assert_eq!(summary.failed, vec!["BAD_KEY"]);
    assert_eq!(summary.imported, vec!["NEXT_KEY"]);
    assert!(!summary.ok());
    assert!(
        terminal
            .said
            .contains("failed BAD_KEY: request rejected with HTTP 422")
    );
    terminal.assert_no_value();
    Ok(())
}

#[test]
fn an_expired_sign_in_stops_the_run_and_says_what_was_not_sent() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|s| s.expire_after_creates = Some(1))?;
    let file = "NEW_KEY=sk_live_new_value_1\nNEXT_KEY=after-value-4\nLAST_KEY=refused-value-3\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &team(), &mut terminal)??;
    assert_eq!(summary.imported, vec!["NEW_KEY"]);
    assert_eq!(summary.unsent, vec!["NEXT_KEY", "LAST_KEY"]);
    assert_eq!(
        server.with(|s| s.create_requests)?,
        2,
        "nothing is sent after the refused request"
    );
    assert!(terminal.said.contains("stopped at NEXT_KEY: not signed in"));
    assert!(terminal.said.contains("2 not sent"));
    terminal.assert_no_value();
    Ok(())
}

#[test]
fn an_unknown_or_ambiguous_sandbox_is_refused_before_anything_is_asked() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    for (wanted, expected) in [
        ("nope/missing", "no sandbox of yours is nope/missing"),
        ("quartio-studio", "no sandbox of yours"),
    ] {
        let mut terminal = Terminal::answering(&["yes"]);
        let outcome = run(
            &server,
            "NEW_KEY=sk_live_new_value_1\n",
            &agent(wanted, false),
            &mut terminal,
        )?;
        match outcome {
            Err(SecretsImportError::Sandbox(message)) => {
                assert!(message.contains(expected), "{message}");
            }
            other => panic!("{wanted}: {other:?}"),
        }
        assert!(terminal.asked.is_empty());
    }
    server.with(|s| {
        s.sandboxes.push((
            "sb-3".into(),
            "yulius".into(),
            "Someone".into(),
            "yulius".into(),
        ));
    })?;
    let mut terminal = Terminal::answering(&["yes"]);
    let outcome = run(
        &server,
        "NEW_KEY=sk_live_new_value_1\n",
        &agent("yulius", false),
        &mut terminal,
    )?;
    assert!(
        matches!(&outcome, Err(SecretsImportError::Sandbox(m)) if m.contains("several sandboxes")),
        "{outcome:?}"
    );
    assert_eq!(server.with(|s| s.creates)?, 0);
    Ok(())
}

#[test]
fn prompted_secrets_are_read_hidden_until_an_empty_name() -> TestResult {
    let server = FakeServer::start()?;
    let mut terminal = Terminal::answering(&["TYPED_KEY", "Typed by hand", "TYPED_KEY", "", "yes"]);
    terminal.hidden = VecDeque::from(["hidden-typed-value-9".to_owned()]);
    let entries = prompt_secrets(&mut terminal)?;
    assert_eq!(entries.len(), 1);
    assert!(terminal.said.contains("TYPED_KEY was already entered"));
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let options = SecretsImportOptions {
        source_file: None,
        ..team()
    };
    let target = prepare_target(&options, &client)?;
    let summary = import_secrets(entries, &target, &client, &mut terminal)?;
    assert_eq!(summary.imported, vec!["TYPED_KEY"]);
    let created = server.with(|s| s.team_created.clone())?;
    assert_eq!(created[0].value.as_deref(), Some("hidden-typed-value-9"));
    assert_eq!(created[0].notes.as_deref(), Some("Typed by hand"));
    assert!(!terminal.said.contains("still holds the values"));
    terminal.assert_no_value();

    let mut closed = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    assert!(matches!(
        prompt_secrets(&mut closed),
        Err(SecretsImportError::Terminal(_))
    ));
    Ok(())
}

/// Review finding: the first line of a key alone (a paste cut at the hidden prompt) is never
/// imported as if it were the key.
#[test]
fn a_key_cut_to_its_first_line_is_skipped() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let file = "CUT_KEY=-----BEGIN OPENSSH PRIVATE KEY-----\n";
    for options in [team(), agent("sb-1", false)] {
        let mut terminal = Terminal::answering(&["yes"]);
        let summary = run(&server, file, &options, &mut terminal)??;
        assert_eq!(summary.invalid, vec!["CUT_KEY"]);
        assert!(
            terminal.said.contains("import a key from a file"),
            "{}",
            terminal.said
        );
    }
    assert_eq!(server.with(|s| s.create_requests)?, 0);

    let mut terminal = Terminal::answering(&["PASTED_KEY", ""]);
    terminal.hidden = VecDeque::from(["-----BEGIN RSA PRIVATE KEY-----".to_owned()]);
    let entries = prompt_secrets(&mut terminal)?;
    assert!(entries.is_empty());
    assert!(
        terminal.said.contains("import a key from a file"),
        "{}",
        terminal.said
    );
    assert!(
        !terminal.asked.iter().any(|a| a.starts_with("Description")),
        "{:?}",
        terminal.asked
    );
    Ok(())
}

/// Review finding: an unreachable server stops the run after one attempt (a create is never
/// retried) and says what was not sent.
#[test]
fn a_server_error_stops_the_run_without_retrying() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|s| s.fail_creates = Some(503))?;
    let file = "NEW_KEY=sk_live_new_value_1\nNEXT_KEY=after-value-4\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &team(), &mut terminal)??;
    assert!(summary.imported.is_empty());
    assert_eq!(summary.unsent, vec!["NEW_KEY", "NEXT_KEY"]);
    assert_eq!(server.with(|s| s.create_requests)?, 1, "never retried");
    terminal.assert_no_value();
    Ok(())
}

/// Review nit: a 403 (no right to manage them) stops the run instead of failing every secret.
#[test]
fn a_forbidden_create_stops_the_run() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    server.with(|s| s.fail_creates = Some(403))?;
    let file = "NEW_KEY=sk_live_new_value_1\nNEXT_KEY=after-value-4\n";
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(&server, file, &agent("sb-1", true), &mut terminal)??;
    assert_eq!(summary.unsent, vec!["NEW_KEY", "NEXT_KEY"]);
    assert_eq!(server.with(|s| s.create_requests)?, 1);
    Ok(())
}

/// Review finding: the target is checked before anything is typed, so a wrong sandbox does not
/// throw away typed values.
#[test]
fn the_target_is_checked_before_secrets_are_typed() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let outcome = prepare_target(&agent("nope/missing", false), &client);
    assert!(matches!(outcome, Err(SecretsImportError::Sandbox(_))));
    Ok(())
}

/// SPEC-256 LOB-002: a run its caller made unattended (an operate token, checked by the CLI)
/// shows the plan, asks nothing and sends; it needs no terminal.
#[test]
fn an_unattended_run_asks_nothing() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = prepare_target(&agent("Quartio-Studio/yulius", false), &client)?
        .unattended()
        .env_files_allowed();
    let mut terminal = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    let summary = import_secrets(
        parse_dotenv("NEW_KEY=sk_live_new_value_1\n")?,
        &target,
        &client,
        &mut terminal,
    )?;
    assert_eq!(summary.imported, vec!["NEW_KEY"]);
    // SPEC-256 LOB-004: created already allowed into `.env` files, as said before sending.
    let created = server.with(|s| s.agent_created.clone())?;
    assert_eq!(created[0].1.env_files, Some(true));
    assert!(terminal.said.contains(".env files"), "{}", terminal.said);
    assert!(terminal.asked.is_empty(), "asked {:?}", terminal.asked);
    assert!(terminal.said.contains("NEW_KEY"), "{}", terminal.said);
    assert!(
        terminal.said.contains("without asking"),
        "{}",
        terminal.said
    );
    terminal.assert_no_value();
    Ok(())
}

/// SPEC-256 LOB-003: a key file is imported by its path, whole, as the Secrets page stores a key,
/// described by its file name; a file that is no text or too large is refused without its content.
#[test]
fn a_key_file_is_imported_by_path() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("deploy_ed25519");
    std::fs::write(&path, format!("{PEM}\n"))?;
    let entry = secret_file_entry("DEPLOY_KEY", &path)?;
    assert_eq!(entry.name, "DEPLOY_KEY");
    assert_eq!(entry.comment, "From the file deploy_ed25519");
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = prepare_target(&agent("Quartio-Studio/yulius", false), &client)?;
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = import_secrets(vec![entry], &target, &client, &mut terminal)?;
    assert_eq!(summary.imported, vec!["DEPLOY_KEY"]);
    let created = server.with(|s| s.agent_created.clone())?;
    assert_eq!(created[0].1.value, PEM.replace('\n', "\\n"));
    assert_eq!(created[0].1.env_files, None, "only when asked for");
    assert_eq!(
        created[0].1.description.as_deref(),
        Some("From the file deploy_ed25519")
    );
    terminal.assert_no_value();

    let binary = dir.path().join("blob");
    std::fs::write(&binary, [0xff_u8, 0xfe, 0x00, 0x62])?;
    let err = secret_file_entry("BLOB", &binary).err().ok_or("not text")?;
    assert!(err.contains("not UTF-8 text"), "{err}");
    let large = dir.path().join("large");
    std::fs::write(&large, "x".repeat(65 * 1024))?;
    let err = secret_file_entry("LARGE", &large)
        .err()
        .ok_or("too large")?;
    assert!(err.contains("64 KiB") && !err.contains("xxxx"), "{err}");
    // Cut at the bound inside a two-byte character: still too large, not "not text".
    std::fs::write(&large, "ł".repeat(40 * 1024))?;
    let err = secret_file_entry("LARGE", &large)
        .err()
        .ok_or("too large")?;
    assert!(err.contains("64 KiB"), "{err}");
    let err = secret_file_entry("GONE", &dir.path().join("gone"))
        .err()
        .ok_or("missing")?;
    assert!(err.contains("gone"), "{err}");
    Ok(())
}

/// SPEC-274 PLW-002: values that are ordinary text (a configuration word, a host, an e-mail
/// address, a URL without credentials) are refused before anything is sent and named with why,
/// with or without --yes; credentials go on.
#[test]
fn ordinary_text_values_are_refused_and_named() -> TestResult {
    let file = "PIPEDREAM_PROJECT_ENVIRONMENT=production\nAPI_HOST=api.example.com\n\
                OPS_EMAIL=ops@example.com\nAPI_URL=https://api.example.com/v1\n\
                DATABASE_URL=postgres://app:s3cretPass@db.internal/app\nNEW_KEY=sk_live_new_value_1\n";
    for unattended in [false, true] {
        let server = FakeServer::start()?;
        with_sandbox(&server)?;
        let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
        let mut target = prepare_target(&agent("Quartio-Studio/yulius", false), &client)?;
        if unattended {
            target = target.unattended();
        }
        let mut terminal = Terminal::answering(&["yes"]);
        let summary = import_secrets(parse_dotenv(file)?, &target, &client, &mut terminal)?;
        assert_eq!(summary.imported, vec!["DATABASE_URL", "NEW_KEY"]);
        assert_eq!(
            summary.invalid,
            vec![
                "PIPEDREAM_PROJECT_ENVIRONMENT",
                "API_HOST",
                "OPS_EMAIL",
                "API_URL"
            ]
        );
        let sent: Vec<String> = server
            .with(|s| s.agent_created.clone())?
            .into_iter()
            .map(|(_, input)| input.name)
            .collect();
        assert_eq!(sent, vec!["DATABASE_URL", "NEW_KEY"]);
        let said = &terminal.said;
        for line in [
            "PIPEDREAM_PROJECT_ENVIRONMENT  skipped: a configuration word",
            "API_HOST                       skipped: a host name or address",
            "OPS_EMAIL                      skipped: an e-mail address",
            "API_URL                        skipped: a URL without credentials",
        ] {
            assert!(said.contains(line), "{line:?} in:\n{said}");
        }
        // Review of SPEC-274: a credential taken for ordinary text never goes to the hand-over.
        assert!(
            said.contains("belongs in configuration or the hand-over")
                && said.contains(
                    "if it is a credential after all, a person adds it on the Secrets page"
                ),
            "{said}"
        );
        // "production deploys" is the run's own text: the values are looked for in the plan.
        let plan: Vec<&str> = said.lines().filter(|l| l.starts_with("  ")).collect();
        for value in [
            "production",
            "api.example.com",
            "ops@example.com",
            "s3cretPass",
        ] {
            assert!(
                plan.iter().all(|l| !l.contains(value)),
                "said {value:?}:\n{said}"
            );
        }
        terminal.assert_no_value();
    }
    Ok(())
}

/// The fake server's sandbox of [`RepoFixture`]'s repository, and a checkout of it holding
/// `acme-bucket-0042` in a tracked file.
fn demo_checkout(server: &FakeServer) -> TestResult<(RepoFixture, SecretsImportOptions)> {
    server.with(|s| {
        s.sandboxes = vec![(
            "sb-demo".into(),
            "quartio-demo".into(),
            "Quartio-Studio".into(),
            "demo".into(),
        )];
    })?;
    let fixture = RepoFixture::new()?;
    fixture.commit_file("deploy/config.yml", "bucket: acme-bucket-0042\n", "config")?;
    let options = SecretsImportOptions {
        target: SecretsTarget::Agent {
            sandbox: None,
            instance: false,
            confirm_hosts: false,
        },
        checkout: fixture.root()?,
        source_file: None,
    };
    Ok((fixture, options))
}

const IN_REPO: &str = "BUCKET=acme-bucket-0042\nNEW_KEY=sk_live_new_value_1\n";

fn sent_names(server: &FakeServer) -> TestResult<Vec<String>> {
    Ok(server
        .with(|s| s.agent_created.clone())?
        .into_iter()
        .map(|(_, input)| input.name)
        .collect())
}

/// SPEC-274 PLW-003: with --yes a value found in a tracked file of the project's checkout is
/// skipped and listed, naming where it was found, never the value.
#[test]
fn an_unattended_run_skips_values_found_in_the_repository() -> TestResult {
    let server = FakeServer::start()?;
    let (_fixture, options) = demo_checkout(&server)?;
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = prepare_target(&options, &client)?.unattended();
    let mut terminal = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    let summary = import_secrets(parse_dotenv(IN_REPO)?, &target, &client, &mut terminal)?;
    assert_eq!(summary.invalid, vec!["BUCKET"]);
    assert_eq!(sent_names(&server)?, vec!["NEW_KEY"]);
    assert!(
        terminal.said.contains(
            "BUCKET   skipped: occurs in 1 tracked file of the repository (deploy/config.yml)"
        ),
        "{}",
        terminal.said
    );
    assert!(
        terminal
            .said
            .contains("a person adds it on the Secrets page"),
        "{}",
        terminal.said
    );
    assert!(terminal.asked.is_empty(), "asked {:?}", terminal.asked);
    terminal.assert_no_value();
    Ok(())
}

/// SPEC-274 PLW-003: at the terminal such a value is asked about: yes imports it, anything else
/// skips it.
#[test]
fn a_value_found_in_the_repository_is_asked_about() -> TestResult {
    for (answer, imported, declined) in [
        ("yes", vec!["BUCKET", "NEW_KEY"], Vec::<&str>::new()),
        ("no", vec!["NEW_KEY"], vec!["BUCKET"]),
    ] {
        let server = FakeServer::start()?;
        let (_fixture, options) = demo_checkout(&server)?;
        let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
        let target = prepare_target(&options, &client)?;
        let mut terminal = Terminal::answering(&[answer, "yes"]);
        let summary = import_secrets(parse_dotenv(IN_REPO)?, &target, &client, &mut terminal)?;
        assert_eq!(summary.imported, imported, "{answer}");
        // A deliberate no is the person's choice, not a failure of the run.
        assert_eq!(summary.declined, declined, "{answer}");
        assert!(summary.invalid.is_empty() && summary.ok(), "{answer}");
        assert!(
            terminal.asked[0]
                .contains("BUCKET occurs in 1 tracked file of the repository (deploy/config.yml)")
                && terminal.asked[0].contains("Import it anyway?"),
            "{:?}",
            terminal.asked
        );
        terminal.assert_no_value();
    }
    Ok(())
}

/// SPEC-274 PLW-003: outside a checkout of the project nothing is looked for, and the run says so.
#[test]
fn outside_the_project_checkout_the_repository_is_not_searched() -> TestResult {
    let server = FakeServer::start()?;
    with_sandbox(&server)?;
    let mut terminal = Terminal::answering(&["yes"]);
    let summary = run(
        &server,
        IN_REPO,
        &agent("Quartio-Studio/yulius", false),
        &mut terminal,
    )??;
    assert_eq!(summary.imported, vec!["BUCKET", "NEW_KEY"]);
    assert!(
        terminal.said.contains(
            "not looked for in the repository: run it in a checkout of Quartio-Studio/yulius"
        ),
        "{}",
        terminal.said
    );
    Ok(())
}

/// Review of SPEC-274: a checkout of the project on a detached HEAD is still searched.
#[test]
fn a_detached_checkout_of_the_project_is_searched() -> TestResult {
    let server = FakeServer::start()?;
    let (fixture, options) = demo_checkout(&server)?;
    crate::support::git(&fixture.repo, &["checkout", "-q", "--detach"])?;
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = prepare_target(&options, &client)?.unattended();
    let mut terminal = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    let summary = import_secrets(parse_dotenv(IN_REPO)?, &target, &client, &mut terminal)?;
    assert_eq!(summary.invalid, vec!["BUCKET"], "{}", terminal.said);
    Ok(())
}

/// Review of SPEC-274: a checkout of the project with no commit yet is still searched.
#[test]
fn a_checkout_without_commits_is_searched() -> TestResult {
    let server = FakeServer::start()?;
    let (_fixture, mut options) = demo_checkout(&server)?;
    let fresh = tempfile::tempdir()?;
    let repo = fresh.path();
    crate::support::git(repo, &["init", "-q", "-b", "main"])?;
    crate::support::git(
        repo,
        &["remote", "add", "origin", crate::support::GITHUB_URL],
    )?;
    std::fs::write(repo.join("config.yml"), "bucket: acme-bucket-0042\n")?;
    crate::support::git(repo, &["add", "config.yml"])?;
    options.checkout = repo.to_path_buf();
    // Without a commit the sandbox is named, as `yulius secrets import --sandbox` does.
    options.target = SecretsTarget::Agent {
        sandbox: Some("Quartio-Studio/demo".into()),
        instance: false,
        confirm_hosts: false,
    };
    let client = VaultClient::new(server.url.clone(), Secret::new(TOKEN))?;
    let target = prepare_target(&options, &client)?.unattended();
    let mut terminal = Terminal {
        no_terminal: true,
        ..Terminal::default()
    };
    let summary = import_secrets(parse_dotenv(IN_REPO)?, &target, &client, &mut terminal)?;
    assert_eq!(summary.invalid, vec!["BUCKET"], "{}", terminal.said);
    Ok(())
}
