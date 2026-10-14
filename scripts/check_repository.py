#!/usr/bin/env python3
"""Validate the repository's narrow, documented engineering contracts."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
from urllib.parse import quote, unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent
SKILL_BODY_LINES = 200
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


def _declared_modules(rust):
    """Names of `mod name;` items, attributes and visibility allowed, comments ignored."""
    code = re.sub(r"/\*.*?\*/", "", rust, flags=re.S)
    code = re.sub(r"//[^\n]*", "", code)
    return set(re.findall(r"^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", code, re.M))


def integration_test_errors(packages):
    """A composition crate links the whole product into every test binary (about 190 MB each, so
    fifty files filled sandboxes: SPEC-033); its integration tests are modules of one
    `tests/main.rs`, and nothing under `tests/` may be left out of it."""
    errors = []
    for package in packages:
        if package.get("metadata", {}).get("yulius", {}).get("layer") != "composition":
            continue
        name = package["name"]
        tests = [t for t in package.get("targets", []) if "test" in t["kind"]]
        main = Path(package["manifest_path"]).parent / "tests" / "main.rs"
        if len(tests) > 1:
            errors.append(f"{name}: {len(tests)} integration test binaries each link the whole product; "
                          "make the files modules of one tests/main.rs (autotests = false)")
            continue
        if not tests or Path(tests[0]["src_path"]) != main:
            if main.parent.is_dir():
                errors.append(f"{name}: its integration tests must be the one [[test]] target at tests/main.rs")
            continue
        declared = _declared_modules(main.read_text())
        for file in sorted(main.parent.glob("*.rs")):
            if file != main and file.stem not in declared:
                errors.append(f"{name}: tests/{file.name} never runs; declare `mod {file.stem};` in tests/main.rs")
        for file in sorted(main.parent.glob("*/main.rs")):
            errors.append(f"{name}: tests/{file.parent.name}/main.rs never runs (autotests = false); "
                          "make it a module of tests/main.rs")
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
    if description and not re.search(r"\bUse (?:when|for|on|before|after|this)\b", description):
        errors.append(f"{path}: description must say when to use the skill (\"Use when ...\")")
    body = text[match.end():]
    if not body.strip():
        errors.append(f"{path}: empty instructions")
    if len(body.splitlines()) > SKILL_BODY_LINES:
        errors.append(f"{path}: body over {SKILL_BODY_LINES} lines; move detail to references/ or assets/")
    return errors


def skill_deployment_errors(source, targets):
    """APM deploys .apm/skills to each target; a hand edit there is drift (ADR-0016)."""
    def files(root):
        return {p.relative_to(root): p.read_bytes() for p in root.rglob("*") if p.is_file()} if root.is_dir() else {}
    wanted = files(source)
    errors = []
    for target in targets:
        found = files(target)
        for rel, data in wanted.items():
            if rel not in found:
                errors.append(f"{target / rel}: missing deployment of .apm/skills/{rel}; run apm install")
            elif found[rel] != data:
                errors.append(f"{target / rel}: differs from .apm/skills/{rel}; edit the source and run apm install")
        for rel in found.keys() - wanted.keys():
            errors.append(f"{target / rel}: not in .apm/skills; edit the source and run apm install")
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


def decision_errors(directory):
    """Each decision record has its own number, also in its title and index line, so "ADR-NNNN"
    names one record.

    Parallel tasks pick the next free number on their own; the second to merge fails here after
    merging dev and renumbers (#317)."""
    errors = []
    taken = {}
    readme = directory / "README.md"
    if not readme.is_file():
        return [f"{directory}: missing README.md index of the decision records"]
    index = readme.read_text()
    for path in sorted(directory.glob("*.md")):
        if path.name == "README.md":
            continue
        match = re.fullmatch(r"(\d{4})-[a-z0-9]+(?:-[a-z0-9]+)*\.md", path.name)
        if not match:
            errors.append(f"{path}: name decision records NNNN-<slug>.md")
            continue
        number = match[1]
        if number in taken:
            errors.append(f"{path}: shares decision number {number} with {taken[number].name}; the record merged later takes the next free number")
        else:
            taken[number] = path
        if not path.read_text().startswith(f"# {number}. "):
            errors.append(f"{path}: title must start with \"# {number}. \"")
        if f"- [{number}]({path.name})" not in index:
            errors.append(f"{path}: list it in {directory.name}/README.md as \"- [{number}]({path.name}): <title>\"")
    return errors


CONFLICT_MARKER = re.compile(r"(?:<{7}|\|{7}|>{7})(?: .*)?|={7}")
VENDORED = ("crates/yulius-server/web/assets/vendor/",)


def conflict_marker_errors(root, names):
    """A merge that commits unresolved conflict markers fails here, in docs and web assets too,
    where no compiler notices them (#364). Vendored and binary files are not read, nor symbolic
    links, whose target is checked under its own name. Git lists an unmerged file once per stage."""
    errors = []
    for name in dict.fromkeys(names):
        path = root / name
        if name.startswith(VENDORED) or path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        if b"\0" in data:
            continue
        for number, line in enumerate(data.decode(errors="replace").split("\n"), 1):
            line = line.removesuffix("\r")
            if CONFLICT_MARKER.fullmatch(line):
                errors.append(f"{name}:{number}: unresolved merge conflict marker {line[:7]}; resolve the merge")
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
NAMED_COLOURS = "aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen".split()
COLOUR_LITERAL = re.compile(r"#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|"
                            r"(?<![\w-])(?:" + "|".join(NAMED_COLOURS) + r")(?![\w-])", re.I)


def style_errors(css, label=STYLESHEET):
    """Design-system rules for the component stylesheet (web/README.md "Theming and branding"):
    colours, type, radii and spacing come only from tokens.css roles and scales."""
    errors = []
    css = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), css, flags=re.S)
    for match in re.finditer(r"(?<![-\w])(--[\w-]+|[a-z-]+)\s*:\s*([^;{}]+)", css):
        prop, value = match.group(1), match.group(2).strip()
        line = css.count("\n", 0, match.start()) + 1
        where = f"{label}:{line}: {prop}: {value}"
        if COLOUR_LITERAL.search(re.sub(r"var\(--[\w-]+\)", "", value)):
            errors.append(f"{where} (colours come from tokens.css roles)")
        elif prop == "font-size" and not re.fullmatch(r"var\(--fs(-[a-z0-9]+)?\)|inherit", value):
            errors.append(f"{where} (use a --fs-* token)")
        elif prop == "line-height" and not re.fullmatch(r"var\(--lh(-[a-z0-9]+)?\)|1|normal|inherit", value):
            errors.append(f"{where} (use a --lh-* token)")
        elif prop == "font-family" and not re.fullmatch(r"var\(--font(-mono|-display)?\)|inherit", value):
            errors.append(f"{where} (use --font, --font-display or --font-mono)")
        elif prop == "font" and value != "inherit" and not (re.search(r"var\(--fs", value) and re.search(r"var\(--font", value)):
            errors.append(f"{where} (shorthand needs --fs-* and --font tokens)")
        elif prop == "font-weight" and value not in WEIGHTS:
            errors.append(f"{where} (weights are 400, 500, 600)")
        elif prop in ("transition", "transition-duration", "animation", "animation-duration") and re.search(r"(?<![\w-])\d*\.?\d+m?s\b", value):
            errors.append(f"{where} (use the --duration* motion tokens)")
        elif prop == "z-index" and not re.fullmatch(r"var\(--z-[a-z-]+\)|0|auto|-1", value):
            errors.append(f"{where} (use a --z-* layer token)")
        elif prop == "border-radius" and any(not re.fullmatch(r"var\(--radius(-[a-z]+)?\)|0|50%|inherit", part) for part in value.split()):
            errors.append(f"{where} (use a --radius-* token, 0 or 50%)")
        elif re.fullmatch(r"padding(-[a-z]+)?|margin(-[a-z]+)?|gap|row-gap|column-gap", prop):
            for number in re.findall(r"(-?\d+(?:\.\d+)?)px", value):
                size = abs(float(number))
                if size not in SPACING_SCALE and not (size == 1 and prop.startswith("margin")):
                    errors.append(f"{where} ({number}px is off the 4 px spacing scale)")
    return errors


CONTROL_SUBJECT = re.compile(r"(?:\.btn[\w-]*|\.input|\.select|\.textarea|\.tab|[\w-]+-tabs?)(?![\w-])")


def control_height_errors(css, label=STYLESHEET):
    """Buttons, inputs, selects and tabs take a --control-* height: one rhythm, and targets of at
    least 24 px (WCAG 2.2 2.5.8)."""
    errors = []
    css = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), css, flags=re.S)
    for rule in re.finditer(r"([^{}]+)\{([^{}]*)\}", css):
        subjects = [part.split()[-1] for part in rule[1].split(",") if part.strip()]
        if not any(CONTROL_SUBJECT.search(subject) and "::" not in subject for subject in subjects):
            continue
        for height in re.finditer(r"(?<![\w-])height\s*:\s*([^;]+)", rule[2]):
            value = height[1].strip()
            if not re.fullmatch(r"var\(--control-(sm|md|lg|xl)\)|auto|100%|inherit", value):
                line = css.count("\n", 0, rule.start(2) + height.start()) + 1
                errors.append(f"{label}:{line}: height: {value} (controls use a --control-* height)")
    return errors


# Text pairs need 4.5:1 and non-text (state marks, focus, control edges) 3:1 (WCAG 2.2 1.4.3,
# 1.4.11). A background "a/b" is a translucent role a composited over b.
CONTRAST_PAIRS = [
    ("--accent-ink", "--accent-strong", 4.5), ("--accent-ink", "--accent-hover", 4.5),
    ("--accent-fg", "--surface", 4.5), ("--accent-fg", "--bg", 4.5),
    ("--text", "--selection/--surface", 4.5),
    ("--term-fg", "--term-selection", 4.5),
    *((fg, bg, 4.5) for fg in ("--text", "--text-2", "--muted", "--text-faint") for bg in ("--bg", "--surface", "--surface-2")),
    ("--text", "--surface-3", 4.5), ("--muted", "--surface-hover", 4.5),
    ("--accent-ink", "--accent", 4.5), ("--term-fg", "--term-bg", 4.5), ("--ansi-bright-black", "--term-bg", 4.5),
    *((f"--{hue}-fg", bg, 4.5) for hue in ("accent", "ok", "warn", "danger", "violet", "info") for bg in ("--surface", f"--{hue}-soft/--surface")),
    ("--warn-fg-strong", "--warn-soft/--surface", 4.5), ("--danger-fg-strong", "--danger-soft/--surface", 4.5),
    ("--accent-fg-strong", "--accent-soft/--surface", 4.5), ("--agent-ink", "--agent", 4.5),
    *((mark, bg, 3.0) for mark in ("--accent", "--ok", "--warn", "--danger", "--violet") for bg in ("--bg", "--surface")),
    ("--border-control", "--bg", 3.0), ("--border-control", "--surface", 3.0), ("--border-control", "--surface-2", 3.0),
    # A past session's faded terminal (SPEC-075): fainter than the live one, still AA.
    *((fg, "--session-bg", 4.5) for fg in ("--session-fg", "--session-dim", "--session-ok", "--session-err")),
    *(("--session-fg", bg, 4.5) for bg in ("--session-prompt-bg", "--session-add-bg", "--session-del-bg")),
    ("--session-dim", "--session-prompt-bg", 4.5),
    *((fg, "--session-web-stuck", 4.5) for fg in ("--session-fg", "--session-dim", "--session-ok", "--session-err")),
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
        names = re.findall(r"data-theme=[\"']?(\w+)", selector)
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


STYLE_WRITE = re.compile(r"""\.style\.[\w]+\s*=\s*(['"])([^'"]*)\1|\.style\.setProperty\([^,]+,\s*(['"])([^'"]*)\3|\.style\.cssText\s*=""")


def inline_style_errors(js, label="web script"):
    """Inline styles may carry computed values only; fixed lengths and colours belong in styles.css."""
    errors = []
    for match in STYLE_WRITE.finditer(js):
        value = match[2] if match[2] is not None else (match[4] or "")
        if match[0].endswith("=") and "cssText" in match[0] or FIXED_STYLE_VALUE.search(f"'{value}'"):
            number = js.count("\n", 0, match.start()) + 1
            errors.append(f"{label}:{number}: fixed value written to an element's style; use a class in styles.css and tokens")
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
    fields = {"markViewBox": "0 0 24 24", "markStyle": "stroke"}
    for field in ("name", "mark", "markViewBox", "markStyle"):
        m = re.search(r"\b" + field + r"""\s*:\s*(['"])(.*?)\1""", brand_js)
        if m:
            fields[field] = m[2]
        elif field in ("name", "mark"):
            raise ValueError(f"{BRAND}: BRAND.{field} not found")
    env = theme_tokens(tokens_css)["dark"]
    bg, surface, mark_colour = (_hex(_resolve(name, env)) for name in ("--bg", "--surface", "--brand-mark"))
    paint = (f"fill='{mark_colour}'" if fields["markStyle"] == "fill"
             else f"stroke='{mark_colour}' stroke-width='2.4' fill='none' stroke-linecap='round' stroke-linejoin='round'")
    svg = (f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><rect width='24' height='24' rx='5.5' fill='{surface}'/>"
           f"<svg x='3' y='3' width='18' height='18' viewBox='{fields['markViewBox']}'><path d='{fields['mark']}' {paint}/></svg></svg>")
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


def brand_name(brand_js):
    m = re.search(r"""name\s*:\s*(['"])(.*?)\1""", brand_js)
    if not m:
        raise ValueError(f"{BRAND}: BRAND.name not found")
    return m[2]


def brand_name_errors(js, name, label="web script"):
    """User-visible strings take the product name from BRAND.name, so a rebrand reaches them all."""
    errors = []
    pattern = re.compile(r"""(['"`])(?:(?!\1).)*?(?<![\w-])""" + re.escape(name) + r"""(?![\w-])(?:(?!\1).)*?\1""")
    js = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), js, flags=re.S)
    for number, line in enumerate(js.splitlines(), 1):
        if not line.lstrip().startswith("//") and pattern.search(line):
            errors.append(f"{label}:{number}: the product name in a string; use BRAND.name (brand.js)")
    return errors


STATIC_IMPORT = re.compile(r"""(?:^|[;\n])\s*(?:import|export)\s+(?:[^'"();]*?\s+from\s+)?(['"])([^'"]+)\1""")
MODULE_PRELOAD = re.compile(r'[ \t]*<link rel="modulepreload" href="([^"]+)">\n')
APP_SCRIPT = '<script type="module" src="/assets/app.js"></script>'


def startup_modules(web_assets, entry="app.js"):
    """Modules the app loads before it can paint: app.js's static imports, followed transitively.

    Dynamic import() (the mock, the terminal core, the editor) is left out: those load later.
    """
    web_assets = web_assets.resolve()
    order, seen, queue = [], set(), [(web_assets / entry).resolve()]
    while queue:
        path = queue.pop(0)
        if path in seen or not path.exists():
            continue
        seen.add(path)
        order.append(path)
        for _, spec in STATIC_IMPORT.findall(_code(path.read_text())):
            if spec.startswith("."):
                queue.append((path.parent / spec).resolve())
    return ["/assets/" + str(path.relative_to(web_assets)) for path in order[1:]]


def preload_lines(web_assets):
    return "".join(f'  <link rel="modulepreload" href="{href}">\n' for href in startup_modules(web_assets))


def preload_errors(index_html, web_assets, label=INDEX):
    """index.html preloads exactly the start-up modules, so the browser fetches them in one wave."""
    if MODULE_PRELOAD.findall(index_html) != startup_modules(web_assets):
        return [f"{label}: modulepreload links do not match app.js's static imports; run scripts/check_repository.py --sync-brand"]
    return []


def sync_brand():
    """Rewrite index.html's generated parts: brand (brand.js, tokens.css) and module preloads."""
    path = ROOT / INDEX
    index_html = path.read_text()
    for key, expected in brand_head((ROOT / BRAND).read_text(), (ROOT / TOKENS).read_text()).items():
        index_html, count = re.subn(BRAND_PATTERNS[key], lambda _: expected, index_html, count=1)
        if not count:
            raise ValueError(f"{INDEX}: no {key} element to update")
    index_html = MODULE_PRELOAD.sub("", index_html)
    if APP_SCRIPT not in index_html:
        raise ValueError(f"{INDEX}: no app.js module script to preload for")
    index_html = index_html.replace(f"  {APP_SCRIPT}", preload_lines((ROOT / STYLESHEET).parent) + f"  {APP_SCRIPT}", 1)
    path.write_text(index_html)
    print(f"{INDEX} now matches brand.js, tokens.css and app.js's imports")


IMPORT_STATEMENT = re.compile(r"import\s*\{([^}]*)\}\s*from\s*'([^']+)'")


def _code(js):
    """Script source without comments: line comments first, so a "/*" inside one (pages/*.js)
    does not open a block comment."""
    code = "\n".join(line for line in js.splitlines() if not line.lstrip().startswith("//"))
    return re.sub(r"/\*.*?\*/", "", code, flags=re.S)
EXPORT_NAME = re.compile(r"^export\s+(?:async\s+)?(?:function\*?|const|let|var|class)\s+([\w$]+)", re.M)
EXPORT_LIST = re.compile(r"^export\s*\{([^}]*)\}", re.M)


def _exports(js):
    names = set(EXPORT_NAME.findall(js))
    for group in EXPORT_LIST.findall(js):
        names.update(part.split(" as ")[-1].strip() for part in group.split(",") if part.strip())
    return names


def module_errors(web_assets, label_root=ROOT):
    """Web scripts: every imported name is exported by its module, and every import is used."""
    errors = []
    files = {p.resolve(): p.read_text() for p in web_assets.glob("**/*.js") if "vendor" not in p.parts}
    for path, js in files.items():
        label = str(path.relative_to(label_root.resolve())) if path.is_relative_to(label_root.resolve()) else path.name
        code = _code(js)
        for names, spec in IMPORT_STATEMENT.findall(code):
            target = (path.parent / spec).resolve()
            imported = [part.split(" as ")[-1].strip() for part in names.split(",") if part.strip()]
            originals = [part.split(" as ")[0].strip() for part in names.split(",") if part.strip()]
            if target in files:
                missing = [n for n in originals if n not in _exports(files[target])]
                if missing:
                    errors.append(f"{label}: imports {', '.join(missing)} that {spec} does not export")
            body = IMPORT_STATEMENT.sub("", code)
            # A use is the name not after another word or a property dot ("...name" spread counts).
            unused = [n for n in imported if not re.search(r"(?<![\w$])(?<![^.]\.)" + re.escape(n) + r"(?![\w$])", body)]
            if unused:
                errors.append(f"{label}: imports {', '.join(unused)} from {spec} but never uses it")
    return errors


def ui_errors():
    """Every web UI check (design tokens, contrast, brand, copy, inline styles); needs no Rust."""
    errors = []
    css = (ROOT / STYLESHEET).read_text()
    errors.extend(style_errors(css))
    errors.extend(control_height_errors(css))
    name = brand_name((ROOT / BRAND).read_text())
    for path in sorted((ROOT / STYLESHEET).parent.glob("**/*.js")):
        if "vendor" in path.parts:
            continue
        js, label = path.read_text(), str(path.relative_to(ROOT))
        errors.extend(copy_errors(js, label))
        errors.extend(inline_style_errors(js, label))
        if path.name not in ("brand.js", "mock.js"):
            errors.extend(brand_name_errors(js, name, label))
    errors.extend(contrast_errors((ROOT / TOKENS).read_text()))
    errors.extend(brand_errors((ROOT / INDEX).read_text(), (ROOT / BRAND).read_text(), (ROOT / TOKENS).read_text()))
    errors.extend(module_errors((ROOT / STYLESHEET).parent))
    errors.extend(preload_errors((ROOT / INDEX).read_text(), (ROOT / STYLESHEET).parent))
    node = shutil.which("node")
    if node:  # syntax of every web script, when Node is installed (it is optional here)
        for path in sorted((ROOT / STYLESHEET).parent.glob("**/*.js")):
            if "vendor" not in path.parts:
                # As an ES module: plain `node --check file.js` can pass module code the browser rejects.
                result = subprocess.run([node, "--input-type=module", "--check"], input=path.read_text(), capture_output=True, text=True)
                if result.returncode:
                    errors.append(f"{path.relative_to(ROOT)}: {result.stderr.strip().splitlines()[-1] if result.stderr.strip() else 'syntax error'}")
    return errors


def check_ui():
    errors = ui_errors()
    if errors:
        raise ValueError("\n".join(errors))
    print("UI contracts passed: design tokens, contrast in every theme, brand, copy, inline styles")


def check():
    errors = []
    names = list(filter(None, output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]).split("\0")))
    for name in names:
        path = ROOT / name
        if path.is_file() and path.suffix == ".md":
            errors.extend(link_errors(path, ROOT))
    errors.extend(conflict_marker_errors(ROOT, names))
    skills = list((ROOT / ".apm/skills").glob("*/SKILL.md"))
    if not skills:
        errors.append("No repository skills found")
    for path in skills:
        errors.extend(skill_errors(path))
    errors.extend(skill_deployment_errors(ROOT / ".apm/skills", [ROOT / ".agents/skills", ROOT / ".claude/skills"]))
    errors.extend(decision_errors(ROOT / "docs/decisions"))
    errors.extend(ui_errors())
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    cargo = ["rustup", "run", channel, "cargo"]
    metadata = json.loads(output(cargo + ["metadata", "--locked", "--no-deps", "--format-version", "1"]))
    packages = [p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]]
    for package in packages:
        path = Path(package["manifest_path"])
        errors.extend(manifest_errors(tomllib.loads(path.read_text()), package["name"]))
    errors.extend(dependency_errors(packages))
    errors.extend(integration_test_errors(packages))
    if errors:
        raise ValueError("\n".join(errors))
    print(f"Repository contracts passed: {len(packages)} crates, {len(skills)} skills, local Markdown links, no conflict markers, unique decision numbers, UI design tokens, one product test binary")


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["--doctor"]:
            doctor()
        elif sys.argv[1:] == ["--sync-brand"]:
            sync_brand()
        elif sys.argv[1:] == ["--ui"]:
            check_ui()
        elif not sys.argv[1:]:
            check()
        else:
            raise ValueError("Usage: check_repository.py [--doctor | --sync-brand | --ui]")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"Repository check failed: {error}")
