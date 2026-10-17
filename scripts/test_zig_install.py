"""Bootstrap's zig for the macOS check: verified before it is unpacked, reused, never fatal elsewhere."""
import hashlib
import io
import tarfile
import tempfile
import unittest
from pathlib import Path
from zig_install import install

VERSION = "0.17.0"
NAME = f"zig-x86_64-linux-{VERSION}"


def tarball():
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode="w:xz") as archive:
        body = b"#!/bin/sh\necho 0.17.0\n"
        info = tarfile.TarInfo(f"{NAME}/zig")
        info.size, info.mode = len(body), 0o755
        archive.addfile(info, io.BytesIO(body))
    return data.getvalue()


class Opener:
    def __init__(self, body):
        self.body, self.urls = body, []

    def __call__(self, url, timeout):
        self.urls.append(url)
        if self.body is None:
            raise AssertionError("downloaded again")
        return io.BytesIO(self.body)


class Install(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.cache, self.bin = self.root / "cache", self.root / ".tools/bin"
        self.bin.mkdir(parents=True)
        (self.root / "scripts").mkdir()
        (self.root / "scripts/zig_macos.py").write_text("")
        self.body = tarball()
        self.pins = {"version": VERSION, "x86_64-linux": hashlib.sha256(self.body).hexdigest()}

    def run_install(self, opener, machine="x86_64", pins=None):
        return install(pins or self.pins, self.cache, self.bin, self.root, machine, opener)

    def test_a_verified_tarball_is_unpacked_and_linked(self):
        opener = Opener(self.body)
        self.run_install(opener)
        self.assertEqual(opener.urls, [f"https://ziglang.org/download/{VERSION}/{NAME}.tar.xz"])
        self.assertEqual((self.bin / "zig").resolve(), (self.cache / f"zig-{VERSION}" / NAME / "zig").resolve())
        for role in ("macos-cc", "macos-ar"):
            self.assertEqual((self.bin / role).resolve(), (self.root / "scripts/zig_macos.py").resolve())

    def test_a_tarball_of_another_digest_is_refused_and_leaves_nothing(self):
        with self.assertRaises(SystemExit):
            self.run_install(Opener(self.body), pins=dict(self.pins, **{"x86_64-linux": "0" * 64}))
        self.assertEqual(sorted(p.name for p in self.cache.iterdir() if not p.name.startswith(".")), [])
        self.assertFalse((self.bin / "zig").exists())

    def test_the_cached_zig_is_reused(self):
        self.run_install(Opener(self.body))
        self.run_install(Opener(None))
        self.assertTrue((self.bin / "zig").exists())

    def test_a_cached_folder_without_its_zig_is_replaced(self):
        stale = self.cache / f"zig-{VERSION}" / NAME
        stale.mkdir(parents=True)
        (stale / "leftover").write_text("")
        self.run_install(Opener(self.body))
        self.assertTrue((stale / "zig").exists())
        self.assertFalse((stale / "leftover").exists())

    def test_an_unpinned_architecture_is_skipped_not_fatal(self):
        message = self.run_install(Opener(None), machine="riscv64")
        self.assertIn("riscv64-linux", message)
        self.assertFalse((self.bin / "zig").exists())

    def test_machine_names_are_normalized(self):
        pins = {"version": VERSION, "aarch64-linux": "0" * 64}
        with self.assertRaises(SystemExit):
            self.run_install(Opener(self.body), machine="ARM64", pins=pins)


if __name__ == "__main__":
    unittest.main()
