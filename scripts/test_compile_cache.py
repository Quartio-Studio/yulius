"""RCC-001..003. The project's Cargo launcher selects the sandbox's cache and preserves failures."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent


@unittest.skipUnless(sys.platform == "linux", "the instance cache runs on Linux")
class CompileCache(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.sandbox = Path(tmp.name) / "project"
        self.task = self.checkout(self.sandbox / "worktrees" / "task")
        self.lane = self.checkout(self.sandbox / "ci" / "jobs" / "test@1")
        self.cargo = Path(tmp.name) / "real-cargo"
        self.cargo.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$RECORD\"\nexit \"$CARGO_RESULT\"\n")
        self.cargo.chmod(0o755)
        self.record = Path(tmp.name) / "calls"

    @staticmethod
    def checkout(path):
        (path / "scripts").mkdir(parents=True)
        for name in ("cargo", "compile_cache.py", "home_lock.py"):
            shutil.copy2(SCRIPTS / name, path / "scripts" / name)
        (path / ".tools/bin").mkdir(parents=True)
        (path / ".config").mkdir()
        shutil.copy2(SCRIPTS.parent / ".config/tools.toml", path / ".config/tools.toml")
        return path

    def launch(self, tree, home, mbx, cargo_result=0):
        binary = tree / ".tools/bin/mbx"
        if mbx:
            binary.write_text("#!/usr/bin/env python3\nimport os,sys\n" + mbx)
            binary.chmod(0o755)
        home.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, HOME=str(home), YULIUS_SANDBOX="project", CARGO=str(self.cargo),
                   RECORD=str(self.record), CARGO_RESULT=str(cargo_result),
                   YULIUS_TOOLS_CACHE=str(home / ".cache/yulius-tools"))
        return subprocess.run([tree / "scripts/cargo", "build"], cwd=tree, env=env,
                              capture_output=True, text=True, timeout=10)

    def test_worktree_and_ci_lane_share_only_their_sandbox_store(self):
        import_script = ("print(__import__('json').dumps([os.environ['MBX_CACHE_DIR'], os.getcwd()]), flush=True)\n"
                         "os.execv(os.environ['CARGO'], [os.environ['CARGO'], *sys.argv[1:]])")
        for tree, home in ((self.task, self.sandbox / "home"), (self.lane, self.sandbox / "ci/home")):
            result = self.launch(tree, home, import_script)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), [str(self.sandbox / "rust-compile-cache"), str(tree)])
        foreign = self.checkout(self.sandbox.parent / "other/worktrees/b")
        result = self.launch(foreign, self.sandbox.parent / "other/home", import_script)
        self.assertEqual(json.loads(result.stdout)[0], str(self.sandbox.parent / "other/rust-compile-cache"))

    def test_a_new_worktree_finds_the_pin_bootstrapped_by_another_task(self):
        import platform
        import tomllib
        home = self.sandbox / "home"
        pins = tomllib.loads((self.task / ".config/tools.toml").read_text())["mbx"]
        binary = home / ".cache/yulius-tools" / f"mbx-{pins['version']}-{platform.machine()}" / "mbx"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\necho shared-tool\nexec \"$CARGO\" \"$@\"\n")
        binary.chmod(0o755)
        result = self.launch(self.task, home, None)
        self.assertEqual(result.stdout.strip(), "shared-tool")
        self.assertEqual(self.record.read_text().strip(), "build")

    def test_missing_tool_and_broken_store_compile_plainly(self):
        home = self.sandbox / "home"
        self.assertEqual(self.launch(self.task, home, None).returncode, 0)
        (self.sandbox / "rust-compile-cache").write_text("broken")
        self.assertEqual(self.launch(self.task, home, "sys.exit(99)").returncode, 0)
        self.assertEqual(self.record.read_text().splitlines(), ["build", "build"])

    def test_a_build_arriving_during_upkeep_runs_without_waiting_for_the_store(self):
        import fcntl
        lease = os.open(self.sandbox, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, lease)
        fcntl.flock(lease, fcntl.LOCK_EX)
        result = self.launch(self.task, self.sandbox / "home", "print('unexpected cache')")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual(self.record.read_text().strip(), "build")

    def test_existing_unowned_directory_stays_outside_disk_upkeep(self):
        cache = self.sandbox / "rust-compile-cache"
        cache.mkdir(parents=True)
        (cache / "keep").write_text("pre-existing")
        result = self.launch(self.task, self.sandbox / "home", "print('unexpected cache')")
        self.assertEqual(result.stdout, "")
        self.assertEqual(self.record.read_text().strip(), "build")
        self.assertEqual(list(cache.iterdir()), [cache / "keep"])

    def test_cache_startup_failure_falls_back_and_cargo_failure_runs_once(self):
        home = self.sandbox / "home"
        failed_start = self.launch(self.task, home, "sys.exit(1)")
        self.assertEqual(failed_start.returncode, 0, failed_start.stderr)
        silent_tool = self.launch(self.task, home, "sys.exit(0)")
        self.assertEqual(silent_tool.returncode, 0)
        delegate = "os.execv(os.environ['CARGO'], [os.environ['CARGO'], *sys.argv[1:]])"
        failed_build = self.launch(self.task, home, delegate, cargo_result=37)
        self.assertEqual(failed_build.returncode, 37, failed_build.stderr)
        self.assertEqual(self.record.read_text().splitlines(), ["build", "build", "build"])

    @unittest.skipUnless((SCRIPTS.parent / ".tools/bin/mbx").exists(), "optional pinned mbx unavailable")
    def test_real_cache_reuses_a_library_and_rebuilds_changed_or_corrupt_outputs(self):
        from concurrent.futures import ThreadPoolExecutor
        real_cargo = subprocess.check_output(["rustup", "which", "cargo"], cwd=SCRIPTS, text=True).strip()
        compiler = subprocess.check_output(["rustup", "which", "rustc"], cwd=SCRIPTS, text=True).strip()
        for tree in (self.task, self.lane):
            (tree / ".tools/bin/mbx").symlink_to((SCRIPTS.parent / ".tools/bin/mbx").resolve())
            (tree / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n')
            (tree / "src").mkdir()
            (tree / "src/lib.rs").write_text('pub fn value() -> u32 { 17 }')
            (tree / "src/main.rs").write_text('fn main(){println!("{} {}", fixture::value(),env!("CARGO_MANIFEST_DIR"));}')

        def build(tree, target):
            home = self.sandbox / ("ci/home" if tree == self.lane else "home")
            home.mkdir(parents=True, exist_ok=True)
            env = dict(os.environ, HOME=str(home), YULIUS_SANDBOX="project", CARGO=real_cargo,
                       RUSTC=compiler, RUSTDOC=str(Path(compiler).with_name("rustdoc")))
            env.pop("RUSTC_WRAPPER", None)
            result = subprocess.run([tree / "scripts/cargo", "build", "--offline", "--target-dir", target],
                                    cwd=tree, env=env, capture_output=True, text=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr)
            binary = tree / target / "debug/fixture"
            return subprocess.check_output([binary], text=True).strip(), result.stderr, binary

        first, _, binary = build(self.task, "a")
        self.assertEqual(first, f"17 {self.task}")
        warm, report, _ = build(self.lane, "b")
        self.assertEqual(warm, f"17 {self.lane}")
        self.assertRegex(report, r"mbx\[cache\]: [1-9][0-9]* hits")
        # Keep the same byte length, which would pass a superficial integrity check.
        contents = binary.read_bytes()
        objects = self.sandbox / "rust-compile-cache/actions/cas"
        stored = next(p for p in objects.rglob("*") if p.is_file()
                      and p.stat().st_size == len(contents) and p.read_bytes() == contents)
        stored.chmod(0o600)
        stored.write_bytes(bytes(len(contents)))
        restored, _, _ = build(self.task, "recovery")
        self.assertEqual(restored, first)

        (self.lane / "src/lib.rs").write_text('pub fn value() -> u32 { 29 }')
        with ThreadPoolExecutor(max_workers=2) as jobs:
            a = jobs.submit(build, self.task, "concurrent")
            b = jobs.submit(build, self.lane, "concurrent")
            self.assertEqual(a.result()[0], f"17 {self.task}")
            self.assertEqual(b.result()[0], f"29 {self.lane}")


class ReleaseInstall(unittest.TestCase):
    def test_only_a_verified_executable_is_published_and_reused(self):
        import hashlib
        import io
        import tarfile
        from compile_cache import install

        archive = io.BytesIO()
        payload = b"pinned binary"
        with tarfile.open(fileobj=archive, mode="w:gz") as package:
            member = tarfile.TarInfo("mbx")
            member.size = len(payload)
            package.addfile(member, io.BytesIO(payload))
        data = archive.getvalue()
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / "tools"
            bins = Path(temp) / "bin"
            bins.mkdir()
            requests = []

            def fetch(url, timeout):
                requests.append(url)
                self.assertGreater(timeout, 0)
                return io.BytesIO(data)

            pins = {"version": "1", "x86_64": "wrong digest"}
            with self.assertRaisesRegex(ValueError, "digest"):
                install(pins, cache, bins, "x86_64", fetch)
            self.assertFalse((bins / "mbx").exists())
            pins["x86_64"] = hashlib.sha256(data).hexdigest()
            install(pins, cache, bins, "x86_64", fetch)
            install(pins, cache, bins, "x86_64", fetch)
            self.assertEqual(len(requests), 2)
            self.assertEqual((bins / "mbx").read_bytes(), payload)
            self.assertTrue(os.access(bins / "mbx", os.X_OK))
