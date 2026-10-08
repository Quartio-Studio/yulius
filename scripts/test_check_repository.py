"""Negative cases prove boundary and integrity checks fail closed."""
from pathlib import Path
import tempfile
import unittest
from check_repository import STYLESHEET, ROOT, dependency_errors, link_errors, manifest_errors, skill_errors, style_errors


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


class StyleContracts(unittest.TestCase):
    ROOT_BLOCK = ":root { --fs: 13px; --lh: 20px; --radius: 6px; --font: system-ui; }\n"

    def test_tokens_and_scales_pass(self):
        css = self.ROOT_BLOCK + ".a { font-size: var(--fs); line-height: var(--lh); border-radius: var(--radius); padding: 4px 12px; gap: 8px; margin-top: 1px; font-weight: 600; }"
        self.assertEqual(style_errors(css), [])

    def test_literal_type_values_rejected(self):
        for rule in ("font-size: 14px", "line-height: 1.45", "font-family: Arial", "font-weight: 700", "font: 600 12px/1.4 system-ui"):
            with self.subTest(rule=rule):
                self.assertTrue(style_errors(self.ROOT_BLOCK + f".a {{ {rule}; }}"))

    def test_off_scale_radius_and_spacing_rejected(self):
        for rule in ("border-radius: 8px", "padding: 10px", "gap: 14px", "margin: 0 18px"):
            with self.subTest(rule=rule):
                self.assertTrue(style_errors(self.ROOT_BLOCK + f".a {{ {rule}; }}"))

    def test_shipped_stylesheet_follows_the_scales(self):
        self.assertEqual(style_errors((ROOT / STYLESHEET).read_text()), [])
