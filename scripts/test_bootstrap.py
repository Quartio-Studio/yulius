"""bootstrap.py installs the toolchain under the home's rustup lock, so two CI runs sharing a home
do not install into ~/.rustup at once (SPEC-499, from #1131)."""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BOOTSTRAP = Path(__file__).resolve().parent / "bootstrap.py"

# A stand-in for rustup: during `toolchain install` it asks for the lock without waiting and records
# whether someone else held it; `which` fails, so bootstrap stops before building any tool.
STAND_IN = """#!{python}
import fcntl, os, sys
if sys.argv[1:3] == ["toolchain", "install"]:
    with open(os.environ["STAND_IN_LOCK"], "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            state = "free"
        except BlockingIOError:
            state = "held"
    with open(os.environ["STAND_IN_RECORD"], "a") as record:
        record.write(state + "\\n")
    sys.exit(0)
sys.exit(3)
"""


@unittest.skipIf(os.name == "nt", "flock is POSIX only; bootstrap holds no lock on Windows")
class BootstrapRustupLock(unittest.TestCase):
    def test_the_toolchain_install_runs_under_the_homes_rustup_lock(self):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            home, bin_dir, record = scratch / "home", scratch / "bin", scratch / "record"
            home.mkdir()
            bin_dir.mkdir()
            lock = home / ".cache/yulius-tools/.rustup.lock"
            stand_in = bin_dir / "rustup"
            stand_in.write_text(STAND_IN.format(python=sys.executable))
            stand_in.chmod(0o755)
            env = {key: value for key, value in os.environ.items() if key != "YULIUS_TOOLS_CACHE"}
            env.update(HOME=str(home), PATH=str(bin_dir) + os.pathsep + env.get("PATH", ""),
                       STAND_IN_LOCK=str(lock), STAND_IN_RECORD=str(record))
            lock.parent.mkdir(parents=True)

            run = subprocess.run([sys.executable, str(BOOTSTRAP), "--install-rustup"], env=env,
                                 stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)

            self.assertNotEqual(run.returncode, 0, "the stand-in's `rustup which` fails the run")
            self.assertEqual(record.read_text(), "held\n", run.stderr)
            with open(lock, "a") as again:
                import fcntl
                fcntl.flock(again, fcntl.LOCK_EX | fcntl.LOCK_NB)  # released once bootstrap ends


if __name__ == "__main__":
    unittest.main()
