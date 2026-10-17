"""Bootstrap's zig for the gate's macOS check on Linux (ADR-0169): the official tarball pinned by
version and SHA-256 in `.config/tools.toml`, verified before it is unpacked, once per home cache."""
from pathlib import Path
import hashlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

MACHINES = {"amd64": "x86_64", "arm64": "aarch64"}


def unpack(tarball, into):
    """Unpacks a verified `.tar.xz`: Python's `data` filter where it has one (3.11.4+), else
    tar, as also where Python lacks lzma."""
    try:
        import lzma  # noqa: F401
        with tarfile.open(tarball) as archive:
            if hasattr(tarfile, "data_filter"):
                archive.extractall(into, filter="data")
                return
    except ImportError:
        pass
    into.mkdir(parents=True, exist_ok=True)
    subprocess.run(["tar", "-xJf", str(tarball), "-C", str(into)], check=True)


def fetch(url, digest, shared, cache, opener):
    """Downloads `url` into `cache`, checks its SHA-256 and moves its folder to `shared`."""
    with tempfile.TemporaryDirectory(dir=cache) as scratch:
        tarball = Path(scratch) / "zig.tar.xz"
        with opener(url, timeout=600) as response, open(tarball, "wb") as out:
            shutil.copyfileobj(response, out)
        found = hashlib.sha256(tarball.read_bytes()).hexdigest()
        if found != digest:
            sys.exit(f"{url}: SHA-256 {found}, expected {digest}")
        unpacked = Path(scratch) / "unpacked"
        unpack(tarball, unpacked)
        if shared.exists():
            shutil.rmtree(shared)  # a folder that lost its zig
        shared.parent.mkdir(parents=True, exist_ok=True)
        (unpacked / shared.name).rename(shared)


def install(pins, cache, bin_dir, root, machine, opener):
    """Links the pinned zig and the macOS check's wrappers into `bin_dir`; returns what it did.
    A host without a pinned tarball is skipped (`scripts/dev macos` then fails on its own)."""
    version = pins["version"]
    machine = MACHINES.get(machine.lower(), machine.lower())
    host = f"{machine}-linux"
    if host not in pins:
        return f"zig skipped: no pinned zig for {host} in .config/tools.toml; scripts/dev macos will not run"
    name = f"zig-{host}-{version}"
    shared = cache / f"zig-{version}" / name
    cache.mkdir(parents=True, exist_ok=True)
    with open(cache / f".zig-{version}.lock", "w") as lock:
        import fcntl
        fcntl.flock(lock, fcntl.LOCK_EX)  # parallel tasks wait for one download
        if not (shared / "zig").exists():
            fetch(f"https://ziglang.org/download/{version}/{name}.tar.xz", pins[host], shared, cache, opener)
    for link, source in [("zig", shared / "zig"), ("macos-cc", root / "scripts/zig_macos.py"),
                         ("macos-ar", root / "scripts/zig_macos.py")]:
        executable = bin_dir / link
        if executable.exists() or executable.is_symlink():
            executable.unlink()
        executable.symlink_to(source)
    return f"zig {version} linked from {shared}"
