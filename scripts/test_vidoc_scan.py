"""The Vidoc security gate (SPEC-128): settings, the repository, the CLI run and the verdict."""
import contextlib
import http.server
import io
import json
import os
import signal
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest

import vidoc_scan
from vidoc_scan import Failed, Usage

SCAN_ID = "019f8a10-9e8d-7c6b-a5f4-3e2d1c0b9a87"
CODEBASE = "019f8a1b-0c2d-73e4-a5f6-7b8c9d0e1f2a"
BRANCH = "feat/109-vidoc"
TOKEN = "yulius-secret-VIDOC_API_KEY"
MERGE_BASE = "b0a35287149d5936984176bb1190dc3e8dca9f9a"


def scan_record(**changes):
    record = {"id": SCAN_ID, "branch": BRANCH, "status": "completed", "codebaseId": CODEBASE}
    record.update(changes)
    return record


def issues_page(issues=(), total=None):
    return {"issues": list(issues), "total": len(issues) if total is None else total,
            "limit": 100, "offset": 0}


ISSUE = {"severity": "high", "name": "Command injection in hook runner",
         "path": "crates/yulius-app/src/hooks.rs", "line": 42,
         "url": "https://app.vidoc.dev/projects/p/issues?issue=i"}


class FakeApi:
    """Answers GET paths from a table; records every request."""

    def __init__(self, answers):
        self.answers = answers
        self.requests = []

    def get(self, path, params=None):
        self.requests.append((path, params))
        answer = self.answers[path]
        if callable(answer):
            answer = answer(params)
        if isinstance(answer, Exception):
            raise answer
        return answer


class FakeCli:
    def __init__(self, code=0, output=f"Indexing...\nScan started with ID: {SCAN_ID}\nDone\n"):
        self.code, self.output, self.env = code, output, None

    def __call__(self, env):
        self.env = env
        return self.code, self.output


class FakeGit:
    """A clean branch two commits ahead of origin/dev unless told otherwise."""

    def __init__(self, **answers):
        self.answers = {
            "symbolic-ref": (0, BRANCH + "\n"),
            "status": (0, ""),
            "remote": (0, "origin\n"),
            "show": (0, "* remote origin\n  Fetch URL: https://github.com/o/r.git\n  HEAD branch: dev\n"),
            "ls-remote": (0, "6f2177e7e7259c81f7454baead867cc121577166\tHEAD\n"),
            "fetch": (0, ""),
            "merge-base": (0, MERGE_BASE + "\n"),
            "diff": (0, "scripts/vidoc_scan.py\ndocs/changes.md\n"),
        }
        self.answers.update(answers)
        self.calls, self.envs = [], {}

    def __call__(self, *args, env=None):
        self.calls.append(args)
        self.envs[args] = env
        rest = list(args)
        while rest[0] == "-c":
            rest = rest[2:]
        key = "show" if rest[:2] == ["remote", "show"] else rest[0]
        return self.answers[key]


def api_for(scan=None, issues=None):
    return FakeApi({
        "/v1/codebase": {"codebases": []},
        f"/v1/scan-workflows/{SCAN_ID}/status": scan if scan is not None else scan_record(),
        "/v1/issues": issues if issues is not None else issues_page(),
    })


def run(api=None, cli=None, git=None, env=None):
    out = io.StringIO()
    env = {"VIDOC_API_KEY": TOKEN} if env is None else env
    code = vidoc_scan.gate(env, git or FakeGit(), api or api_for(), cli or FakeCli(),
                           Path("/tmp/vidoc.log"), out)
    return code, out.getvalue()


class Switch(unittest.TestCase):
    """SPEC-131: `.config/vidoc.toml` turns the gate on or off; it ships on."""

    def root_with(self, text=None):
        root = Path(tempfile.mkdtemp(prefix="vidoc-switch-"))
        self.addCleanup(shutil.rmtree, root)
        if text is not None:
            (root / ".config").mkdir()
            (root / ".config" / "vidoc.toml").write_text(text)
        return root

    def test_the_repository_ships_the_gate_enabled(self):
        self.assertTrue(vidoc_scan.switch(Path(__file__).resolve().parent.parent))

    def test_on_off_and_a_missing_file_which_means_on(self):
        self.assertTrue(vidoc_scan.switch(self.root_with("enabled = true\n")))
        self.assertFalse(vidoc_scan.switch(self.root_with("# off for now\nenabled = false\n")))
        self.assertTrue(vidoc_scan.switch(self.root_with()))

    def test_a_malformed_switch_is_not_run_and_names_the_file(self):
        for text in ("enabled = \n", 'enabled = "false"\n', "enabled = 0\n", "# nothing\n",
                     "enabled = true\nscan = false\n", "[vidoc]\nenabled = false\n"):
            with self.subTest(text=text), self.assertRaises(Usage) as caught:
                vidoc_scan.switch(self.root_with(text))
            self.assertIn(".config/vidoc.toml", str(caught.exception))

    def run_main(self, root):
        previous = {s: signal.getsignal(s) for s in (signal.SIGTERM, signal.SIGHUP)}
        self.addCleanup(lambda: [signal.signal(s, h) for s, h in previous.items()])
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = vidoc_scan.main(root=root, env={"PATH": os.environ.get("PATH", "")})
        return code, out.getvalue(), err.getvalue()

    def committed(self, text):
        root = self.root_with(text)
        env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
        for args in (["init", "-q"], ["add", "-A"],
                     ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "switch"]):
            subprocess.run(["git", *args], cwd=root, env=env, check=True, capture_output=True)
        return root

    def test_only_a_committed_switch_turns_the_gate_off(self):
        vidoc_scan.committed_switch(FakeGit(status=(0, "")))
        for status in ((0, " M .config/vidoc.toml\n"), (0, "?? .config/vidoc.toml\n"),
                       (128, "")):
            with self.subTest(status=status), self.assertRaises(Usage) as caught:
                vidoc_scan.committed_switch(FakeGit(status=status))
            self.assertIn(".config/vidoc.toml", str(caught.exception))

    def test_an_uncommitted_off_switch_is_not_run(self):
        root = self.committed("enabled = true\n")
        (root / ".config" / "vidoc.toml").write_text("enabled = false\n")
        code, out, err = self.run_main(root)
        self.assertEqual(code, 2)
        self.assertNotIn("disabled", out)
        self.assertIn("uncommitted", err)

    def test_disabled_passes_at_once_without_a_key_and_says_nothing_was_scanned(self):
        code, out, err = self.run_main(self.committed("enabled = false\n"))
        self.assertEqual(code, 0)
        self.assertIn("Security scan disabled: .config/vidoc.toml sets enabled = false; "
                      "nothing was scanned.", out)
        self.assertEqual(err, "")

    def test_enabled_still_needs_a_key(self):
        code, _, err = self.run_main(self.root_with("enabled = true\n"))
        self.assertEqual(code, 2)
        self.assertIn("No Vidoc API key", err)

    def test_a_malformed_switch_exits_2_from_main(self):
        code, _, err = self.run_main(self.root_with('enabled = "no"\n'))
        self.assertEqual(code, 2)
        self.assertIn("Security scan not run: .config/vidoc.toml", err)


class Settings(unittest.TestCase):
    def test_token_prefers_vidoc_token_then_the_agent_secret(self):
        self.assertEqual(vidoc_scan.settings({"VIDOC_TOKEN": "a", "VIDOC_API_KEY": "b"})[0], "a")
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": "b"})[0], "b")

    def test_missing_key_is_a_usage_error_that_says_how_to_add_one(self):
        with self.assertRaises(Usage) as caught:
            vidoc_scan.settings({"VIDOC_TOKEN": "  "})
        self.assertIn("VIDOC_API_KEY", str(caught.exception))
        self.assertIn("api.vidoc.dev", str(caught.exception))

    def test_a_key_with_whitespace_or_control_characters_is_refused_without_echoing_it(self):
        for key in ["abc def", "abc\ndef", "abc\x00def"]:
            with self.subTest(key=key), self.assertRaises(Usage) as caught:
                vidoc_scan.settings({"VIDOC_TOKEN": key})
            self.assertNotIn("def", str(caught.exception))

    def test_api_defaults_to_vidoc_cloud_and_drops_a_trailing_slash(self):
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": "k"})[1], "https://api.vidoc.dev")
        self.assertEqual(
            vidoc_scan.settings({"VIDOC_API_KEY": "k", "VIDOC_API_URL": "https://api.example.com/"})[1],
            "https://api.example.com")

    def test_api_must_be_https_without_a_path(self):
        for url in ["http://api.vidoc.dev", "https://api.vidoc.dev/v1", "api.vidoc.dev", "https://"]:
            with self.subTest(url=url), self.assertRaises(Usage):
                vidoc_scan.settings({"VIDOC_API_KEY": "k", "VIDOC_API_URL": url})

    def test_node_major_version(self):
        self.assertEqual(vidoc_scan.node_major("v24.21.0\n"), 24)
        self.assertIsNone(vidoc_scan.node_major("command not found"))

    def test_node_20_and_npm_are_required(self):
        def which(found):
            return lambda name: f"/usr/bin/{name}" if name in found else None
        vidoc_scan.require_node(which({"node", "npm"}), lambda node: "v24.21.0\n")
        for found, version in [({"node", "npm"}, "v18.19.0\n"), ({"npm"}, ""), ({"node"}, "v24.1.0\n")]:
            with self.subTest(found=found, version=version), self.assertRaises(Usage):
                vidoc_scan.require_node(which(found), lambda node, v=version: v)


class ScanId(unittest.TestCase):
    def test_reads_the_one_scan_id(self):
        self.assertEqual(vidoc_scan.parse_scan_id(f"x\nScan started with ID: {SCAN_ID}\n"), SCAN_ID)

    def test_none_without_or_with_two_different_ids(self):
        self.assertIsNone(vidoc_scan.parse_scan_id("Scan started with ID: not-a-uuid"))
        other = "019f8a10-9e8d-7c6b-a5f4-3e2d1c0b9a88"
        self.assertIsNone(vidoc_scan.parse_scan_id(
            f"Scan started with ID: {SCAN_ID}\nScan started with ID: {other}\n"))


class CliEnvironment(unittest.TestCase):
    def test_passes_only_what_the_cli_and_git_need_plus_the_vidoc_settings(self):
        env = vidoc_scan.cli_env({
            "PATH": "/bin", "HOME": "/h", "HTTPS_PROXY": "http://p", "NODE_EXTRA_CA_CERTS": "/ca",
            "GIT_CONFIG_COUNT": "1", "YULIUS_TASK_TOKEN": "t", "YULIUS_URL": "http://y",
            "SSH_AUTH_SOCK": "/s", "XDG_CONFIG_HOME": "/x", "VIDOC_PROFILE": "x", "GITHUB_TOKEN": "g", "GITHUB_EVENT_PATH": "/e", "DEBUG": "1",
            "ANTHROPIC_API_KEY": "a"}, "tok", "https://a.b", Path("/tmp/l/vidoc.log"))
        self.assertEqual(env, {
            "PATH": "/bin", "HOME": "/h", "HTTPS_PROXY": "http://p", "NODE_EXTRA_CA_CERTS": "/ca",
            "GIT_CONFIG_COUNT": "1", "YULIUS_TASK_TOKEN": "t", "YULIUS_URL": "http://y",
            "SSH_AUTH_SOCK": "/s", "XDG_CONFIG_HOME": "/x", "VIDOC_TOKEN": "tok", "VIDOC_API_URL": "https://a.b", "CI": "true",
            "VIDOC_LOG_FILE": "/tmp/l/vidoc.log"})


class Install(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        (self.dir / "package.json").write_text(json.dumps(
            {"dependencies": {"@vidocsecurity/cli": "0.1.25"}}))
        (self.dir / "package-lock.json").write_text('{"lockfileVersion": 3}')

    def installed(self, version, stamp=True):
        package = self.dir / "node_modules/@vidocsecurity/cli"
        package.mkdir(parents=True, exist_ok=True)
        (package / "package.json").write_text(json.dumps({"version": version}))
        (self.dir / "node_modules/.bin").mkdir(exist_ok=True)
        (self.dir / "node_modules/.bin/vidoc").write_text("")
        if stamp:
            vidoc_scan.mark_installed(self.dir)

    def test_installs_when_missing_or_another_version(self):
        self.assertTrue(vidoc_scan.needs_install(self.dir))
        self.installed("0.1.21")
        self.assertTrue(vidoc_scan.needs_install(self.dir))

    def test_keeps_the_pinned_version(self):
        self.installed("0.1.25")
        self.assertFalse(vidoc_scan.needs_install(self.dir))

    def test_reinstalls_after_the_lockfile_changed_or_an_unmarked_install(self):
        self.installed("0.1.25", stamp=False)
        self.assertTrue(vidoc_scan.needs_install(self.dir))
        vidoc_scan.mark_installed(self.dir)
        (self.dir / "package-lock.json").write_text('{"lockfileVersion": 3, "x": 1}')
        self.assertTrue(vidoc_scan.needs_install(self.dir))

    def test_the_repository_pins_an_exact_version_with_integrity(self):
        tool = Path(__file__).resolve().parent.parent / "tools/vidoc"
        pin = json.loads((tool / "package.json").read_text())["dependencies"]["@vidocsecurity/cli"]
        self.assertRegex(pin, r"^\d+\.\d+\.\d+$")
        lock = json.loads((tool / "package-lock.json").read_text())
        locked = lock["packages"]["node_modules/@vidocsecurity/cli"]
        self.assertEqual(locked["version"], pin)
        self.assertTrue(locked["integrity"].startswith("sha512-"))


class Repository(unittest.TestCase):
    def test_a_detached_head_is_not_run(self):
        cli = FakeCli()
        with self.assertRaises(Usage):
            run(cli=cli, git=FakeGit(**{"symbolic-ref": (1, "")}))
        self.assertIsNone(cli.env)

    def test_uncommitted_or_untracked_files_are_not_run_because_the_cli_uploads_them(self):
        git, cli = FakeGit(status=(0, "?? scratch.txt\n")), FakeCli()
        with self.assertRaises(Usage) as caught:
            run(cli=cli, git=git)
        self.assertIn("commit", str(caught.exception))
        self.assertIsNone(cli.env)
        self.assertIn(("status", "--porcelain", "--untracked-files=all"), git.calls)

    def test_an_unknown_default_branch_is_not_run(self):
        cli = FakeCli()
        for show in [(128, ""), (0, "* remote origin\n"), (0, "  HEAD branch: (unknown)\n")]:
            with self.subTest(show=show), self.assertRaises(Usage):
                run(cli=cli, git=FakeGit(show=show))
        self.assertIsNone(cli.env)

    def test_origin_unreachable_the_way_the_cli_checks_it_is_not_run(self):
        git = FakeGit(**{"ls-remote": (128, "")})
        with self.assertRaises(Usage):
            run(git=git)
        check = [c for c in git.calls if "ls-remote" in c][0]
        self.assertEqual(check, ("-c", "credential.interactive=never", "-c", "core.askPass=", "-c",
                                 "core.sshCommand=ssh -oBatchMode=yes -oStrictHostKeyChecking=yes",
                                 "ls-remote", "origin", "HEAD"))
        env = git.envs[check]
        self.assertEqual(env["VIDOC_TOKEN"], TOKEN)
        self.assertEqual(env["GIT_TERMINAL_PROMPT"], "0")

    def test_a_first_remote_other_than_origin_is_not_run(self):
        with self.assertRaises(Usage):
            run(git=FakeGit(remote=(0, "upstream\norigin\n")))

    def test_no_merge_base_with_the_default_branch_is_not_run(self):
        git = FakeGit(**{"merge-base": (1, "")})
        with self.assertRaises(Usage):
            run(git=git)
        self.assertIn(("merge-base", "HEAD", "origin/dev"), git.calls)


class Gate(unittest.TestCase):
    def test_clean_scan_passes_and_names_it(self):
        api, cli = api_for(), FakeCli()
        code, out = run(api, cli)
        self.assertEqual(code, 0)
        self.assertIn(SCAN_ID, out)
        self.assertEqual(cli.env["VIDOC_TOKEN"], TOKEN)
        issue_reads = [params for path, params in api.requests if path == "/v1/issues"]
        self.assertEqual(issue_reads, [
            {"codebaseId": CODEBASE, "branch": BRANCH, "status": "open",
             "severity": "critical,high", "limit": "100"},
            {"codebaseId": CODEBASE, "branch": BRANCH, "status": "open", "limit": "1"}])

    def test_findings_fail_and_are_printed(self):
        code, out = run(api_for(issues=issues_page([ISSUE])))
        self.assertEqual(code, 1)
        self.assertIn("[high] Command injection in hook runner", out)
        # The API's line is 0-based: the CLI prints line + 1.
        self.assertIn("crates/yulius-app/src/hooks.rs:43", out)
        self.assertIn(ISSUE["url"], out)

    def test_findings_wrapped_like_the_cli_endpoint_are_printed_too(self):
        code, out = run(api_for(issues=issues_page([{"issue": ISSUE}])))
        self.assertEqual(code, 1)
        self.assertIn("[high] Command injection in hook runner", out)

    def test_open_critical_or_high_counted_without_the_severity_filter_fail_a_zero_total(self):
        def answer(params):
            if "severity" in params:
                return issues_page()
            page = issues_page(total=3)
            page["stats"] = {"severityCounts": {"critical": 0, "high": 1, "medium": 2}}
            return page
        api = api_for()
        api.answers["/v1/issues"] = answer
        with self.assertRaises(Failed):
            run(api)

    def test_more_findings_than_one_page_still_fail_with_the_total(self):
        code, out = run(api_for(issues=issues_page([ISSUE], total=130)))
        self.assertEqual(code, 1)
        self.assertIn("130", out)

    def test_the_key_is_never_printed(self):
        for api in [api_for(), api_for(issues=issues_page([ISSUE]))]:
            _, out = run(api)
            self.assertNotIn(TOKEN, out)
        for raised, path, status in [(Usage, "/v1/codebase", 401), (Failed, "/v1/issues", 500)]:
            api = api_for()
            api.answers[path] = vidoc_scan.HttpError(status, "no")
            with self.assertRaises(raised) as caught:
                run(api)
            self.assertNotIn(TOKEN, str(caught.exception))

    def test_a_rejected_key_stops_before_the_upload(self):
        api = api_for()
        api.answers["/v1/codebase"] = vidoc_scan.HttpError(401, "Invalid or expired token")
        cli = FakeCli()
        with self.assertRaises(Usage) as caught:
            run(api, cli)
        self.assertIsNone(cli.env)
        self.assertIn("api.vidoc.dev", str(caught.exception))

    def test_a_cli_error_fails(self):
        with self.assertRaises(Failed):
            run(cli=FakeCli(code=1))

    def test_no_scan_id_fails(self):
        with self.assertRaises(Failed):
            run(cli=FakeCli(output="Scan did not complete successfully\n"))

    def test_a_fixed_finding_the_cli_still_lists_does_not_fail_an_open_total_of_zero(self):
        output = f"Scan started with ID: {SCAN_ID}\n[High] SQL injection - src/a.rs:3 [Validated]\n"
        code, _ = run(cli=FakeCli(output=output))
        self.assertEqual(code, 0)

    def test_nothing_to_scan_with_changed_source_files_is_not_run(self):
        output = "No scannable files matched the selected scope in the current directory.\n"
        with self.assertRaises(Usage):
            run(cli=FakeCli(output=output), git=FakeGit(diff=(0, "crates/a/src/lib.rs\n")))

    def test_nothing_to_scan_passes_when_sources_changed_only_where_the_cli_never_looks(self):
        output = "No scannable files matched the selected scope in the current directory.\n"
        changed = "crates/yulius-ptyhost/src/bin/yulius-ptyhost.rs\nweb/assets/vendor/monaco/x.js\n"
        git = FakeGit(diff=(0, changed))
        code, _ = run(cli=FakeCli(output=output), git=git)
        self.assertEqual(code, 0)
        diff = [c for c in git.calls if c[0] == "diff"][0]
        self.assertIn("--diff-filter=d", diff)

    def test_the_base_is_fetched_before_its_merge_base_like_the_cli_does(self):
        git = FakeGit()
        run(git=git)
        self.assertLess(git.calls.index(("fetch", "--no-tags", "origin", "dev")),
                        git.calls.index(("merge-base", "HEAD", "origin/dev")))
        with self.assertRaises(Usage):
            run(git=FakeGit(fetch=(128, "")))

    def test_vidoc_unreachable_or_out_of_quota_before_the_scan_is_not_run(self):
        for status in [0, 402, 429, 503]:
            api = api_for()
            api.answers["/v1/codebase"] = vidoc_scan.HttpError(status, "down")
            with self.subTest(status=status), self.assertRaises(Usage):
                run(api)

    def test_nothing_to_scan_passes_and_names_the_changed_files(self):
        output = "No scannable files matched the selected scope in the current directory.\n"
        api = api_for()
        code, out = run(api, FakeCli(output=output), FakeGit(diff=(0, "docs/a.md\nAGENTS.md\n")))
        self.assertEqual(code, 0)
        self.assertIn("nothing to scan", out)
        self.assertIn("2 changed file(s) since origin/dev", out)
        self.assertEqual([p for p, _ in api.requests], ["/v1/codebase"])

    def test_an_unfinished_or_foreign_scan_fails(self):
        for scan in [scan_record(status="failed"), scan_record(status="in_progress"),
                     scan_record(branch="dev"), scan_record(id="other"), scan_record(codebaseId=None),
                     ["not", "an", "object"]]:
            with self.subTest(scan=scan), self.assertRaises(Failed):
                run(api_for(scan=scan))

    def test_malformed_issue_answers_fail(self):
        for page in [{"issues": []}, {"issues": [], "total": -1}, {"issues": [], "total": 1.5},
                     {"issues": [], "total": "0"}, {"total": 0}, []]:
            with self.subTest(page=page), self.assertRaises(Failed):
                run(api_for(issues=page))

    def test_an_api_error_after_the_scan_fails(self):
        api = api_for()
        api.answers["/v1/issues"] = vidoc_scan.HttpError(500, "boom")
        with self.assertRaises(Failed):
            run(api)


class Redirecting(http.server.BaseHTTPRequestHandler):
    seen = []

    def do_GET(self):
        Redirecting.seen.append((self.path, self.headers.get("x-vidoc-token")))
        if self.path.startswith("/v1/"):
            self.send_response(302)
            self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}/elsewhere")
            self.end_headers()
        else:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")

    def log_message(self, *args):
        pass


class Http(unittest.TestCase):
    def test_redirects_are_refused_so_the_key_never_follows_them(self):
        server = http.server.HTTPServer(("127.0.0.1", 0), Redirecting)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        Redirecting.seen = []
        api = vidoc_scan.Api(f"http://127.0.0.1:{server.server_port}", "the-key")
        with self.assertRaises(vidoc_scan.HttpError) as caught:
            api.get("/v1/codebase")
        self.assertEqual(caught.exception.status, 302)
        self.assertEqual(Redirecting.seen, [("/v1/codebase", "the-key")])


class Unsendable(unittest.TestCase):
    def test_a_request_that_cannot_be_sent_does_not_echo_the_key(self):
        api = vidoc_scan.Api("https://127.0.0.1:9", "bad\nkey")
        with self.assertRaises(vidoc_scan.HttpError) as caught:
            api.get("/v1/codebase")
        self.assertNotIn("bad", str(caught.exception))


FAKE_CLI = (
    "import os, subprocess, sys, time\n"
    "assert sys.argv[1:] == ['ci'], sys.argv\n"
    "print('Indexing', flush=True)\n"
    "print('Scan started with ID: " + SCAN_ID + "', flush=True)\n"
    "if os.environ.get('FAKE_CHILD'):\n"
    "    child = subprocess.Popen(['sleep', '60'], stdout=subprocess.DEVNULL)\n"
    "    print('child', child.pid, flush=True)\n"
    "time.sleep(float(os.environ.get('FAKE_SLEEP', '0')))\n"
    "sys.exit(int(os.environ.get('FAKE_EXIT', '0')))\n")


def alive(pid):
    try:
        with open(f"/proc/{pid}/stat") as stat:
            return stat.read().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


class RealRunner(unittest.TestCase):
    def runner(self, timeout_s=30):
        return vidoc_scan.run_cli([sys.executable, "-c", FAKE_CLI], Path.cwd(), timeout_s)

    def test_echoes_and_returns_the_output_and_exit_code(self):
        echoed = io.StringIO()
        with contextlib.redirect_stdout(echoed):
            code, output = self.runner()({**os.environ, "FAKE_EXIT": "3"})
        self.assertEqual(code, 3)
        self.assertEqual(vidoc_scan.parse_scan_id(output), SCAN_ID)
        self.assertIn("Indexing", echoed.getvalue())

    @unittest.skipUnless(Path("/proc/self/stat").exists(), "needs /proc")
    def test_a_cli_past_its_bound_is_killed_with_its_children_and_fails(self):
        echoed = io.StringIO()
        started = time.monotonic()
        with contextlib.redirect_stdout(echoed), self.assertRaises(Failed):
            self.runner(timeout_s=1)({**os.environ, "FAKE_SLEEP": "60", "FAKE_CHILD": "1"})
        self.assertLess(time.monotonic() - started, 30)
        child = int(echoed.getvalue().split("child ", 1)[1].split()[0])
        deadline = time.monotonic() + 10
        while alive(child) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(alive(child))

    @unittest.skipUnless(Path("/proc/self/stat").exists(), "needs /proc")
    def test_sigterm_kills_the_cli_with_its_children_and_fails(self):
        previous = {s: signal.getsignal(s) for s in (signal.SIGTERM, signal.SIGHUP)}
        self.addCleanup(lambda: [signal.signal(s, h) for s, h in previous.items()])
        vidoc_scan.install_signal_handlers()
        echoed = io.StringIO()
        timer = threading.Timer(1.0, os.kill, (os.getpid(), signal.SIGTERM))
        timer.start()
        self.addCleanup(timer.cancel)
        with contextlib.redirect_stdout(echoed), self.assertRaises(Failed) as caught:
            self.runner()({**os.environ, "FAKE_SLEEP": "60", "FAKE_CHILD": "1"})
        self.assertIn("signal", str(caught.exception))
        child = int(echoed.getvalue().split("child ", 1)[1].split()[0])
        deadline = time.monotonic() + 10
        while alive(child) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(alive(child))


if __name__ == "__main__":
    unittest.main()
