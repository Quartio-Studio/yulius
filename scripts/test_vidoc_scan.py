"""The Vidoc security scan on vidoc-1 (SPEC-195): config, file selection, chunking, the answer
contract, retries, the verdict and the CI mode, against real Git repositories and a fake Vidoc."""
import contextlib
import http.server
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest

import vidoc_scan
from vidoc_scan import Failed, Usage

KEY = "yulius-secret-VIDOC_API_KEY"
PLANTED = 'import os\n\n\ndef run(name):\n    os.system("ls " + name)\n'
CLEAN = "def add(a, b):\n    return a + b\n"


def answer(findings=(), text=None):
    """A Messages API answer whose text block is the findings JSON (or `text`)."""
    body = text if text is not None else json.dumps({"findings": list(findings)})
    return {"id": "msg_1", "type": "message", "role": "assistant", "model": "vidoc-1",
            "content": [{"type": "thinking", "thinking": "Looking.", "signature": "s"},
                        {"type": "text", "text": body}],
            "stop_reason": "end_turn", "usage": {"input_tokens": 10, "output_tokens": 5}}


def files_of(content):
    """The paths of the file markers in a request's user message."""
    return [line.split(" ", 2)[1] for line in content.splitlines()
            if line.startswith(vidoc_scan.FILE_MARK)]


def judge(body, headers=None):
    """The fake model: a critical finding on every `os.system(` line, else nothing."""
    content = body["messages"][0]["content"]
    findings, path = [], None
    for line in content.splitlines():
        if line.startswith(vidoc_scan.FILE_MARK):
            path = line.split(" ", 2)[1]
        elif "os.system(" in line and "|" in line:
            number = int(line.split("|", 1)[0])
            findings.append({"severity": "critical", "file": path, "line": number,
                             "title": "OS command injection",
                             "detail": "name reaches a shell; pass arguments as a list."})
    return 200, answer(findings), {}


class FakeVidoc:
    """A Messages API on loopback. `behave(body, headers) -> (status, json, headers)`."""

    def __init__(self, behave=judge):
        self.behave = behave
        self.requests = []
        fake = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get("content-length") or 0)
                body = json.loads(self.rfile.read(length))
                headers = {k.lower(): v for k, v in self.headers.items()}
                fake.requests.append((self.path, headers, body))
                status, payload, extra = fake.behave(body, headers)
                data = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                for name, value in extra.items():
                    self.send_header(name, value)
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def scanned(self):
        return sorted({p for _, _, body in self.requests for p in files_of(body["messages"][0]["content"])})


def git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True,
                   env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
                        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com"})


def write(root, files):
    for path, text in files.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(text, bytes):
            target.write_bytes(text)
        else:
            target.write_text(text)


class Repo:
    """A repository whose `origin/dev` (origin's HEAD) holds `base`, checked out on a feature
    branch that adds `change` in one commit."""

    def __init__(self, test, base, change=None, origin_head=True):
        self.root = Path(tempfile.mkdtemp(prefix="vidoc-repo-"))
        test.addCleanup(shutil.rmtree, self.root, True)
        git(self.root, "init", "-q", "-b", "dev")
        write(self.root, base)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        git(self.root, "update-ref", "refs/remotes/origin/dev", "HEAD")
        if origin_head:
            git(self.root, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/dev")
        git(self.root, "checkout", "-q", "-b", "feat/x")
        if change:
            self.commit(change)

    def commit(self, files, message="change"):
        write(self.root, files)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", message)


class Scanning(unittest.TestCase):
    def fake(self, behave=judge):
        server = FakeVidoc(behave)
        self.addCleanup(server.close)
        return server

    def run_scan(self, repo, server, *argv, env=None):
        environment = {"VIDOC_API_KEY": KEY, "VIDOC_API_URL": server.url if server else ""}
        environment.update(env or {})
        out, err = io.StringIO(), io.StringIO()
        sleeps = []
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = vidoc_scan.main(list(argv), env=environment, root=repo.root,
                                   sleep=sleeps.append)
        self.sleeps = sleeps
        return code, out.getvalue() + err.getvalue()


class ChangeScan(Scanning):
    """VSC-003, VSC-008: the change scan sends only the branch's files and fails on findings."""

    def test_a_planted_injection_fails_the_change_scan(self):
        repo = Repo(self, {"lib/ok.py": CLEAN}, {"app/run.py": PLANTED})
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 1, output)
        self.assertIn("[critical] app/run.py:5 OS command injection", output)
        self.assertIn("pass arguments as a list", output)
        self.assertEqual(server.scanned(), ["app/run.py"], "only the changed file is sent")

    def test_clean_changes_pass_and_say_what_was_scanned(self):
        repo = Repo(self, {"app/run.py": PLANTED}, {"lib/ok.py": CLEAN})
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 0, output)
        self.assertEqual(server.scanned(), ["lib/ok.py"])
        self.assertIn("change scan", output)
        self.assertIn("no open critical or high finding", output)

    def test_each_request_is_a_vidoc_1_message_with_the_key_in_x_api_key_only(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        server = self.fake()
        self.assertEqual(self.run_scan(repo, server)[0], 0)
        path, headers, body = server.requests[0]
        self.assertEqual(path, "/v1/messages")
        self.assertEqual(headers["x-api-key"], KEY)
        self.assertEqual(headers["anthropic-version"], "2023-06-01")
        self.assertNotIn("authorization", headers)
        self.assertEqual(body["model"], "vidoc-1")
        self.assertIn("untrusted", body["system"])
        self.assertIn("1| def add(a, b):", body["messages"][0]["content"])
        self.assertEqual([k for k, v in headers.items() if KEY in v], ["x-api-key"])

    def test_the_scan_reads_commits_not_the_working_tree(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        (repo.root / "b.py").write_text(PLANTED)
        (repo.root / "untracked.py").write_text(PLANTED)
        git(repo.root, "checkout", "-q", "--detach")
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 0, output)
        self.assertEqual(server.scanned(), ["b.py"])

    def test_deleted_files_and_nothing_changed(self):
        repo = Repo(self, {"a.py": PLANTED, "b.py": CLEAN})
        git(repo.root, "rm", "-q", "a.py")
        git(repo.root, "commit", "-q", "-m", "drop")
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 0, output)
        self.assertEqual(server.requests, [])
        self.assertIn("nothing to scan", output)

    def test_an_explicit_base_wins_and_no_base_is_not_run(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN}, origin_head=False)
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 2, output)
        self.assertIn("--base", output)
        code, output = self.run_scan(repo, server, "--base", "origin/dev")
        self.assertEqual(code, 0, output)
        self.assertEqual(server.scanned(), ["b.py"])


class BroadScan(Scanning):
    """VSC-003, VSC-005: --all sends every scannable tracked file in bounded chunks."""

    def test_every_scannable_file_is_sent_in_bounded_chunks(self):
        big = "".join(f"value_{i} = {i}  # {'x' * 60}\n" for i in range(3000))
        repo = Repo(self, {"a.py": CLEAN, "src/big.py": big, "src/run.py": PLANTED,
                           "README.md": "# docs\n", "vendor/lib.js": "eval(x)\n",
                           "logo.png": b"\x89PNG\r\n\x1a\n\x00\x00"})
        server = self.fake()
        code, output = self.run_scan(repo, server, "--all")
        self.assertEqual(code, 1, output)
        self.assertEqual(server.scanned(), ["a.py", "src/big.py", "src/run.py"])
        for _, _, body in server.requests:
            self.assertLessEqual(len(body["messages"][0]["content"].encode()),
                                 vidoc_scan.CHUNK_BYTES + 2048)
        parts = [line for _, _, body in server.requests
                 for line in body["messages"][0]["content"].splitlines()
                 if line.startswith(vidoc_scan.FILE_MARK + " src/big.py")]
        self.assertGreater(len(parts), 2, "the big file is split into line ranges")
        self.assertIn("lines 1-", parts[0])
        self.assertIn("broad scan", output)
        self.assertIn("[critical] src/run.py:5", output)
        self.assertIn("skipped 3 file(s)", output)

    def test_past_the_budget_the_rest_is_listed_and_the_scan_is_not_run(self):
        files = {f"m{i}.py": f"# {'y' * 200}\n" * 200 for i in range(6)}
        repo = Repo(self, files)
        server = self.fake()
        code, output = self.run_scan(repo, server, "--all", "--max-bytes", "90000")
        self.assertEqual(code, 2, output)
        self.assertIn("not scanned", output)
        self.assertIn("m5.py", output)
        self.assertLess(len(server.scanned()), 6)

    def test_findings_still_fail_an_incomplete_scan(self):
        files = {"a.py": PLANTED, **{f"m{i}.py": f"# {'y' * 200}\n" * 200 for i in range(6)}}
        repo = Repo(self, files)
        code, output = self.run_scan(repo, self.fake(), "--all", "--max-bytes", "90000",
                                     "--jobs", "1")
        self.assertEqual(code, 1, output)

    def test_a_report_file_holds_findings_and_unscanned_files(self):
        repo = Repo(self, {"a.py": PLANTED, "b.md": "x\n"})
        report = repo.root.parent / f"{repo.root.name}-report.json"
        self.addCleanup(report.unlink, True)
        code, _ = self.run_scan(repo, self.fake(), "--all", "--report", str(report))
        self.assertEqual(code, 1)
        data = json.loads(report.read_text())
        self.assertEqual(data["mode"], "broad")
        self.assertEqual([f["file"] for f in data["findings"]], ["a.py"])
        self.assertEqual(data["skipped"], [{"path": "b.md", "reason": "prose"}])
        self.assertEqual(data["unscanned"], [])


class Service(Scanning):
    """VSC-007, VSC-008: retries, refusals, quota and unreadable answers."""

    def test_rate_limits_are_retried_with_retry_after(self):
        calls = []

        def behave(body, headers):
            calls.append(1)
            if len(calls) < 3:
                return 429, {"type": "error", "error": {"message": "slow down"}}, {"retry-after": "7"}
            return judge(body)

        repo = Repo(self, {"a.py": CLEAN}, {"b.py": PLANTED})
        code, output = self.run_scan(repo, self.fake(behave))
        self.assertEqual(code, 1, output)
        self.assertEqual(len(calls), 3)
        self.assertEqual(self.sleeps, [7, 7])

    def test_an_outage_past_the_retries_is_not_run(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        server = self.fake(lambda body, headers: (503, {"error": {"message": "down"}}, {}))
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 2, output)
        self.assertEqual(len(server.requests), vidoc_scan.ATTEMPTS)
        self.assertEqual(self.sleeps, [2, 4, 8, 16])
        self.assertIn("cannot be reached", output)

    def test_a_refused_key_or_no_quota_is_not_run(self):
        for status, words in ((401, "refused the key"), (403, "refused the key"),
                              (402, "out of quota")):
            with self.subTest(status=status):
                repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
                server = self.fake(lambda body, headers, s=status: (s, {"error": {"message": "no"}}, {}))
                code, output = self.run_scan(repo, server)
                self.assertEqual(code, 2, output)
                self.assertIn(words, output)
                self.assertEqual(len(server.requests), 1, "never retried")

    def test_an_unreachable_service_is_not_run(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        server = self.fake()
        url = server.url
        server.close()
        code, output = self.run_scan(repo, None, env={"VIDOC_API_URL": url})
        self.assertEqual(code, 2, output)
        self.assertIn("cannot be reached", output)

    def test_an_unreadable_answer_is_retried_then_fails(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        server = self.fake(lambda body, headers: (200, answer(text="Sure! Looks fine."), {}))
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 1, output)
        self.assertEqual(len(server.requests), 3)
        self.assertIn("could not be read", output)

    def test_another_client_error_fails(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        server = self.fake(lambda body, headers: (400, {"error": {"message": "bad"}}, {}))
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 1, output)
        self.assertEqual(len(server.requests), 1)

    def test_no_key_is_not_run(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        code, output = self.run_scan(repo, self.fake(), env={"VIDOC_API_KEY": ""})
        self.assertEqual(code, 2, output)
        self.assertIn("No Vidoc key", output)


class Config(Scanning):
    """VSC-002, VSC-004, VSC-009, ADR-0107: the committed `.config/vidoc.toml`."""

    def test_a_committed_switch_turns_the_scan_off(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": PLANTED, ".config/vidoc.toml": "enabled = false\n"})
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 0, output)
        self.assertIn("Security scan disabled", output)
        self.assertEqual(server.requests, [])

    def test_an_uncommitted_switch_does_not_count(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": PLANTED})
        write(repo.root, {".config/vidoc.toml": "enabled = false\n"})
        code, output = self.run_scan(repo, self.fake())
        self.assertEqual(code, 1, output)
        self.assertIn("committed", output)

    def test_excluded_paths_are_not_sent(self):
        repo = Repo(self, {"a.py": CLEAN},
                    {"gen/x.py": PLANTED, "b.py": CLEAN,
                     ".config/vidoc.toml": 'enabled = true\nexclude = ["gen/*"]\n'})
        server = self.fake()
        code, output = self.run_scan(repo, server)
        self.assertEqual(code, 0, output)
        self.assertNotIn("gen/x.py", server.scanned())

    def test_an_ignored_false_positive_passes_and_is_shown(self):
        config = ('enabled = true\n[[ignore]]\npath = "app/*.py"\ntitle = "command injection"\n'
                  'reason = "name is a fixed constant (reviewed in #9)"\n')
        repo = Repo(self, {"a.py": CLEAN}, {"app/run.py": PLANTED, ".config/vidoc.toml": config})
        code, output = self.run_scan(repo, self.fake())
        self.assertEqual(code, 0, output)
        self.assertIn("ignored", output)
        self.assertIn("name is a fixed constant", output)

    def test_malformed_configs_are_not_run(self):
        for text in ("enabled = 1\n", "enabled = true\nmystery = 2\n", "[[ignore]]\npath = 'a'\n",
                     "enabled = true\n[[ignore]]\npath = 'a'\ntitle = 'b'\n", "enabled = [\n",
                     "exclude = 'x'\n"):
            with self.subTest(text=text):
                with self.assertRaises(Usage):
                    vidoc_scan.parse_config(text, "test")

    def test_a_valid_config(self):
        config = vidoc_scan.parse_config(
            'enabled = true\nexclude = ["a/*"]\n[[ignore]]\npath = "b"\ntitle = "T"\nreason = "r"\n',
            "test")
        self.assertTrue(config.enabled)
        self.assertEqual(config.exclude, ["a/*"])
        self.assertEqual(config.ignore[0].reason, "r")
        self.assertTrue(vidoc_scan.parse_config("", "test").enabled, "no keys: on")


class Selection(unittest.TestCase):
    """VSC-004: what is never sent."""

    def test_skip_reasons(self):
        config = vidoc_scan.Config(True, ["secret/*"], [])
        cases = {
            ("src/a.rs", b"fn main() {}\n"): None,
            ("Dockerfile", b"FROM x\n"): None,
            ("web/vendor/monaco.js", b"x\n"): "vendored or built",
            ("node_modules/x/i.js", b"x\n"): "vendored or built",
            ("Cargo.lock", b"x\n"): "lockfile",
            ("e2e/package-lock.json", b"{}\n"): "lockfile",
            ("a.min.js", b"x\n"): "minified",
            ("a.js.map", b"{}\n"): "minified",
            ("docs/x.md", b"# x\n"): "prose",
            ("logo.svg", b"<svg/>\n"): "media",
            ("bin.dat", b"a\x00b"): "binary",
            ("latin.txt2", b"\xff\xfe"): "binary",
            ("secret/k.py", b"x\n"): "excluded",
            ("empty.py", b""): "empty",
            ("huge.py", b"x" * (vidoc_scan.MAX_FILE_BYTES + 1)): "larger than 1 MiB",
            ("long.js", b"x" * (vidoc_scan.CHUNK_BYTES + 1)): "minified",
        }
        for (path, data), reason in cases.items():
            with self.subTest(path=path):
                self.assertEqual(vidoc_scan.skip_reason(path, data, config), reason)


class Chunking(unittest.TestCase):
    def test_files_are_packed_in_order_and_numbered(self):
        chunks = vidoc_scan.make_chunks([("a.py", "x = 1\ny = 2\n"), ("b.py", "z = 3\n")], 1000)
        self.assertEqual(len(chunks), 1)
        text = chunks[0].text
        self.assertIn(f"{vidoc_scan.FILE_MARK} a.py lines 1-2 of 2", text)
        self.assertIn("2| y = 2", text)
        self.assertEqual([(p.path, p.first, p.last) for p in chunks[0].parts],
                         [("a.py", 1, 2), ("b.py", 1, 1)])

    def test_a_large_file_is_split_at_line_boundaries(self):
        text = "".join(f"line {i}\n" for i in range(1, 401))
        chunks = vidoc_scan.make_chunks([("big.py", text)], 1500)
        ranges = [(p.first, p.last) for c in chunks for p in c.parts]
        self.assertGreater(len(ranges), 2)
        self.assertEqual(ranges[0][0], 1)
        self.assertEqual(ranges[-1][1], 400)
        for (_, last), (first, _) in zip(ranges, ranges[1:]):
            self.assertEqual(first, last + 1, "no gap, no overlap")
        for chunk in chunks:
            self.assertLessEqual(len(chunk.text.encode()), 1500 + 200)


class Answers(unittest.TestCase):
    """VSC-006: the answer contract."""

    def chunk(self):
        return vidoc_scan.make_chunks([("a.py", "x\n" * 10), ("b.py", "y\n" * 5)], 10_000)[0]

    def test_fenced_json_with_synonyms_and_bad_lines(self):
        text = "```json\n" + json.dumps({"findings": [
            {"severity": "HIGH", "file": "a.py", "line": 3, "title": "T", "detail": "D"},
            {"severity": "informational", "file": "b.py", "line": 99, "title": "U", "detail": ""},
            {"severity": "moderate", "file": "elsewhere.py", "line": 1, "title": "V"},
        ]}) + "\n```"
        findings = vidoc_scan.parse_answer(answer(text=text), self.chunk())
        self.assertEqual([(f.severity, f.file, f.line) for f in findings],
                         [("high", "a.py", 3), ("low", "b.py", None), ("medium", "elsewhere.py", None)])

    def test_unreadable_answers(self):
        for text in ("no json here", json.dumps({"issues": []}),
                     json.dumps({"findings": [{"severity": "apocalyptic", "file": "a.py"}]}),
                     json.dumps({"findings": [{"severity": "high"}]}),
                     json.dumps({"findings": "none"})):
            with self.subTest(text=text):
                with self.assertRaises(vidoc_scan.Unreadable):
                    vidoc_scan.parse_answer(answer(text=text), self.chunk())
        with self.assertRaises(vidoc_scan.Unreadable):
            vidoc_scan.parse_answer({"content": [], "stop_reason": "max_tokens"}, self.chunk())


class Settings(unittest.TestCase):
    """VSC-010."""

    def test_key_and_address(self):
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": " k-123 "}),
                         ("k-123", "https://model.vidocsecurity.com"))
        self.assertEqual(vidoc_scan.settings({"VIDOC_API_KEY": "k", "VIDOC_API_URL": "http://127.0.0.1:9/"}),
                         ("k", "http://127.0.0.1:9"))
        for env in ({}, {"VIDOC_API_KEY": "a b"},
                    {"VIDOC_API_KEY": "k", "VIDOC_API_URL": "http://example.com"},
                    {"VIDOC_API_KEY": "k", "VIDOC_API_URL": "https://example.com/v1"}):
            with self.subTest(env=env):
                with self.assertRaises(Usage):
                    vidoc_scan.settings(env)


class CiMode(Scanning):
    """VSC-011, SPEC-162: pull request heads only; not run ends skipped."""

    def test_a_pull_request_head_scans_and_fails_on_findings(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": PLANTED})
        code, output = self.run_scan(repo, self.fake(), "--ci",
                                     env={"YULIUS_CI_REF": "pr/7", "YULIUS_CI_SKIP_CODE": "78"})
        self.assertEqual(code, 1, output)

    def test_not_run_ends_skipped(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": CLEAN})
        for skip, expected in (("78", 78), (None, 0)):
            with self.subTest(skip=skip):
                env = {"YULIUS_CI_REF": "pr/7", "VIDOC_API_KEY": ""}
                if skip:
                    env["YULIUS_CI_SKIP_CODE"] = skip
                code, output = self.run_scan(repo, self.fake(), "--ci", env=env)
                self.assertEqual(code, expected, output)
                self.assertIn("[yulius] security scan skipped", output)

    def test_other_heads_are_not_scanned(self):
        repo = Repo(self, {"a.py": CLEAN}, {"b.py": PLANTED})
        for ref in ("branch/dev", "branch/main", "train/3+4", ""):
            with self.subTest(ref=ref):
                server = self.fake()
                code, output = self.run_scan(repo, server, "--ci",
                                             env={"YULIUS_CI_REF": ref, "YULIUS_CI_SKIP_CODE": "78"})
                self.assertEqual(code, 78, output)
                self.assertEqual(server.requests, [])
                self.assertIn("pull request", output)


class SecurityJob(unittest.TestCase):
    """SPEC-195 VIB-003: this repository's `security` job runs the in-tree scanner in CI mode with
    its grant."""

    def job(self):
        import tomllib
        config = tomllib.loads((Path(__file__).resolve().parent.parent / "yulius.toml").read_text())
        jobs = [job for job in config["ci"]["jobs"] if job["name"] == "security"]
        self.assertEqual(len(jobs), 1, "one security job")
        return jobs[0]

    def test_it_is_granted_the_key_and_runs_the_in_tree_scanner(self):
        self.assertEqual(self.job().get("secrets"), "VIDOC_API_KEY")
        self.assertEqual(self.job()["command"], "python3 scripts/vidoc_scan.py --ci")


class NoOldScan(unittest.TestCase):
    """VSC-012: the dead API and CLI are gone from code and the living docs."""

    def test_no_old_api_or_cli(self):
        root = Path(__file__).resolve().parent.parent
        self.assertFalse((root / "tools" / "vidoc").exists())
        listed = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True,
                                check=True).stdout.split()
        history = (".ai/specs/", "docs/decisions/", "docs/changes.md", "scripts/test_vidoc_scan.py")
        stale = []
        for path in listed:
            if path.startswith(history) or not (root / path).is_file():
                continue
            try:
                text = (root / path).read_text()
            except (UnicodeDecodeError, OSError):
                continue
            if any(word in text for word in ("api.vidoc.dev", "@vidocsecurity/cli", "VIDOC_TOKEN")):
                stale.append(path)
        self.assertEqual(stale, [])


if __name__ == "__main__":
    unittest.main()
