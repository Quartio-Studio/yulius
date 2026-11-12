use std::path::Path;

use crate::instance::migrate::remote::*;
use crate::instance::migrate::target::Reply;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_destination_is_a_user_and_a_host_and_never_an_option() {
    for good in [
        "root@185.189.47.160",
        "new-host.example.com",
        "root@[2001:db8::1]",
        "deploy@new_host",
    ] {
        assert!(destination_ok(good), "{good}");
    }
    for bad in [
        "",
        "-oProxyCommand=sh",
        "root@-oProxyCommand=sh",
        "root@host;reboot",
        "root@host name",
        "a@b@c",
        "@host",
        "root@",
        "$(id)@host",
    ] {
        assert!(!destination_ok(bad), "{bad}");
    }
    assert_eq!(host_part("root@185.189.47.160"), "185.189.47.160");
    assert_eq!(host_part("new-host"), "new-host");
}

#[test]
fn every_remote_command_is_a_constant_and_values_stay_out_of_it() {
    let ssh = Ssh::new(
        "root@new-host",
        2222,
        Path::new("/var/lib/yulius-migrate"),
        "yulius-migrate-ab12",
    );
    let args = ssh.args(true, STEP);
    // The destination and the command come last, after the end of options.
    let n = args.len();
    assert_eq!(args[n - 3..], ["--", "root@new-host", STEP]);
    assert_eq!(args[..2], ["-p", "2222"]);
    for option in [
        "BatchMode=yes",
        "IdentitiesOnly=yes",
        "StrictHostKeyChecking=accept-new",
        "UserKnownHostsFile=/var/lib/yulius-migrate/known_hosts",
    ] {
        assert!(
            args.iter().any(|a| a == option),
            "{option} missing: {args:?}"
        );
    }
    // Every call is a connection of its own, whatever root's ssh configuration says: no
    // shared master outlives the key.
    for option in ["ControlMaster=no", "ControlPath=none"] {
        assert!(args.iter().any(|a| a == option), "{args:?}");
    }
    // The key lands on a line of its own, also after a file without a final newline.
    assert!(ADD_KEY.contains("{ echo; cat; } >> ~/.ssh/authorized_keys"));
    let key = args
        .iter()
        .position(|a| a == "-i")
        .map(|i| args[i + 1].as_str());
    assert_eq!(key, Some("/var/lib/yulius-migrate/key"));
    // The operator's own sign-in: no key, no batch mode, so ssh may ask.
    let own = ssh.args(false, ADD_KEY);
    assert!(
        !own.iter().any(|a| a == "-i" || a == "BatchMode=yes"),
        "{own:?}"
    );
    assert_eq!(own.last().map(String::as_str), Some(ADD_KEY));
    // Nothing a request could name is in a command.
    for command in [PUT_BUILD, STEP, RECEIVE, UNTAR, ADD_KEY, SINK, HELLO] {
        assert!(
            !command.contains("{}") && !command.contains('$') && !command.contains('%'),
            "{command}"
        );
    }
}

#[test]
fn the_reply_is_the_last_marked_line_of_a_steps_output() {
    let output = "Reading package lists...\nYULIUS-REPLY {\"lines\":[]}\nSetting up caddy\n\
                  YULIUS-REPLY {\"lines\":[{\"name\":\"packages\",\"outcome\":\"ok\",\"detail\":\"d\"}]}\n";
    let reply = parse_reply(output).unwrap_or_default();
    assert_eq!(reply.lines.len(), 1);
    assert_eq!(reply.lines[0].name, "packages");
    assert_eq!(parse_reply("apt said things\n"), None);
    assert_eq!(parse_reply("YULIUS-REPLY not json\n"), None);
    assert_eq!(parse_reply("YULIUS-REPLY {}\n"), Some(Reply::default()));
}

#[test]
fn a_connection_without_its_key_says_so_and_a_forgotten_key_is_such_a_one() -> TestResult {
    // The wording a move's finish tells "deleted here" by (SPEC-467 LFT-004); `ssh` is not run.
    let dir = tempfile::tempdir()?;
    let mut ssh = Ssh::new("root@new-host", 22, dir.path(), "yulius-migrate-ab12");
    assert!(ssh.reachable().is_err_and(|why| why.contains(NO_KEY)));
    // What a finish leaves: no key, no public half, no known hosts.
    for name in ["key", "key.pub", "known_hosts"] {
        std::fs::write(dir.path().join(name), "x")?;
    }
    ssh.forget();
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    assert!(ssh.reachable().is_err_and(|why| why.contains(NO_KEY)));
    assert!(!NO_KEY.contains(KEY_REFUSED));
    Ok(())
}
