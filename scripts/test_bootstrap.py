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
        # rustup installs into the home whatever the tools cache: two runs sharing a home but not
        # a cache must still take one lock.
        for moved_cache in (False, True):
            with self.subTest(moved_cache=moved_cache):
                self.bootstrap_holds_the_home_lock(moved_cache)

    def bootstrap_holds_the_home_lock(self, moved_cache):
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
            if moved_cache:
                env["YULIUS_TOOLS_CACHE"] = str(scratch / "elsewhere")
            lock.parent.mkdir(parents=True)

            run = subprocess.run([sys.executable, str(BOOTSTRAP), "--install-rustup"], env=env,
                                 stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)

            self.assertNotEqual(run.returncode, 0, "the stand-in's `rustup which` fails the run")
            self.assertEqual(record.read_text(), "held\n", run.stderr)


class BootstrapToolBuild(unittest.TestCase):
    def test_a_cold_registry_install_builds_on_disk_and_reclaims_its_target(self):
        import runpy
        import shutil
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as root:
            tree = Path(root)
            script = tree / "scripts/bootstrap.py"
            script.parent.mkdir()
            shutil.copyfile(BOOTSTRAP, script)
            (tree / "rust-toolchain.toml").write_text('[toolchain]\nchannel="stable"\nprofile="minimal"\ncomponents=[]\n')
            (tree / ".config").mkdir()
            (tree / ".config/tools.toml").write_text('[tools]\nfixture="1.0.0"\n')
            cache, scratch = tree / "tools-cache", tree / "disk-scratch"
            scratch.mkdir()
            for parent in (scratch, cache):
                targets = []

                def run(args, **unused):
                    if "install" in args and "cargo" in args:
                        self.assertIn("--target-dir", args, "Cargo installs registry tools in /tmp by default")
                        target = Path(args[args.index("--target-dir") + 1])
                        self.assertEqual(target.parent, parent)
                        (target / "object").touch()
                        targets.append(target)
                        raise subprocess.CalledProcessError(1, args)

                with (
                    self.subTest(parent=parent),
                    patch.dict(os.environ, HOME=str(tree), YULIUS_TOOLS_CACHE=str(cache),
                               YULIUS_BUILD_SCRATCH=str(scratch)),
                    patch.object(sys, "path", list(sys.path)),
                    patch("shutil.which", return_value="rustup"),
                    patch("subprocess.check_output", return_value="/tools/rustc"),
                    patch("subprocess.run", run),
                ):
                    if parent == cache:
                        del os.environ["YULIUS_BUILD_SCRATCH"]
                    with self.assertRaises(subprocess.CalledProcessError):
                        runpy.run_path(str(script))
                    self.assertEqual(len(targets), 1)
                    self.assertFalse(targets[0].exists())


if __name__ == "__main__":
    unittest.main()
