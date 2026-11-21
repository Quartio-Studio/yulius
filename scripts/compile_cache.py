"""The project's optional Linux compiler cache. The sandbox mount is the sharing boundary."""
import hashlib
import errno
import os
import platform
import signal
import stat
import subprocess
import sys
import tomllib
from pathlib import Path
import shutil
import tarfile
import tempfile

from build_subreaper import UnreapedBuild, enable_build_subreaper, reap_adopted_workers
from home_lock import exclusive


def foreground_terminal():
    """A program can open its controlling terminal even when standard input is redirected."""
    try:
        terminal = os.open("/dev/tty", os.O_RDONLY | os.O_NONBLOCK | os.O_NOCTTY)
    except OSError:
        return False
    try:
        return os.tcgetpgrp(terminal) == os.getpgrp()
    except OSError:
        return True
    finally:
        os.close(terminal)


def open_store(cache, checkout):
    """Publish the complete ownership stamp in one rename, even on competing first builds."""
    stamp = ".yulius-mbx-v1"
    contents = b"mbx-v1\n"
    if not os.path.lexists(cache):
        staging = checkout / ".tools"
        staging.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=staging) as scratch:
            new = Path(scratch) / "store"
            new.mkdir(mode=0o700)
            (new / stamp).write_bytes(contents)
            try:
                new.rename(cache)
            except OSError as error:
                if error.errno not in (errno.EEXIST, errno.ENOTEMPTY):
                    raise
    directory = os.open(cache, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        with os.fdopen(os.open(stamp, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW,
                               dir_fd=directory), "rb") as owner:
            return stat.S_ISREG(os.fstat(owner.fileno()).st_mode) and owner.read(len(contents) + 1) == contents
    finally:
        os.close(directory)


def process_rows(columns, timeout):
    with tempfile.TemporaryFile() as listing:
        subprocess.run(["/bin/ps", "-A", "-o", columns], stdout=listing, check=True, timeout=timeout)
        if listing.tell() > 16 * 1024 * 1024:
            raise UnreapedBuild("process listing exceeded 16 MiB")
        listing.seek(0)
        for row in listing:
            yield row.split()


def session_members(session):
    """Live PIDs in a terminal session on systems without Linux subreapers."""
    try:
        members = set()
        for pid, state in process_rows("pid=,stat=", 2):
            try:
                if not state.startswith(b"Z") and os.getsid(int(pid)) == session:
                    members.add(int(pid))
            except ProcessLookupError:
                pass
        return members
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise UnreapedBuild(f"cannot observe build session {session}: {error}") from error


def run_disposable(command, env):
    """Keep Cargo in the shell job. Release the target only after its workers exit."""
    subreaper = sys.platform == "linux"
    if subreaper:
        enable_build_subreaper()
    else:
        from build_groups import group_snapshot
        previous_groups = group_snapshot()
        previous_members = session_members(os.getsid(0))

    requested = None

    def cancel(signum, _frame):
        nonlocal requested
        if requested is None:
            requested = signum

    signals = (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM)
    previous = {sig: signal.signal(sig, cancel) for sig in signals}
    try:
        if requested is not None:
            return -requested
        # Inherit the caller's process group and inheritable jobserver descriptors.
        # The shell delivers stop, continue and terminal ownership to the whole job.
        child = subprocess.Popen(command, env=env, close_fds=False)
        try:
            while requested is None:
                try:
                    child.wait(timeout=0.1)
                    break
                except subprocess.TimeoutExpired:
                    continue
            if requested is not None and child.poll() is None:
                child.send_signal(requested)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=3)
            if subreaper:
                reap_adopted_workers()
            else:
                current = group_snapshot()
                if (session_members(os.getsid(0)) - previous_members
                        or any(set(members).isdisjoint(previous_groups.get(group, ()))
                               for group, members in current.items())):
                    raise UnreapedBuild("new processes may still own the build target")
        except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as error:
            raise UnreapedBuild(f"cannot confirm build worker shutdown: {error}") from error
        return -requested if requested is not None else child.returncode
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def run_session(command, env, lease):
    """Forward cancellation to the whole Cargo command, escalating any surviving descendants."""
    child = None
    requested = None

    class Cancelled(BaseException):
        pass

    def cancel(signum, _frame):
        nonlocal requested
        requested = signum
        if child is not None:
            raise Cancelled

    signals = (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM)
    previous = {sig: signal.signal(sig, cancel) for sig in signals}
    try:
        child = subprocess.Popen(command, env=env, pass_fds=(lease,), process_group=0)
        if requested is not None:
            raise Cancelled
        return child.wait()
    except Cancelled:
        for sig in signals:
            signal.signal(sig, signal.SIG_IGN)
        try:
            os.killpg(child.pid, requested)
            child.wait(timeout=3)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        return -requested
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def sandbox_for(checkout, home):
    """Agent and CI homes live under the same sandbox. Require both known directory layouts."""
    if home.name != "home":
        return None
    sandbox = home.parent.parent if home.parent.name == "ci" else home.parent
    if not checkout.is_relative_to(sandbox):
        return None
    parts = checkout.relative_to(sandbox).parts
    if parts[:1] in {("worktrees",), ("repo-worktrees",), ("repo",)} or parts[:2] == ("ci", "jobs"):
        return sandbox
    return None


def installed(root):
    """A new worktree can already use the version bootstrap installed in this agent's home."""
    local = root / ".tools/bin/mbx"
    if os.access(local, os.X_OK):
        return local
    pins = tomllib.loads((root / ".config/tools.toml").read_text())["mbx"]
    cache = Path(os.environ.get("YULIUS_TOOLS_CACHE") or Path.home() / ".cache/yulius-tools")
    return cache / f"mbx-{pins['version']}-{platform.machine()}" / "mbx"


def install(pins, cache, bin_dir, machine, opener):
    """Publish only the digest-checked release executable. A failed install leaves Cargo usable."""
    digest = pins.get(machine)
    if not digest:
        return f"Rust compile cache unavailable for {machine}"
    version = pins["version"]
    shared = cache / f"mbx-{version}-{machine}" / "mbx"
    name = f"mbx-{machine}-unknown-linux-musl.tar.gz"
    url = f"https://github.com/jdx/mr-boxington/releases/download/v{version}/{name}"
    with exclusive(cache / f".mbx-{version}-{machine}.lock"):
        if not shared.is_file():
            shared.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(dir=shared.parent) as scratch:
                archive = Path(scratch) / name
                with opener(url, timeout=60) as response, archive.open("wb") as out:
                    shutil.copyfileobj(response, out)
                with archive.open("rb") as src:
                    if hashlib.file_digest(src, "sha256").hexdigest() != digest:
                        raise ValueError(f"unexpected digest for {name}")
                executable = Path(scratch) / "mbx"
                with tarfile.open(archive, "r:gz") as package:
                    member = package.getmember("mbx")
                    if not member.isfile():
                        raise ValueError(f"{name} has no regular mbx executable")
                    with package.extractfile(member) as src, executable.open("wb") as out:
                        shutil.copyfileobj(src, out)
                executable.chmod(0o755)
                executable.replace(shared)
        with tempfile.TemporaryDirectory(dir=bin_dir) as scratch:
            link = Path(scratch) / "mbx"
            link.symlink_to(shared)
            link.replace(bin_dir / "mbx")
    return f"mbx {version} linked from {shared}"
