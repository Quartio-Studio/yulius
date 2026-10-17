#!/usr/bin/env python3
"""C compiler and archiver of the gate's macOS check on Linux (ADR-0169).

Bootstrap links this file as `.tools/bin/macos-cc` and `.tools/bin/macos-ar`; the name it is run
by picks the role. Both run the pinned zig of `.tools/bin/zig`, whose bundled macOS headers compile
the C that dependencies vendor (ring, SQLite) for `aarch64-apple-darwin`. Nothing is linked: the
check only type-checks the Rust.
"""
from pathlib import Path
import os
import sys

ZIG_TARGET = "aarch64-macos"
# Flags cc-rs gives a clang for an Apple target; zig takes the target from `-target` alone and
# refuses Apple's arch names ("arm64").
DROPPED_PREFIXES = ("--target=", "-mmacosx-version-min=")
DROPPED_WITH_VALUE = ("-arch", "-target", "--target")


def compiler_args(args):
    """Returns `args` without the target flags zig would refuse or that would contradict its own."""
    kept = []
    skip = False
    for arg in args:
        if skip:
            skip = False
        elif arg in DROPPED_WITH_VALUE:
            skip = True
        elif not arg.startswith(DROPPED_PREFIXES):
            kept.append(arg)
    return kept


def command(role, zig, args):
    """The zig command line for `role` (`macos-cc` or `macos-ar`) given cc-rs's `args`."""
    if role == "macos-cc":
        return [zig, "cc", "-target", ZIG_TARGET, *compiler_args(args)]
    if role == "macos-ar":
        return [zig, "ar", *args]
    raise ValueError(f"run this as macos-cc or macos-ar, not {role}")


def main():
    zig = str(Path(__file__).resolve().parent.parent / ".tools/bin/zig")
    argv = command(Path(sys.argv[0]).name, zig, sys.argv[1:])
    os.execv(argv[0], argv)


if __name__ == "__main__":
    main()
