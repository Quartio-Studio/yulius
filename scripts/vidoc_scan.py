#!/usr/bin/env python3
"""Security gate (SPEC-128): scan the branch with Vidoc and fail on open critical or high findings.

`vidoc ci` uploads the working tree's files (untracked ones too) and scans the files that changed
since the merge base with origin's default branch, as they are in the working tree; it exits 0
even when it found issues. So the gate runs it only on a clean tree whose base it can find, and
takes the verdict from the REST API: the scan this run started must be complete for this branch,
and the branch must have no open critical or high finding. Exit codes: 0 passed, 1 failed, 2 not
run (no key or a rejected one, no Node, a detached HEAD, uncommitted files, no base).
"""
from collections import deque
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
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
MIN_NODE = 20
# A whole scan takes 10 to 15 minutes (Vidoc documentation); past this the CLI is killed.
CLI_TIMEOUT_S = 45 * 60
HTTP_TIMEOUT_S = 30
GIT_TIMEOUT_S = 60
NPM_TIMEOUT_S = 10 * 60
INSTALL_STAMP = ".yulius-lock-sha256"
# Changed files of these kinds are scanned by Vidoc: "nothing to scan" with one of them is wrong.
SOURCE_SUFFIXES = {".rs", ".py", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".go", ".java",
                   ".rb", ".php"}
CLI_FINDING = re.compile(r"^\s*\[(Critical|High)\] ", re.MULTILINE)
# How the CLI itself checks it can reach origin before asking it for the default branch; when the
# check fails it quietly compares with origin/main.
CLI_REMOTE_CHECK = ("-c", "credential.interactive=never", "-c", "core.askPass=", "-c",
                    "core.sshCommand=ssh -oBatchMode=yes -oStrictHostKeyChecking=yes",
                    "ls-remote", "origin", "HEAD")
MAX_ANSWER_BYTES = 8 * 1024 * 1024
OUTPUT_LINES = 2000
NOTHING_TO_SCAN = "No scannable files matched the selected scope"
SCAN_STARTED = re.compile(
    r"Scan started with ID: ([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\b")
HOW_TO_ADD_KEY = (
    "Agents get the project's agent secret VIDOC_API_KEY once api.vidoc.dev is bound to it "
    "(secret_request_hosts, allowed on the Secrets page); elsewhere set VIDOC_TOKEN.")
# What the CLI gets of our environment: the system basics, the egress proxy and its CA, and Git's
# own configuration with what Yulius's credential helper needs, so the CLI's `git remote show
# origin` finds the default branch (without it the CLI falls back to origin/main). Not a
# containment boundary (the CLI runs as us); it keeps GitHub tokens (PR comments), DEBUG (files
# in the tree) and model keys away from it.
CLI_ENV = {
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TZ",
    "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "no_proxy",
    "SSL_CERT_FILE", "NODE_EXTRA_CA_CERTS", "NODE_USE_ENV_PROXY", "YULIUS_URL", "YULIUS_TASK_TOKEN",
    "SSH_AUTH_SOCK", "XDG_CONFIG_HOME",
}


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
    if any(c.isspace() or not c.isprintable() for c in token):
        raise Usage("the Vidoc API key contains whitespace or control characters")
    api = (env.get("VIDOC_API_URL") or DEFAULT_API).strip().rstrip("/")
    parts = urllib.parse.urlsplit(api)
    if parts.scheme != "https" or not parts.hostname or parts.path or parts.query or parts.fragment:
        raise Usage(f"VIDOC_API_URL must be an https:// address without a path, not {api!r}")
    return token, api


def node_major(version):
    """The major version in `node --version` output (`v24.21.0`), or None."""
    match = re.match(r"\s*v(\d+)\.", version)
    return int(match.group(1)) if match else None


def require_node(which, version_of):
    """Refuses (exit 2) without npm and Node 20+; `version_of(node)` is `node --version`."""
    node = which("node")
    version = version_of(node) if node else ""
    if not which("npm") or (node_major(version) or 0) < MIN_NODE:
        raise Usage(f"the Vidoc CLI needs Node {MIN_NODE}+ and npm on PATH "
                    f"(found {version.strip() or 'no node'}{'' if which('npm') else ', no npm'})")


def parse_scan_id(output):
    """The one scan ID the CLI printed, or None."""
    ids = {match.lower() for match in SCAN_STARTED.findall(output)}
    return ids.pop() if len(ids) == 1 else None


def cli_env(env, token, api, log):
    """The CLI's environment: the allowed part of ours, plus the key, the API, CI mode and a log
    file outside the working tree."""
    out = {k: v for k, v in env.items() if k in CLI_ENV or k.startswith("GIT_")}
    out.update({"VIDOC_TOKEN": token, "VIDOC_API_URL": api, "CI": "true", "VIDOC_LOG_FILE": str(log)})
    return out


def _lock_digest(tool):
    return hashlib.sha256((tool / "package-lock.json").read_bytes()).hexdigest()


def needs_install(tool):
    """Whether `tool/node_modules` is not an install of the current `tool/package-lock.json` with
    the CLI version pinned in `tool/package.json`."""
    pin = json.loads((tool / "package.json").read_text())["dependencies"][CLI_PACKAGE]
    modules = tool / "node_modules"
    try:
        return (json.loads((modules / CLI_PACKAGE / "package.json").read_text()).get("version") != pin
                or not (modules / ".bin" / "vidoc").exists()
                or (modules / INSTALL_STAMP).read_text().strip() != _lock_digest(tool))
    except (OSError, ValueError):
        return True


def mark_installed(tool):
    (tool / "node_modules" / INSTALL_STAMP).write_text(_lock_digest(tool) + "\n")


def check_repository(git, cli_environment):
    """The branch, the base the CLI will compare with and the files changed since, once the tree is
    something the CLI scans faithfully. `git(*args, env=None) -> (exit code, stdout)`; the
    reachability check runs as the CLI's does, in the CLI's environment."""
    code, branch = git("symbolic-ref", "--quiet", "--short", "HEAD")
    if code != 0 or not branch.strip():
        raise Usage("HEAD is detached: Vidoc names the scan after the branch; check out a branch")
    code, status = git("status", "--porcelain", "--untracked-files=all")
    if code != 0 or status.strip():
        raise Usage("the working tree has uncommitted or untracked files: Vidoc uploads and scans "
                    "the working tree, not the commits; commit or remove them first")
    code, remotes = git("remote")
    if code != 0 or (remotes.split() or [""])[0] != "origin":
        raise Usage("the CLI reads the first remote as the repository; it must be origin")
    check_env = {k: v for k, v in cli_environment.items()
                 if k not in ("GIT_ASKPASS", "SSH_ASKPASS", "ASKPASS")}
    check_env["GIT_TERMINAL_PROMPT"] = "0"
    code, _ = git(*CLI_REMOTE_CHECK, env=check_env)
    if code != 0:
        raise Usage("the CLI cannot reach origin with its environment (`git ls-remote origin HEAD` "
                    "failed); it would compare with origin/main instead of the default branch")
    code, shown = git("remote", "show", "origin", env=check_env)
    head = re.search(r"^\s*HEAD branch:\s*(\S+)\s*$", shown, re.MULTILINE) if code == 0 else None
    if head is None or head.group(1) == "(unknown)":
        raise Usage("cannot read origin's default branch (`git remote show origin` needs access to "
                    "origin); the CLI would compare with origin/main instead")
    base = f"origin/{head.group(1)}"
    code, merge_base = git("merge-base", "HEAD", base)
    if code != 0 or not merge_base.strip():
        raise Usage(f"the branch has no merge base with {base}: fetch it (no shallow clone)")
    _, changed = git("diff", "--name-only", f"{merge_base.strip()}..HEAD")
    return branch.strip(), base, [line for line in changed.splitlines() if line.strip()]


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
    # The CLI's own issue endpoint wraps each row as {"issue": {...}}; accept both shapes.
    if isinstance(issue.get("issue"), dict):
        issue = issue["issue"]
    where = issue.get("path") or "?"
    if issue.get("line") is not None:
        where = f"{where}:{issue['line']}"
    return f"  [{issue.get('severity')}] {issue.get('name')} - {where} {issue.get('url') or ''}".rstrip()


def gate(env, git, api, cli, log, out):
    """Runs the scan with `cli(env) -> (exit code, output)` and judges it through `api.get`."""
    token, url = settings(env)
    environment = cli_env(env, token, url, log)
    branch, base, changed = check_repository(git, environment)
    try:
        api.get("/v1/codebase")
    except HttpError as err:
        if err.status in (401, 403):
            raise Usage(f"Vidoc rejected the key ({err}): the host api.vidoc.dev may not be bound "
                        f"to the secret yet, or the key expired. {HOW_TO_ADD_KEY}") from err
        if err.status == 0 or err.status >= 500:
            raise Usage(f"Vidoc API {url} cannot be reached: {err}") from err
        raise Failed(f"Vidoc API {url} is not usable: {err}") from err
    print(f"Vidoc: scanning {branch}: {len(changed)} changed file(s) since {base}.", file=out, flush=True)
    code, output = cli(environment)
    if code != 0:
        raise Failed(f"vidoc ci failed with exit code {code}; its log is {log}")
    scan_id = parse_scan_id(output)
    if scan_id is None:
        if NOTHING_TO_SCAN in output:
            sources = [path for path in changed if Path(path).suffix in SOURCE_SUFFIXES]
            if sources:
                raise Failed(f"vidoc ci found nothing to scan, but the branch changed source files "
                             f"since {base} ({', '.join(sources[:5])}): the CLI chose another base")
            print(f"Vidoc: nothing to scan on {branch}: none of its {len(changed)} changed file(s) "
                  f"since {base} is a file Vidoc scans.", file=out)
            return 0
        raise Failed(f"vidoc ci printed no scan ID: no scan was started; see its output and {log}")
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
    cli_findings = len(CLI_FINDING.findall(output))
    if not total and cli_findings:
        raise Failed(f"vidoc ci printed {cli_findings} critical or high issue(s) but the API reports "
                     f"none for {branch}: the verdict cannot be trusted")
    if total:
        print(f"Vidoc found {total} open critical or high issue(s) on {branch} (scan {scan_id}):", file=out)
        for issue in issues:
            if isinstance(issue, dict):
                print(finding_line(issue), file=out)
        return 1
    print(f"Vidoc scan {scan_id} completed: no open critical or high issue on {branch}.", file=out)
    return 0


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """urllib would resend the key header to wherever a redirect points."""

    def redirect_request(self, *args):
        return None


class Api:
    """GET requests to the Vidoc REST API; the key goes in `x-vidoc-token`, the only header it
    reads. In agent sessions the key is a placeholder the egress proxy replaces."""

    def __init__(self, base, token):
        self.base, self.token = base, token
        self.opener = urllib.request.build_opener(_NoRedirect)

    def get(self, path, params=None):
        url = self.base + path + ("?" + urllib.parse.urlencode(params) if params else "")
        request = urllib.request.Request(url, headers={
            "x-vidoc-token": self.token, "Accept": "application/json", "User-Agent": "yulius-security-gate"})
        try:
            with self.opener.open(request, timeout=HTTP_TIMEOUT_S) as answer:
                body = answer.read(MAX_ANSWER_BYTES + 1)
        except urllib.error.HTTPError as err:
            raise HttpError(err.code, _message(err.read(4096))) from err
        except (urllib.error.URLError, OSError) as err:
            raise HttpError(0, f"{url}: {getattr(err, 'reason', err)}") from err
        except (ValueError, http.client.HTTPException) as err:
            # The message can quote the header, that is the key.
            raise HttpError(0, f"the request to {url} could not be sent ({type(err).__name__})") from None
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
    """A runner for `gate`: runs `argv ci` in its own process group, echoing its output. The group
    is killed past `timeout_s` and whenever the wait ends early (an exception, a signal turned
    into one). Returns (exit code, the kept output)."""

    def runner(env):
        kept, marks, lock = deque(maxlen=OUTPUT_LINES), deque(maxlen=OUTPUT_LINES), threading.Lock()
        process = None
        try:
            process = subprocess.Popen([*argv, "ci"], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                       errors="replace", start_new_session=True)

            def pump():
                for line in process.stdout:
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    with lock:
                        kept.append(line)
                        if (SCAN_STARTED.search(line) or NOTHING_TO_SCAN in line
                                or CLI_FINDING.search(line)):
                            marks.append(line)

            reader = threading.Thread(target=pump, daemon=True)
            reader.start()
            code = process.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            raise Failed(f"vidoc ci ran longer than {timeout_s // 60} minutes and was killed") from None
        finally:
            # Whatever ended the wait, nothing of the CLI's group outlives the gate.
            if process is not None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
        reader.join(timeout=10)
        with lock:
            return code, "".join(marks) + "".join(kept)

    return runner


def git_in(root):
    def git(*args, env=None):
        env = env if env is not None else {**os.environ, "GIT_TERMINAL_PROMPT": "0"}
        try:
            result = subprocess.run(["git", *args], cwd=root, env=env, capture_output=True,
                                    text=True, timeout=GIT_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            return 124, ""
        return result.returncode, result.stdout
    return git


def _interrupted(signum, _frame):
    raise Failed(f"interrupted by signal {signum}")


def install_signal_handlers():
    """SIGTERM and SIGHUP end the gate like a failure, so the CLI's group is killed on the way."""
    for signum in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, _interrupted)


def main():
    root = Path(__file__).resolve().parent.parent
    tool = root / "tools" / "vidoc"
    env = dict(os.environ)
    install_signal_handlers()
    log = None
    try:
        token, url = settings(env)
        require_node(shutil.which, lambda node: subprocess.run(
            [node, "--version"], capture_output=True, text=True, timeout=GIT_TIMEOUT_S).stdout)
        if needs_install(tool):
            subprocess.run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"], cwd=tool,
                           check=True, timeout=NPM_TIMEOUT_S)
            mark_installed(tool)
        handle, name = tempfile.mkstemp(prefix="vidoc-", suffix=".log")
        os.close(handle)
        log = Path(name)
        cli = run_cli([str(tool / "node_modules" / ".bin" / "vidoc")], root, CLI_TIMEOUT_S)
        code = gate(env, git_in(root), Api(url, token), cli, log, sys.stdout)
    except Usage as err:
        print(f"Security scan not run: {err}", file=sys.stderr)
        code = 2
    except (Failed, subprocess.CalledProcessError, subprocess.TimeoutExpired) as err:
        print(f"Security scan failed: {err}", file=sys.stderr)
        return 1
    if log is not None and code != 1:
        log.unlink(missing_ok=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
