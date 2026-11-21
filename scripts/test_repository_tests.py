"""Regression cases for the pre-merge file-reader job."""
from pathlib import Path
from fnmatch import fnmatchcase
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RepositoryTests(unittest.TestCase):
    def test_the_gate_selects_repository_readers_before_merge(self):
        jobs = {job["name"]: job for job in tomllib.loads((ROOT / "yulius.toml").read_text())["ci"]["jobs"]}
        job = jobs["file-contracts"]
        self.assertFalse(job.get("after_merge"))
        self.assertIn("scripts/repository_tests.py", job["command"])
        for path in ("crates/**", "catalog/**", "docs/**"):
            self.assertIn(path, job["paths"])
        self.assertTrue(jobs["test"]["after_merge"])
        self.assertTrue(jobs["e2e"]["after_merge"])

    def test_native_paths_cover_every_discovered_tracked_input(self):
        from repository_tests import inventory
        jobs = tomllib.loads((ROOT / "yulius.toml").read_text())["ci"]["jobs"]
        patterns = next(job["paths"] for job in jobs if job["name"] == "file-contracts")
        # Include the ancestors of tracked files, as a reader may enumerate a directory.
        tracked = set()
        for line in subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0"):
            if line:
                path = Path(line.decode())
                tracked.add(str(path))
                tracked.update(str(parent) for parent in path.parents)
        inputs = {dep for reader in inventory(ROOT) for dep in reader.inputs}
        missing = {dep for dep in inputs & tracked if not any(
            fnmatchcase(dep, pattern) or pattern == dep + "/**" for pattern in patterns)}
        self.assertEqual(missing, set())

    def test_selects_all_web_contracts_and_other_text_readers(self):
        from repository_tests import inventory, select
        readers = inventory(ROOT)
        def names(*paths):
            return {reader.name for reader in select(readers, paths)}
        web = names("crates/yulius-server/web/assets/mock.js")
        self.assertIn("the_composer_offers_antigravity_on_its_default_model", web)
        self.assertIn("every_end_has_the_web_uis_words", names("crates/yulius-server/web/assets/status.js"))
        self.assertIn("the_composer_offers_the_models_the_server_checks", names("crates/yulius-server/web/assets/models.js"))
        self.assertIn("the_operations_guide_shows_the_units_the_setup_writes", names("docs/operations.md"))
        for path in ("docs/engineering/workflow.md", "catalog/skills/new/SKILL.md", "AGENTS.md"):
            self.assertIn("every_place_states_one_rule_for_local_checks", names(path))
        self.assertIn("default_workflows_follow_the_discipline", names("catalog/workflows/fix.md"))
        self.assertIn("the_repositorys_own_cd_is_defined", names("yulius.toml"))
        self.assertFalse(names("some-unrelated-file.txt"))

    def test_a_new_reader_and_a_shared_helper_need_no_registration(self):
        from repository_tests import inventory, select
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            crate = root / "crates/example"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text('[package]\nname = "example"\n')
            (crate / "src/lib.rs").write_text('''
#[cfg(test)]
mod tests {
    fn words() -> &'static str { include_str!(
        "../../../docs/guide.md"
    ) }
    #[test]
    fn reads_words() { assert!(!words().is_empty()); }
    #[tokio::test]
    async fn reads_words_again() { assert!(words().contains("guide")); }
}
''')
            readers = select(inventory(root), ["docs/guide.md"])
            self.assertEqual({reader.name for reader in readers}, {"reads_words", "reads_words_again"})

    def test_workspace_root_helper_and_literal_runtime_read(self):
        from repository_tests import inventory, select
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            crate = root / "crates/sample"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text('[package]\nname = "sample"\n')
            (crate / "src/lib.rs").write_text('''
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
#[test]
fn reads_a_guide() {
    std::fs::read(workspace().join("docs/guide.md")).unwrap();
}
#[test]
fn reads_a_joined_directory() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("examples");
    std::fs::read_dir(directory).unwrap();
}
#[test]
fn reads_an_embedded_config() {
    let config = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/new.toml"));
    assert!(!config.is_empty());
}
#[test]
fn reads_a_config() {
    std::fs::read_to_string("../../settings.ini").unwrap();
}
''')
            readers = inventory(root)
            self.assertIn("reads_a_guide", {r.name for r in select(readers, ["docs/new.md"])})
            self.assertIn("reads_a_joined_directory", {r.name for r in select(readers, ["examples/new.txt"])})
            self.assertIn("reads_an_embedded_config", {r.name for r in select(readers, ["deploy/new.toml"])})
            self.assertIn("reads_a_config", {r.name for r in select(readers, ["settings.ini"])})

    def test_a_reader_in_a_separate_helper_cannot_silently_disappear(self):
        from repository_tests import inventory
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            crate = root / "crates/sample"
            (crate / "tests/support").mkdir(parents=True)
            (crate / "Cargo.toml").write_text('[package]\nname = "sample"\n')
            (crate / "tests/support/data.rs").write_text(
                'pub const GUIDE: &str = include_str!("../../../../docs/guide.md");')
            with self.assertRaisesRegex(ValueError, "tests/support/data.rs"):
                inventory(root)

    def test_diff_includes_deletions_and_both_sides_of_a_rename(self):
        from repository_tests import changed_paths
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            def git(*args):
                subprocess.run(["git", "-C", root, *args], check=True, capture_output=True)
            git("init", "-q")
            for name in ("before", "deleted"):
                (root / name).write_text(name)
            git("add", ".")
            git("-c", "user.name=test", "-c", "user.email=test@example.com", "commit", "-qm", "base")
            git("branch", "base")
            (root / "before").rename(root / "after")
            (root / "deleted").unlink()
            git("add", "-A")
            git("-c", "user.name=test", "-c", "user.email=test@example.com", "commit", "-qm", "change")
            self.assertEqual(set(changed_paths(root, "base")), {"before", "after", "deleted"})
            self.assertIsNone(changed_paths(root, "missing"))
