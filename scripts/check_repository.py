#!/usr/bin/env python3
"""Validate the repository's narrow, documented engineering contracts."""
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent
LAYERS = {
    "domain": {"domain"},
    "contracts": {"contracts"},
    "application": {"domain", "contracts", "application"},
    "adapter": {"domain", "contracts", "application", "adapter"},
    "interface": {"contracts", "application"},
    "composition": {"domain", "contracts", "application", "adapter", "interface", "composition"},
    "tooling": {"domain", "contracts", "application", "adapter", "interface", "composition", "tooling"},
}


def manifest_errors(manifest, label):
    errors = []
    for field in ("edition", "rust-version", "license"):
        if manifest.get("package", {}).get(field) != {"workspace": True}:
            errors.append(f"{label}: package.{field} must inherit workspace metadata")
    if manifest.get("lints") != {"workspace": True}:
        errors.append(f"{label}: lints must inherit the workspace")
    layer = manifest.get("package", {}).get("metadata", {}).get("yulius", {}).get("layer")
    if layer not in LAYERS:
        errors.append(f"{label}: missing or unknown package.metadata.yulius.layer")
    return errors


def dependency_errors(packages):
    """Cargo metadata names dependencies by real package name, even with rename."""
    errors = []
    by_path = {str(Path(p["manifest_path"]).parent.resolve()): p for p in packages}
    for package in packages:
        layer = package.get("metadata", {}).get("yulius", {}).get("layer")
        if layer not in LAYERS:
            errors.append(f"{package['name']}: unknown layer {layer!r}")
            continue
        for dep in package.get("dependencies", []):
            target = by_path.get(str(Path(dep["path"]).resolve())) if dep.get("path") else None
            if dep.get("path") and target is None:
                errors.append(f"{package['name']}: path dependency {dep['name']} is outside classified workspace")
            elif target:
                target_layer = target.get("metadata", {}).get("yulius", {}).get("layer")
                if target_layer not in LAYERS[layer]:
                    errors.append(f"{package['name']} ({layer}) cannot depend on {target['name']} ({target_layer})")
            elif layer == "domain" and dep["name"] != "thiserror":
                errors.append(f"{package['name']}: domain external dependency {dep['name']} needs a boundary decision")
    return errors


def skill_errors(path):
    """Repository skills deliberately use a plain scalar frontmatter subset."""
    text = path.read_text()
    errors = []
    match = re.match(r"\A---\n(.*?)\n---\n", text, re.S)
    if not match:
        return [f"{path}: missing skill frontmatter"]
    fields = {}
    for line in match[1].splitlines():
        key, separator, value = line.partition(": ")
        if not separator or key in fields or key not in {"name", "description"}:
            errors.append(f"{path}: use unique plain name/description frontmatter scalars")
        fields[key] = value
    name = fields.get("name", "")
    if name != path.parent.name or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", name) or len(name) > 64:
        errors.append(f"{path}: invalid or mismatched skill name")
    description = fields.get("description", "")
    if not description or len(description) > 1024 or ": " in description:
        errors.append(f"{path}: invalid plain scalar description")
    if not text[match.end():].strip():
        errors.append(f"{path}: empty instructions")
    return errors


def link_errors(path, root):
    errors = []
    text = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
    for raw in re.findall(r"\[[^\]]*\]\(([^)]+)\)", text):
        target = raw.strip().strip("<>")
        parsed = urlsplit(target)
        if parsed.scheme or not parsed.path:
            continue
        destination = (path.parent / unquote(parsed.path)).resolve()
        if not destination.is_relative_to(root.resolve()) or not destination.exists():
            errors.append(f"{path.relative_to(root)}: missing/outside local link {target}")
    return errors


SPACING_SCALE = {0, 2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48}
WEIGHTS = {"400", "500", "600", "inherit", "normal"}
TOKENS = "crates/yulius-server/web/assets/tokens.css"
STYLESHEET = "crates/yulius-server/web/assets/styles.css"
COLOUR_LITERAL = re.compile(r"#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(|"
                            r"\b(?:white|black|red|green|blue|yellow|orange|purple|pink|gray|grey|silver|navy|teal)\b")


def style_errors(css, label=STYLESHEET):
    """Design-system rules for the component stylesheet (web/README.md "Theming and branding"):
    colours, type, radii and spacing come only from tokens.css roles and scales."""
    errors = []
    css = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), css, flags=re.S)
    for match in re.finditer(r"(?<![-\w])(--[\w-]+|[a-z-]+)\s*:\s*([^;{}]+)", css):
        prop, value = match.group(1), match.group(2).strip()
        line = css.count("\n", 0, match.start()) + 1
        where = f"{label}:{line}: {prop}: {value}"
        if COLOUR_LITERAL.search(re.sub(r"var\([^)]*\)", "", value)):
            errors.append(f"{where} (colours come from tokens.css roles)")
        elif prop == "font-size" and not re.fullmatch(r"var\(--fs(-[a-z0-9]+)?\)|inherit", value):
            errors.append(f"{where} (use a --fs-* token)")
        elif prop == "line-height" and not re.fullmatch(r"var\(--lh(-[a-z0-9]+)?\)|1|normal|inherit", value):
            errors.append(f"{where} (use a --lh-* token)")
        elif prop == "font-family" and not re.fullmatch(r"var\(--font(-mono)?\)|inherit", value):
            errors.append(f"{where} (use --font or --font-mono)")
        elif prop == "font" and value != "inherit" and not (re.search(r"var\(--fs", value) and re.search(r"var\(--font", value)):
            errors.append(f"{where} (shorthand needs --fs-* and --font tokens)")
        elif prop == "font-weight" and value not in WEIGHTS:
            errors.append(f"{where} (weights are 400, 500, 600)")
        elif prop == "border-radius" and any(not re.fullmatch(r"var\(--radius(-[a-z]+)?\)|0|50%|inherit", part) for part in value.split()):
            errors.append(f"{where} (use a --radius-* token, 0 or 50%)")
        elif re.fullmatch(r"padding(-[a-z]+)?|margin(-[a-z]+)?|gap|row-gap|column-gap", prop):
            for number in re.findall(r"(-?\d+(?:\.\d+)?)px", value):
                size = abs(float(number))
                if size not in SPACING_SCALE and not (size == 1 and prop.startswith("margin")):
                    errors.append(f"{where} ({number}px is off the 4 px spacing scale)")
    return errors


def output(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def doctor():
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    compiler = output(["rustup", "run", channel, "rustc", "--version"])
    if compiler.split()[1] != channel:
        raise ValueError(f"Compiler mismatch: {compiler}")
    print(compiler)
    tools = tomllib.loads((ROOT / ".config/tools.toml").read_text())["tools"]
    for name, version in tools.items():
        executable = ROOT / ".tools/bin" / name
        result = output([str(executable), "--version"])
        if version not in result.split():
            raise ValueError(f"{name}: expected {version}, got {result}; rerun bootstrap")
        print(result)
    print(f"Python {sys.version.split()[0]}; pinned tools ready")


def check():
    errors = []
    names = output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]).split("\0")
    for name in filter(None, names):
        path = ROOT / name
        if path.is_file() and path.suffix == ".md":
            errors.extend(link_errors(path, ROOT))
    skills = list((ROOT / ".agents/skills").glob("*/SKILL.md"))
    if not skills:
        errors.append("No repository skills found")
    for path in skills:
        errors.extend(skill_errors(path))
    errors.extend(style_errors((ROOT / STYLESHEET).read_text()))
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    cargo = ["rustup", "run", channel, "cargo"]
    metadata = json.loads(output(cargo + ["metadata", "--locked", "--no-deps", "--format-version", "1"]))
    packages = [p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]]
    for package in packages:
        path = Path(package["manifest_path"])
        errors.extend(manifest_errors(tomllib.loads(path.read_text()), package["name"]))
    errors.extend(dependency_errors(packages))
    if errors:
        raise ValueError("\n".join(errors))
    print(f"Repository contracts passed: {len(packages)} crates, {len(skills)} skills, local Markdown links, UI design tokens")


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["--doctor"]:
            doctor()
        elif not sys.argv[1:]:
            check()
        else:
            raise ValueError("Usage: check_repository.py [--doctor]")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"Repository check failed: {error}")
