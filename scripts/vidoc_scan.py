#!/usr/bin/env python3
"""Security gate (SPEC-128): scan the branch with Vidoc and fail on open critical or high findings.

`vidoc ci` uploads the new and changed files, scans the files the branch changed since its merge
base with the default branch and waits; it exits 0 even when it found issues, so the verdict comes
from the REST API: the scan this run started must be complete for this branch, and the branch must
have no open critical or high finding. Exit codes: 0 passed, 1 failed, 2 not runnable here (no key,
a rejected key, a detached HEAD).
"""
from collections import deque
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import threading
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_API = "https://api.vidoc.dev"
CLI_PACKAGE = "@vidocsecurity/cli"
# A whole scan takes 10 to 15 minutes (Vidoc documentation); past this the CLI is killed.
CLI_TIMEOUT_S = 45 * 60
HTTP_TIMEOUT_S = 30
MAX_ANSWER_BYTES = 8 * 1024 * 1024
OUTPUT_LINES = 2000
NOTHING_TO_SCAN = "No scannable files matched the selected scope"
SCAN_STARTED = re.compile(
    r"Scan started with ID: ([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\b")
HOW_TO_ADD_KEY = (
    "Agents get the project's agent secret VIDOC_API_KEY once api.vidoc.dev is bound to it "
    "(secret_request_hosts, allowed on the Secrets page); elsewhere set VIDOC_TOKEN.")


class Usage(Exception):
    """The gate cannot run here: exit 2."""


class Failed(Exception):
    """The gate ran and failed: exit 1."""


class HttpError(Exception):
    def __init__(self, status, message):
        super().__init__(f"HTTP {status}: {message}" if status else message)
        self.status = status


def settings(env):
    """The API key and the API address (`https://host[:port]`, no path)."""
    token = (env.get("VIDOC_TOKEN") or "").strip() or (env.get("VIDOC_API_KEY") or "").strip()
    if not token:
        raise Usage(f"No Vidoc API key: set VIDOC_TOKEN or VIDOC_API_KEY. {HOW_TO_ADD_KEY}")
    api = (env.get("VIDOC_API_URL") or DEFAULT_API).strip().rstrip("/")
    parts = urllib.parse.urlsplit(api)
    if parts.scheme != "https" or not parts.hostname or parts.path or parts.query or parts.fragment:
        raise Usage(f"VIDOC_API_URL must be an https:// address without a path, not {api!r}")
    return token, api


def parse_scan_id(output):
    """The one scan ID the CLI printed, or None."""
    ids = {match.lower() for match in SCAN_STARTED.findall(output)}
    return ids.pop() if len(ids) == 1 else None


def cli_env(env, token, api, log):
    """The CLI's environment: ours without other Vidoc settings, plus the key, the API, CI mode
    and a log file outside the working tree."""
    out = {k: v for k, v in env.items() if not k.startswith("VIDOC_")}
    out.update({"VIDOC_TOKEN": token, "VIDOC_API_URL": api, "CI": "true", "VIDOC_LOG_FILE": str(log)})
    return out


def needs_install(tool):
    """Whether `tool/node_modules` lacks the CLI version pinned in `tool/package.json`."""
    pin = json.loads((tool / "package.json").read_text())["dependencies"][CLI_PACKAGE]
    installed = tool / "node_modules" / CLI_PACKAGE / "package.json"
    try:
        return json.loads(installed.read_text()).get("version") != pin
    except (OSError, ValueError):
        return True


def check_scan(scan, scan_id, branch):
    """The scan's codebase ID once it is this run's completed scan of `branch`."""
    if not isinstance(scan, dict) or scan.get("id") != scan_id:
        raise Failed(f"Vidoc answered for another scan than {scan_id}")
    if scan.get("status") != "completed":
        raise Failed(f"Vidoc scan {scan_id} ended {scan.get('status')!r}, not completed")
    if scan.get("branch") != branch:
        raise Failed(f"Vidoc scan {scan_id} is of branch {scan.get('branch')!r}, not {branch!r}")
    codebase = scan.get("codebaseId")
    if not isinstance(codebase, str) or not codebase:
        raise Failed(f"Vidoc scan {scan_id} has no repository ID")
    return codebase


def finding_line(issue):
    where = issue.get("path") or "?"
    if issue.get("line") is not None:
        where = f"{where}:{issue['line']}"
    return f"  [{issue.get('severity')}] {issue.get('name')} - {where} {issue.get('url') or ''}".rstrip()


def gate(env, branch, api, cli, log, out):
    """Runs the scan with `cli(env) -> (exit code, output)` and judges it through `api.get`."""
    token, url = settings(env)
    if not branch:
        raise Usage("HEAD is detached: Vidoc names the scan after the branch; check out a branch")
    try:
        api.get("/v1/codebase")
    except HttpError as err:
        if err.status in (401, 403):
            raise Usage(f"Vidoc rejected the key ({err}): the host api.vidoc.dev may not be bound "
                        f"to the secret yet, or the key expired. {HOW_TO_ADD_KEY}") from err
        raise Failed(f"Vidoc API {url} is not usable: {err}") from err
    code, output = cli(cli_env(env, token, url, log))
    if code != 0:
        raise Failed(f"vidoc ci failed with exit code {code}; its log is {log}")
    scan_id = parse_scan_id(output)
    if scan_id is None:
        if NOTHING_TO_SCAN in output:
            print(f"Vidoc: nothing to scan on {branch} (no changed file Vidoc scans).", file=out)
            return 0
        raise Failed("vidoc ci printed no scan ID: no scan was started; read its output above")
    try:
        codebase = check_scan(api.get(f"/v1/scan-workflows/{scan_id}/status"), scan_id, branch)
        page = api.get("/v1/issues", {"codebaseId": codebase, "branch": branch, "status": "open",
                                      "severity": "critical,high", "limit": "100"})
    except HttpError as err:
        raise Failed(f"reading the result of Vidoc scan {scan_id} failed: {err}") from err
    total = page.get("total") if isinstance(page, dict) else None
    issues = page.get("issues") if isinstance(page, dict) else None
    if type(total) is not int or total < 0 or not isinstance(issues, list):
        raise Failed(f"Vidoc answered an unexpected issue list for scan {scan_id}")
    if total:
        print(f"Vidoc found {total} open critical or high issue(s) on {branch} (scan {scan_id}):", file=out)
        for issue in issues:
            if isinstance(issue, dict):
                print(finding_line(issue), file=out)
        return 1
    print(f"Vidoc scan {scan_id} completed: no open critical or high issue on {branch}.", file=out)
    return 0


class Api:
    """GET requests to the Vidoc REST API; the key goes in `x-vidoc-token`, the only header it
    reads. In agent sessions the key is a placeholder the egress proxy replaces."""

    def __init__(self, base, token):
        self.base, self.token = base, token

    def get(self, path, params=None):
        url = self.base + path + ("?" + urllib.parse.urlencode(params) if params else "")
        request = urllib.request.Request(url, headers={
            "x-vidoc-token": self.token, "Accept": "application/json", "User-Agent": "yulius-security-gate"})
        try:
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_S) as answer:
                body = answer.read(MAX_ANSWER_BYTES + 1)
        except urllib.error.HTTPError as err:
            raise HttpError(err.code, _message(err.read(4096))) from err
        except (urllib.error.URLError, OSError) as err:
            raise HttpError(0, f"{url}: {getattr(err, 'reason', err)}") from err
        if len(body) > MAX_ANSWER_BYTES:
            raise HttpError(0, f"{url}: the answer is larger than {MAX_ANSWER_BYTES} bytes")
        try:
            return json.loads(body)
        except ValueError as err:
            raise HttpError(0, f"{url}: the answer is not JSON") from err


def _message(body):
    try:
        return str(json.loads(body).get("message"))
    except (ValueError, AttributeError):
        return body.decode("utf-8", "replace")[:200]


def run_cli(argv, cwd, timeout_s):
    """A runner for `gate`: runs `argv ci` in its own process group, echoing its output, and kills
    the group past `timeout_s`. Returns (exit code, the kept output)."""

    def runner(env):
        process = subprocess.Popen([*argv, "ci"], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                   errors="replace", start_new_session=True)
        kept, marks = deque(maxlen=OUTPUT_LINES), []

        def pump():
            for line in process.stdout:
                sys.stdout.write(line)
                sys.stdout.flush()
                kept.append(line)
                if SCAN_STARTED.search(line) or NOTHING_TO_SCAN in line:
                    marks.append(line)

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        try:
            code = process.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise Failed(f"vidoc ci ran longer than {timeout_s // 60} minutes and was killed") from None
        reader.join(timeout=10)
        return code, "".join(marks) + "".join(kept)

    return runner


def current_branch(root):
    result = subprocess.run(["git", "symbolic-ref", "--quiet", "--short", "HEAD"], cwd=root,
                            capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else None


def main():
    root = Path(__file__).resolve().parent.parent
    tool = root / "tools" / "vidoc"
    env = dict(os.environ)
    try:
        token, url = settings(env)
        if subprocess.run(["git", "status", "--porcelain"], cwd=root, capture_output=True,
                          text=True).stdout.strip():
            print("Note: uncommitted changes are not scanned; Vidoc scans the branch's commits.", flush=True)
        if needs_install(tool):
            subprocess.run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"], cwd=tool, check=True)
        # Kept after the run: a failure names it.
        handle, log = tempfile.mkstemp(prefix="vidoc-", suffix=".log")
        os.close(handle)
        cli = run_cli([str(tool / "node_modules" / ".bin" / "vidoc")], root, CLI_TIMEOUT_S)
        return gate(env, current_branch(root), Api(url, token), cli, Path(log), sys.stdout)
    except Usage as err:
        print(f"Security scan not run: {err}", file=sys.stderr)
        return 2
    except (Failed, subprocess.CalledProcessError) as err:
        print(f"Security scan failed: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
