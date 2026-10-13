"""Negative cases prove boundary and integrity checks fail closed."""
from pathlib import Path
import tempfile
import re
import unittest
from check_repository import STYLESHEET, TOKENS, ROOT, INDEX, BRAND, brand_errors, decision_errors, module_errors, preload_errors, startup_modules, brand_name_errors, inline_style_errors, control_height_errors, contrast_errors, copy_errors, contrast_ratio, theme_tokens, _resolve, dependency_errors, integration_test_errors, link_errors, manifest_errors, skill_deployment_errors, skill_errors, style_errors


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

    def test_composition_crate_links_its_integration_tests_into_one_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            tests = Path(tmp) / "tests"
            tests.mkdir()
            (tests / "support").mkdir()
            (tests / "support" / "mod.rs").write_text("")
            for name in ("disk", "cli"):
                (tests / f"{name}.rs").write_text("#[test]\nfn works() {}\n")

            def crate(layer, *files):
                targets = [{"name": "app", "kind": ["lib"], "src_path": f"{tmp}/src/lib.rs"}]
                targets += [{"name": Path(f).stem, "kind": ["test"], "src_path": str(tests / f)} for f in files]
                return {"name": "app", "manifest_path": f"{tmp}/Cargo.toml",
                        "metadata": {"yulius": {"layer": layer}}, "targets": targets}

            per_file = integration_test_errors([crate("composition", "disk.rs", "cli.rs")])
            self.assertEqual(len(per_file), 1)
            self.assertIn("2 integration test binaries", per_file[0])
            self.assertEqual(integration_test_errors([crate("adapter", "disk.rs", "cli.rs")]), [])
            no_target = integration_test_errors([crate("composition")])
            self.assertEqual(len(no_target), 1)
            self.assertIn("tests/main.rs", no_target[0])

            (tests / "main.rs").write_text("mod support;\nmod disk;\n// mod cli;\n/* old:\nmod cli;\n*/\n")
            stray = integration_test_errors([crate("composition", "main.rs")])
            self.assertEqual(len(stray), 1)
            self.assertIn("tests/cli.rs", stray[0])
            (tests / "main.rs").write_text("mod support;\n#[cfg(unix)] pub(crate) mod disk;\n#[cfg(unix)]\nmod cli;\n")
            self.assertEqual(integration_test_errors([crate("composition", "main.rs")]), [])

            (tests / "flows").mkdir()
            (tests / "flows" / "main.rs").write_text("#[test]\nfn works() {}\n")
            directory = integration_test_errors([crate("composition", "main.rs")])
            self.assertEqual(len(directory), 1)
            self.assertIn("tests/flows/main.rs", directory[0])

    def test_skill_directory_must_match_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "sample-skill"
            directory.mkdir()
            skill = directory / "SKILL.md"
            skill.write_text("---\nname: sample-skill\ndescription: Do the thing. Use for sample work.\n---\nInstructions.\n")
            self.assertEqual(skill_errors(skill), [])
            skill.write_text(skill.read_text().replace("name: sample-skill", "name: wrong"))
            self.assertTrue(skill_errors(skill))

    def test_two_decision_records_cannot_share_a_number(self):
        with tempfile.TemporaryDirectory() as tmp:
            decisions = Path(tmp)
            index = "# Decision records\n\n- [0006](0006-first.md): First\n- [0007](0007-second.md): Second\n"
            (decisions / "README.md").write_text(index)
            (decisions / "0006-first.md").write_text("# 0006. First\n")
            (decisions / "0007-second.md").write_text("# 0007. Second\n")
            self.assertEqual(decision_errors(decisions), [])
            # A record the index does not list is reported.
            (decisions / "README.md").write_text(index.replace("- [0007](0007-second.md): Second\n", ""))
            errors = decision_errors(decisions)
            self.assertEqual(len(errors), 1)
            self.assertIn("0007-second.md", errors[0])
            (decisions / "README.md").write_text(index + "- [0007](0007-parallel.md): Parallel\n- [0008](0008-parallel.md): Parallel\n")
            (decisions / "0007-parallel.md").write_text("# 0007. Parallel\n")
            errors = decision_errors(decisions)
            self.assertEqual(len(errors), 1)
            self.assertIn("0007-parallel.md", errors[0])
            self.assertIn("0007-second.md", errors[0])
            # A renumbered record whose title kept the old number is caught too.
            (decisions / "0007-parallel.md").rename(decisions / "0008-parallel.md")
            errors = decision_errors(decisions)
            self.assertEqual(len(errors), 1)
            self.assertIn("0008-parallel.md", errors[0])
            (decisions / "0008-parallel.md").write_text("# 0008. Parallel\n")
            (decisions / "notes.md").write_text("# Notes\n")
            self.assertEqual(len(decision_errors(decisions)), 1)
        self.assertEqual(decision_errors(ROOT / "docs/decisions"), [])



class StyleContracts(unittest.TestCase):

    def test_skill_description_must_say_when_to_use_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "sample-skill"
            directory.mkdir()
            skill = directory / "SKILL.md"
            skill.write_text("---\nname: sample-skill\ndescription: Does the thing.\n---\nInstructions.\n")
            self.assertTrue(any("Use when" in e for e in skill_errors(skill)))
            skill.write_text(skill.read_text().replace("Does the thing.", "Does the thing, use it often."))
            self.assertTrue(skill_errors(skill))

    def test_skill_body_over_the_limit_must_move_detail_to_references(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "sample-skill"
            directory.mkdir()
            skill = directory / "SKILL.md"
            head = "---\nname: sample-skill\ndescription: Do it. Use for sample work.\n---\n"
            skill.write_text(head + "line\n" * 200)
            self.assertEqual(skill_errors(skill), [])
            skill.write_text(head + "line\n" * 201)
            self.assertTrue(any("references/" in e for e in skill_errors(skill)))

    def deployed(self, tmp):
        source, target = Path(tmp) / "src", Path(tmp) / "out"
        (source / "a").mkdir(parents=True)
        (source / "a/SKILL.md").write_text("one")
        (target / "a").mkdir(parents=True)
        (target / "a/SKILL.md").write_text("one")
        return source, target

    def test_deployed_skills_equal_their_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = self.deployed(tmp)
            self.assertEqual(skill_deployment_errors(source, [target]), [])

    def test_hand_edited_missing_and_extra_deployed_files_are_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, target = self.deployed(tmp)
            (target / "a/SKILL.md").write_text("edited")
            self.assertTrue(any("a/SKILL.md" in e for e in skill_deployment_errors(source, [target])))
            (target / "a/SKILL.md").unlink()
            self.assertTrue(any("missing" in e for e in skill_deployment_errors(source, [target])))
            (target / "a/SKILL.md").write_text("one")
            (target / "b").mkdir()
            (target / "b/SKILL.md").write_text("x")
            self.assertTrue(any("not in .apm/skills" in e for e in skill_deployment_errors(source, [target])))

    def test_tokens_and_scales_pass(self):
        css = ".a { color: var(--text); background: var(--surface); box-shadow: var(--shadow); font-size: var(--fs); line-height: var(--lh); border-radius: var(--radius); padding: 4px 12px; gap: 8px; margin-top: 1px; font-weight: 600; white-space: nowrap; }"
        self.assertEqual(style_errors(css), [])

    def test_literal_type_values_rejected(self):
        for rule in ("font-size: 14px", "line-height: 1.45", "font-family: Arial", "font-weight: 700", "font: 600 12px/1.4 system-ui"):
            with self.subTest(rule=rule):
                self.assertTrue(style_errors(f".a {{ {rule}; }}"))

    def test_off_scale_radius_and_spacing_rejected(self):
        for rule in ("border-radius: 8px", "padding: 10px", "gap: 14px", "margin: 0 18px", "transition: opacity .15s", "transition: width 0.3s ease", "z-index: 999"):
            with self.subTest(rule=rule):
                self.assertTrue(style_errors(f".a {{ {rule}; }}"))

    def test_colour_literals_rejected(self):
        for rule in ("color: #fff", "background: rgba(0, 0, 0, 0.5)", "border-color: hsl(10 50% 50%)", "fill: white", "--x: #123456"):
            with self.subTest(rule=rule):
                self.assertTrue(style_errors(f".a {{ {rule}; }}"))

    def test_shipped_stylesheet_follows_the_scales(self):
        self.assertEqual(style_errors((ROOT / STYLESHEET).read_text()), [])


class ContrastContracts(unittest.TestCase):
    def test_wcag_ratio_matches_reference_values(self):
        self.assertAlmostEqual(contrast_ratio((0, 0, 0, 1), (255, 255, 255, 1)), 21.0, places=2)
        self.assertAlmostEqual(contrast_ratio((118, 118, 118, 1), (255, 255, 255, 1)), 4.54, places=2)

    def test_tokens_resolve_through_var_and_color_mix(self):
        env = theme_tokens(":root { --brand: #4c9dff; } :root, [data-theme=\"dark\"] { --accent: var(--brand); } :root { --soft: color-mix(in srgb, var(--accent) 14%, transparent); }")["dark"]
        r, g, b, a = _resolve("--soft", env)
        self.assertEqual((round(r), round(g), round(b), round(a, 2)), (76, 157, 255, 0.14))

    def test_low_contrast_role_is_reported_per_theme(self):
        # A pale --text-faint in the light theme only (whatever the palette): reported for [light].
        tokens = (ROOT / TOKENS).read_text()
        light = tokens.index('[data-theme="light"]')
        css = tokens[:light] + re.sub(r"--text-faint: [^;]+;", "--text-faint: #c0c4cc;", tokens[light:], count=1)
        self.assertTrue(any("[light]: --text-faint" in e for e in contrast_errors(css)))

    def test_shipped_tokens_meet_contrast(self):
        self.assertEqual(contrast_errors((ROOT / TOKENS).read_text()), [])


class CopyTests(unittest.TestCase):
    def test_requirement_ids_in_strings(self):
        self.assertEqual(copy_errors("toast('Merged (GIT-002).');"), ["web script:1: requirement ID in a user-visible string; say it in words"])
        self.assertEqual(copy_errors("// GIT-002: merges are human actions\n/* SPEC-001 §6.2 */\nconst a = 'Merged';"), [])



class BrandTests(unittest.TestCase):
    def test_index_follows_brand_and_tokens(self):
        index, brand, tokens = ((ROOT / path).read_text() for path in (INDEX, BRAND, TOKENS))
        self.assertEqual(brand_errors(index, brand, tokens), [])
        renamed = brand.replace("name: 'Yulius'", "name: 'Acme'")
        self.assertEqual(sorted(e.split(": ")[1].split(" ")[0] for e in brand_errors(index, renamed, tokens)), ["noscript", "title"])
        restyled = brand.replace("markStyle: 'fill'", "markStyle: 'stroke'")
        self.assertTrue(any(": icon " in e for e in brand_errors(index, restyled, tokens)))
        recoloured = tokens.replace("--brand-mark: var(--text);", "--brand-mark: var(--accent);")
        self.assertTrue(any(": icon " in e for e in brand_errors(index, brand, recoloured)))



class InlineStyleTests(unittest.TestCase):
    def test_only_computed_values_inline(self):
        self.assertEqual(inline_style_errors("h('i', { style: { width: `${pct}%`, '--cols': String(n), color: 'var(--ok)' } })"), [])
        self.assertTrue(inline_style_errors("h('i', { style: { maxWidth: '640px' } })"))
        self.assertTrue(inline_style_errors("h('i', { style: { color: '#ff0000' } })"))



class ControlHeightTests(unittest.TestCase):
    def test_controls_use_tokens(self):
        self.assertEqual(control_height_errors(".btn { height: var(--control-md); } .tab-dot { height: 7px; } .btn.busy::after { height: 12px; }"), [])
        self.assertTrue(control_height_errors(".ed-tab { height: 30px; }"))
        self.assertEqual(control_height_errors((ROOT / STYLESHEET).read_text()), [])



class BrandNameTests(unittest.TestCase):
    def test_name_only_from_brand(self):
        self.assertTrue(brand_name_errors("toast('Cannot reach the Acme server.');", "Acme"))
        self.assertEqual(brand_name_errors("headers['X-Acme-CSRF'] = t; // Acme notes\ntoast(`${BRAND.name} restarted`);", "Acme"), [])



class BypassTests(unittest.TestCase):
    def test_closed_bypasses(self):
        for rule in ("color: var(--x, #f00)", "background: crimson", "border-color: Cyan", "animation: spin 1s linear", "color: color(display-p3 1 0 0)"):
            self.assertTrue(style_errors(f".a {{ {rule}; }}"), rule)
        self.assertEqual(style_errors(".a { color: var(--text); border-color: transparent; animation: spin var(--duration-spin) linear infinite; }"), [])
        self.assertTrue(inline_style_errors("el.style.color = '#ff0000';"))
        self.assertTrue(inline_style_errors("el.style.setProperty('width', '640px');"))
        self.assertEqual(inline_style_errors("el.style.width = `${pct}%`; el.style.opacity = '0.6';"), [])
        css = (ROOT / TOKENS).read_text().replace('[data-theme="light"]', "[data-theme=light]")
        self.assertIn("light", theme_tokens(css))



class ModuleTests(unittest.TestCase):
    def test_imports_match_exports(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a.js").write_text("export function used() {}\nexport const other = 1;\n")
            (root / "a.js").write_text("export function used() {}\nexport const other = 1;\nexport const spread = {};\n")
            (root / "b.js").write_text("import { used, missing, other, spread } from './a.js';\nused();\nconst x = { ...spread };\nx.other = 2;\n")
            errors = module_errors(root, root)
            self.assertTrue(any("missing" in e and "does not export" in e for e in errors))
            self.assertTrue(any("other" in e and "never uses" in e for e in errors))
        self.assertEqual(module_errors((ROOT / STYLESHEET).parent), [])

    def test_preloads_follow_static_imports(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "pages").mkdir()
            (root / "app.js").write_text("import { a } from './a.js';\nimport './pages/p.js';\nconst lazy = () => import('./lazy.js');\n")
            (root / "a.js").write_text("// pages/*.js are pages\nexport { b } from './b.js';\n// import { x } from './commented.js';\nexport const a = 1; /* note */\n")
            (root / "b.js").write_text("export const b = 2;\n")
            (root / "pages" / "p.js").write_text("import { a } from '../a.js';\n")
            (root / "lazy.js").write_text("export const z = 0;\n")
            self.assertEqual(startup_modules(root), ["/assets/a.js", "/assets/pages/p.js", "/assets/b.js"])
            good = '<link rel="modulepreload" href="/assets/a.js">\n<link rel="modulepreload" href="/assets/pages/p.js">\n<link rel="modulepreload" href="/assets/b.js">\n'
            self.assertEqual(preload_errors(good, root), [])
            self.assertTrue(preload_errors(good.replace("/assets/b.js", "/assets/lazy.js"), root))
        self.assertEqual(preload_errors((ROOT / INDEX).read_text(), (ROOT / STYLESHEET).parent), [])


if __name__ == "__main__":
    unittest.main()
