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
import time

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


def process_rows(columns, timeout, *, threads=False):
    args = ["/bin/ps", "-A"]
    if threads and sys.platform == "linux":
        args.append("-L")  # A zombie leader can have other live threads.
    with tempfile.TemporaryFile() as listing:
        subprocess.run([*args, "-o", columns], stdout=listing, check=True, timeout=timeout)
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


def stop_build_group(group, requested=None):
    """The leader is still waitable, so the kernel cannot recycle its group ID."""
    def send(sig):
        try:
            os.killpg(group, sig)
        except ProcessLookupError:
            pass

    def stopped_within(seconds, *, force):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if force:
                send(signal.SIGKILL)
            try:
                rows = process_rows("pgid=,stat=", max(0.01, deadline - time.monotonic()), threads=True)
                if not any(int(pgid) == group and not state.startswith(b"Z") for pgid, state in rows):
                    return True
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                raise UnreapedBuild(f"cannot verify build group {group}: {error}") from error
            time.sleep(0.05)
        return False

    if requested is not None:
        send(requested)
        try:
            if stopped_within(3, force=False):
                return
        except UnreapedBuild:
            pass  # Still escalate if process observation failed during graceful shutdown.
    if not stopped_within(3, force=True):
        raise UnreapedBuild(f"build group {group} still has live workers after SIGKILL")


def observe_build_exit(pid):
    """Observe exit while the unreaped leader still pins the process group."""
    if hasattr(os, "waitid"):
        os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)
        return
    # Python 3.11 and 3.12 on macOS expose kqueue but not waitid.
    import select
    with select.kqueue() as events:
        exit_event = select.kevent(pid, filter=select.KQ_FILTER_PROC,
                                   flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                   fflags=select.KQ_NOTE_EXIT)
        try:
            event, = events.control([exit_event], 1, None)
            if event.flags & select.KQ_EV_ERROR and event.data != errno.ESRCH:
                raise OSError(event.data, os.strerror(event.data))
        except ProcessLookupError:
            pass  # The child had already exited before registration.


def run_session(command, env, lease=None, *, disposable=False):
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

    subreaper = disposable and sys.platform == "linux"
    previous_members = None
    previous_groups = None
    if subreaper:
        enable_build_subreaper()
    elif disposable:
        from build_groups import group_snapshot
        previous_groups = group_snapshot()
        previous_members = session_members(os.getsid(0))

    def reap():
        status = child.wait()
        try:
            if subreaper:
                reap_adopted_workers()
            elif previous_groups is not None:
                after = group_snapshot()
                if (session_members(os.getsid(0)) - previous_members
                        or any(set(members).isdisjoint(previous_groups.get(group, ()))
                               for group, members in after.items())):
                    raise UnreapedBuild("new processes may still own the build target")
        except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as error:
            raise UnreapedBuild(f"cannot confirm build worker shutdown: {error}") from error
        return status

    terminal = None
    if disposable and foreground_terminal():
        terminal = os.open("/dev/tty", os.O_RDONLY | os.O_NOCTTY)

    def foreground(group):
        # The supervisor is in the background when restoring its foreground group.
        previous_ttou = signal.signal(signal.SIGTTOU, signal.SIG_IGN)
        try:
            os.tcsetpgrp(terminal, group)
        finally:
            signal.signal(signal.SIGTTOU, previous_ttou)

    signals = (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM)
    previous = {sig: signal.signal(sig, cancel) for sig in signals}
    try:
        # Direct Cargo inherits exactly the descriptors its caller left inheritable,
        # including a Make jobserver. Only the private lease crosses into mbx.
        child = subprocess.Popen(command, env=env, close_fds=lease is not None,
                                 pass_fds=() if lease is None else (lease,), process_group=0)
        if terminal is not None:
            try:
                foreground(child.pid)
                # A fast reader may have received SIGTTIN before the handoff.
                os.killpg(child.pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        if requested is not None:
            raise Cancelled
        # Terminal Ctrl-C goes straight to the foreground group. Cargo may
        # report a signal or translate it to an ordinary exit code.
        observe_build_exit(child.pid)
        if disposable:
            stop_build_group(child.pid)
        # The last group operation precedes reaping. A delayed cancellation must
        # never signal a process group whose leader PID the kernel can reuse.
        for sig in signals:
            signal.signal(sig, signal.SIG_IGN)
        return reap()
    except Cancelled:
        for sig in signals:
            signal.signal(sig, signal.SIG_IGN)
        stop_build_group(child.pid, requested)
        reap()
        return -requested
    except UnreapedBuild:
        raise
    except BaseException:
        if child is not None and child.returncode is None:
            for sig in signals:
                signal.signal(sig, signal.SIG_IGN)
            stop_build_group(child.pid)
            reap()
        raise
    finally:
        pending = sys.exception()
        try:
            if terminal is not None:
                try:
                    foreground(os.getpgrp())
                finally:
                    os.close(terminal)
        except OSError as error:
            # A hung-up terminal no longer has a foreground group to restore.
            # Even unexpected restoration errors must preserve an uncertain shutdown.
            if error.errno not in (errno.ENOTTY, errno.EIO):
                if pending is None:
                    raise
                pending.add_note(f"cannot restore terminal: {error}")
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
