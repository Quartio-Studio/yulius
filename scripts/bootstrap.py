#!/usr/bin/env python3
"""Install the repository compiler and exact tooling without changing global defaults."""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile
import tomllib
import urllib.request

root = Path(__file__).resolve().parent.parent
cargo_bin = Path.home() / ".cargo/bin"
rustup = shutil.which("rustup") or shutil.which("rustup", path=str(cargo_bin))
unattended = bool(os.environ.get("CI")) or not sys.stdin.isatty()
if not rustup and ("--install-rustup" in sys.argv[1:] or unattended):
    # Clean machines (CI, which takes this script's command from the integration branch): official
    # installer, no toolchain yet and no shell profile edits. An interactive terminal keeps the message.
    with tempfile.TemporaryDirectory() as scratch:
        installer = Path(scratch) / "rustup-init.sh"
        with urllib.request.urlopen("https://sh.rustup.rs", timeout=60) as response:
            installer.write_bytes(response.read())
        subprocess.run(["sh", str(installer), "-y", "--default-toolchain", "none", "--no-modify-path"],
                       check=True)
    rustup = shutil.which("rustup", path=str(cargo_bin))
if not rustup:
    sys.exit("Install rustup from https://rustup.rs first (or pass --install-rustup; unattended runs install it). "
             "Python 3.11+ and a C linker are required.")
config = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]
channel = config["channel"]
command = [rustup, "toolchain", "install", channel, "--profile", config["profile"]]
for component in config["components"]:
    command += ["--component", component]
for target in config.get("targets", []):
    command += ["--target", target]
subprocess.run(command, cwd=root, check=True)
compiler = subprocess.check_output([rustup, "which", "--toolchain", channel, "rustc"], text=True).strip()
toolchain_bin = Path(compiler).parent
install_env = dict(os.environ, RUSTC=compiler, RUSTDOC=str(toolchain_bin / "rustdoc"))
install_env["PATH"] = str(toolchain_bin) + os.pathsep + os.environ.get("PATH", "")
pinned = tomllib.loads((root / ".config/tools.toml").read_text())
tools = pinned["tools"]
# Each pinned tool is built once per home (shared by every worktree and CI run of a sandbox) and
# linked into this worktree's .tools/bin: compiling them per worktree cost every new task 10-40
# minutes (F 2026-10-12).
cache = Path(os.environ.get("YULIUS_TOOLS_CACHE") or (Path.home() / ".cache/yulius-tools"))
bin_dir = root / ".tools/bin"
bin_dir.mkdir(parents=True, exist_ok=True)
for name, version in tools.items():
    exe_name = name + (".exe" if os.name == "nt" else "")
    executable = bin_dir / exe_name
    if executable.exists():
        found = subprocess.run([str(executable), "--version"], capture_output=True, text=True)
        if found.returncode == 0 and version in found.stdout.split():
            print(f"{name} {version} already installed", flush=True)
            continue
    shared_root = cache / f"{name}-{version}"
    shared = shared_root / "bin" / exe_name
    cache.mkdir(parents=True, exist_ok=True)
    with open(cache / f".{name}-{version}.lock", "w") as lock:
        if os.name != "nt":
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)  # parallel tasks wait for one build
        if not shared.exists():
            subprocess.run([rustup, "run", channel, "cargo", "install", "--locked", "--version", version,
                            "--root", str(shared_root), name], cwd=root, env=install_env, check=True)
    if executable.exists() or executable.is_symlink():
        executable.unlink()
    try:
        executable.symlink_to(shared)
    except OSError:
        shutil.copy2(shared, executable)
    print(f"{name} {version} linked from {shared_root}", flush=True)


def install_zig(zig):
    """Links the pinned zig and the macOS check's wrappers into .tools/bin (ADR-0169); a Mac
    compiles for itself and needs neither."""
    import hashlib
    import platform
    import tarfile
    version = zig["version"]
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    host = f"{machine}-linux"
    if host not in zig:
        sys.exit(f"No pinned zig for {host}: add its tarball's SHA-256 to .config/tools.toml")
    name = f"zig-{host}-{version}"
    cache.mkdir(parents=True, exist_ok=True)
    shared = cache / f"zig-{version}" / name / "zig"
    with open(cache / f".zig-{version}.lock", "w") as lock:
        import fcntl
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not shared.exists():
            url = f"https://ziglang.org/download/{version}/{name}.tar.xz"
            with tempfile.TemporaryDirectory(dir=cache) as scratch:
                tarball = Path(scratch) / f"{name}.tar.xz"
                with urllib.request.urlopen(url, timeout=600) as response, open(tarball, "wb") as out:
                    shutil.copyfileobj(response, out)
                digest = hashlib.sha256(tarball.read_bytes()).hexdigest()
                if digest != zig[host]:
                    sys.exit(f"{url}: SHA-256 {digest}, expected {zig[host]}")
                unpacked = Path(scratch) / "unpacked"
                with tarfile.open(tarball) as archive:
                    archive.extractall(unpacked, filter="data")
                (cache / f"zig-{version}").mkdir(exist_ok=True)
                (unpacked / name).rename(shared.parent)
    for link, source in [("zig", shared), ("macos-cc", root / "scripts/zig_macos.py"),
                         ("macos-ar", root / "scripts/zig_macos.py")]:
        executable = bin_dir / link
        if executable.exists() or executable.is_symlink():
            executable.unlink()
        executable.symlink_to(source)
    print(f"zig {version} linked from {shared.parent}", flush=True)


if sys.platform == "linux":
    install_zig(pinned["zig"])
print("Ready. Run scripts/dev doctor, then scripts/dev ci.", flush=True)
