//! `yulius secrets import` against the fake server (SPEC-205 AC-2..AC-6): what is created, what is
//! skipped, the confirmation, failures, and that no value is ever said.

use std::collections::VecDeque;
use std::io;

use yulius_import::{
    Secret, SecretsImportError, SecretsImportOptions, SecretsIo, SecretsSummary, SecretsTarget,
    VaultClient, import_secrets, parse_dotenv, prepare_target, prompt_secrets,
};

use crate::support::TestResult;
use crate::support::server::{FakeServer, TOKEN};

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
         lower_key=refused-value-3\nDEPLOY_KEY='{PEM}'\nNOTE=\"multi-line-value\nsecond\"\nTINY=tiny-v1\n"
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
        vec!["NEW_KEY", "GLOBAL_KEY", "DEPLOY_KEY"]
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
    assert!(terminal.said.contains("every sandbox (instance)"));
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
