#!/usr/bin/env python3
"""Validate the repository's narrow, documented engineering contracts."""
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from urllib.parse import quote, unquote, urlsplit

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
INDEX = "crates/yulius-server/web/index.html"
BRAND = "crates/yulius-server/web/assets/brand.js"
# The brand parts of index.html: what shows before the app's scripts run (brand.js, tokens.css).
BRAND_PATTERNS = {
    "title": r"<title>[^<]*</title>",
    "theme-color": r'<meta name="theme-color" content="[^"]*">',
    "icon": r'<link rel="icon" href="[^"]*">',
    "noscript": r"<noscript>[^<]*</noscript>",
}
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
        elif prop in ("transition", "transition-duration") and re.search(r"(?<![\w-])\d*\.?\d+m?s\b", value):
            errors.append(f"{where} (use the --duration* motion tokens)")
        elif prop == "border-radius" and any(not re.fullmatch(r"var\(--radius(-[a-z]+)?\)|0|50%|inherit", part) for part in value.split()):
            errors.append(f"{where} (use a --radius-* token, 0 or 50%)")
        elif re.fullmatch(r"padding(-[a-z]+)?|margin(-[a-z]+)?|gap|row-gap|column-gap", prop):
            for number in re.findall(r"(-?\d+(?:\.\d+)?)px", value):
                size = abs(float(number))
                if size not in SPACING_SCALE and not (size == 1 and prop.startswith("margin")):
                    errors.append(f"{where} ({number}px is off the 4 px spacing scale)")
    return errors


# Text pairs need 4.5:1 and non-text (state marks, focus, control edges) 3:1 (WCAG 2.2 1.4.3,
# 1.4.11). A background "a/b" is a translucent role a composited over b.
CONTRAST_PAIRS = [
    *((fg, bg, 4.5) for fg in ("--text", "--text-2", "--muted", "--text-faint") for bg in ("--bg", "--surface", "--surface-2")),
    ("--text", "--surface-3", 4.5), ("--muted", "--surface-hover", 4.5),
    ("--accent-ink", "--accent", 4.5), ("--term-fg", "--term-bg", 4.5), ("--ansi-bright-black", "--term-bg", 4.5),
    *((f"--{hue}-fg", bg, 4.5) for hue in ("accent", "ok", "warn", "danger", "violet") for bg in ("--surface", f"--{hue}-soft/--surface")),
    ("--warn-fg-strong", "--warn-soft/--surface", 4.5), ("--danger-fg-strong", "--danger-soft/--surface", 4.5),
    ("--accent-fg-strong", "--accent-soft/--surface", 4.5), ("--agent-ink", "--agent", 4.5),
    *((mark, bg, 3.0) for mark in ("--accent", "--ok", "--warn", "--danger", "--violet") for bg in ("--bg", "--surface")),
    ("--border-control", "--bg", 3.0), ("--border-control", "--surface", 3.0), ("--border-control", "--surface-2", 3.0),
]


def _rgba(text):
    text = text.strip()
    if text.startswith("#"):
        h = text[1:]
        h = "".join(c * 2 for c in h) if len(h) in (3, 4) else h
        parts = [int(h[i:i + 2], 16) for i in range(0, len(h), 2)]
        return (*parts[:3], parts[3] / 255 if len(parts) == 4 else 1.0)
    m = re.fullmatch(r"rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)(?:[ ,/]+([\d.]+))?\s*\)", text)
    if m:
        return (float(m[1]), float(m[2]), float(m[3]), float(m[4]) if m[4] else 1.0)
    if text == "transparent":
        return (0.0, 0.0, 0.0, 0.0)
    raise ValueError(f"unsupported colour {text!r}")


def _resolve(name, env, seen=()):
    if name in seen or name not in env:
        raise ValueError(f"unresolved token {name}")
    value = env[name]
    value = re.sub(r"var\((--[\w-]+)\)", lambda m: _format(_resolve(m[1], env, seen + (name,))), value)
    m = re.fullmatch(r"color-mix\(in srgb,\s*(.+?)\s+([\d.]+)%,\s*(.+)\)", value.strip())
    if m:
        a, share, b = _rgba(m[1]), float(m[2]) / 100, _rgba(m[3])
        # Premultiplied mix, as in CSS Color 5.
        alpha = a[3] * share + b[3] * (1 - share)
        if alpha == 0:
            return (0.0, 0.0, 0.0, 0.0)
        rgb = [(a[i] * a[3] * share + b[i] * b[3] * (1 - share)) / alpha for i in range(3)]
        return (*rgb, alpha)
    return _rgba(value)


def _format(rgba):
    return f"rgba({rgba[0]}, {rgba[1]}, {rgba[2]}, {rgba[3]})"


def _over(top, bottom):
    a = top[3]
    return tuple(top[i] * a + bottom[i] * (1 - a) for i in range(3)) + (1.0,)


def _luminance(rgba):
    def channel(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (channel(v) for v in rgba[:3])
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(fg, bg):
    fg = _over(fg, bg)
    hi, lo = sorted((_luminance(fg), _luminance(bg)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def theme_tokens(css):
    """Resolvable token maps per theme from tokens.css: {'dark': {...}, 'light': {...}}."""
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    base, themes = {}, {}
    for selector, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css):
        decls = dict((k.strip(), v.strip()) for k, v in re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", body))
        names = re.findall(r'data-theme="(\w+)"', selector)
        if ":root" in selector:
            base.update(decls)
        for name in names:
            themes.setdefault(name, {}).update(decls)
    return {name: {**base, **decls} for name, decls in themes.items()}


def contrast_errors(css, label=TOKENS):
    errors = []
    for theme, env in theme_tokens(css).items():
        for fg, bg, minimum in CONTRAST_PAIRS:
            try:
                parts = bg.split("/")
                back = _resolve(parts[-1], env)
                for layer in reversed(parts[:-1]):
                    back = _over(_resolve(layer, env), back)
                ratio = contrast_ratio(_resolve(fg, env), back)
            except ValueError as error:
                errors.append(f"{label} [{theme}]: {error}")
                continue
            if ratio < minimum:
                errors.append(f"{label} [{theme}]: {fg} on {bg} is {ratio:.2f}:1, needs {minimum}:1")
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


STYLE_OBJECT = re.compile(r"style:\s*\{([^{}]*)\}")
FIXED_STYLE_VALUE = re.compile(r"""(['"])[^'"]*?(?:\d(?:px|rem|em|%|vh|vw)\b|#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\()[^'"]*\1""")


def inline_style_errors(js, label="web script"):
    """Inline styles may carry computed values only; fixed lengths and colours belong in styles.css."""
    errors = []
    for match in STYLE_OBJECT.finditer(js):
        if FIXED_STYLE_VALUE.search(match[1]):
            number = js.count("\n", 0, match.start()) + 1
            errors.append(f"{label}:{number}: fixed value in an inline style; use a class in styles.css and tokens")
    return errors


REQUIREMENT_IN_STRING = re.compile(r"""(['"`])(?:(?!\1).)*?\b[A-Z]{2,4}-\d{3}\b(?:(?!\1).)*?\1""")


def copy_errors(js, label="web script"):
    """Requirement IDs such as GIT-002 belong in comments and specs, not in user-visible strings."""
    errors = []
    js = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), js, flags=re.S)
    for number, line in enumerate(js.splitlines(), 1):
        if line.lstrip().startswith("//"):
            continue
        if REQUIREMENT_IN_STRING.search(line):
            errors.append(f"{label}:{number}: requirement ID in a user-visible string; say it in words")
    return errors


def _hex(rgba):
    return "#%02x%02x%02x" % tuple(round(c) for c in rgba[:3])


def brand_head(brand_js, tokens_css):
    """index.html's brand parts as brand.js and the default (dark) theme tokens make them."""
    fields = {}
    for field in ("name", "mark"):
        m = re.search(field + r"""\s*:\s*(['"])(.*?)\1""", brand_js)
        if not m:
            raise ValueError(f"{BRAND}: BRAND.{field} not found")
        fields[field] = m[2]
    env = theme_tokens(tokens_css)["dark"]
    bg, surface, accent = (_hex(_resolve(name, env)) for name in ("--bg", "--surface", "--accent"))
    svg = (f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><rect width='24' height='24' rx='5.5' fill='{surface}'/>"
           f"<path d='{fields['mark']}' stroke='{accent}' stroke-width='2.4' fill='none' stroke-linecap='round' stroke-linejoin='round'/></svg>")
    return {
        "title": f"<title>{fields['name']}</title>",
        "theme-color": f'<meta name="theme-color" content="{bg}">',
        "icon": f'<link rel="icon" href="data:image/svg+xml,{quote(svg, safe=chr(39) + " /:=.-")}">',
        "noscript": f"<noscript>{fields['name']} needs JavaScript.</noscript>",
    }


def brand_errors(index_html, brand_js, tokens_css, label=INDEX):
    """index.html must show the same name, colours and mark as brand.js and tokens.css."""
    errors = []
    for key, expected in brand_head(brand_js, tokens_css).items():
        found = re.search(BRAND_PATTERNS[key], index_html)
        if not found or found[0] != expected:
            errors.append(f"{label}: {key} does not match brand.js and tokens.css; run scripts/check_repository.py --sync-brand")
    return errors


def sync_brand():
    """Rewrite index.html's brand parts from brand.js and tokens.css."""
    path = ROOT / INDEX
    index_html = path.read_text()
    for key, expected in brand_head((ROOT / BRAND).read_text(), (ROOT / TOKENS).read_text()).items():
        index_html, count = re.subn(BRAND_PATTERNS[key], lambda _: expected, index_html, count=1)
        if not count:
            raise ValueError(f"{INDEX}: no {key} element to update")
    path.write_text(index_html)
    print(f"{INDEX} now matches brand.js and tokens.css")


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
    for path in sorted((ROOT / STYLESHEET).parent.glob("**/*.js")):
        if "vendor" not in path.parts:
            errors.extend(copy_errors(path.read_text(), str(path.relative_to(ROOT))))
            errors.extend(inline_style_errors(path.read_text(), str(path.relative_to(ROOT))))
    errors.extend(contrast_errors((ROOT / TOKENS).read_text()))
    errors.extend(brand_errors((ROOT / INDEX).read_text(), (ROOT / BRAND).read_text(), (ROOT / TOKENS).read_text()))
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
        elif sys.argv[1:] == ["--sync-brand"]:
            sync_brand()
        elif not sys.argv[1:]:
            check()
        else:
            raise ValueError("Usage: check_repository.py [--doctor | --sync-brand]")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"Repository check failed: {error}")
