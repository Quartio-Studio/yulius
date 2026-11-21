"""The project's optional Linux compiler cache. The sandbox mount is the sharing boundary."""
import hashlib
import os
import platform
import tomllib
from pathlib import Path
import shutil
import tarfile
import tempfile

from home_lock import exclusive


def sandbox_for(checkout, home):
    """Agent and CI homes live under the same sandbox. Require both known directory layouts."""
    if home.name != "home":
        return None
    sandbox = home.parent.parent if home.parent.name == "ci" else home.parent
    if not checkout.is_relative_to(sandbox):
        return None
    parts = checkout.relative_to(sandbox).parts
    if parts[:1] in {("worktrees",), ("repo-worktrees",), ("repo",)} or parts[:2] == ("ci", "jobs"):
        return sandbox
    return None


def installed(root):
    """A new worktree can already use the version bootstrap installed in this agent's home."""
    local = root / ".tools/bin/mbx"
    if os.access(local, os.X_OK):
        return local
    pins = tomllib.loads((root / ".config/tools.toml").read_text())["mbx"]
    cache = Path(os.environ.get("YULIUS_TOOLS_CACHE") or Path.home() / ".cache/yulius-tools")
    return cache / f"mbx-{pins['version']}-{platform.machine()}" / "mbx"


def install(pins, cache, bin_dir, machine, opener):
    """Publish only the digest-checked release executable. A failed install leaves Cargo usable."""
    digest = pins.get(machine)
    if not digest:
        return f"Rust compile cache unavailable for {machine}"
    version = pins["version"]
    shared = cache / f"mbx-{version}-{machine}" / "mbx"
    name = f"mbx-{machine}-unknown-linux-musl.tar.gz"
    url = f"https://github.com/jdx/mr-boxington/releases/download/v{version}/{name}"
    with exclusive(cache / f".mbx-{version}-{machine}.lock"):
        if not shared.is_file():
            shared.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(dir=shared.parent) as scratch:
                archive = Path(scratch) / name
                with opener(url, timeout=60) as response, archive.open("wb") as out:
                    shutil.copyfileobj(response, out)
                with archive.open("rb") as src:
                    if hashlib.file_digest(src, "sha256").hexdigest() != digest:
                        raise ValueError(f"unexpected digest for {name}")
                executable = Path(scratch) / "mbx"
                with tarfile.open(archive, "r:gz") as package:
                    member = package.getmember("mbx")
                    if not member.isfile():
                        raise ValueError(f"{name} has no regular mbx executable")
                    with package.extractfile(member) as src, executable.open("wb") as out:
                        shutil.copyfileobj(src, out)
                executable.chmod(0o755)
                executable.replace(shared)
        with tempfile.TemporaryDirectory(dir=bin_dir) as scratch:
            link = Path(scratch) / "mbx"
            link.symlink_to(shared)
            link.replace(bin_dir / "mbx")
    return f"mbx {version} linked from {shared}"
