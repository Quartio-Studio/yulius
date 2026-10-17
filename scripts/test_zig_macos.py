"""The macOS check's zig wrapper keeps cc-rs's flags but the Apple target ones zig refuses."""
import unittest
from zig_macos import command, compiler_args


class CompilerArgs(unittest.TestCase):
    def test_apple_target_flags_are_dropped_with_their_values(self):
        args = ["-O0", "--target=arm64-apple-macosx", "-arch", "arm64", "-mmacosx-version-min=11.0",
                "-target", "arm64-apple-macosx11.0", "-I", "include", "-c", "ring.c", "-o", "ring.o"]
        self.assertEqual(compiler_args(args), ["-O0", "-I", "include", "-c", "ring.c", "-o", "ring.o"])

    def test_other_flags_pass_unchanged(self):
        args = ["-ffunction-sections", "-fPIC", "-DSQLITE_CORE", "-Wall", "-E", "detect.c"]
        self.assertEqual(compiler_args(args), args)


class Command(unittest.TestCase):
    def test_the_compiler_targets_aarch64_macos(self):
        self.assertEqual(command("macos-cc", "/z", ["-arch", "arm64", "-c", "a.c"]),
                         ["/z", "cc", "-target", "aarch64-macos", "-c", "a.c"])

    def test_the_archiver_passes_everything(self):
        self.assertEqual(command("macos-ar", "/z", ["cq", "lib.a", "a.o"]), ["/z", "ar", "cq", "lib.a", "a.o"])

    def test_another_name_is_refused(self):
        with self.assertRaises(ValueError):
            command("zig_macos.py", "/z", [])


if __name__ == "__main__":
    unittest.main()
