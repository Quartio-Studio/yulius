#!/usr/bin/env python3
"""Run doctests for workspace libraries; Cargo rejects --doc on binary-only workspaces."""
import json
import os
import subprocess


def library_packages(metadata):
    return [package["name"] for package in metadata["packages"]
            if package["id"] in metadata["workspace_members"]
            and any(target.get("doctest", False) and set(target["kind"]) & {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
                    for target in package["targets"])]


def main():
    cargo = os.environ.get("CARGO", "cargo")
    metadata = json.loads(subprocess.check_output(
        [cargo, "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True))
    packages = library_packages(metadata)
    if not packages:
        print("Doctests: not applicable (no workspace library targets with doctests enabled)")
        return
    command = [cargo, "test", "--locked", "--all-features", "--doc"]
    for name in packages:
        command += ["--package", name]
    subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
