//! The root move helper against a fake command, a fake `systemctl` and real files and signals
//! (SPEC-470 MVB-003 to MVB-006).

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use yulius_contracts::migrate::{
    HELPER_STATUS_FILE, MAX_REQUEST_BYTES, MoveAction, MoveHelperState, MoveHelperStatus,
    MoveRequest, REQUEST_DIR, REQUEST_FILE, REQUEST_TTL_MS, STATUS_FILE, VERSION,
};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NOW: i64 = 1_800_000_000_000;

fn script(path: &Path, body: &str) -> TestResult {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn plan(id: &str) -> MoveRequest {
    MoveRequest {
        version: VERSION,
        id: id.into(),
        action: MoveAction::Plan,
        target: Some("root@new.example".into()),
        ssh_port: Some(2222),
        public_url: Some("https://new.example".into()),
        data_disks: vec!["/dev/vdc".into(), "/dev/vdd".into()],
        requested_at: now_ms(),
    }
}

fn abort(id: &str) -> MoveRequest {
    MoveRequest {
        action: MoveAction::Abort,
        target: None,
        ssh_port: None,
        public_url: None,
        data_disks: Vec::new(),
        ..plan(id)
    }
}

/// Waits until `met` says so; a test that never gets there fails with `what`.
fn until(what: &str, mut met: impl FnMut() -> bool) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !met() {
        if Instant::now() > deadline {
            return Err(format!("never happened: {what}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Waits for the helper's thread. One that never ends fails the test instead of hanging it, and
/// the fake command it waits for is ended, so nothing runs on behind a failed test.
fn ended(run: std::thread::JoinHandle<Result<(), String>>, env: &Env) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !run.is_finished() {
        if Instant::now() > deadline {
            if let Some(pid) = std::fs::read_to_string(env.bin.join("command.pid"))
                .ok()
                .and_then(|pid| pid.trim().parse::<i32>().ok())
            {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            return Err("the helper never ended".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    run.join().map_err(|_| "the helper panicked")??;
    Ok(())
}

/// A data directory of an unprivileged owner, a state directory, a fake `systemctl` whose
/// `is-active` answers what `bin/update-state` holds, and a fake command that writes each
/// argument on its own line to `bin/command.log` and then does what `bin/command.do` says.
struct Env {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    args: MoveHelperArgs,
}

/// A fake command still running when a test ends (it failed half way) is ended with it: left
/// alone it would run on, holding the test run's output open.
impl Drop for Env {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(self.bin.join("command.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<i32>().ok())
        {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

impl Env {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let data = root.join("data");
        std::fs::create_dir_all(&data)?;
        if nix::unistd::getuid().is_root() {
            // The helper reads only requests of the (unprivileged) data directory's owner.
            nix::unistd::chown(&data, Some(nix::unistd::Uid::from_raw(65534)), None)?;
        }
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)?;
        script(
            &bin.join("systemctl"),
            r#"d="$(dirname "$0")"
echo "$*" >> "$d/systemctl.log"
case "$1" in
  is-active) state="$(cat "$d/update-state" 2>/dev/null || echo inactive)"; echo "$state"; [ "$state" = active ] || exit 3 ;;
  *) exit 1 ;;
esac"#,
        )?;
        script(
            &bin.join("command"),
            r#"d="$(dirname "$0")"
echo run >> "$d/command.runs"
echo $$ > "$d/command.pid"
printf '%s\n' "$@" > "$d/command.log"
[ -f "$YULIUS_TEST_STATE/status.json" ] && echo yes > "$d/status-was-there"
if [ -f "$d/command.do" ]; then . "$d/command.do"; fi"#,
        )?;
        let args = MoveHelperArgs {
            data_dir: data,
            state_dir: root.join("state"),
            service: "yqa-test.service".into(),
            update_service: "yqa-test-update.service".into(),
            targets: Vec::new(),
            systemctl: bin.join("systemctl"),
            program: Some(bin.join("command")),
        };
        Ok(Self {
            _dir: dir,
            bin,
            args,
        })
    }

    fn request(&self, request: &MoveRequest) -> TestResult {
        self.raw_request(&serde_json::to_vec(request)?)
    }

    /// Written aside and renamed into place, as the server does.
    fn raw_request(&self, bytes: &[u8]) -> TestResult {
        let dir = self.args.data_dir.join(REQUEST_DIR);
        std::fs::create_dir_all(&dir)?;
        let staged = dir.join("request.json.new");
        std::fs::write(&staged, bytes)?;
        if nix::unistd::getuid().is_root() {
            nix::unistd::chown(&staged, Some(nix::unistd::Uid::from_raw(65534)), None)?;
        }
        std::fs::rename(&staged, dir.join(REQUEST_FILE))?;
        Ok(())
    }

    fn helper(&self) -> Helper {
        let mut helper = Helper::new(self.args.clone());
        helper.pace = Pace {
            poll: Duration::from_millis(5),
            watch: Duration::from_millis(20),
            heartbeat: Duration::from_millis(30),
        };
        // The fake command looks for the checklist where the helper keeps it.
        helper.env = vec![(
            "YULIUS_TEST_STATE".into(),
            self.args.state_dir.clone().into_os_string(),
        )];
        helper
    }

    fn run(&self) -> TestResult {
        Ok(self.helper().run()?)
    }

    fn status(&self) -> Option<MoveHelperStatus> {
        let bytes = std::fs::read(self.args.state_dir.join(HELPER_STATUS_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn said(&self) -> Result<MoveHelperStatus, Box<dyn std::error::Error>> {
        Ok(self.status().ok_or("the helper wrote no status")?)
    }

    /// The arguments of the command's last run.
    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.bin.join("command.log"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.bin.join("command.runs"))
            .map_or(0, |text| text.lines().count())
    }

    fn does(&self, body: &str) -> TestResult {
        Ok(std::fs::write(self.bin.join("command.do"), body)?)
    }

    fn unit_options(&self) -> Vec<String> {
        vec![
            "--data-dir".into(),
            self.args.data_dir.display().to_string(),
            "--service".into(),
            "yqa-test.service".into(),
            "--update-service".into(),
            "yqa-test-update.service".into(),
            "--state-dir".into(),
            self.args.state_dir.display().to_string(),
            // Whatever is asked: root's helper never signs in as root does.
            "--no-sign-in".into(),
        ]
    }
}

fn json(change: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value = serde_json::to_value(MoveRequest {
        requested_at: NOW,
        ..plan("0123abcd-plan")
    })
    .unwrap_or_default();
    change(&mut value);
    serde_json::to_vec(&value).unwrap_or_default()
}

/// MVB-003: the exact schema, the version, the id and the age; then the field rules.
#[test]
fn a_request_is_parsed_strictly() -> TestResult {
    let request = parse_request(&json(|_| {}), NOW)?;
    assert_eq!(request.target.as_deref(), Some("root@new.example"));
    let refused = |change: &dyn Fn(&mut serde_json::Value), now: i64| {
        parse_request(&json(|v| change(v)), now)
            .err()
            .unwrap_or_default()
    };
    assert_eq!(
        refused(&|v| v["yes"] = true.into(), NOW),
        "the request is not a valid request"
    );
    assert_eq!(
        refused(&|v| v["action"] = "install".into(), NOW),
        "the request is not a valid request"
    );
    assert_eq!(
        parse_request(b"{", NOW).err().unwrap_or_default(),
        "the request is not a valid request"
    );
    assert_eq!(
        refused(&|v| v["version"] = 2.into(), NOW),
        "the request has another protocol version"
    );
    for id in ["short", "has space-1", "../../etc/x"] {
        assert_eq!(
            refused(&|v| v["id"] = id.into(), NOW),
            "the request id is malformed"
        );
    }
    // 15 minutes old at most, 60 s ahead at most.
    assert_eq!(refused(&|_| {}, NOW + REQUEST_TTL_MS), "");
    assert_eq!(
        refused(&|_| {}, NOW + REQUEST_TTL_MS + 1),
        "the request is stale"
    );
    assert_eq!(refused(&|_| {}, NOW - 60_000), "");
    assert_eq!(refused(&|_| {}, NOW - 60_001), "the request is stale");
    // The field rules (MVB-002) are the server's.
    assert_eq!(
        refused(&|v| v["target"] = "-oProxyCommand=evil".into(), NOW),
        "the new host must be `root@host` or `host`"
    );
    assert_eq!(
        refused(&|v| v["action"] = "abort".into(), NOW),
        "an abort names no host, port, URL or disk"
    );
    Ok(())
}

/// MVB-004: each value is one argument, the target comes after `--`, and only a move says yes.
#[test]
fn the_command_gets_each_value_as_one_argument() -> TestResult {
    let env = Env::new()?;
    let mut expected = vec!["instance".to_owned(), "migrate".to_owned()];
    expected.extend(env.unit_options());
    expected.extend(
        [
            "--ssh-port=2222",
            "--public-url=https://new.example",
            "--data-disk=/dev/vdc",
            "--data-disk=/dev/vdd",
            "--",
            "root@new.example",
        ]
        .map(str::to_owned),
    );
    assert_eq!(command_args(&env.args, &plan("0123abcd-plan")), expected);

    let moved = MoveRequest {
        action: MoveAction::Move,
        public_url: None,
        data_disks: Vec::new(),
        ..plan("0123abcd-move")
    };
    let mut expected = vec!["instance".to_owned(), "migrate".to_owned()];
    expected.extend(env.unit_options());
    expected.extend(["--ssh-port=2222", "--yes", "--", "root@new.example"].map(str::to_owned));
    assert_eq!(command_args(&env.args, &moved), expected);

    // An abort is of the recorded move, and nobody answers its question for a person.
    let mut expected = vec![
        "instance".to_owned(),
        "migrate".to_owned(),
        "--abort".to_owned(),
    ];
    expected.extend(env.unit_options());
    let args = command_args(&env.args, &abort("0123abcd-abort"));
    assert_eq!(args, expected);
    assert!(!args.iter().any(|arg| arg == "--yes"));
    Ok(())
}

/// MVB-003, MVB-004, MVB-005: an accepted request runs the command once and is reported.
#[test]
fn a_plan_runs_the_command_once_and_reports_how_it_ended() -> TestResult {
    let env = Env::new()?;
    std::fs::create_dir_all(&env.args.state_dir)?;
    std::fs::write(
        env.args.state_dir.join(STATUS_FILE),
        "{\"an\":\"earlier run\"}",
    )?;
    let request = plan("0123abcd-plan");
    env.request(&request)?;
    env.run()?;

    assert_eq!(env.argv(), command_args(&env.args, &request));
    // The checklist that is there is the command's to replace, once it has accepted the run:
    // a run it refuses (a move to another host is recorded) must change nothing.
    assert!(env.bin.join("status-was-there").exists());
    assert_eq!(
        std::fs::read_to_string(env.args.state_dir.join(STATUS_FILE))?,
        "{\"an\":\"earlier run\"}",
        "the recorded checklist was touched"
    );
    let said = env.said()?;
    assert_eq!(said.id, "0123abcd-plan");
    assert_eq!(said.action, MoveAction::Plan);
    assert!(request.same_values(&said.values));
    assert_eq!(said.state, MoveHelperState::Ended);
    assert_eq!(said.exit_code, Some(0));
    assert_eq!(said.error, None);
    assert!(said.ended_at.is_some_and(|at| at >= said.started_at));
    let mode = |path: &Path| std::fs::metadata(path).map(|m| m.permissions().mode() & 0o7777);
    assert_eq!(mode(&env.args.state_dir.join(HELPER_STATUS_FILE))?, 0o644);
    assert_eq!(mode(&env.args.state_dir)?, 0o711);

    // The same file again (the path unit fires on every change): nothing runs.
    env.run()?;
    env.request(&request)?;
    env.run()?;
    assert_eq!(env.runs(), 1);
    assert_eq!(env.said()?, said);
    // And no request at all.
    std::fs::remove_file(env.args.data_dir.join(REQUEST_DIR).join(REQUEST_FILE))?;
    env.run()?;
    assert_eq!(env.runs(), 1);
    Ok(())
}

/// MVB-003: one request once holds only while the helper can record which one it took. When it
/// cannot (the state directory is full, or damaged), nothing runs and the unit fails.
#[test]
fn a_request_that_cannot_be_recorded_is_not_run() -> TestResult {
    let env = Env::new()?;
    // Where the taken requests' ids are kept there is something that cannot be replaced.
    std::fs::create_dir_all(env.args.state_dir.join(HANDLED_FILE).join("kept"))?;
    env.request(&plan("0123abcd-plan"))?;
    let said = env.helper().run().err().ok_or("the helper ended well")?;
    assert!(said.contains("cannot record the request"), "{said}");
    // It is not taken for an empty record and written over: the ids it holds would be lost.
    assert!(said.contains("what was handled cannot be read"), "{said}");
    assert_eq!(env.runs(), 0, "a request that could be run again was run");
    assert!(env.status().is_none());

    // A refusal is still said: it runs nothing, however often it is looked at.
    let mut bad = serde_json::to_value(plan("0123abcd-bad"))?;
    bad["ssh_port"] = 0.into();
    env.raw_request(&serde_json::to_vec(&bad)?)?;
    env.run()?;
    assert_eq!(env.said()?.state, MoveHelperState::Refused);
    assert_eq!(env.runs(), 0);
    Ok(())
}

/// MVB-003: an id is acted on once, also when other requests were handled since: the helper
/// remembers every id for as long as its request could still be taken.
#[test]
fn a_request_handled_before_another_is_not_acted_on_again() -> TestResult {
    let env = Env::new()?;
    let first = plan("0123abcd-first");
    env.request(&first)?;
    env.run()?;
    env.request(&plan("0123abcd-second"))?;
    env.run()?;
    assert_eq!(env.runs(), 2);
    // The first again, still fresh, from whoever kept a copy of it.
    env.request(&first)?;
    env.run()?;
    assert_eq!(env.runs(), 2, "an earlier request was acted on again");
    assert_eq!(env.said()?.id, "0123abcd-second");
    // A refusal in between is remembered beside them, not instead of them.
    let mut bad = serde_json::to_value(plan("0123abcd-bad"))?;
    bad["ssh_port"] = 0.into();
    env.raw_request(&serde_json::to_vec(&bad)?)?;
    env.run()?;
    assert_eq!(env.said()?.state, MoveHelperState::Refused);
    for again in [&first, &plan("0123abcd-second")] {
        let again = MoveRequest {
            requested_at: again.requested_at,
            ..(*again).clone()
        };
        env.request(&again)?;
        env.run()?;
    }
    assert_eq!(env.runs(), 2);
    assert_eq!(
        env.said()?.id,
        "0123abcd-bad",
        "the refusal's report was replaced"
    );
    // A record that is damaged in part (bytes that are no text) still tells what it holds: its
    // other lines are read, and they stay when the next id is added.
    let record = env.args.state_dir.join(HANDLED_FILE);
    let mut bytes = std::fs::read(&record)?;
    bytes.splice(0..0, [0xff, 0xfe, b'\n']);
    std::fs::write(&record, &bytes)?;
    env.request(&first)?;
    env.run()?;
    assert_eq!(env.runs(), 2, "a damaged record forgot what it held");
    env.request(&plan("0123abcd-third"))?;
    env.run()?;
    assert_eq!(env.runs(), 3);
    env.request(&first)?;
    env.run()?;
    assert_eq!(env.runs(), 3, "adding an id dropped the others");
    // Only root reads what it handled.
    let mode = std::fs::metadata(env.args.state_dir.join(HANDLED_FILE))?
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode, 0o600);
    Ok(())
}

/// MVB-003: an id is forgotten once its request could no longer be taken (it would be refused
/// as stale), lines that are not the helper's are ignored, and the list stays bounded.
#[test]
fn handled_ids_are_kept_while_their_request_could_be_taken() {
    let text = format!(
        "{} 0123abcd-old\n{} 0123abcd-fresh\nnot a line\n\n12 \n{} with spaces in it\n",
        NOW - 1,
        NOW,
        NOW + 5
    );
    assert_eq!(
        still_handled(&text, NOW),
        [(NOW, "0123abcd-fresh".to_owned())]
    );
    // Kept until the request is stale by the helper's own clock, however it was dated.
    assert_eq!(
        handled_until(NOW),
        NOW + REQUEST_TTL_MS + yulius_contracts::migrate::REQUEST_AHEAD_MS
    );
    let many: String = (0..KEPT_REQUESTS + 10)
        .map(|n| format!("{} 0123abcd-{n}\n", NOW + n as i64))
        .collect();
    let kept = with_handled(&many, "0123abcd-new", NOW);
    assert_eq!(kept.lines().count(), KEPT_REQUESTS);
    assert!(kept.ends_with(&format!("{} 0123abcd-new\n", handled_until(NOW))));
    assert!(!kept.contains(" 0123abcd-0\n"), "the oldest stayed");
    // The same id again replaces its line.
    let twice = with_handled(&kept, "0123abcd-new", NOW + 1);
    assert_eq!(twice.matches("0123abcd-new").count(), 1);
}

/// MVB-003: a request that breaks a rule runs nothing; its report gives the reason without the
/// request's text, and it is not looked at twice.
#[test]
fn a_refused_request_runs_nothing_and_is_reported_without_its_text() -> TestResult {
    let env = Env::new()?;
    let cases: [(&str, serde_json::Value, &str); 6] = [
        (
            "target",
            "-oProxyCommand=evil".into(),
            "the new host must be",
        ),
        ("target", "admin@new.example".into(), "reached as root"),
        ("ssh_port", 0.into(), "the SSH port"),
        (
            "public_url",
            "https://new.example/evil".into(),
            "the public URL",
        ),
        (
            "data_disks",
            serde_json::json!(["/etc/evil"]),
            "a data disk",
        ),
        ("requested_at", 5.into(), "stale"),
    ];
    for (n, (field, value, why)) in cases.into_iter().enumerate() {
        let id = format!("0123abcd-bad-{n}");
        let mut bad = serde_json::to_value(plan(&id))?;
        bad[field] = value;
        env.raw_request(&serde_json::to_vec(&bad)?)?;
        env.run()?;
        let said = env.said()?;
        assert_eq!(said.id, id, "{field}");
        assert_eq!(said.state, MoveHelperState::Refused, "{field}");
        let error = said.error.clone().unwrap_or_default();
        assert!(error.starts_with("The request was refused: "), "{error}");
        assert!(error.contains(why), "{field}: {error}");
        assert!(!error.contains("evil"), "{error}");
        // The report echoes no value of a request that was not accepted.
        assert_eq!(said.values, Values::default(), "{field}");
        assert_eq!(said.exit_code, None);
        // Not acted on again, and its report stays.
        std::fs::remove_file(env.args.state_dir.join(HELPER_STATUS_FILE))?;
        env.run()?;
        assert!(env.status().is_none(), "{field}: looked at twice");
    }
    assert_eq!(env.runs(), 0);
    Ok(())
}

/// MVB-003: a file that is not the service user's own regular file, or too large, or that names
/// no usable id, is not read into a report and runs nothing.
#[test]
fn a_file_that_is_not_the_service_users_request_is_ignored() -> TestResult {
    let env = Env::new()?;
    let dir = env.args.data_dir.join(REQUEST_DIR);
    let path = dir.join(REQUEST_FILE);

    // A link to a valid request elsewhere.
    env.request(&plan("0123abcd-link"))?;
    let elsewhere = env.args.data_dir.join("elsewhere.json");
    std::fs::rename(&path, &elsewhere)?;
    symlink(&elsewhere, &path)?;
    env.run()?;
    assert!(env.status().is_none(), "a link was read");
    std::fs::remove_file(&path)?;

    // Larger than 16 KiB.
    let mut big = serde_json::to_value(plan("0123abcd-big"))?;
    big["id"] = format!("0123abcd-{}", "x".repeat(MAX_REQUEST_BYTES as usize)).into();
    env.raw_request(&serde_json::to_vec(&big)?)?;
    env.run()?;
    assert!(env.status().is_none(), "an oversized file was read");

    // Not JSON, and JSON without a usable id.
    for bytes in [&b"not json"[..], br#"{"id":"../x","action":"plan"}"#] {
        env.raw_request(bytes)?;
        env.run()?;
        assert!(env.status().is_none());
    }

    // Owned by someone else than the data directory's owner (needs root to arrange).
    if nix::unistd::getuid().is_root() {
        env.request(&plan("0123abcd-root"))?;
        nix::unistd::chown(&path, Some(nix::unistd::Uid::from_raw(0)), None)?;
        env.run()?;
        assert!(env.status().is_none(), "root's own file was read");
        // And a data directory that is root's is nobody's to ask from, not even with a request
        // that is root's too.
        env.request(&plan("0123abcd-own"))?;
        nix::unistd::chown(
            &env.args.data_dir,
            Some(nix::unistd::Uid::from_raw(0)),
            None,
        )?;
        nix::unistd::chown(&path, Some(nix::unistd::Uid::from_raw(0)), None)?;
        env.run()?;
        assert!(
            env.status().is_none(),
            "a root-owned data directory was trusted"
        );
    }
    assert_eq!(env.runs(), 0);
    Ok(())
}

/// MVB-003: never while an update runs. A oneshot unit whose command runs is `activating`.
#[test]
fn nothing_runs_while_an_update_runs() -> TestResult {
    let env = Env::new()?;
    for (n, state) in ["activating", "active", "reloading"]
        .into_iter()
        .enumerate()
    {
        std::fs::write(env.bin.join("update-state"), state)?;
        for (request, kind) in [
            (plan(&format!("0123abcd-plan-{n}")), "plan"),
            (abort(&format!("0123abcd-abort-{n}")), "abort"),
        ] {
            env.request(&request)?;
            env.run()?;
            let said = env.said()?;
            assert_eq!(said.state, MoveHelperState::Refused, "{state} {kind}");
            assert_eq!(
                said.error.as_deref(),
                Some(
                    "The request was refused: an update is running (yqa-test-update.service): \
                     ask again once it has finished."
                ),
                "{state} {kind}"
            );
        }
    }
    assert_eq!(env.runs(), 0);
    let asked = std::fs::read_to_string(env.bin.join("systemctl.log"))?;
    assert!(
        asked
            .lines()
            .all(|line| line == "is-active yqa-test-update.service"),
        "{asked}"
    );

    for state in ["inactive", "failed"] {
        std::fs::write(env.bin.join("update-state"), state)?;
        env.request(&plan(&format!("0123abcd-{state}")))?;
        env.run()?;
        assert_eq!(env.said()?.state, MoveHelperState::Ended, "{state}");
    }
    assert_eq!(env.runs(), 2);

    // A systemd that cannot be asked does not say that no update runs.
    let mut blind = Env::new()?;
    blind.args.systemctl = blind.bin.join("missing");
    blind.request(&plan("0123abcd-blind"))?;
    blind.run()?;
    let said = blind.said()?;
    assert_eq!(said.state, MoveHelperState::Refused);
    assert!(
        said.error
            .unwrap_or_default()
            .contains("cannot ask systemd")
    );
    assert_eq!(blind.runs(), 0);
    Ok(())
}

/// MVB-003: the root-owned unit may list the only hosts a request may name.
#[test]
fn only_listed_hosts_when_the_unit_lists_them() -> TestResult {
    let mut env = Env::new()?;
    env.args.targets = vec![" a.example ".into(), String::new(), "10.0.0.7".into()];
    env.request(&plan("0123abcd-other"))?;
    env.run()?;
    let said = env.said()?;
    assert_eq!(said.state, MoveHelperState::Refused);
    assert_eq!(
        said.error.as_deref(),
        Some(
            "The request was refused: this unit allows a move only to the hosts of \
             YULIUS_MIGRATE_TARGETS, and the new host is not one of them."
        )
    );
    assert_eq!(env.runs(), 0);

    for (n, target) in ["root@a.example", "a.example", "root@10.0.0.7"]
        .into_iter()
        .enumerate()
    {
        env.request(&MoveRequest {
            target: Some(target.into()),
            ..plan(&format!("0123abcd-listed-{n}"))
        })?;
        env.run()?;
        assert_eq!(env.said()?.state, MoveHelperState::Ended, "{target}");
    }
    assert_eq!(env.runs(), 3);
    // An abort names no host: it goes back to this one.
    env.request(&abort("0123abcd-abort"))?;
    env.run()?;
    assert_eq!(env.said()?.state, MoveHelperState::Ended);
    assert_eq!(env.runs(), 4);
    Ok(())
}

/// MVB-005: while the command runs the report says so and its time advances; then how it ended.
#[test]
fn a_running_command_is_reported_with_a_heartbeat_and_then_its_exit_code() -> TestResult {
    let env = Env::new()?;
    env.does(r#"while [ ! -f "$d/go" ]; do sleep 0.02; done; exit 1"#)?;
    env.request(&plan("0123abcd-slow"))?;
    let helper = env.helper();
    let run = std::thread::spawn(move || helper.run());
    until("the command runs", || env.runs() == 1)?;
    until("the report says running", || {
        env.status()
            .is_some_and(|s| s.state == MoveHelperState::Running)
    })?;
    let first = env.said()?;
    assert_eq!((first.exit_code, first.ended_at), (None, None));
    until("the heartbeat advances", || {
        env.status()
            .is_some_and(|s| s.updated_at > first.updated_at)
    })?;
    assert_eq!(env.said()?.state, MoveHelperState::Running);

    std::fs::write(env.bin.join("go"), "")?;
    ended(run, &env)?;
    let said = env.said()?;
    assert_eq!(said.state, MoveHelperState::Ended);
    assert_eq!(said.exit_code, Some(1));
    assert_eq!(
        said.error, None,
        "a failed step is in the checklist, not here"
    );
    assert_eq!(said.started_at, first.started_at);
    Ok(())
}

/// MVB-006: an abort that arrives while a command runs ends it by SIGTERM and is handled after
/// it; another request meanwhile, or the same one written again, stops nothing.
#[test]
fn an_abort_stops_the_run_and_is_handled_after_it() -> TestResult {
    let env = Env::new()?;
    // The move: runs until it is told to stop, as the command does before it freezes.
    env.does(
        r#"case "$*" in
  *--abort*) echo aborted >> "$d/order"; exit 0 ;;
esac
trap 'echo terminated >> "$d/order"; exit 1' TERM
while :; do sleep 0.02; done"#,
    )?;
    let moved = MoveRequest {
        action: MoveAction::Move,
        ..plan("0123abcd-move")
    };
    env.request(&moved)?;
    let helper = env.helper();
    let run = std::thread::spawn(move || helper.run());
    until("the move runs", || {
        env.status()
            .is_some_and(|s| s.state == MoveHelperState::Running)
    })?;

    // The same request written again, another plan, an unreadable file: the move runs on.
    let path = env.args.data_dir.join(REQUEST_DIR).join(REQUEST_FILE);
    env.request(&moved)?;
    std::thread::sleep(Duration::from_millis(80));
    env.request(&plan("0123abcd-other-plan"))?;
    std::thread::sleep(Duration::from_millis(80));
    env.raw_request(b"not json")?;
    std::thread::sleep(Duration::from_millis(80));
    assert!(!env.bin.join("order").exists(), "the move was stopped");
    assert_eq!(env.said()?.id, "0123abcd-move");

    env.request(&abort("0123abcd-abort"))?;
    assert!(path.exists());
    ended(run, &env)?;
    assert_eq!(
        std::fs::read_to_string(env.bin.join("order"))?,
        "terminated\naborted\n"
    );
    assert_eq!(env.runs(), 2);
    let said = env.said()?;
    assert_eq!(said.id, "0123abcd-abort");
    assert_eq!(said.action, MoveAction::Abort);
    assert_eq!(said.state, MoveHelperState::Ended);
    assert_eq!(said.exit_code, Some(0));
    assert_eq!(
        env.argv(),
        command_args(&env.args, &abort("0123abcd-abort"))
    );
    Ok(())
}

/// MVB-006: an abort that runs is not stopped by its own request, which stays in its file
/// until the server removes it.
#[test]
fn an_abort_that_runs_is_not_stopped_by_its_own_request() -> TestResult {
    let env = Env::new()?;
    env.does(
        r#"trap 'echo terminated >> "$d/order"; exit 1' TERM
while [ ! -f "$d/go" ]; do sleep 0.02; done
exit 0"#,
    )?;
    env.request(&abort("0123abcd-abort"))?;
    let helper = env.helper();
    let run = std::thread::spawn(move || helper.run());
    until("the abort runs", || {
        env.status()
            .is_some_and(|s| s.state == MoveHelperState::Running)
    })?;
    // Its file is looked at meanwhile: a heartbeat comes after a look.
    for _ in 0..2 {
        let before = env.said()?.updated_at;
        until("the heartbeat advances", || {
            env.status().is_some_and(|s| s.updated_at > before)
        })?;
    }
    std::fs::write(env.bin.join("go"), "")?;
    ended(run, &env)?;
    assert!(!env.bin.join("order").exists(), "the abort was stopped");
    let said = env.said()?;
    assert_eq!(
        (said.id.as_str(), said.exit_code),
        ("0123abcd-abort", Some(0))
    );
    assert_eq!(env.runs(), 1);
    Ok(())
}

/// MVB-006: a command that ignores the signal (the new instance's start is ordered) ends by
/// itself; the helper waits for it and only then handles the abort.
#[test]
fn a_command_that_ignores_the_signal_ends_by_itself_first() -> TestResult {
    let env = Env::new()?;
    env.does(
        r#"case "$*" in
  *--abort*) echo aborted >> "$d/order"; exit 0 ;;
esac
trap 'echo ignored >> "$d/order"' TERM
while [ ! -f "$d/go" ]; do sleep 0.02; done
echo moved >> "$d/order"; exit 0"#,
    )?;
    env.request(&MoveRequest {
        action: MoveAction::Move,
        ..plan("0123abcd-move")
    })?;
    let helper = env.helper();
    let run = std::thread::spawn(move || helper.run());
    until("the move runs", || {
        env.status()
            .is_some_and(|s| s.state == MoveHelperState::Running)
    })?;
    env.request(&abort("0123abcd-abort"))?;
    until("the signal arrived", || env.bin.join("order").exists())?;
    assert_eq!(env.said()?.id, "0123abcd-move", "the abort did not wait");
    assert_eq!(env.said()?.state, MoveHelperState::Running);
    std::fs::write(env.bin.join("go"), "")?;
    ended(run, &env)?;
    let order = std::fs::read_to_string(env.bin.join("order"))?;
    // Sent once, however long the command takes.
    assert_eq!(order, "ignored\nmoved\naborted\n");
    Ok(())
}

/// MVB-006: a stop of the helper itself (systemd stops the unit) is passed on to the command,
/// and the helper ends after it with its report written.
#[test]
fn a_stop_of_the_helper_ends_the_command_first() -> TestResult {
    let env = Env::new()?;
    env.does(
        r#"trap 'echo terminated >> "$d/order"; exit 1' TERM
while :; do sleep 0.02; done"#,
    )?;
    env.request(&plan("0123abcd-plan"))?;
    let mut helper = env.helper();
    let stop = Arc::new(AtomicBool::new(false));
    helper.stop = Arc::clone(&stop);
    let run = std::thread::spawn(move || helper.run());
    until("the command runs", || {
        env.status()
            .is_some_and(|s| s.state == MoveHelperState::Running)
    })?;
    // A request that waits behind the command is not taken by a helper that was stopped.
    env.request(&plan("0123abcd-next"))?;
    stop.store(true, Ordering::SeqCst);
    ended(run, &env)?;
    assert_eq!(
        std::fs::read_to_string(env.bin.join("order"))?,
        "terminated\n"
    );
    let said = env.said()?;
    assert_eq!(said.id, "0123abcd-plan");
    assert_eq!(said.state, MoveHelperState::Ended);
    assert_eq!(said.exit_code, Some(1));
    assert_eq!(env.runs(), 1);
    Ok(())
}

/// MVB-005: a command that refuses before its checklist says why on standard error; a signal
/// leaves no exit code; a program that cannot be started is said.
#[test]
fn a_command_that_ends_before_its_checklist_is_reported_with_its_words() -> TestResult {
    let env = Env::new()?;
    env.does(
        r#"echo "Warning: Permanently added 'new.example' to the list of known hosts." >&2
echo "yulius instance migrate: a move to root@old.example is recorded here: finish it with the same command" >&2
exit 2"#,
    )?;
    env.request(&plan("0123abcd-refused"))?;
    env.run()?;
    let said = env.said()?;
    assert_eq!(
        (said.state, said.exit_code),
        (MoveHelperState::Ended, Some(2))
    );
    assert_eq!(
        said.error.as_deref(),
        Some("a move to root@old.example is recorded here: finish it with the same command")
    );

    // Exit 2 with other words only: nothing is made up.
    env.does(r#"echo "something else" >&2; exit 2"#)?;
    env.request(&plan("0123abcd-quiet"))?;
    env.run()?;
    assert_eq!(env.said()?.error, None);

    // A step that failed (exit 1), and a move that went on after a signal (exit 0), are the
    // checklist's to tell: what the command said meanwhile is no error then.
    for code in [1, 0] {
        env.does(&format!(
            r#"echo "yulius instance migrate: signal 15 ignored: the move goes on" >&2; exit {code}"#
        ))?;
        env.request(&plan(&format!("0123abcd-exit-{code}")))?;
        env.run()?;
        let said = env.said()?;
        assert_eq!((said.exit_code, said.error), (Some(code), None));
    }

    // Words that carry control characters reach the report without them.
    env.does(r#"printf 'yulius instance migrate: a\033[31mb\tc\n' >&2; exit 2"#)?;
    env.request(&plan("0123abcd-control"))?;
    env.run()?;
    assert_eq!(env.said()?.error.as_deref(), Some("a [31mb c"));

    env.does(r#"kill -KILL $$"#)?;
    env.request(&plan("0123abcd-killed"))?;
    env.run()?;
    let said = env.said()?;
    assert_eq!((said.state, said.exit_code), (MoveHelperState::Ended, None));

    let mut missing = Env::new()?;
    missing.args.program = Some(missing.bin.join("missing"));
    missing.request(&plan("0123abcd-missing"))?;
    missing.run()?;
    let said = missing.said()?;
    assert_eq!((said.state, said.exit_code), (MoveHelperState::Ended, None));
    assert!(
        said.error
            .unwrap_or_default()
            .starts_with("the command could not be started: "),
    );
    Ok(())
}

/// A command whose child keeps its error output open (an `ssh` step still running on the new
/// host after the command died) does not hold the helper.
#[test]
fn a_child_that_outlives_the_command_does_not_hold_the_helper() -> TestResult {
    let env = Env::new()?;
    env.does(r#"(sleep 6 & echo $! > "$d/orphan.pid") ; exit 0"#)?;
    env.request(&plan("0123abcd-orphan"))?;
    let started = Instant::now();
    env.run()?;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(env.said()?.exit_code, Some(0));
    // The child is ended with the test: left alone it holds the test run's output open.
    let orphan: i32 = std::fs::read_to_string(env.bin.join("orphan.pid"))?
        .trim()
        .parse()?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(orphan),
        nix::sys::signal::Signal::SIGKILL,
    )?;
    Ok(())
}

/// MVB-007: the units the repository ships start this helper, as root, on the request file the
/// server writes, with no bound on how long a move takes.
#[test]
fn the_shipped_units_start_the_helper_on_the_request_file() {
    let path = include_str!("../../../../deploy/systemd/yulius-migrate.path");
    let service = include_str!("../../../../deploy/systemd/yulius-migrate.service");
    let has = |text: &str, line: &str| text.lines().any(|l| l.trim() == line);
    assert!(has(
        path,
        &format!("PathChanged=/var/lib/yulius/{REQUEST_DIR}/{REQUEST_FILE}")
    ));
    assert!(has(path, "Unit=yulius-migrate.service"));
    assert!(has(path, "WantedBy=multi-user.target"));
    for line in [
        "Type=oneshot",
        "User=root",
        "ExecStart=/usr/local/bin/yulius migrate-helper",
        "Environment=YULIUS_DATA=/var/lib/yulius",
        "Environment=YULIUS_MIGRATE_STATE=/var/lib/yulius-migrate",
        "Environment=YULIUS_MIGRATE_SERVICE=yulius.service",
        "Environment=YULIUS_MIGRATE_UPDATE_SERVICE=yulius-update.service",
        "TimeoutStartSec=infinity",
        "TimeoutStopSec=10min",
    ] {
        assert!(has(service, line), "{line}");
    }
    // Requests that come quickly do not switch the helper off: with systemd's default start
    // limit (5 in 10 s) a sixth fails the path unit, which then watches nothing (seen on a
    // real host).
    let unit_section = service.split("[Service]").next().unwrap_or_default();
    assert!(has(unit_section, "StartLimitIntervalSec=0"));
    // A stop of the unit reaches the helper alone, which tells the command; systemd's default
    // (every process of the unit at once) would end the step in flight under a command that
    // has to finish a cutover it began.
    assert!(has(service, "KillMode=mixed"));
    // The command reads the whole host and writes the proxy's configuration: nothing here
    // may hide the file system from it, and no list of targets is set for everyone.
    for line in service.lines().map(str::trim) {
        assert!(
            !line.starts_with("ProtectSystem=")
                && !line.starts_with("PrivateTmp=")
                && !line.starts_with("Environment=YULIUS_MIGRATE_TARGETS"),
            "{line}"
        );
    }
    // Its name is the server unit's, as the command derives it to carry it.
    assert_eq!(
        yulius_sandbox::units::migrate_unit("yulius.service").as_deref(),
        Some("yulius-migrate.service")
    );
    assert_eq!(
        yulius_sandbox::units::migrate_path_unit("yulius.service").as_deref(),
        Some("yulius-migrate.path")
    );
    assert_eq!(yulius_sandbox::units::migrate_path_unit("yulius"), None);
}
