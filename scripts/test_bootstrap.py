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


@unittest.skipIf(os.name == "nt", "POSIX shell stand-ins")
class DevBootstrapsInCi(unittest.TestCase):
    """A Yulius CI job's command is `scripts/dev <job>`: on CI's clean machine scripts/dev installs
    rustup and the pinned tools first; elsewhere it leaves that to the person."""

    def dev(self, **env):
        import shutil
        with tempfile.TemporaryDirectory() as root:
            tree = Path(root)
            (tree / "scripts").mkdir()
            shutil.copyfile(BOOTSTRAP.with_name("dev"), tree / "scripts/dev")
            (tree / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.0.0"\n')
            record = tree / "record"
            # bootstrap.py installs rustup where scripts/dev looks for it, then exits with $BOOT_EXIT.
            (tree / "scripts/bootstrap.py").write_text(
                "import os, sys\nfrom pathlib import Path\n"
                "open(os.environ['RECORD'], 'a').write('bootstrap ' + ' '.join(sys.argv[1:]) + '\\n')\n"
                "rustup = Path.home() / '.cargo/bin/rustup'\nrustup.parent.mkdir(parents=True)\n"
                "rustup.write_text(os.environ['RUSTUP'])\nrustup.chmod(0o755)\n"
                "sys.exit(int(os.environ.get('BOOT_EXIT', '0')))\n")
            rustup = ('#!/bin/sh\nif [ "$1" = which ]; then echo /pinned/rustc; exit 0; fi\n'
                      f'echo "rustup $* marker=$YULIUS_BOOTSTRAPPED" >> "{record}"\n')
            # Only python on PATH: a system rustup (apt's, Homebrew's) must not stand in for the stub.
            bin_dir = tree / "bin"
            bin_dir.mkdir()
            (bin_dir / "python3").symlink_to(sys.executable)
            base = {"HOME": str(tree / "home"), "PATH": str(bin_dir), "RECORD": str(record), "RUSTUP": rustup}
            run = subprocess.run([sys.executable, str(tree / "scripts/dev"), "lint"], env={**base, **env},
                                 stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
            return run, record.read_text() if record.exists() else ""

    def test_in_ci_scripts_dev_bootstraps_before_it_enters_the_toolchain(self):
        run, record = self.dev(YULIUS_CI="1")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(record, "bootstrap --install-rustup\nrustup run 1.0.0 cargo xtask lint marker=1\n")

    def test_a_failed_bootstrap_fails_the_job_with_its_code(self):
        run, record = self.dev(YULIUS_CI="1", BOOT_EXIT="5")
        self.assertEqual(run.returncode, 5, run.stderr)
        self.assertEqual(record, "bootstrap --install-rustup\n")

    def test_outside_ci_scripts_dev_does_not_bootstrap(self):
        for env in ({}, {"YULIUS_CI": "0"}, {"YULIUS_CI": "off"}):
            with self.subTest(env=env):
                run, record = self.dev(**env)
                self.assertNotEqual(run.returncode, 0)
                self.assertIn("rustup is required", run.stderr)
                self.assertEqual(record, "")

    def test_a_gate_scripts_dev_starts_inside_a_bootstrapped_one_does_not_bootstrap_again(self):
        # The gate's environment carries the marker the outer scripts/dev set (a test shard's
        # `scripts/dev test` after its archive build).
        run, record = self.dev(YULIUS_CI="1", YULIUS_BOOTSTRAPPED="1", BOOT_EXIT="5")
        self.assertNotEqual(run.returncode, 5, run.stderr)
        self.assertNotIn("bootstrap", record)
        self.assertIn("rustup is required", run.stderr)


if __name__ == "__main__":
    unittest.main()
