"""Negative cases prove boundary and integrity checks fail closed."""
from pathlib import Path
import tempfile
import unittest
from check_repository import dependency_errors, link_errors, manifest_errors, skill_errors


def package(name, layer, dependencies=()):
    return {"name": name, "manifest_path": f"/workspace/{name}/Cargo.toml",
            "metadata": {"yulius": {"layer": layer}}, "dependencies": list(dependencies)}


class RepositoryContracts(unittest.TestCase):
    def test_domain_cannot_import_adapter_through_renamed_target_dependency(self):
        dep = {"name": "storage", "rename": "db", "path": "/workspace/storage", "kind": "build", "target": "cfg(unix)"}
        self.assertTrue(dependency_errors([package("core", "domain", [dep]), package("storage", "adapter")]))

    def test_adapter_can_depend_on_domain(self):
        dep = {"name": "core", "path": "/workspace/core"}
        self.assertEqual(dependency_errors([package("core", "domain"), package("storage", "adapter", [dep])]), [])

    def test_product_cannot_depend_on_tooling(self):
        dep = {"name": "xtask", "path": "/workspace/xtask", "kind": "dev"}
        self.assertTrue(dependency_errors([package("app", "composition", [dep]), package("xtask", "tooling")]))

    def test_unknown_layer_and_unclassified_path_fail_closed(self):
        self.assertTrue(dependency_errors([package("app", "other")]))
        self.assertTrue(dependency_errors([package("app", "adapter", [{"name": "outside", "path": "/elsewhere"}])]))

    def test_domain_external_io_dependency_rejected(self):
        self.assertTrue(dependency_errors([package("core", "domain", [{"name": "tokio"}])]))

    def test_lint_opt_out_rejected(self):
        manifest = {"package": {"edition": {"workspace": True}, "rust-version": {"workspace": True},
                               "license": {"workspace": True}, "metadata": {"yulius": {"layer": "tooling"}}},
                    "lints": {"workspace": True}}
        self.assertEqual(manifest_errors(manifest, "test"), [])
        manifest["lints"] = {}
        self.assertTrue(manifest_errors(manifest, "test"))

    def test_missing_and_outside_links_rejected_but_web_and_code_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            doc = root / "doc.md"
            doc.write_text('[bad](absent.md) [outside](../outside.md) [web](https://example.com)\n```\n[example](ignored.md)\n```')
            self.assertEqual(len(link_errors(doc, root)), 2)
            (root / "exists.md").write_text("# Exists")
            doc.write_text("[good](exists.md#heading)")
            self.assertEqual(link_errors(doc, root), [])

    def test_skill_directory_must_match_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "sample-skill"
            directory.mkdir()
            skill = directory / "SKILL.md"
            skill.write_text("---\nname: sample-skill\ndescription: Do the thing.\n---\nInstructions.\n")
            self.assertEqual(skill_errors(skill), [])
            skill.write_text(skill.read_text().replace("name: sample-skill", "name: wrong"))
            self.assertTrue(skill_errors(skill))


if __name__ == "__main__":
    unittest.main()
