"""Tests of Yulius's own CD scripts, scripts/cd (SPEC-591, ADR-0430).

The scripts run unchanged. `ssh` is a stub that checks the options CD must pass (the pinned host
key, batch mode, the alias), that the key is held by the agent the script started, and then runs
the remote command locally against a temporary root (`YULIUS_CD_ROOT`). On that root `systemctl`,
`curl`, `runuser`, `sudo`, `id` and `chown` are stubs, and `/usr/local/bin/yulius` is a fake whose
start (and `store-migrate`) migrates a real SQLite store as a release of the given schema would.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sqlite3
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CD = REPO / "scripts" / "cd"
SCRIPTS = ("lib.sh", "deploy.sh", "verify.sh", "rollback.sh", "remote.sh", "build.sh")
DELETES = CD / "migration_deletes.py"

OLD = "3fc0e5976ae2ce2da68a81c400866e2669316f66"
NEW = "86eb47202aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
NEWER = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
UNIT_ENV = (
    "YULIUS_DATA=/var/lib/yulius YULIUS_PUBLIC_URL=https://staging.example "
    "YULIUS_BIND=127.0.0.1:8080 YULIUS_INTERNAL_BIND=127.0.0.1:8081 YULIUS_CONTAINED=true "
    "PATH=/usr/local/bin:/usr/bin:/bin HOME=/var/lib/yulius"
)
TOOLS = ("ssh-agent", "ssh-add", "ssh-keygen", "python3", "sha256sum", "flock")


def write_exe(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text).lstrip())
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def dump(path: Path) -> str:
    """The store's contents as SQL: the same for the same schema and rows, whatever its pages."""
    c = sqlite3.connect(path)
    try:
        return "\n".join(c.iterdump()) + f"\nuser_version {c.execute('PRAGMA user_version').fetchone()[0]}"
    finally:
        c.close()


class CdFilesTest(unittest.TestCase):
    """What needs no tools: the scripts parse, the pin, the schema count."""

    def test_every_script_parses(self) -> None:
        for name in SCRIPTS:
            with self.subTest(name):
                subprocess.run(["sh", "-n", str(CD / name)], check=True)

    def test_the_staging_host_key_is_pinned(self) -> None:
        lines = [
            line.split()
            for line in (CD / "known_hosts").read_text().splitlines()
            if line and not line.startswith("#")
        ]
        self.assertEqual(
            lines,
            [["yulius-staging", "ssh-ed25519", "AAAAC3NzaC1lZDI1NTE5AAAAIFdVdbuV7//7CeSrfLXUR6NxGdnz1p3a0yvT33KyU6Fm"]],
        )

    def test_the_expected_schema_is_the_migrations_array(self) -> None:
        """verify.sh counts the migration files; the store's schema is the MIGRATIONS array."""
        lib = (REPO / "crates" / "yulius-store" / "src" / "lib.rs").read_text()
        body = lib.split("const MIGRATIONS: &[&str] = &[", 1)[1].split("\n];", 1)[0]
        entries = [
            line for line in body.splitlines()
            if line.strip().startswith(("include_str!(", "REUSABLE_SANDBOX_PORTS"))
        ]
        files = list((REPO / "crates" / "yulius-store" / "src" / "migrations").glob("*.sql"))
        self.assertEqual(len(entries), len(files))

    def test_the_migrations_that_delete_rows_are_named_by_their_schema(self) -> None:
        """Their position in MIGRATIONS, not their file number; 0077's trigger deletes nothing."""
        out = subprocess.run(["python3", "-I", str(DELETES), str(REPO)], capture_output=True, text=True, check=True)
        self.assertEqual(out.stdout.strip(), "69:session_chunks,72:session_chunks")

    def test_comments_and_trigger_bodies_are_no_deletion(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "crates" / "yulius-store" / "src"
            (src / "migrations").mkdir(parents=True)
            (src / "migrations" / "0001_a.sql").write_text("CREATE TABLE a (x);\n")
            (src / "migrations" / "0002_b.sql").write_text(
                "-- DELETE FROM a\nCREATE TRIGGER t AFTER INSERT ON a BEGIN\n  DELETE FROM b;\nEND;\n"
                "delete from Notes WHERE 1;\n"
                "/* DELETE FROM blocked */\n"
                "CREATE TRIGGER u AFTER UPDATE ON a BEGIN\n"
                "  UPDATE a SET x = CASE WHEN NEW.x IS NULL THEN 0 ELSE NEW.x END;\n"
                "  DELETE FROM c;\n"
                "END;\n"
                "CREATE TRIGGER v BEFORE DELETE ON a BEGIN\n"
                "  SELECT RAISE(ABORT, 'no; DELETE FROM e here');\n"
                "END;\n"
                # Without skipping strings, these name e (a quote ends the trigger early) and g.
                "CREATE TRIGGER w AFTER DELETE ON a BEGIN\n"
                "  SELECT RAISE(ABORT, 'at its end'); DELETE FROM e;\n"
                "END;\n"
                "INSERT INTO a VALUES ('DELETE FROM g');\n"
                "DELETE FROM d WHERE x = 'it''s';\n"
            )
            (src / "migrations" / "0003_c.sql").write_text("")
            (src / "lib.rs").write_text(
                'const LATER: &str = include_str!("migrations/0003_c.sql");\n'
                "const MIGRATIONS: &[&str] = &[\n"
                '    include_str!("migrations/0001_a.sql"),\n'
                "    // Schema version 2.\n"
                '    include_str!("migrations/0002_b.sql"),\n'
                "    LATER,\n"
                "];\n"
            )
            out = subprocess.run(["python3", "-I", str(DELETES), tmp], capture_output=True, text=True, check=True)
            self.assertEqual(out.stdout.strip(), "2:d,2:notes")


class CdScriptsTest(unittest.TestCase):
    """The scripts against a fake VM."""

    @classmethod
    def setUpClass(cls) -> None:
        missing = [t for t in TOOLS if shutil.which(t) is None]
        if missing:
            # In CI a missing tool is a failure, never a gate that tested nothing.
            if os.environ.get("YULIUS_CI"):
                raise RuntimeError(f"scripts/test_cd.py needs {', '.join(missing)}")
            raise unittest.SkipTest(f"needs {', '.join(missing)}")
        cls.keys = Path(tempfile.mkdtemp(prefix="yulius-cd-keys-"))
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "test", "-f", str(cls.keys / "id")],
            check=True,
        )
        cls.key = (cls.keys / "id").read_text()

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.keys, ignore_errors=True)

    def setUp(self) -> None:
        # A short path: the agent's socket lives under it.
        self.tmp = Path(tempfile.mkdtemp(prefix="ycd-", dir="/tmp"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        shutil.copytree(CD, self.repo / "scripts" / "cd")
        shutil.copy2(REPO / "rust-toolchain.toml", self.repo / "rust-toolchain.toml")
        self.migrations = self.repo / "crates" / "yulius-store" / "src" / "migrations"
        self.migrations.mkdir(parents=True)
        entries = []
        for n in range(1, 6):
            (self.migrations / f"{n:04}_m.sql").write_text("")
            entries.append(f'    include_str!("migrations/{n:04}_m.sql"),')
        (self.migrations.parent / "lib.rs").write_text(
            "const MIGRATIONS: &[&str] = &[\n" + "\n".join(entries) + "\n];\n"
        )
        self.root = self.tmp / "root"
        self.run_dir = self.root / "run"
        self.run_dir.mkdir(parents=True)
        self.data = self.root / "var" / "lib" / "yulius"
        self.data.mkdir(parents=True)
        self.releases = self.root / "var" / "lib" / "yulius-releases"
        self.cache = self.tmp / "cache"
        self.cache.mkdir()
        self.bin = self.tmp / "bin"
        self.stubs()
        self.release(OLD, schema=4, install=True)
        self.store(schema=4)
        self.start()

    # --- the fake VM --------------------------------------------------------------------

    def stubs(self) -> None:
        log = self.tmp / "ssh.log"
        run = self.run_dir
        write_exe(
            self.bin / "ssh",
            f"""\
            #!/bin/sh
            # Stub ssh: the options CD must pass, then the command run here on the fake root.
            printf '%s\\n' "$*" >> {log}
            for want in BatchMode=yes StrictHostKeyChecking=yes HostKeyAlias=yulius-staging GlobalKnownHostsFile=/dev/null; do
              case " $* " in *" -o $want "*) ;; *) echo "stub ssh: missing -o $want" >&2; exit 255 ;; esac
            done
            case " $* " in *" -o UserKnownHostsFile="*) ;; *) echo "stub ssh: no known-hosts file" >&2; exit 255 ;; esac
            ssh-add -l >/dev/null 2>&1 || {{ echo "stub ssh: no key in the agent" >&2; exit 255; }}
            while [ $# -gt 0 ]; do
              case "$1" in
                -o|-p|-F) shift 2 ;;
                *@*) dest="$1"; shift; break ;;
                *) echo "stub ssh: unexpected $1" >&2; exit 255 ;;
              esac
            done
            case "$dest" in root@vm.example|deploy@vm.example) ;; *) echo "stub ssh: destination $dest" >&2; exit 255 ;; esac
            export YULIUS_CD_ROOT={self.root} YULIUS_CD_START_WAIT=3
            exec sh -c "$*"
            """,
        )
        write_exe(
            self.bin / "systemctl",
            f"""\
            #!/bin/sh
            run={run}
            case "$1" in
              show)
                case "$*" in
                  *Environment*) echo "{UNIT_ENV}" ;;
                  *User*) echo yulius ;;
                esac ;;
              is-active)
                case "$2" in
                  yulius.service) [ -e $run/active ] && {{ echo active; exit 0; }}; echo inactive; exit 3 ;;
                  # A running oneshot is "activating", and is-active exits 3 for it.
                  yulius-update.service) [ -e $run/update-active ] && {{ echo activating; exit 3; }}; echo inactive; exit 3 ;;
                esac ;;
              stop) rm -f $run/active $run/version; echo "stop $2" >> $run/systemctl.log ;;
              start|restart)
                echo "$1 $2" >> $run/systemctl.log
                [ "$2" = yulius.service ] || {{ echo "stub systemctl: only yulius.service: $2" >&2; exit 1; }}
                if [ -e $run/kill-on-start ]; then rm -f $run/kill-on-start; kill -9 $PPID; exit 1; fi
                touch $run/active
                if {self.root}/usr/local/bin/yulius serve-fake </dev/null; then
                  {self.root}/usr/local/bin/yulius version-json > $run/version
                fi ;;
              *) echo "stub systemctl: $*" >&2; exit 1 ;;
            esac
            """,
        )
        write_exe(
            self.bin / "curl",
            f"""\
            #!/bin/sh
            run={run}
            for arg; do url="$arg"; done
            case "$url" in
              http://127.0.0.1:8081/internal/version) [ -e $run/version ] || exit 7; cat $run/version ;;
              https://staging.example/healthz)
                if [ -e $run/https-down ]; then printf 000; exit 7; fi; printf 200 ;;
              *) echo "stub curl: $url" >&2; exit 2 ;;
            esac
            """,
        )
        write_exe(
            self.bin / "runuser",
            """\
            #!/bin/sh
            [ "$1" = -u ] && [ "$3" = -- ] || { echo "stub runuser: $*" >&2; exit 2; }
            shift 3
            exec "$@"
            """,
        )
        write_exe(
            self.bin / "sudo",
            """\
            #!/bin/sh
            [ "$1" = -n ] || { echo "stub sudo: $*" >&2; exit 2; }
            shift
            exec "$@"
            """,
        )
        write_exe(self.bin / "id", "#!/bin/sh\necho 0\n")
        write_exe(self.bin / "chown", "#!/bin/sh\nexit 0\n")
        real_cp = shutil.which("cp")
        write_exe(
            self.bin / "cp",
            f"""\
            #!/bin/sh
            # A snapshot that fails (a full disk) when {run}/fail-snapshot exists, and a copy from a
            # path holding the text of {run}/fail-cp-from.
            if [ -e {run}/fail-snapshot ]; then
              case "$*" in *store.new*) echo "cp: no space left on device" >&2; exit 1 ;; esac
            fi
            if [ -e {run}/fail-cp-from ]; then
              case "$1" in *"$(cat {run}/fail-cp-from)"*) echo "cp: input/output error" >&2; exit 1 ;; esac
            fi
            exec {real_cp} "$@"
            """,
        )
        write_exe(
            self.bin / "docker",
            f"""\
            #!/bin/sh
            # Stub docker: the architecture, and a build that writes a fake release of the commit
            # it was given (or of another, when {run}/build-lies exists).
            case "$1" in
              info) uname -m ;;
              build)
                echo "$*" >> {run}/docker.log
                for arg; do
                  case "$arg" in
                    type=local,dest=*) dest="${{arg#type=local,dest=}}" ;;
                    YULIUS_BUILD_COMMIT=*) commit="${{arg#YULIUS_BUILD_COMMIT=}}" ;;
                  esac
                done
                [ -e {run}/build-lies ] && commit={NEWER}
                mkdir -p "$dest"
                if [ -e {run}/built-$commit ]; then
                  cp {run}/built-$commit "$dest/yulius"
                else
                  printf '#!/bin/sh\\necho "yulius 0.1.0 (%s)"\\n' "$commit" > "$dest/yulius"
                fi
                chmod +x "$dest/yulius" ;;
              *) echo "stub docker: $*" >&2; exit 1 ;;
            esac
            """,
        )

    def fake_yulius(self, sha: str, schema: int, mode: str = "ok") -> str:
        """A fake binary of one release: its version, its migration, its doctor.

        Modes: ok; broken (its start fails before it touches the store); halfway (its start
        migrates and writes, then fails); lossy and drops (its migration deletes a row of `tasks`
        or drops `notes`).
        """
        run = self.run_dir
        db = self.data / "yulius.db"
        return textwrap.dedent(
            f"""\
            #!/bin/sh
            # fake yulius {sha}
            migrate() {{
              python3 -I - "$1" {schema} {mode} <<'EOF'
            import sqlite3, sys
            db, schema, mode = sys.argv[1], int(sys.argv[2]), sys.argv[3]
            c = sqlite3.connect(db)
            c.execute("PRAGMA journal_mode=WAL")
            have = c.execute("PRAGMA user_version").fetchone()[0]
            if have > schema:
                sys.exit("database schema version %d is newer than supported %d" % (have, schema))
            for n in range(have + 1, schema + 1):
                c.execute(f"CREATE TABLE IF NOT EXISTS added_{{n}} (x)")
            if mode == "lossy":
                c.execute("DELETE FROM tasks WHERE rowid = (SELECT min(rowid) FROM tasks)")
            if mode == "drops":
                c.execute("DROP TABLE notes")
            c.execute(f"PRAGMA user_version = {{schema}}")
            c.commit()
            print("schema %d (from %d)" % (schema, have))
            c.close()
            EOF
            }}
            case "$1" in
              version) echo "yulius 0.1.0 ({sha})" ;;
              version-json) echo '{{"commit":"{sha}","version":"0.1.0"}}' ;;
              store-migrate) [ "$2" = --db ] && [ -f "$3" ] || exit 2; migrate "$3" ;;
              serve-fake)
                [ "{mode}" != broken ] || exit 1
                migrate {db} >/dev/null || exit 1
                if [ "{mode}" = halfway ]; then
                  python3 -I -c "import sqlite3; c = sqlite3.connect('{db}'); c.execute(\\"INSERT INTO tasks VALUES ('half')\\"); c.commit()"
                  exit 1
                fi ;;
              doctor)
                if [ "${{2:-}}" = --docker-smoke ]; then
                  echo "smoke $*" >> {run}/smoke.log
                  exit "$(cat {run}/smoke-rc 2>/dev/null || echo 0)"
                fi
                [ "$YULIUS_CONTAINED" = true ] || {{ echo "FAIL  not contained"; exit 1; }}
                echo "ok    sandbox Docker         each isolated sandbox gets its own engine"
                exit "$(cat {run}/doctor-rc 2>/dev/null || echo 0)" ;;
              *) exit 2 ;;
            esac
            """
        )

    def release(self, sha: str, schema: int, mode: str = "ok", install: bool = False) -> Path:
        """A built release in the CD cache (and installed, for the one running first)."""
        binary = self.cache / "releases" / sha / "yulius"
        write_exe(binary, self.fake_yulius(sha, schema, mode))
        if install:
            target = self.root / "usr" / "local" / "bin" / "yulius"
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(binary, target)
        return binary

    def store(self, schema: int) -> None:
        c = sqlite3.connect(self.data / "yulius.db")
        c.execute("PRAGMA journal_mode=WAL")
        c.execute("CREATE TABLE tasks (title)")
        c.execute("CREATE TABLE notes (body)")
        c.executemany("INSERT INTO tasks VALUES (?)", [("a",), ("b",), ("c",)])
        c.execute("INSERT INTO notes VALUES ('n')")
        c.execute(f"PRAGMA user_version = {schema}")
        c.commit()
        c.close()

    def start(self) -> None:
        subprocess.run([self.bin / "systemctl", "start", "yulius.service"], check=True)

    def live(self, sql: str):
        c = sqlite3.connect(self.data / "yulius.db")
        try:
            return c.execute(sql).fetchone()[0]
        finally:
            c.close()

    def write(self, sql: str) -> None:
        c = sqlite3.connect(self.data / "yulius.db")
        c.execute(sql)
        c.commit()
        c.close()

    def running(self) -> str:
        return json.loads((self.run_dir / "version").read_text())["commit"]

    def systemctl_log(self) -> list[str]:
        path = self.run_dir / "systemctl.log"
        return path.read_text().splitlines() if path.exists() else []

    def env(self, **extra: str) -> dict[str, str]:
        env = {
            "PATH": f"{self.bin}:{os.environ.get('PATH', '/usr/bin:/bin')}",
            "HOME": str(self.tmp),
            "STAGING_VM_HOST": "vm.example",
            "STAGING_VM_SSH_KEY": self.key,
            "YULIUS_CD_CACHE": str(self.cache),
            "YULIUS_CD_ENV": "dev",
        }
        env.update(extra)
        return env

    def cd(self, script: str, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["sh", str(self.repo / "scripts" / "cd" / script), *args],
            env=self.env(**extra),
            capture_output=True,
            text=True,
            timeout=120,
        )

    def ok(self, script: str, *args: str, **extra: str) -> str:
        out = self.cd(script, *args, **extra)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        return out.stdout + out.stderr

    def fails(self, script: str, *args: str, **extra: str) -> str:
        out = self.cd(script, *args, **extra)
        self.assertNotEqual(out.returncode, 0, out.stdout + out.stderr)
        return out.stdout + out.stderr

    def deploy(self, sha: str, **extra: str) -> str:
        return self.ok("deploy.sh", "dev", YULIUS_CD_SHA=sha, YULIUS_CD_KIND="deploy", **extra)

    def deploy_fails(self, sha: str) -> str:
        return self.fails("deploy.sh", "dev", YULIUS_CD_SHA=sha, YULIUS_CD_KIND="deploy")

    def verify(self, sha: str, kind: str = "deploy", **extra: str) -> subprocess.CompletedProcess[str]:
        if kind == "deploy":
            return self.cd("verify.sh", "dev", YULIUS_CD_SHA=sha, YULIUS_CD_KIND=kind, **extra)
        return self.cd("verify.sh", "dev", YULIUS_CD_RELEASE=sha, YULIUS_CD_KIND=kind, **extra)

    def verified(self, sha: str, kind: str = "deploy") -> str:
        out = self.verify(sha, kind)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        return out.stdout + out.stderr

    def rollback(self, sha: str) -> str:
        return self.ok("rollback.sh", "dev", YULIUS_CD_RELEASE=sha, YULIUS_CD_KIND="rollback")

    def remote(self, *args: str, pipe: str = "", **extra: str) -> subprocess.CompletedProcess[str]:
        """remote.sh run here on the fake root, as SSH runs it there (optionally piped)."""
        script = self.repo / "scripts" / "cd" / "remote.sh"
        command = f"sh {script} {' '.join(args)}{pipe}"
        return subprocess.run(
            ["sh", "-c", command],
            env={**self.env(), "YULIUS_CD_ROOT": str(self.root), "YULIUS_CD_START_WAIT": "3", **extra},
            capture_output=True,
            text=True,
            timeout=120,
        )

    # --- refusals -----------------------------------------------------------------------

    def test_only_the_dev_environment_is_known(self) -> None:
        for script in ("deploy.sh", "verify.sh", "rollback.sh"):
            with self.subTest(script):
                out = self.cd(script, "prod", YULIUS_CD_SHA=NEW, YULIUS_CD_RELEASE=OLD)
                self.assertEqual(out.returncode, 2, out.stderr)
                self.assertIn("usage", out.stderr)

    def test_missing_secrets_refuse_with_what_to_do(self) -> None:
        out = self.fails("deploy.sh", "dev", YULIUS_CD_SHA=NEW, STAGING_VM_SSH_KEY="", STAGING_VM_HOST="")
        self.assertIn("STAGING_VM_HOST STAGING_VM_SSH_KEY", out)
        self.assertIn("Secrets page", out)

    def test_a_host_that_is_not_user_host_port_is_refused(self) -> None:
        for host in ("-oProxyCommand=x", "root@", "vm.example:22x", "a b", "-x@vm.example"):
            with self.subTest(host):
                out = self.fails("verify.sh", "dev", STAGING_VM_HOST=host)
                self.assertIn("STAGING_VM_HOST", out)
        self.assertFalse((self.tmp / "ssh.log").exists())

    def test_without_the_pin_nothing_connects(self) -> None:
        (self.repo / "scripts" / "cd" / "known_hosts").write_text("# none\n")
        out = self.fails("verify.sh", "dev")
        self.assertIn("no host key for yulius-staging", out)
        self.assertFalse((self.tmp / "ssh.log").exists())

    def test_rollback_needs_a_release_and_its_dry_run_changes_nothing(self) -> None:
        out = self.fails("rollback.sh", "dev")
        self.assertIn("YULIUS_CD_RELEASE", out)
        self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE="not-a-commit")
        out = self.ok("rollback.sh", "dev", YULIUS_CD_RELEASE=OLD, YULIUS_CD_DRY_RUN="1")
        self.assertIn("dry run", out)
        self.assertFalse((self.tmp / "ssh.log").exists())

    def test_deploy_needs_a_commit(self) -> None:
        out = self.fails("deploy.sh", "dev", YULIUS_CD_SHA="")
        self.assertIn("YULIUS_CD_SHA", out)

    def test_a_user_a_port_and_a_one_line_key_work(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        one_line = self.key.strip().replace("\n", "\\n")
        out = self.verify(NEW, STAGING_VM_HOST="deploy@vm.example:2222", STAGING_VM_SSH_KEY=one_line)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        last = (self.tmp / "ssh.log").read_text().splitlines()[-1]
        self.assertIn("-p 2222 deploy@vm.example sudo -n sh -s -- verify", last)

    # --- the build ----------------------------------------------------------------------

    def test_the_build_is_the_commit_with_the_pinned_toolchain(self) -> None:
        out = self.ok("build.sh", NEWER, str(self.tmp / "out"))
        self.assertIn(f"built yulius 0.1.0 ({NEWER})", out)
        log = (self.run_dir / "docker.log").read_text()
        self.assertIn("RUST_TOOLCHAIN=1.", log)
        self.assertIn(f"YULIUS_BUILD_COMMIT={NEWER}", log)
        self.assertIn("deploy/build/Dockerfile", log)

    def test_a_build_that_names_another_commit_is_refused(self) -> None:
        (self.run_dir / "build-lies").touch()
        out = self.fails("build.sh", NEW, str(self.tmp / "out"))
        self.assertIn(f"not commit {NEW}", out)
        self.fails("build.sh", NEW[:12], str(self.tmp / "out"))

    def test_deploy_builds_a_release_it_has_not_built(self) -> None:
        (self.run_dir / f"built-{NEWER}").write_text(self.fake_yulius(NEWER, 5))
        out = self.deploy(NEWER)
        self.assertIn("building release", out)
        self.assertEqual(self.running(), NEWER)
        self.assertTrue((self.cache / "releases" / NEWER / "yulius").is_file())
        out = self.deploy(NEWER)
        self.assertIn("built already", out)

    # --- deploy -------------------------------------------------------------------------

    def test_deploy_installs_the_release_and_keeps_the_previous_one_with_its_store(self) -> None:
        self.release(NEW, schema=5)
        out = self.deploy(NEW)
        self.assertIn(f"::yulius-release {NEW}", out)
        self.assertIn("migrates a copy of the live store from schema 4 to 5", out)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)
        self.assertEqual((self.releases / "current").read_text().strip(), NEW)
        # The replaced release's binary and the store it left, before the migration.
        self.assertTrue((self.releases / OLD / "yulius").is_file())
        snap = sqlite3.connect(self.releases / OLD / "store" / "yulius.db")
        self.assertEqual(snap.execute("PRAGMA user_version").fetchone()[0], 4)
        snap.close()
        # Only yulius.service was stopped and started.
        self.assertEqual(self.systemctl_log(), ["start yulius.service", "stop yulius.service", "start yulius.service"])
        self.verified(NEW)

    def test_a_second_deploy_of_the_running_release_changes_nothing(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        before = self.systemctl_log()
        out = self.deploy(NEW)
        self.assertIn("installed already", out)
        self.assertEqual(self.systemctl_log(), before)
        self.verified(NEW)

    def test_a_migration_that_loses_rows_or_a_table_changes_nothing(self) -> None:
        for mode, word in (("lossy", "tasks: 3 rows before, 2 now"), ("drops", "notes: gone")):
            with self.subTest(mode):
                self.setUp()
                before = dump(self.data / "yulius.db")
                self.release(NEW, schema=5, mode=mode)
                out = self.deploy_fails(NEW)
                self.assertIn(word, out)
                self.assertIn("nothing changed", out)
                self.assertEqual(self.running(), OLD)
                self.assertEqual(self.systemctl_log(), ["start yulius.service"], "nothing stopped")
                self.assertEqual(dump(self.data / "yulius.db"), before)

    def test_a_migration_may_delete_rows_from_a_table_it_names(self) -> None:
        (self.migrations / "0005_m.sql").write_text("DELETE FROM tasks WHERE title = 'a';\n")
        self.release(NEW, schema=5, mode="lossy")
        self.deploy(NEW)
        self.assertEqual(self.running(), NEW)
        self.verified(NEW)

    def test_a_deletion_counts_only_in_a_migration_the_deploy_applies_and_not_in_a_trigger(self) -> None:
        for name, sql in (
            ("0003_m.sql", "DELETE FROM tasks;\n"),  # schema 3: the store is at 4 already
            ("0005_m.sql", "CREATE TRIGGER t AFTER INSERT ON notes BEGIN DELETE FROM tasks; END;\n"),
        ):
            with self.subTest(name):
                self.setUp()
                (self.migrations / name).write_text(sql)
                self.release(NEW, schema=5, mode="lossy")
                out = self.deploy_fails(NEW)
                self.assertIn("tasks: 3 rows before, 2 now", out)
                self.assertEqual(self.running(), OLD)

    def test_the_vm_refuses_a_deletion_list_that_is_not_schema_and_table(self) -> None:
        self.releases.mkdir(parents=True)
        (self.releases / NEW).mkdir()
        shutil.copy2(self.release(NEW, schema=5), self.releases / NEW / "yulius")
        for bad in ("tasks", "5:Tasks", "5:ta-sks", "x:tasks", "69:a,b", "69:a,7:b:c", "69:a,"):
            with self.subTest(bad):
                out = self.remote("activate", NEW, bad)
                self.assertNotEqual(out.returncode, 0)
                self.assertIn("not a list of <schema>:<table>", out.stderr)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.systemctl_log(), ["start yulius.service"])
        # The real shape, several pairs, is accepted.
        out = self.remote("activate", NEW, "69:session_chunks,72:session_chunks")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.running(), NEW)

    def test_rows_deleted_after_the_start_do_not_fail_verify(self) -> None:
        """Retention at the server's start or a person's delete is no migration loss."""
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.write("DELETE FROM tasks WHERE title = 'a'")
        self.verified(NEW)
        self.deploy(NEW)
        self.write("DELETE FROM tasks WHERE title = 'b'")
        self.verified(NEW)

    def test_a_release_that_never_reports_its_commit_is_undone_with_the_store_it_replaced(self) -> None:
        before = dump(self.data / "yulius.db")
        self.release(NEW, schema=5, mode="halfway")
        out = self.deploy_fails(NEW)
        self.assertIn("put back", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(dump(self.data / "yulius.db"), before)
        self.assertEqual(self.live("PRAGMA user_version"), 4)
        self.assertEqual(self.live("SELECT count(*) FROM sqlite_master WHERE name = 'added_5'"), 0)
        self.assertEqual(self.live("SELECT count(*) FROM tasks WHERE title = 'half'"), 0)
        self.assertFalse((self.releases / "current").exists())

    def test_deploy_refuses_while_the_update_helper_runs(self) -> None:
        self.release(NEW, schema=5)
        (self.run_dir / "update-active").touch()
        out = self.deploy_fails(NEW)
        self.assertIn("update helper (yulius-update.service) is activating", out)
        self.assertEqual(self.running(), OLD)

    def test_an_upload_with_another_digest_or_commit_is_refused(self) -> None:
        self.releases.mkdir(parents=True)
        incoming = self.releases / f".incoming-{NEW}"
        incoming.write_text(self.fake_yulius(NEW, 5))
        out = self.remote("receive", NEW, "0" * 64)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("does not have SHA-256", out.stderr)
        self.assertFalse(incoming.exists())
        incoming.write_text(self.fake_yulius(NEWER, 5))
        out = self.remote("receive", NEW, digest(incoming))
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("names another commit", out.stderr)
        self.assertFalse((self.releases / NEW).exists())

    def test_the_last_ten_releases_are_kept_also_after_a_refused_upload(self) -> None:
        refused = "e" * 40
        self.release(refused, schema=5, mode="lossy")
        self.deploy_fails(refused)
        self.assertTrue((self.releases / refused / "yulius").is_file())
        shas = [f"{n:x}" * 40 for n in range(1, 13)]
        for sha in shas:
            self.release(sha, schema=5)
            self.deploy(sha)
        kept = sorted(p.name for p in self.releases.iterdir() if p.is_dir() and not p.name.startswith("."))
        self.assertEqual(len(kept), 10, kept)
        self.assertIn(shas[-1], kept)
        self.assertNotIn(OLD, kept)
        self.assertNotIn(refused, kept)

    # --- interruptions ------------------------------------------------------------------

    def test_a_lost_connection_does_not_stop_a_switch_half_way(self) -> None:
        self.releases.mkdir(parents=True)
        (self.releases / NEW).mkdir()
        shutil.copy2(self.release(NEW, schema=5), self.releases / NEW / "yulius")
        out = self.remote("activate", NEW, "-", pipe=" | head -n 1")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.running(), NEW)
        self.assertEqual((self.releases / "current").read_text().strip(), NEW)

    def test_a_failed_snapshot_starts_the_server_again(self) -> None:
        self.release(NEW, schema=5)
        (self.run_dir / "fail-snapshot").touch()
        out = self.deploy_fails(NEW)
        self.assertIn("interrupted before anything changed", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.systemctl_log()[-2:], ["stop yulius.service", "start yulius.service"])

    def test_one_cd_step_at_a_time(self) -> None:
        self.deployed()
        holder = subprocess.Popen(
            ["flock", str(self.releases / ".lock"), "sleep", "30"], start_new_session=True
        )

        def stop_holder() -> None:
            # flock's child holds the lock too: end the whole group (once).
            if holder.poll() is None:
                try:
                    os.killpg(holder.pid, 9)
                except ProcessLookupError:
                    pass
                holder.wait()

        self.addCleanup(stop_holder)
        deadline = __import__("time").monotonic() + 10
        while subprocess.run(["flock", "-n", str(self.releases / ".lock"), "true"]).returncode == 0:
            self.assertLess(__import__("time").monotonic(), deadline, "the holder never took the lock")
        out = self.remote("verify", NEW, "5", "0", YULIUS_CD_LOCK_WAIT="1")
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("another CD step holds", out.stderr)
        stop_holder()
        self.assertEqual(self.remote("verify", NEW, "5", "0").returncode, 0)

    def test_a_rollback_whose_store_restore_fails_keeps_the_release_and_store_it_had(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.write("INSERT INTO tasks VALUES ('written by new')")
        before = dump(self.data / "yulius.db")
        (self.run_dir / "fail-cp-from").write_text(f"/{OLD}/store/")
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=OLD, YULIUS_CD_KIND="rollback")
        self.assertIn(f"putting back release {NEW} and its store", out)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(dump(self.data / "yulius.db"), before)

    def test_a_put_back_that_cannot_restore_the_store_leaves_the_server_stopped(self) -> None:
        """No start on a store that is not the binary's (or none): the snapshot stays intact."""
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.write("INSERT INTO tasks VALUES ('written by new')")
        (self.run_dir / "fail-cp-from").write_text("/store/")
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=OLD, YULIUS_CD_KIND="rollback")
        self.assertIn("stays stopped", out)
        self.assertIn(f"restore by hand from {self.releases}/{NEW}", out)
        self.assertFalse((self.run_dir / "active").exists())
        snapshot = self.releases / NEW / "store" / "yulius.db"
        (self.run_dir / "fail-cp-from").unlink()
        self.assertEqual(
            sqlite3.connect(snapshot).execute("SELECT count(*) FROM tasks WHERE title = 'written by new'").fetchone()[0],
            1,
        )
        self.assert_held(out, NEW, snapshot)

    def test_an_undo_that_cannot_restore_the_store_leaves_the_server_stopped(self) -> None:
        self.release(NEW, schema=5, mode="halfway")
        (self.run_dir / "fail-cp-from").write_text("/store/")
        out = self.deploy_fails(NEW)
        self.assertIn("stays stopped", out)
        self.assertFalse((self.run_dir / "active").exists())
        snapshot = self.releases / OLD / "store" / "yulius.db"
        self.assertEqual(sqlite3.connect(snapshot).execute("PRAGMA user_version").fetchone()[0], 4)
        (self.run_dir / "fail-cp-from").unlink()
        self.assert_held(out, OLD, snapshot)

    def assert_held(self, out: str, prev: str, snapshot: Path) -> None:
        """Held after a failed put back: Yulius is told not to restore staging by itself, and the
        rollback it would start, a redeploy and a verify all refuse until a person removed the
        marker, never starting the server on another or an empty store."""
        self.assertIn("\n::yulius-rollback-barrier staging is held", "\n" + out)
        held = self.releases / "HELD"
        self.assertTrue(held.is_file())
        before = digest(snapshot)
        for script, extra in (
            ("rollback.sh", {"YULIUS_CD_RELEASE": prev, "YULIUS_CD_KIND": "rollback"}),
            ("deploy.sh", {"YULIUS_CD_SHA": prev, "YULIUS_CD_KIND": "deploy"}),
            ("verify.sh", {"YULIUS_CD_RELEASE": prev, "YULIUS_CD_KIND": "rollback"}),
        ):
            with self.subTest(script):
                refused = self.fails(script, "dev", **extra)
                self.assertIn("staging is held", refused)
                self.assertIn(f"remove {held}", refused)
        self.assertFalse((self.run_dir / "active").exists())
        self.assertEqual(digest(snapshot), before)

    def test_a_killed_switch_and_a_redeploy_keep_the_release_to_go_back_to(self) -> None:
        self.release(NEW, schema=5)
        (self.run_dir / "kill-on-start").touch()
        self.deploy_fails(NEW)  # killed after the install: no trap runs
        self.assertFalse((self.run_dir / "active").exists())
        out = self.deploy(NEW)
        self.assertIn("installed already", out)
        self.assertEqual(self.running(), NEW)
        self.assertTrue((self.releases / OLD / "yulius").is_file())
        self.assertTrue((self.releases / OLD / "store" / "yulius.db").is_file())
        self.rollback(OLD)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("PRAGMA user_version"), 4)

    # --- verify -------------------------------------------------------------------------

    def deployed(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)

    def test_verify_runs_the_docker_smoke_on_a_deploy_only(self) -> None:
        self.deployed()
        self.verified(NEW)
        smoke = (self.run_dir / "smoke.log").read_text().splitlines()
        self.assertEqual(len(smoke), 1)
        self.assertIn(f"--data-dir {self.data}", smoke[0])
        self.verified(NEW, kind="drill")
        self.assertEqual(len((self.run_dir / "smoke.log").read_text().splitlines()), 1)

    def test_verify_skips_the_smoke_where_sandboxes_get_no_docker(self) -> None:
        self.deployed()
        (self.run_dir / "smoke-rc").write_text("1")
        for path in (self.root / "usr" / "local" / "bin" / "yulius", self.releases / NEW / "yulius"):
            path.write_text(path.read_text().replace('echo "ok    sandbox Docker', 'echo "warn  sandbox Docker'))
        out = self.verified(NEW)
        self.assertIn("does not apply", out)

    def test_verify_fails_on_each_broken_promise(self) -> None:
        self.deployed()
        migration = self.migrations / "0006_m.sql"
        installed = self.root / "usr" / "local" / "bin" / "yulius"
        kept = installed.read_text()

        def then(change, sha=NEW):
            def case():
                change()
                return self.verify(sha)

            return case

        cases = {
            f"is current, not {NEWER}": then(lambda: None, NEWER),
            "is not release": then(lambda: installed.write_text(kept + "# changed\n")),
            "the contained doctor": then(lambda: (self.run_dir / "doctor-rc").write_text("1")),
            "answers 000, not 200, over HTTPS": then(lambda: (self.run_dir / "https-down").touch()),
            "smoke test failed": then(lambda: (self.run_dir / "smoke-rc").write_text("1")),
            "schema 5, not 6": then(lambda: migration.write_text("")),
            "no migration check of release": then(
                lambda: (self.releases / NEW / "migration.json").rename(self.tmp / "m.json")
            ),
            f"reports {NEWER}, not release {NEW}": then(
                lambda: (self.run_dir / "version").write_text(json.dumps({"commit": NEWER}))
            ),
            "yulius.service is not active": then(lambda: (self.run_dir / "active").unlink()),
        }
        for why, case in cases.items():
            with self.subTest(why):
                out = case()
                self.assertNotEqual(out.returncode, 0, out.stdout + out.stderr)
                self.assertIn(why, out.stderr)
                for f in ("doctor-rc", "https-down", "smoke-rc"):
                    (self.run_dir / f).unlink(missing_ok=True)
                migration.unlink(missing_ok=True)
                installed.write_text(kept)
                if (self.tmp / "m.json").exists():
                    (self.tmp / "m.json").rename(self.releases / NEW / "migration.json")
                self.start()

    # --- rollback and the drill ---------------------------------------------------------

    def test_a_rollback_across_a_migration_restores_the_releases_store(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.write("INSERT INTO tasks VALUES ('written by new')")
        out = self.rollback(OLD)
        self.assertIn("is restored", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("PRAGMA user_version"), 4)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 3)
        self.verified(OLD, kind="rollback")
        # The drill's step forward: the store the newer release left comes back.
        self.rollback(NEW)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 4)
        self.verified(NEW, kind="drill")

    def test_verify_after_a_rollback_wants_the_schema_the_release_left(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.rollback(OLD)
        self.write("PRAGMA user_version = 6")
        out = self.verify(OLD, kind="rollback")
        self.assertNotEqual(out.returncode, 0)
        self.assertIn(f"the store has schema 6, not the 4 release {OLD} left", out.stderr)

    def test_a_rollback_without_a_schema_change_keeps_the_live_store(self) -> None:
        self.release(NEW, schema=4)
        self.deploy(NEW)
        self.write("INSERT INTO tasks VALUES ('kept')")
        out = self.rollback(OLD)
        self.assertIn("the live store is kept", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 4)
        self.verified(OLD, kind="rollback")

    def test_a_rollback_uploads_from_the_cd_cache_when_the_vm_dropped_it(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.release(NEWER, schema=5)
        self.deploy(NEWER)
        shutil.rmtree(self.releases / NEW)
        out = self.rollback(NEW)
        self.assertIn("copying release", out)
        self.assertEqual(self.running(), NEW)

    def test_a_rollback_to_a_release_kept_nowhere_fails_without_building(self) -> None:
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=NEWER, YULIUS_CD_KIND="rollback")
        self.assertIn("never builds", out)
        self.assertEqual(self.running(), OLD)

    def test_a_rollback_that_does_not_start_puts_back_what_ran_with_its_rows(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.write("INSERT INTO tasks VALUES ('written by new')")
        (self.releases / OLD / "yulius").write_text(self.fake_yulius(OLD, 4, mode="halfway"))
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=OLD, YULIUS_CD_KIND="rollback")
        self.assertIn("put back", out)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)
        self.assertEqual(self.live("SELECT count(*) FROM tasks WHERE title = 'written by new'"), 1)
        self.assertEqual(self.live("SELECT count(*) FROM tasks WHERE title = 'half'"), 0)


if __name__ == "__main__":
    unittest.main()
