"""The project's merge-first quality jobs cover the entire former quality gate."""
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
        self.assertEqual(jobs["test"]["command"], "scripts/dev test")
        self.assertNotIn("matrix", jobs["test"])
        self.assertFalse(jobs["security"].get("after_merge"))
