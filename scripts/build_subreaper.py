"""Supervise one Linux build, including workers that create new POSIX sessions."""
import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


class UnreapedBuild(RuntimeError):
    """At least one worker could still be using the build tree."""


def enable_build_subreaper():
    prctl = ctypes.CDLL(None, use_errno=True).prctl
    prctl.argtypes = [ctypes.c_int, *([ctypes.c_ulong] * 4)]
    if prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def reap_adopted_workers():
    # Called after the build leader has been waited for. This single-threaded
    # monitor owns each orphan until it waits for it, preventing PID reuse.
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        try:
            pid, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if pid:
            continue
        try:
            children = Path("/proc/thread-self/children").read_text().split()
        except OSError as error:
            raise UnreapedBuild("cannot enumerate adopted build workers") from error
        for child in children:
            try:
                os.kill(int(child), signal.SIGKILL)
            except ProcessLookupError:
                pass
        time.sleep(0.025)
    raise UnreapedBuild("adopted build workers did not exit after SIGKILL")


def main():
    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    os.setsid()
    enable_build_subreaper()
    # Only the monitor can acknowledge that it waited for the last descendant.
    # Open the receipt before spawning and keep its descriptor out of the build.
    with (open(os.environ.pop("YULIUS_BUILD_STATUS"), "w", encoding="ascii") as receipt,
          open(os.environ.pop("YULIUS_BUILD_CANCEL"), "rb", buffering=0) as cancellation):
        if stopping or cancellation.read(1):
            receipt.write(str(-signal.SIGTERM))
            return
        try:
            leader = subprocess.Popen(sys.argv[1:], close_fds=False)
        except OSError as error:
            print(error, file=sys.stderr)
            code = 127
        else:
            while leader.poll() is None and not (stopping or cancellation.read(1)):
                time.sleep(0.025)
            if leader.poll() is None:
                leader.terminate()
                try:
                    leader.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    leader.kill()
            code = leader.wait(timeout=2)
            reap_adopted_workers()
        receipt.write(str(code))


if __name__ == "__main__":
    main()
