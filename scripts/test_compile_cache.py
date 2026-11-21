"""RCC-001..003. The project's Cargo launcher selects the sandbox's cache and preserves failures."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent


@unittest.skipUnless(sys.platform == "linux", "the instance cache runs on Linux")
class CompileCache(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.sandbox = Path(tmp.name) / "project"
        self.task = self.checkout(self.sandbox / "worktrees" / "task")
        self.lane = self.checkout(self.sandbox / "ci" / "jobs" / "test@1")
        self.cargo = Path(tmp.name) / "real-cargo"
        self.cargo.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$RECORD\"\nexit \"$CARGO_RESULT\"\n")
        self.cargo.chmod(0o755)
        self.record = Path(tmp.name) / "calls"

    @staticmethod
    def checkout(path):
        (path / "scripts").mkdir(parents=True)
        for name in ("cargo", "cache-rustc", "compile_cache.py", "home_lock.py", "build_subreaper.py", "build_groups.py"):
            shutil.copy2(SCRIPTS / name, path / "scripts" / name)
        (path / ".tools/bin").mkdir(parents=True)
        (path / ".config").mkdir()
        shutil.copy2(SCRIPTS.parent / ".config/tools.toml", path / ".config/tools.toml")
        return path

    def cargo_env(self, home, cargo):
        home.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, HOME=str(home), CARGO=str(cargo), YULIUS_SANDBOX="project",
                   YULIUS_TOOLS_CACHE=str(home / ".cache/yulius-tools"), XDG_CONFIG_HOME=str(home / ".config"))
        for name in ("RUSTC_WRAPPER", "CARGO_INCREMENTAL", "MAKEFLAGS", "MFLAGS", "CARGO_MAKEFLAGS",
                     "MBX_REMOTE_URL", "YULIUS_CI", "YULIUS_CI_RUN"):
            env.pop(name, None)
        return env

    def launch(self, tree, home, mbx, cargo_result=0):
        binary = tree / ".tools/bin/mbx"
        if mbx:
            binary.write_text("#!/usr/bin/env python3\nimport os,sys\n" + mbx)
            binary.chmod(0o755)
        env = self.cargo_env(home, self.cargo)
        env.update(RECORD=str(self.record), CARGO_RESULT=str(cargo_result))
        return subprocess.run([tree / "scripts/cargo", "build"], cwd=tree, env=env,
                              start_new_session=True, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10)

    def test_disposable_target_is_removed_after_success_or_failure(self):
        self.cargo.write_text('#!/bin/sh\necho "$CARGO_TARGET_DIR"\ntouch "$CARGO_TARGET_DIR/artifact"\nexit "$CARGO_RESULT"\n')
        env = self.cargo_env(self.sandbox / "home", self.cargo)
        for status, parent in ((0, self.task / "target"), (7, self.sandbox / "run-scratch")):
            env.pop("YULIUS_BUILD_SCRATCH", None)
            if status:
                env["YULIUS_BUILD_SCRATCH"] = str(parent)
            result = subprocess.run([self.task / "scripts/cargo", "--scratch-target", "check"],
                                    env=dict(env, CARGO_RESULT=str(status)), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, status, result.stderr)
            target = Path(result.stdout.strip())
            self.assertEqual(target.parent, parent)
            self.assertFalse(target.exists())

    def test_disposable_real_cargo_owns_both_output_directories(self):
        self.create_fixture(self.task, 'fn main() { println!("{}", std::env::args().nth(1).unwrap()); }')
        cargo = Path(subprocess.check_output(["rustup", "which", "cargo"], text=True).strip())
        env = self.cargo_env(self.sandbox / "home", cargo)
        outside = self.sandbox / "outside"
        parent = self.sandbox / "scratch"
        env.update(YULIUS_BUILD_SCRATCH=str(parent), CARGO_BUILD_BUILD_DIR=str(outside),
                   RUSTC=str(cargo.with_name("rustc")))
        settings = self.task / "elsewhere.toml"
        settings.write_text(f'[build]\nbuild-dir = "{outside}"')
        for args, expected in [(["clean", "--offline"], 0),
                               (["run", "--quiet", "--offline", "--", "--target-dir"], 0),
                               (["build", "--offline", "--target-dir=" + str(outside)], 1),
                               (["build", "--offline", "--target-dir", str(outside)], 1),
                               (["build", "--config", f'build.build-dir="{outside}"'], 1),
                               (["build", "--config=" + str(settings)], 1),
                               (["check", "--offline", "--config", "build.jobs=1"], 0)]:
            with self.subTest(args=args):
                result = subprocess.run([self.task / "scripts/cargo", "--scratch-target", *args],
                                        cwd=self.task, env=env, capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, expected, result.stderr)
                if args[0] == "run":
                    self.assertEqual(result.stdout.strip(), "--target-dir")
                self.assertFalse(outside.exists(), "intermediate artifacts escaped disk scratch")
                self.assertFalse(list(parent.iterdir()))

    def test_unconfirmed_shutdown_retains_its_target_and_runs_cargo_once(self):
        root = self.sandbox / "scratch"
        env = self.caller_fixture("from pathlib import Path\n"
                                  "target = Path(os.environ['CARGO_TARGET_DIR'])\n"
                                  "with open(os.environ['RECORD'], 'a') as record: print(target, file=record)\n"
                                  "(target / 'keep').write_text('build')")
        env.update(RECORD=str(self.record), YULIUS_BUILD_SCRATCH=str(root))
        with (self.task / "scripts/compile_cache.py").open("a") as fixture:
            fixture.write("\ndef stop_build_group(group, requested=None):\n"
                          "    raise UnreapedBuild('process observation unavailable')\n")
        result = subprocess.run([self.task / "scripts/cargo", "--scratch-target", "build"],
                                env=env, capture_output=True, text=True, timeout=5, start_new_session=True)
        self.assertNotEqual(result.returncode, 0)
        targets = self.record.read_text().splitlines()
        self.assertEqual(len(targets), 1)
        self.assertIn("retained build target " + targets[0], result.stderr)
        self.assertTrue((Path(targets[0]) / "keep").is_file())

    def test_portable_supervisor_keeps_target_for_worker_in_another_session(self):
        import signal
        env = self.caller_fixture("""
from pathlib import Path
reader, writer = os.pipe()
pid = os.fork()
if pid == 0:
    os.close(reader)
    os.setsid()
    os.closerange(0, 3)
    os.write(writer, b'1')
    import signal
    signal.pause()
    os._exit(0)
os.close(writer)
os.read(reader, 1)
Path(os.environ['RECORD']).write_text(str(pid))
Path(os.environ['CARGO_TARGET_DIR'], 'keep').touch()
""")
        # Exercise the non-subreaper supervisor with real POSIX process groups on Linux.
        adapter = self.task / "scripts/compile_cache.py"
        code = adapter.read_text()
        selection = 'subreaper = disposable and sys.platform == "linux"'
        self.assertIn(selection, code)
        adapter.write_text(code.replace(selection, 'subreaper = False'))
        parent = self.sandbox / "scratch"
        env.update(YULIUS_BUILD_SCRATCH=str(parent), RECORD=str(self.record), RUSTC_WRAPPER="fixture")
        try:
            result = subprocess.run([self.task / "scripts/cargo", "--scratch-target", "check"],
                                    env=env, start_new_session=True, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0, "detached worker lost its build target")
            target, = parent.iterdir()
            self.assertIn("retained build target " + str(target), result.stderr)
            self.assertTrue((target / "keep").is_file())
        finally:
            if self.record.exists():
                try:
                    os.kill(int(self.record.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_worktree_and_ci_lane_share_only_their_sandbox_store(self):
        import_script = ("print(__import__('json').dumps([os.environ['MBX_CACHE_DIR'], os.getcwd()]), flush=True)\n"
                         "os.execv(os.environ['CARGO'], [os.environ['CARGO'], *sys.argv[1:]])")
        for tree, home in ((self.task, self.sandbox / "home"), (self.lane, self.sandbox / "ci/home")):
            result = self.launch(tree, home, import_script)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), [str(self.sandbox / "rust-compile-cache"), str(tree)])
        foreign = self.checkout(self.sandbox.parent / "other/worktrees/b")
        result = self.launch(foreign, self.sandbox.parent / "other/home", import_script)
        self.assertEqual(json.loads(result.stdout)[0], str(self.sandbox.parent / "other/rust-compile-cache"))

    def test_a_new_worktree_finds_the_pin_bootstrapped_by_another_task(self):
        import platform
        import tomllib
        home = self.sandbox / "home"
        pins = tomllib.loads((self.task / ".config/tools.toml").read_text())["mbx"]
        binary = home / ".cache/yulius-tools" / f"mbx-{pins['version']}-{platform.machine()}" / "mbx"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\necho shared-tool\nexec \"$CARGO\" \"$@\"\n")
        binary.chmod(0o755)
        result = self.launch(self.task, home, None)
        self.assertEqual(result.stdout.strip(), "shared-tool")
        self.assertEqual(self.record.read_text().strip(), "build")

    def test_missing_tool_and_broken_store_compile_plainly(self):
        home = self.sandbox / "home"
        self.assertEqual(self.launch(self.task, home, None).returncode, 0)
        (self.sandbox / "rust-compile-cache").write_text("broken")
        self.assertEqual(self.launch(self.task, home, "sys.exit(99)").returncode, 0)
        self.assertEqual(self.record.read_text().splitlines(), ["build", "build"])

    def test_tools_can_select_the_project_launcher_as_cargo(self):
        backend = self.sandbox / "backend"
        backend.mkdir()
        (backend / "cargo").symlink_to(self.cargo)
        launcher = self.task / "scripts/cargo"
        env = self.cargo_env(self.sandbox / "home", launcher)
        env.update(PATH=os.pathsep.join([str(launcher.parent), str(backend), env["PATH"]]),
                   RECORD=str(self.record), CARGO_RESULT="0")
        for configured in (str(launcher), "cargo"):
            env["CARGO"] = configured
            result = subprocess.run([launcher, "metadata"], cwd=self.task, env=env,
                                    stdin=subprocess.DEVNULL, capture_output=True, timeout=4)
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.record.read_text().splitlines(), ["metadata", "metadata"])

    def test_a_build_arriving_during_upkeep_runs_without_waiting_for_the_store(self):
        import fcntl
        lease = os.open(self.sandbox, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, lease)
        fcntl.flock(lease, fcntl.LOCK_EX)
        result = self.launch(self.task, self.sandbox / "home", "print('unexpected cache')")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual(self.record.read_text().strip(), "build")

    def test_existing_unowned_directory_stays_outside_disk_upkeep(self):
        cache = self.sandbox / "rust-compile-cache"
        cache.mkdir(parents=True)
        (cache / "keep").write_text("pre-existing")
        result = self.launch(self.task, self.sandbox / "home", "print('unexpected cache')")
        self.assertEqual(result.stdout, "")
        self.assertEqual(self.record.read_text().strip(), "build")
        self.assertEqual(list(cache.iterdir()), [cache / "keep"])

    def test_builds_only_use_the_complete_disk_upkeep_stamp(self):
        cache = self.sandbox / "rust-compile-cache"
        cache.mkdir()
        for contents in (b"", b"mbx-v1", b"mbx-v1\nextra"):
            (cache / ".yulius-mbx-v1").write_bytes(contents)
            result = self.launch(self.task, self.sandbox / "home", "print('unexpected cache')")
            self.assertEqual(result.stdout, "")

    def test_unfinished_eviction_uses_cargo_until_upkeep_finishes(self):
        home = self.sandbox / "home"
        cache = self.sandbox / "rust-compile-cache"
        dispatch = "print('cached', flush=True); os.execv(os.environ['CARGO'], [os.environ['CARGO'], *sys.argv[1:]])"
        self.assertEqual(self.launch(self.task, home, dispatch).stdout.strip(), "cached")
        pending = cache / ".yulius-mbx-pruning"
        ready = cache / ".yulius-mbx-v1"
        ready.rename(pending)
        self.assertEqual(self.launch(self.task, home, dispatch).stdout, "")
        self.assertEqual(self.record.read_text().splitlines(), ["build", "build"])
        pending.rename(ready)
        self.assertEqual(self.launch(self.task, home, dispatch).stdout.strip(), "cached")

    def test_interrupted_publication_leaves_the_next_launcher_able_to_initialize(self):
        from unittest.mock import patch
        import compile_cache
        cache = self.sandbox / "rust-compile-cache"
        with patch.object(Path, "rename", side_effect=InterruptedError), self.assertRaises(InterruptedError):
            compile_cache.open_store(cache, self.task)
        self.assertFalse(cache.exists())
        result = self.launch(self.task, self.sandbox / "home", "print('cached')")
        self.assertEqual(result.stdout.strip(), "cached")
        self.assertEqual((cache / ".yulius-mbx-v1").read_bytes(), b"mbx-v1\n")

    def test_terminating_the_launcher_stops_its_descendants_and_keeps_the_lease_private(self):
        self.check_cached_cancellation(scratch=False)

    def test_cancelling_a_cached_disposable_target_reaps_its_backend(self):
        self.check_cached_cancellation(scratch=True)

    def test_cancelled_cached_build_reaps_a_separate_test_group(self):
        self.check_cached_cancellation(scratch=True, separate_group=True)

    def check_cached_cancellation(self, scratch, separate_group=False):
        import fcntl
        import select
        import signal
        import socket
        home = self.sandbox / "home"
        home.mkdir()
        ready = self.sandbox.parent / "ready"
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(ready))
            listener.listen()
            listener.settimeout(5)
            self.cargo.write_text("#!/usr/bin/env python3\nimport os,signal,socket,sys\n"
                                  + ("if os.fork(): signal.pause(); sys.exit(1)\nos.setpgid(0,0)\n"
                                     if separate_group else "")
                                  + "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                                  "s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[2])\n"
                                  "s.sendall((str(os.getpid())+chr(10)+repr([os.readlink('/proc/self/fd/'+x) for x in os.listdir('/proc/self/fd') "
                                  "if os.path.exists('/proc/self/fd/'+x)])).encode());signal.pause()")
            mbx = self.task / ".tools/bin/mbx"
            mbx.write_text('#!/bin/sh\n' + ('exec ' if scratch else '') + '"$CARGO" "$@"\n')
            mbx.chmod(0o755)
            env = self.cargo_env(home, self.cargo)
            target_root = self.sandbox / "scratch"
            env["YULIUS_BUILD_SCRATCH"] = str(target_root)
            flag = ["--scratch-target"] if scratch else []
            backend = None
            proc = subprocess.Popen([self.task / "scripts/cargo", *flag, "run", ready], env=env,
                                    stdin=subprocess.DEVNULL, start_new_session=True)
            try:
                conn, _ = listener.accept()
                with conn:
                    pid, descriptors = conn.recv(4096).decode().split("\n", 1)
                    backend = int(pid)
                    proc.terminate()
                    proc.wait(timeout=8)
                    self.assertTrue(select.select([conn], [], [], 1)[0], "Cargo child survived cancellation")
                    self.assertEqual(conn.recv(1), b"")
                    backend = None
                    if scratch:
                        self.assertEqual(list(target_root.iterdir()), [])
                    self.assertNotIn(str(self.sandbox) + "'", descriptors)
                    lease = os.open(self.sandbox, os.O_RDONLY | os.O_DIRECTORY)
                    try:
                        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    finally:
                        os.close(lease)
            finally:
                if backend is not None:
                    try:
                        os.kill(backend, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait(timeout=5)

    def test_cache_startup_failure_falls_back_and_cargo_failure_runs_once(self):
        home = self.sandbox / "home"
        failed_start = self.launch(self.task, home, "sys.exit(1)")
        self.assertEqual(failed_start.returncode, 0, failed_start.stderr)
        silent_tool = self.launch(self.task, home, "sys.exit(0)")
        self.assertEqual(silent_tool.returncode, 0)
        delegate = "os.execv(os.environ['CARGO'], [os.environ['CARGO'], *sys.argv[1:]])"
        failed_build = self.launch(self.task, home, delegate, cargo_result=37)
        self.assertEqual(failed_build.returncode, 37, failed_build.stderr)
        self.assertEqual(self.record.read_text().splitlines(), ["build", "build", "build"])

    def caller_fixture(self, cargo_source):
        home = self.sandbox / "home"
        home.mkdir()
        self.cargo.write_text("#!/usr/bin/env python3\nimport os,sys\n" + cargo_source)
        mbx = self.task / ".tools/bin/mbx"
        mbx.write_text('#!/bin/sh\nexec "$CARGO" "$@"\n')
        mbx.chmod(0o755)
        return self.cargo_env(home, self.cargo)

    def test_a_make_jobserver_token_reaches_cargo(self):
        env = self.caller_fixture("print(os.read(int(os.environ['TOKEN_FD']),1).decode())")
        read, write = os.pipe()
        try:
            for flag in ([], ["--scratch-target"]):
                os.write(write, b"X")
                env.update(CARGO_MAKEFLAGS=f"-j --jobserver-auth={read},{write}", TOKEN_FD=str(read))
                result = subprocess.run([self.task / "scripts/cargo", *flag, "build"], env=env,
                                        start_new_session=True, stdin=subprocess.DEVNULL, pass_fds=(read, write),
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), "X")
        finally:
            os.close(read)
            os.close(write)

    def test_a_foreground_test_reads_its_controlling_terminal(self):
        self.check_foreground_terminal(redirect_stdin=False)

    def test_a_foreground_command_with_redirected_input_can_read_dev_tty(self):
        self.check_foreground_terminal(redirect_stdin=True)

    def test_disposable_target_read_from_tty_and_cleanup_on_term(self):
        self.check_foreground_terminal(redirect_stdin=False, scratch=True)

    def test_terminal_ctrl_c_reaps_a_resistant_worker_before_releasing_scratch(self):
        self.check_terminal_interrupt("signal.SIG_DFL")

    def test_a_handled_terminal_interrupt_reaps_workers_before_releasing_scratch(self):
        self.check_terminal_interrupt("lambda *_: sys.exit(130)")

    def test_terminal_hangup_preserves_the_cargo_exit_status(self):
        self.check_terminal_interrupt("signal.SIG_DFL", hangup=True)

    def test_hangup_keeps_scratch_when_worker_shutdown_is_unconfirmed(self):
        self.check_terminal_interrupt("signal.SIG_DFL", hangup=True, unconfirmed=True)

    def test_foreground_interrupt_reaps_a_worker_in_a_separate_test_group(self):
        self.check_terminal_interrupt("signal.SIG_DFL", separate_group=True)

    def check_terminal_interrupt(self, handler, *, hangup=False, unconfirmed=False, separate_group=False):
        import pty
        import select
        import signal
        import socket
        ready = self.sandbox.parent / "worker.sock"
        root = self.sandbox / "scratch"
        env = self.caller_fixture(f"""
import signal, socket
signal.signal(signal.SIGINT, {handler})
if os.fork() == 0:
    if {separate_group!r}: os.setpgid(0, 0)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(os.environ["READY"])
        conn.sendall(str(os.getpid()).encode())
        signal.pause()
signal.pause()
""")
        if unconfirmed:
            with (self.task / "scripts/compile_cache.py").open("a") as fixture:
                fixture.write("\ndef stop_build_group(group, requested=None):\n"
                              "    raise UnreapedBuild('process observation unavailable')\n")
        env.update(READY=str(ready), YULIUS_BUILD_SCRATCH=str(root))
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(ready))
            listener.listen()
            listener.settimeout(6)
            pid, terminal = pty.fork()
            if pid == 0:
                with (self.sandbox / "launcher.stderr").open("w") as errors:
                    os.dup2(errors.fileno(), 2)
                os.execve(self.task / "scripts/cargo", ["cargo", "--scratch-target", "build"], env)
            worker = None
            reaped = False
            try:
                conn, _ = listener.accept()
                with conn, os.fdopen(os.pidfd_open(pid), "rb", buffering=0) as launcher:
                    worker = int(conn.recv(32))
                    if hangup:
                        os.close(terminal)
                        terminal = None
                    else:
                        os.write(terminal, b"\x03")
                    self.assertTrue(select.select([launcher], [], [], 8)[0], "launcher missed terminal signal")
                    _, status = os.waitpid(pid, 0)
                    reaped = True
                    if unconfirmed:
                        self.assertNotEqual(os.waitstatus_to_exitcode(status), 0)
                        self.assertIn("retained build target", (self.sandbox / "launcher.stderr").read_text())
                        self.assertEqual(len(list(root.iterdir())), 1)
                        return
                    self.assertEqual(os.waitstatus_to_exitcode(status), 129 if hangup else 130)
                    self.assertTrue(select.select([conn], [], [], 0)[0], "worker survived launcher exit")
                    self.assertEqual(conn.recv(1), b"")
                    worker = None
                    self.assertEqual(list(root.iterdir()), [])
            finally:
                if worker is not None:
                    try:
                        os.kill(worker, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                if not reaped:
                    os.kill(pid, signal.SIGKILL)
                    os.waitpid(pid, 0)
                if terminal is not None:
                    os.close(terminal)

    def test_shell_can_foreground_a_disposable_build_started_in_background(self):
        import pty
        import select
        import signal
        import time
        env = self.caller_fixture("print('reader ready',flush=True); print('received '+input(),flush=True)")
        shell = shutil.which("bash")
        env.update(YULIUS_BUILD_SCRATCH=str(self.sandbox / "scratch"), PS1="shell> ", PROMPT_COMMAND="")
        pid, terminal = pty.fork()
        if pid == 0:
            os.chdir(self.task)
            os.execve(shell, [shell, "--noprofile", "--norc", "-i"], env)

        def read_until(marker):
            output = b""
            deadline = time.monotonic() + 5
            while marker not in output:
                self.assertTrue(select.select([terminal], [], [], max(0, deadline - time.monotonic()))[0],
                                f"shell did not report {marker!r}: {output!r}")
                output += os.read(terminal, 4096)
            return output

        job = None
        try:
            read_until(b"shell> ")
            os.write(terminal, b"scripts/cargo --scratch-target run &\n")
            output = read_until(b"reader ready")
            import re
            job = int(re.search(rb"\[1\] (\d+)", output).group(1))
            # Bash waits for its job leader to stop, as it does before offering fg.
            os.write(terminal, b"wait %1; jobs\n")
            read_until(b"Stopped")
            os.write(terminal, b"fg\n")
            read_until(b"scripts/cargo --scratch-target run")
            os.write(terminal, b"X\n")
            read_until(b"received X")
            os.write(terminal, b"exit 0\n")
            _, status = os.waitpid(pid, 0)
            pid = None
            job = None
            self.assertEqual(os.waitstatus_to_exitcode(status), 0)
            self.assertFalse(list((self.sandbox / "scratch").iterdir()))
        finally:
            if job:
                try:
                    os.kill(job, signal.SIGTERM)
                    os.kill(job, signal.SIGCONT)
                except ProcessLookupError:
                    pass
            if pid:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            os.close(terminal)

    def test_terminal_suspends_and_resumes_the_disposable_build(self):
        import pty
        import select
        import signal
        import time
        env = self.caller_fixture("print('ready',flush=True); print('received '+input(),flush=True)")
        target_root = self.sandbox / "scratch"
        env["YULIUS_BUILD_SCRATCH"] = str(target_root)
        pid, terminal = pty.fork()
        if pid == 0:
            os.execve(self.task / "scripts/cargo", ["cargo", "--scratch-target", "run"], env)
        reaped = False
        try:
            self.assertTrue(select.select([terminal], [], [], 5)[0])
            self.assertIn(b"ready", os.read(terminal, 4096))
            os.write(terminal, b'\x1a')
            deadline = time.monotonic() + 3
            while True:
                child, status = os.waitpid(pid, os.WUNTRACED | os.WNOHANG)
                if child:
                    self.assertTrue(os.WIFSTOPPED(status), status)
                    break
                self.assertLess(time.monotonic(), deadline, "wrapper never suspended with Cargo")
                time.sleep(0.01)
            os.kill(pid, signal.SIGCONT)
            os.write(terminal, b"X\n")
            received = b""
            while b"received X" not in received:
                self.assertTrue(select.select([terminal], [], [], 5)[0])
                received += os.read(terminal, 4096)
            _, status = os.waitpid(pid, 0)
            reaped = True
            self.assertEqual(os.waitstatus_to_exitcode(status), 0)
            self.assertFalse(list(target_root.iterdir()))
        finally:
            if not reaped:
                os.kill(pid, signal.SIGCONT)
                os.kill(pid, signal.SIGTERM)
                os.waitpid(pid, 0)
            os.close(terminal)

    def check_foreground_terminal(self, redirect_stdin, scratch=False):
        import pty
        import select
        import signal
        import time
        read = "open('/dev/tty').readline().strip()" if redirect_stdin else "input()"
        env = self.caller_fixture(f"print('ready',flush=True);print('received '+{read},flush=True)" + (";input()" if scratch else ""))
        args = ["cargo", "test", "--", "--nocapture"]
        if scratch:
            root = self.sandbox / "scratch"
            root.mkdir()
            env.update(YULIUS_BUILD_SCRATCH=str(root))
            args.insert(1, "--scratch-target")
        pid, terminal = pty.fork()
        if pid == 0:
            if redirect_stdin:
                with open(os.devnull, "rb") as empty:
                    os.dup2(empty.fileno(), 0)
            os.execve(self.task / "scripts/cargo", args, env)
        try:
            received = b""
            for expected in (b"ready", b"received X"):
                deadline = time.monotonic() + 4
                while expected not in received:
                    self.assertTrue(select.select([terminal], [], [], max(0, deadline - time.monotonic()))[0],
                                    f"command stopped before {expected!r}")
                    received += os.read(terminal, 4096)
                if expected == b"ready":
                    os.write(terminal, b"X\n")
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
            os.close(terminal)

        if scratch:
            self.assertEqual(list(root.iterdir()), [], "cancelled foreground build retained its target")

    @unittest.skipUnless(shutil.which("script"), "Linux CI terminal required")
    def test_unattended_ci_caches_inside_the_original_runner_session(self):
        import tomllib
        env = self.caller_fixture("print('cargo-session',os.getsid(0))")
        env.update(YULIUS_CI="1", YULIUS_CI_RUN="fixture-run")
        tools = self.task / "fixture-bin"
        tools.mkdir()
        # Let the checked-in CI command skip its bootstrap in this synthetic checkout.
        bootstrap = tools / "python3"
        bootstrap.write_text(f'#!/bin/sh\n[ "$1" = scripts/bootstrap.py ] && exit 0\nexec "{sys.executable}" "$@"\n')
        bootstrap.chmod(0o755)
        env["PATH"] = str(tools) + os.pathsep + env["PATH"]
        dev = self.task / "scripts/dev"
        dev.write_text(f"#!{sys.executable}\nimport os,sys\n"
                       "if sys.argv[1]=='help': print('  checks ');sys.exit(0)\n"
                       "os.execv('scripts/cargo',['scripts/cargo','check'])")
        dev.chmod(0o755)
        mbx = self.task / ".tools/bin/mbx"
        mbx.write_text(f"#!{sys.executable}\nimport os,sys\nprint('cache-session',os.getsid(0),flush=True)\n"
                       "os.execv(os.environ['CARGO'],[os.environ['CARGO'],*sys.argv[1:]])")
        jobs = tomllib.loads((SCRIPTS.parent / "yulius.toml").read_text())["ci"]["jobs"]
        for job in jobs:
            if job["name"] in {"quality", "test"}:
                script = "exec >ci.log 2>&1 </dev/null\n" + f"{sys.executable} -c 'import os;print(os.getsid(0))'\n" + job["command"]
                result = subprocess.run(["script", "-qec", script, os.devnull], cwd=self.task, env=env,
                                        stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10)
                log = (self.task / "ci.log").read_text()
                self.assertEqual(result.returncode, 0, log)
                session = log.splitlines()[0]
                self.assertIn(f"cache-session {session}", log)
                self.assertIn(f"cargo-session {session}", log)

    def build_fixture(self, tree, target, *, plain=False, incremental=None, package=None):
        home = self.sandbox / ("ci/home" if tree == self.lane else "home")
        home.mkdir(parents=True, exist_ok=True)
        toolchain = Path(subprocess.check_output(["rustup", "which", "cargo"], cwd=SCRIPTS, text=True).strip()).parent
        mbx = tree / ".tools/bin/mbx"
        if not mbx.exists():
            mbx.symlink_to((SCRIPTS.parent / ".tools/bin/mbx").resolve())
        env = self.cargo_env(home, toolchain / "cargo")
        env.update(CARGO_HOME=str(home / ".cargo"), RUSTC=str(toolchain / "rustc"), RUSTDOC=str(toolchain / "rustdoc"))
        if incremental is not None:
            env["CARGO_INCREMENTAL"] = incremental
        command = toolchain / "cargo" if plain else tree / "scripts/cargo"
        args = [command, "build", "--offline", "--target-dir", target]
        if package:
            args.extend(["--package", package])
        result = subprocess.run(args, cwd=tree, start_new_session=True,
                                env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr)
        return tree / target / "debug/fixture", result.stderr

    def create_fixture(self, tree, source, manifest=""):
        (tree / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n' + manifest)
        (tree / "src").mkdir()
        (tree / "src/main.rs").write_text(source)

    @unittest.skipUnless((SCRIPTS.parent / ".tools/bin/mbx").exists(), "optional pinned mbx unavailable")
    def test_cached_registry_file_macro_agrees_with_plain_cargo_in_each_home(self):
        for tree, home in ((self.task, self.sandbox / "home"), (self.lane, self.sandbox / "ci/home")):
            registry = home / ".cargo/registry/src/index.crates.io-1949cf8c6b5b557f"
            dep = registry / "loc-0.1.0"
            (dep / "src").mkdir(parents=True)
            (dep / "src/lib.rs").write_text('pub fn f()-> &\'static str { file!() }')
            (dep / "Cargo.toml").write_text('[package]\nname="loc"\nversion="0.1.0"\nedition="2024"\n')
            (dep / ".cargo-checksum.json").write_text('{"files":{},"package":null}')
            (tree / ".cargo").mkdir()
            (tree / ".cargo/config.toml").write_text('[source.crates-io]\nreplace-with="local"\n'
                                                     f'[source.local]\ndirectory="{registry}"\n')
            self.create_fixture(tree, 'fn main(){println!("{}",loc::f());}',
                                '[dependencies]\nloc="=0.1.0"\n[profile.dev]\ndebug=false\n')
            expected = subprocess.check_output([self.build_fixture(tree, "plain", plain=True)[0]], text=True)
            actual = subprocess.check_output([self.build_fixture(tree, "cached")[0]], text=True)
            self.assertEqual(expected.strip(), str(dep / "src/lib.rs"))
            self.assertEqual(actual, expected)
            # This registry's literal source root is unchanged in another fresh target.
            _, report = self.build_fixture(tree, "fresh", package="loc")
            self.assertRegex(report, r"mbx\[cache\]: [1-9][0-9]* hits")

    @unittest.skipUnless((SCRIPTS.parent / ".tools/bin/mbx").exists() and shutil.which("readelf"),
                         "pinned mbx and readelf required")
    def test_debug_compilation_directory_belongs_to_the_current_worktree(self):
        for tree in (self.task, self.lane):
            self.create_fixture(tree, 'fn main(){println!("debug");}')
            for target in ("plain", "cached"):
                binary, _ = self.build_fixture(tree, target, plain=target == "plain")
                debug = subprocess.check_output(["readelf", "--debug-dump=info", binary], text=True)
                self.assertTrue(any("DW_AT_comp_dir" in line and str(tree) in line
                                    for line in debug.splitlines()), f"missing debug root for {tree} in {target}")

    @unittest.skipUnless((SCRIPTS.parent / ".tools/bin/mbx").exists(), "optional pinned mbx unavailable")
    def test_explicit_incremental_build_keeps_private_compiler_state(self):
        self.create_fixture(self.task, 'fn main(){}')
        self.build_fixture(self.task, "target", incremental="1")
        self.assertTrue(list((self.task / "target/debug/incremental").rglob("dep-graph.bin")))

    @unittest.skipUnless((SCRIPTS.parent / ".tools/bin/mbx").exists(), "optional pinned mbx unavailable")
    def test_real_cache_reuses_a_library_and_rebuilds_changed_or_corrupt_outputs(self):
        from concurrent.futures import ThreadPoolExecutor
        for tree in (self.task, self.lane):
            self.create_fixture(tree, 'fn main(){println!("{} {}", fixture::value(),env!("CARGO_MANIFEST_DIR"));}',
                                '[profile.dev]\ndebug=false\n')
            (tree / "src/lib.rs").write_text('pub fn value() -> u32 { 17 }')

        def build(tree, target):
            binary, report = self.build_fixture(tree, target)
            return subprocess.check_output([binary], text=True).strip(), report, binary

        first, _, binary = build(self.task, "a")
        self.assertEqual(first, f"17 {self.task}")
        warm, report, _ = build(self.lane, "b")
        self.assertEqual(warm, f"17 {self.lane}")
        self.assertRegex(report, r"mbx\[cache\]: [1-9][0-9]* hits")
        # Keep the same byte length, which would pass a superficial integrity check.
        contents = binary.read_bytes()
        objects = self.sandbox / "rust-compile-cache/actions/cas"
        stored = next(p for p in objects.rglob("*") if p.is_file()
                      and p.stat().st_size == len(contents) and p.read_bytes() == contents)
        stored.chmod(0o600)
        stored.write_bytes(bytes(len(contents)))
        restored, _, _ = build(self.task, "recovery")
        self.assertEqual(restored, first)

        (self.lane / "src/lib.rs").write_text('pub fn value() -> u32 { 29 }')
        with ThreadPoolExecutor(max_workers=2) as jobs:
            a = jobs.submit(build, self.task, "concurrent")
            b = jobs.submit(build, self.lane, "concurrent")
            self.assertEqual(a.result()[0], f"17 {self.task}")
            self.assertEqual(b.result()[0], f"29 {self.lane}")

        # Rust does not record ambient I/O during procedural macro expansion in dep-info.
        # Re-export the macro through a library so checking only direct .so inputs is insufficient.
        for tree in (self.task, self.lane):
            (tree / "macro/src").mkdir(parents=True)
            (tree / "forward/src").mkdir(parents=True)
            (tree / "macro/Cargo.toml").write_text('[package]\nname="location"\nversion="0.1.0"\nedition="2024"\n'
                                                  '[lib]\nproc-macro=true\n')
            (tree / "macro/src/lib.rs").write_text(
                '#[proc_macro] pub fn location(_:proc_macro::TokenStream)->proc_macro::TokenStream {'
                'proc_macro::TokenTree::Literal(proc_macro::Literal::string(&std::env::var("CARGO_MANIFEST_DIR").unwrap())).into()}')
            (tree / "forward/Cargo.toml").write_text('[package]\nname="forward"\nversion="0.1.0"\nedition="2024"\n'
                                                     '[dependencies]\nlocation={path="../macro"}\n')
            (tree / "forward/src/lib.rs").write_text('pub use location::location;')
            (tree / "Cargo.toml").write_text((tree / "Cargo.toml").read_text() + '[dependencies]\nforward={path="forward"}\n')
            (tree / "src/lib.rs").write_text('pub use forward::location;')
            (tree / "src/main.rs").write_text('fn main(){println!("{}",fixture::location!());}')
        for tree in (self.task, self.lane):
            value, report, _ = build(tree, "proc-macro")
            self.assertEqual(value, str(tree))
            self.assertRegex(report, r"yulius\[cache\]: [1-9][0-9]* compiler invocations bypassed")


class ReleaseInstall(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "the optional release installs on Linux")
    def test_interrupted_optional_download_allows_bootstrap_to_finish(self):
        import http.client
        import runpy
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temp:
            sandbox = Path(temp)
            tree = CompileCache.checkout(sandbox / "worktrees/task")
            shutil.copy2(SCRIPTS / "bootstrap.py", tree / "scripts/bootstrap.py")
            (tree / "rust-toolchain.toml").write_text('[toolchain]\nchannel="stable"\nprofile="minimal"\ncomponents=[]\n')
            (tree / ".config/tools.toml").write_text('[tools]\n[mbx]\n[zig]\n')
            with (patch.dict(os.environ, HOME=str(sandbox / "home"), YULIUS_SANDBOX="fixture",
                             YULIUS_TOOLS_CACHE=str(sandbox / "cache")),
                  patch.object(sys, "path", list(sys.path)),
                  patch("shutil.which", return_value="/fixture/rustup"),
                  patch("subprocess.run"), patch("subprocess.check_output", return_value="/fixture/rustc"),
                  patch("compile_cache.install", side_effect=http.client.IncompleteRead(b"truncated")),
                  patch("zig_install.install", return_value="zig installed") as zig):
                runpy.run_path(str(tree / "scripts/bootstrap.py"))
                zig.assert_called_once()

    def test_only_a_verified_executable_is_published_and_reused(self):
        import hashlib
        import io
        import tarfile
        from compile_cache import install

        archive = io.BytesIO()
        payload = b"pinned binary"
        with tarfile.open(fileobj=archive, mode="w:gz") as package:
            member = tarfile.TarInfo("mbx")
            member.size = len(payload)
            package.addfile(member, io.BytesIO(payload))
        data = archive.getvalue()
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / "tools"
            bins = Path(temp) / "bin"
            bins.mkdir()
            requests = []

            def fetch(url, timeout):
                requests.append(url)
                self.assertGreater(timeout, 0)
                return io.BytesIO(data)

            pins = {"version": "1", "x86_64": "wrong digest"}
            with self.assertRaisesRegex(ValueError, "digest"):
                install(pins, cache, bins, "x86_64", fetch)
            self.assertFalse((bins / "mbx").exists())
            pins["x86_64"] = hashlib.sha256(data).hexdigest()
            install(pins, cache, bins, "x86_64", fetch)
            install(pins, cache, bins, "x86_64", fetch)
            self.assertEqual(len(requests), 2)
            self.assertEqual((bins / "mbx").read_bytes(), payload)
            self.assertTrue(os.access(bins / "mbx", os.X_OK))
