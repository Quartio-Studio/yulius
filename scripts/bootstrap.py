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
if not rustup and "--install-rustup" in sys.argv[1:]:
    # Clean machines (CI): official installer, no toolchain yet and no shell profile edits.
    with tempfile.TemporaryDirectory() as scratch:
        installer = Path(scratch) / "rustup-init.sh"
        with urllib.request.urlopen("https://sh.rustup.rs", timeout=60) as response:
            installer.write_bytes(response.read())
        subprocess.run(["sh", str(installer), "-y", "--default-toolchain", "none", "--no-modify-path"],
                       check=True)
    rustup = shutil.which("rustup", path=str(cargo_bin))
if not rustup:
    sys.exit("Install rustup from https://rustup.rs first (or pass --install-rustup). "
             "Python 3.11+ and a C linker are required.")
config = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]
channel = config["channel"]
command = [rustup, "toolchain", "install", channel, "--profile", config["profile"]]
for component in config["components"]:
    command += ["--component", component]
subprocess.run(command, cwd=root, check=True)
compiler = subprocess.check_output([rustup, "which", "--toolchain", channel, "rustc"], text=True).strip()
toolchain_bin = Path(compiler).parent
install_env = dict(os.environ, RUSTC=compiler, RUSTDOC=str(toolchain_bin / "rustdoc"))
install_env["PATH"] = str(toolchain_bin) + os.pathsep + os.environ.get("PATH", "")
tools = tomllib.loads((root / ".config/tools.toml").read_text())["tools"]
for name, version in tools.items():
    executable = root / ".tools/bin" / (name + (".exe" if os.name == "nt" else ""))
    if executable.exists():
        found = subprocess.run([str(executable), "--version"], capture_output=True, text=True, check=True)
        if version in found.stdout.split():
            print(f"{name} {version} already installed", flush=True)
            continue
    subprocess.run([rustup, "run", channel, "cargo", "install", "--locked", "--version", version,
                    "--root", str(root / ".tools"), name], cwd=root, env=install_env, check=True)
print("Ready. Run scripts/dev doctor, then scripts/dev ci.", flush=True)
