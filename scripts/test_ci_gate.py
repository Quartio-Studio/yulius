"""The project's merge-first quality jobs cover the entire former quality gate."""
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent


class Gate(unittest.TestCase):
    def test_quality_passes_have_independent_pr_results(self):
        jobs = {job["name"]: job for job in tomllib.loads(
            (ROOT / "yulius.toml").read_text())["ci"]["jobs"]}
        for name, commands in {
            "quality": ["scripts/dev quality"],
            "lint": ["scripts/dev lint"],
            "macos": ["scripts/dev macos"],
            "docs": ["scripts/dev docs"],
            "deps": ["scripts/dev deps"],
        }.items():
            with self.subTest(name=name):
                self.assertFalse(jobs[name].get("after_merge"))
                self.assertEqual(jobs[name]["command"], " && ".join(commands))
        self.assertTrue(jobs["e2e"]["after_merge"])
        self.assertTrue(jobs["test"]["after_merge"])
        self.assertEqual(jobs["test"]["command"], "scripts/dev test-shard")
        self.assertFalse(jobs["security"].get("after_merge"))


class SharedTestBuild(unittest.TestCase):
    def setUp(self):
        # Fixture stage markers must not open spans in the enclosing quality job.
        self.enterContext(redirect_stdout(StringIO()))

    def test_two_shards_share_one_build_and_run_concurrently(self):
        from ci_test_shard import run_shard
        from concurrent.futures import ThreadPoolExecutor
        import tempfile
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            barrier = threading.Barrier(2, timeout=5)
            builds = []

            def build(archive):
                builds.append(archive)
                archive.write_bytes(b"this run")
                return 0

            def run(archive, both):
                self.assertFalse(both)
                barrier.wait()
                self.assertEqual(archive.read_bytes(), b"this run")
                return 0

            with ThreadPoolExecutor(2) as workers:
                results = [workers.submit(run_shard, home, home / f"test.{i}@1",
                                          "run-a", "head-a", i, build, run) for i in (2, 1)]
                self.assertEqual([r.result() for r in results], [0, 0])
            self.assertEqual(len(builds), 1)
            self.assertFalse(list(home.rglob("*.tar.zst")))

    def test_failed_and_partial_builds_never_reach_a_consumer(self):
        from ci_test_shard import run_shard
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            builds = []

            def fail(archive):
                builds.append(archive)
                archive.write_bytes(b"incomplete")
                return 1

            for index in (1, 2):
                self.assertEqual(run_shard(home, home / f"test.{index}", "old", "head",
                                           index, fail, self.fail), 1 if index == 1 else 78)
            self.assertEqual(len(builds), 1)

            def build(archive):
                archive.write_bytes(b"new")
                return 0

            def failed_test(archive, both):
                self.assertFalse(both)
                self.assertEqual(archive.read_bytes(), b"new")
                return 1

            self.assertEqual(run_shard(home, home / "test.1", "new", "head", 1,
                                       build, failed_test), 1)

    def test_recovery_and_changed_definition_keep_archive_identity(self):
        from ci_test_shard import run_shard
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)

            def execute(cache, run_id, sha, index, content):
                def build(archive):
                    archive.write_bytes(content)
                    return 0

                def check(archive, both):
                    self.assertFalse(both)
                    self.assertEqual(archive.read_bytes(), content)
                    return 0

                return run_shard(home, home / cache, run_id, sha, index, build, check)

            def interrupted(archive):
                archive.write_bytes(b"partial")
                raise InterruptedError()

            with self.assertRaises(InterruptedError):
                run_shard(home, home / "test.2@1", "first", "sha", 2, interrupted, self.fail)
            self.assertEqual(execute("test.1@1", "first", "sha", 1, b"recovered"), 0)
            self.assertEqual(execute("test.change.2@1", "first", "sha", 2, b"candidate"), 0)
            self.assertEqual(execute("test.2@2", "other", "sha", 2, b"other lane"), 0)
            self.assertEqual(execute("test.2@1", "first", "sha", 2, b"recovered"), 0)
            self.assertEqual(execute("test.change.1@1", "first", "sha", 1, b"candidate"), 0)
            self.assertEqual(execute("test.1@2", "new", "different", 1, b"new run"), 0)

    def test_retry_after_peer_skipped_covers_the_whole_suite(self):
        from ci_test_shard import run_shard
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            def execute(index, build, run):
                return run_shard(home, home / f"test.{index}", "run", "sha", index, build, run)
            self.assertEqual(execute(2, lambda _: 1, self.fail), 1)
            self.assertEqual(execute(1, self.fail, self.fail), 78)

            def recover(archive):
                archive.write_bytes(b"recovered build")
                return 0

            covered = []
            self.assertEqual(execute(2, recover, lambda archive, both=False: covered.append(both) or 0), 0)
            self.assertEqual(covered, [True])
            self.assertFalse(list(home.rglob("*.tar.zst")))
