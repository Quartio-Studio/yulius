"""Run Rust tests that read inputs changed in the CI checkout."""
from dataclasses import dataclass
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
# Rustfmt keeps attributes with their function. A shared helper outside the
# test bodies supplies inputs to every case in the file.
TEST = re.compile(
    r"(?m)^(?P<indent>[ \t]*)#\[(?:\w+::)?test(?:\([^\]]*\))?\]\s*"
    r"(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?P<name>\w+)"
)
READ_FUNCTION = r"\b(?:read(?:_to_string|_dir)?|File::open)\s*\(\s*"
BOUND_READ = re.compile(READ_FUNCTION + r"&?(\w+)\s*\)")
PATH_ARGUMENT = r'(?:&\s*)?(?:(?:std::path::)?(?:Path::new|PathBuf::from)\s*\(\s*)?(?:r#*)?"([^"\\]+)"'
LITERAL_BINDING = re.compile(
    r"\b(?:let|const|static)\s+(?:mut\s+)?(\w+)\s*(?::[^=;]+)?=\s*" + PATH_ARGUMENT
)
READ_CALL = re.compile(READ_FUNCTION)
LITERAL_CONCAT = re.compile(r'\bconcat!\s*\(\s*((?:"[^"\\]*"\s*,\s*)*"[^"\\]*"\s*,?)\s*\)')
DIRECT_READ = re.compile(READ_FUNCTION + PATH_ARGUMENT)
INCLUDE = re.compile(r'\binclude_(?:str|bytes)!\s*\(\s*(?:r#*)?"([^"\\]+)"')
PUBLIC_INCLUDE = re.compile(
    r"\bpub(?:\([^)]*\))?\s+(?:const|static)\s+(\w+)\s*:[^;=]+?\s*=\s*" + INCLUDE.pattern
)
MANIFEST = re.compile(r'env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*\)')
CONCAT_MANIFEST = re.compile(r'concat!\s*\(\s*env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"/([^"\\]+)"')
PATH_LITERAL = re.compile(r'"([\w./-]+)"')
JOIN = re.compile(r'\s*\.join\(\s*"([\w./-]+)"\s*\)')
# Compiled-in data also reaches tests through these accessors, routes and headers.
EMBEDDED = {
    "catalog::default_workflow(": "catalog/workflows",
    "catalog::defaults(": "catalog/workflows",
    "/automation-templates": "catalog/automations",
    '"content-security-policy"': "crates/yulius-server/src/content-security-policy.txt",
    "header::CONTENT_SECURITY_POLICY": "crates/yulius-server/src/content-security-policy.txt",
    "templates::template(": "catalog/automations",
    "pack::PACKS": "catalog/skills",
}


@dataclass
class Reader:
    package: str
    target: tuple
    source: str
    inputs: set
    name: str


def inputs_for(root, crate, path, text, embedded, caller_text=None):
    inputs = set()

    def add(base, literal):
        absolute = Path(os.path.normpath(base / literal))
        if absolute.is_relative_to(root) and absolute != root:
            inputs.add(absolute.relative_to(root).as_posix())

    # Rust concatenates these literals before resolving an include or a read.
    if "concat!" in text:
        text = LITERAL_CONCAT.sub(lambda m: '"' + "".join(re.findall(r'"([^"]*)"', m[1])) + '"', text)
    if READ_CALL.search(text):
        # Also cover a path passed via a local binding. The crate directory is
        # Cargo's working directory for the test.
        variables = set(BOUND_READ.findall(text))
        if variables:
            for variable, literal in LITERAL_BINDING.findall(caller_text or text):
                if variable in variables:
                    add(crate, literal)
    for literal in DIRECT_READ.findall(text):
        add(crate, literal)
    for literal in INCLUDE.findall(text):
        add(path.parent, literal)
    for suffix in CONCAT_MANIFEST.findall(text):
        add(crate, suffix)
    for match in MANIFEST.finditer(text):
        base = crate
        offset = match.end()
        while (join := JOIN.match(text, offset)) is not None:
            base = Path(os.path.normpath(base / join.group(1)))
            offset = join.end()
        if base == root:
            # A workspace-rooted reader may iterate an array, or pass the path
            # into a closure. Include all path literals in the test source.
            for name in PATH_LITERAL.findall(caller_text or text):
                if "/" in name or "." in name:
                    add(root, name)
                    # Workspace text contracts compare policy across many docs
                    # and catalog entries. Keep these families together.
                    family = name.split("/", 1)[0]
                    if family in {"docs", "catalog"}:
                        inputs.add(family)
            for name in JOIN.findall(caller_text or text):
                add(root, name)
        elif base != crate:
            add(base, ".")
        else:
            for name in JOIN.findall(caller_text or text):
                add(crate, name)
    for accessor, dependency in embedded:
        if accessor in text:
            inputs.add(dependency)
    return inputs


def test_sections(text):
    """Separate direct reads in rustfmt-formatted tests from shared file inputs.

    A premature close (e.g. a brace inside a raw string) leaves the rest in the
    shared portion, so its dependencies still select all sibling tests.
    """
    sections = {}
    shared = []
    previous = 0
    for match in TEST.finditer(text):
        shared.append(text[previous:match.start()])
        rest = text[match.end():]
        close = re.search(r"(?m)^" + match["indent"] + r"\}\s*$", rest)
        # Rustfmt can keep an empty body on the signature line. For an
        # unfamiliar layout, treating all inputs as shared is conservative.
        if not close or TEST.search(rest, 0, close.start()):
            return {m["name"]: "" for m in TEST.finditer(text)}, text
        previous = match.end() + close.end()
        name = match["name"]
        sections[name] = sections.get(name, "") + text[match.start():previous]
    shared.append(text[previous:])
    return sections, "\n".join(shared)


def cargo_target(crate, config, path):
    relative = path.relative_to(crate)
    if relative.parts[0] == "tests":
        if config["package"].get("autotests", True):
            return ("--test", Path(relative.parts[1]).stem)
        # Yulius consolidates all these modules in one declared binary.
        [target] = config["test"]
        return ("--test", target["name"])
    if relative == Path("src/main.rs") or relative.parts[:2] == ("src", "bin"):
        return ("--bins",)
    return ("--lib",) if (crate / "src/lib.rs").is_file() else ("--bins",)


def inventory(root):
    manifests = sorted([*root.glob("crates/*/Cargo.toml"), *root.glob("tools/*/Cargo.toml")])
    embedded = list(EMBEDDED.items())
    # Tests can consume a public Rust constant in another module or crate.
    # Derive its input at the definition. Equal names conservatively select both.
    for manifest in manifests:
        for source in (manifest.parent / "src").rglob("*.rs"):
            text = source.read_text()
            for name, relative in PUBLIC_INCLUDE.findall(text):
                dependency = Path(os.path.normpath(source.parent / relative))
                if dependency.is_relative_to(root):
                    embedded.append((name, dependency.relative_to(root).as_posix()))
    anchors = ("include_str!", "include_bytes!", "CARGO_MANIFEST_DIR", *(name for name, _ in embedded))
    readers = []
    for manifest in manifests:
        crate = manifest.parent
        config = tomllib.loads(manifest.read_text())
        package = config["package"]["name"]
        for folder in ("src", "tests"):
            for path in sorted((crate / folder).rglob("*.rs")):
                text = path.read_text()
                if not any(key in text for key in anchors) and not READ_CALL.search(text):
                    continue
                if not TEST.search(text):
                    if "tests" in path.relative_to(crate).parts and inputs_for(root, crate, path, text, embedded):
                        raise ValueError(f"{path.relative_to(root)} reads repository data in a shared "
                                         "test helper. Associate it with its caller tests in the selector.")
                    continue
                target = cargo_target(crate, config, path)
                source = path.relative_to(root).as_posix()
                all_inputs = inputs_for(root, crate, path, text, embedded)
                if not all_inputs:
                    continue
                if folder == "src":
                    # Unit tests can read the same inputs through their implementation
                    # (e.g. the embedded skill, workflow and automation catalogs).
                    readers.extend(Reader(package, target, source, all_inputs, match["name"])
                                   for match in TEST.finditer(text))
                    continue
                sections, shared = test_sections(text)
                common = inputs_for(root, crate, path, shared, embedded, caller_text=text)
                per_test = {name: inputs_for(root, crate, path, body, embedded) for name, body in sections.items()}
                # Preserve any file-wide read that could not be attributed to one
                # test (for instance, a root and a path literal in separate items).
                common.update(all_inputs.difference(*per_test.values()))
                for name, deps in per_test.items():
                    inputs = common | deps
                    if inputs:
                        readers.append(Reader(package, target, source, inputs, name))
    return readers


def select(readers, paths):
    if paths is None:
        return readers
    changed = set(paths)
    ancestors = changed.copy()
    for path in changed:
        ancestors.update(parent.as_posix() for parent in Path(path).parents)
    return [reader for reader in readers if reader.inputs & ancestors]


def changed_paths(root, base):
    # No rename detection: both the removed and added path are inputs.
    result = subprocess.run(["git", "diff", "--no-renames", "--name-only", "-z",
                             "--end-of-options", f"{base}...HEAD", "--"],
                            cwd=root, capture_output=True)
    if result.returncode:
        print("Cannot determine the changed paths. Running all repository readers.", file=sys.stderr)
        return None
    return [os.fsdecode(path) for path in result.stdout.split(b"\0") if path]


def run(readers, bootstrap=False):
    if not readers:
        print("No repository reader covers the changed paths.")
        return int(os.environ.get("YULIUS_CI_SKIP_CODE", "0"))
    packages = sorted({reader.package for reader in readers})
    targets = sorted({reader.target for reader in readers})
    filters = []
    for package in packages:
        names = sorted({reader.name for reader in readers if reader.package == package})
        filters.append(f"(package(={package}) & test(/(^|::)({'|'.join(names)})$/))")
    print("Repository reader sources:", *sorted({reader.source for reader in readers}), sep="\n  ", flush=True)
    if bootstrap:
        subprocess.run([sys.executable, "scripts/bootstrap.py", "--install-rustup"], cwd=ROOT, check=True)
    command = [str(ROOT / "scripts/cargo"), "nextest", "run", "--locked"]
    for package in packages:
        command.extend(["-p", package])
    for target in targets:
        command.extend(target)
    return subprocess.run([*command, "-E", " | ".join(filters)], cwd=ROOT).returncode


if __name__ == "__main__":
    # On protected branch runs the two after-merge shards already run every reader.
    if os.environ.get("YULIUS_CI_REF", "").startswith("branch/"):
        sys.exit(int(os.environ["YULIUS_CI_SKIP_CODE"]))
    paths = sys.argv[1:] or changed_paths(ROOT, "origin/" + os.environ.get("YULIUS_INTEGRATION_BRANCH", "dev"))
    sys.exit(run(select(inventory(ROOT), paths), bootstrap=os.environ.get("YULIUS_CI") == "1"))
