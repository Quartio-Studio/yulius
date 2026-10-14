"""The Vidoc security gate (SPEC-128): settings, the CLI run and the verdict, with fakes."""
import contextlib
import io
import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

import vidoc_scan
from vidoc_scan import Failed, Usage

SCAN_ID = "019f8a10-9e8d-7c6b-a5f4-3e2d1c0b9a87"
CODEBASE = "019f8a1b-0c2d-73e4-a5f6-7b8c9d0e1f2a"
BRANCH = "feat/109-vidoc"


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
        if isinstance(answer, Exception):
            raise answer
        return answer


class FakeCli:
    def __init__(self, code=0, output=f"Indexing...\nScan started with ID: {SCAN_ID}\nDone\n"):
        self.code, self.output, self.env = code, output, None

    def __call__(self, env):
        self.env = env
        return self.code, self.output


def api_for(scan=None, issues=None):
    return FakeApi({
        "/v1/codebase": {"codebases": []},
        f"/v1/scan-workflows/{SCAN_ID}/status": scan if scan is not None else scan_record(),
        "/v1/issues": issues if issues is not None else issues_page(),
    })


def run(api=None, cli=None, branch=BRANCH, env=None):
    out = io.StringIO()
    env = {"VIDOC_API_KEY": "yulius-secret-VIDOC_API_KEY"} if env is None else env
    code = vidoc_scan.gate(env, branch, api or api_for(), cli or FakeCli(), Path("/tmp/vidoc.log"), out)
    return code, out.getvalue()


class Settings(unittest.TestCase):
    def test_token_prefers_vidoc_token_then_the_agent_secret(self):
        self.assertEqual(vidoc_scan.settings({"VIDOC_TOKEN": "a", "VIDOC_API_KEY": "b"})[0], "a")
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": "b"})[0], "b")

    def test_missing_key_is_a_usage_error_that_says_how_to_add_one(self):
        with self.assertRaises(Usage) as caught:
            vidoc_scan.settings({"VIDOC_TOKEN": "  "})
        self.assertIn("VIDOC_API_KEY", str(caught.exception))
        self.assertIn("api.vidoc.dev", str(caught.exception))

    def test_api_defaults_to_vidoc_cloud_and_drops_a_trailing_slash(self):
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": "k"})[1], "https://api.vidoc.dev")
        self.assertEqual(
            vidoc_scan.settings({"VIDOC_API_KEY": "k", "VIDOC_API_URL": "https://api.example.com/"})[1],
            "https://api.example.com")

    def test_api_must_be_https_without_a_path(self):
        for url in ["http://api.vidoc.dev", "https://api.vidoc.dev/v1", "api.vidoc.dev", "https://"]:
            with self.subTest(url=url), self.assertRaises(Usage):
                vidoc_scan.settings({"VIDOC_API_KEY": "k", "VIDOC_API_URL": url})


class ScanId(unittest.TestCase):
    def test_reads_the_one_scan_id(self):
        self.assertEqual(vidoc_scan.parse_scan_id(f"x\nScan started with ID: {SCAN_ID}\n"), SCAN_ID)

    def test_none_without_or_with_two_different_ids(self):
        self.assertIsNone(vidoc_scan.parse_scan_id("Scan started with ID: not-a-uuid"))
        other = "019f8a10-9e8d-7c6b-a5f4-3e2d1c0b9a88"
        self.assertIsNone(vidoc_scan.parse_scan_id(
            f"Scan started with ID: {SCAN_ID}\nScan started with ID: {other}\n"))


class CliEnvironment(unittest.TestCase):
    def test_passes_token_api_ci_and_a_log_outside_the_tree(self):
        env = vidoc_scan.cli_env({"PATH": "/bin", "VIDOC_PROFILE": "x"}, "tok", "https://a.b",
                                 Path("/tmp/l/vidoc.log"))
        self.assertEqual(env["VIDOC_TOKEN"], "tok")
        self.assertEqual(env["VIDOC_API_URL"], "https://a.b")
        self.assertEqual(env["CI"], "true")
        self.assertEqual(env["VIDOC_LOG_FILE"], "/tmp/l/vidoc.log")
        self.assertEqual(env["PATH"], "/bin")
        self.assertNotIn("VIDOC_PROFILE", env)


class Install(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        (self.dir / "package.json").write_text(json.dumps(
            {"dependencies": {"@vidocsecurity/cli": "0.1.25"}}))

    def installed(self, version):
        package = self.dir / "node_modules/@vidocsecurity/cli"
        package.mkdir(parents=True, exist_ok=True)
        (package / "package.json").write_text(json.dumps({"version": version}))

    def test_installs_when_missing_or_another_version(self):
        self.assertTrue(vidoc_scan.needs_install(self.dir))
        self.installed("0.1.21")
        self.assertTrue(vidoc_scan.needs_install(self.dir))

    def test_keeps_the_pinned_version(self):
        self.installed("0.1.25")
        self.assertFalse(vidoc_scan.needs_install(self.dir))

    def test_the_repository_pins_an_exact_version_with_integrity(self):
        tool = Path(__file__).resolve().parent.parent / "tools/vidoc"
        pin = json.loads((tool / "package.json").read_text())["dependencies"]["@vidocsecurity/cli"]
        self.assertRegex(pin, r"^\d+\.\d+\.\d+$")
        lock = json.loads((tool / "package-lock.json").read_text())
        locked = lock["packages"]["node_modules/@vidocsecurity/cli"]
        self.assertEqual(locked["version"], pin)
        self.assertTrue(locked["integrity"].startswith("sha512-"))


class Gate(unittest.TestCase):
    def test_clean_scan_passes_and_names_it(self):
        api, cli = api_for(), FakeCli()
        code, out = run(api, cli)
        self.assertEqual(code, 0)
        self.assertIn(SCAN_ID, out)
        self.assertEqual(cli.env["VIDOC_TOKEN"], "yulius-secret-VIDOC_API_KEY")
        path, params = api.requests[-1]
        self.assertEqual(path, "/v1/issues")
        self.assertEqual(params, {"codebaseId": CODEBASE, "branch": BRANCH, "status": "open",
                                  "severity": "critical,high", "limit": "100"})

    def test_findings_fail_and_are_printed(self):
        code, out = run(api_for(issues=issues_page([ISSUE])))
        self.assertEqual(code, 1)
        self.assertIn("[high] Command injection in hook runner", out)
        self.assertIn("crates/yulius-app/src/hooks.rs:42", out)
        self.assertIn(ISSUE["url"], out)

    def test_more_findings_than_one_page_still_fail_with_the_total(self):
        code, out = run(api_for(issues=issues_page([ISSUE], total=130)))
        self.assertEqual(code, 1)
        self.assertIn("130", out)

    def test_a_rejected_key_stops_before_the_upload(self):
        api = api_for()
        api.answers["/v1/codebase"] = vidoc_scan.HttpError(401, "Invalid or expired token")
        cli = FakeCli()
        with self.assertRaises(Usage) as caught:
            run(api, cli)
        self.assertIsNone(cli.env)
        self.assertIn("api.vidoc.dev", str(caught.exception))

    def test_a_detached_head_is_a_usage_error(self):
        with self.assertRaises(Usage):
            run(branch=None)

    def test_a_cli_error_fails(self):
        with self.assertRaises(Failed):
            run(cli=FakeCli(code=1))

    def test_no_scan_id_fails(self):
        with self.assertRaises(Failed):
            run(cli=FakeCli(output="Scan did not complete successfully\n"))

    def test_nothing_to_scan_passes_and_says_so(self):
        output = "No scannable files matched the selected scope in the current directory.\n"
        api = api_for()
        code, out = run(api, FakeCli(output=output))
        self.assertEqual(code, 0)
        self.assertIn("nothing to scan", out)
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


FAKE_CLI = (
    "import os, sys, time\n"
    "assert sys.argv[1:] == ['ci'], sys.argv\n"
    "print('Indexing', flush=True)\n"
    "print('Scan started with ID: " + SCAN_ID + "', flush=True)\n"
    "time.sleep(float(os.environ.get('FAKE_SLEEP', '0')))\n"
    "sys.exit(int(os.environ.get('FAKE_EXIT', '0')))\n")


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

    def test_a_cli_past_its_bound_is_killed_and_fails(self):
        started = time.monotonic()
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(Failed):
            self.runner(timeout_s=1)({**os.environ, "FAKE_SLEEP": "60"})
        self.assertLess(time.monotonic() - started, 30)


if __name__ == "__main__":
    unittest.main()
