"""Tests of Yulius's own CD scripts, scripts/cd (SPEC-591, ADR-0430).

The scripts run unchanged. `ssh` is a stub that checks the options CD must pass (the pinned host
key, batch mode, the alias), that the key is held by the agent the script started, and then runs
the remote command locally against a temporary root (`YULIUS_CD_ROOT`). On that root `systemctl`,
`curl`, `runuser`, `id` and `chown` are stubs, and `/usr/local/bin/yulius` is a fake whose start
migrates a real SQLite store as a release of the given schema would.
"""

from __future__ import annotations

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

OLD = "3fc0e5976ae2ce2da68a81c400866e2669316f66"
NEW = "86eb47202aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
NEWER = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
UNIT_ENV = (
    "YULIUS_DATA=/var/lib/yulius YULIUS_PUBLIC_URL=https://staging.example "
    "YULIUS_BIND=127.0.0.1:8080 YULIUS_INTERNAL_BIND=127.0.0.1:8081 YULIUS_CONTAINED=true "
    "PATH=/usr/local/bin:/usr/bin:/bin HOME=/var/lib/yulius"
)


def write_exe(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text).lstrip())
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def tool(name: str) -> bool:
    return shutil.which(name) is not None


@unittest.skipUnless(
    all(tool(t) for t in ("ssh-agent", "ssh-add", "ssh-keygen", "python3", "sha256sum")),
    "OpenSSH client tools are needed",
)
class CdScriptsTest(unittest.TestCase):
    """The scripts against a fake VM."""

    @classmethod
    def setUpClass(cls) -> None:
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
        migrations = self.repo / "crates" / "yulius-store" / "src" / "migrations"
        migrations.mkdir(parents=True)
        for n in range(1, 6):
            (migrations / f"{n:04}_m.sql").write_text("")
        self.root = self.tmp / "root"
        self.run_dir = self.root / "run"
        self.run_dir.mkdir(parents=True)
        (self.root / "var" / "lib" / "yulius").mkdir(parents=True)
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
            [ "$dest" = "root@vm.example" ] || {{ echo "stub ssh: destination $dest" >&2; exit 255; }}
            export YULIUS_CD_ROOT={self.root} YULIUS_CD_START_WAIT=3
            exec sh -c "$*"
            """,
        )
        write_exe(
            self.bin / "systemctl",
            f"""\
            #!/bin/sh
            run={self.run_dir}
            case "$1" in
              show)
                case "$*" in
                  *Environment*) echo "{UNIT_ENV}" ;;
                  *User*) echo yulius ;;
                esac ;;
              is-active)
                case "$2" in
                  yulius.service) [ -e $run/active ] && {{ echo active; exit 0; }}; echo inactive; exit 3 ;;
                  yulius-update.service) [ -e $run/update-active ] && {{ echo active; exit 0; }}; echo inactive; exit 3 ;;
                esac ;;
              stop) rm -f $run/active $run/version; echo stop >> $run/systemctl.log ;;
              start|restart)
                echo "$1 $2" >> $run/systemctl.log
                [ "$2" = yulius.service ] || {{ echo "stub systemctl: only yulius.service: $2" >&2; exit 1; }}
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
            run={self.run_dir}
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
        write_exe(self.bin / "id", "#!/bin/sh\necho 0\n")
        write_exe(self.bin / "chown", "#!/bin/sh\nexit 0\n")
        write_exe(
            self.bin / "docker",
            f"""\
            #!/bin/sh
            # Stub docker: the architecture, and a build that writes a fake release of the commit
            # it was given (or of another, when {self.run_dir}/build-lies exists).
            case "$1" in
              info) uname -m ;;
              build)
                echo "$*" >> {self.run_dir}/docker.log
                for arg; do
                  case "$arg" in
                    type=local,dest=*) dest="${{arg#type=local,dest=}}" ;;
                    YULIUS_BUILD_COMMIT=*) commit="${{arg#YULIUS_BUILD_COMMIT=}}" ;;
                  esac
                done
                [ -e {self.run_dir}/build-lies ] && commit={NEWER}
                mkdir -p "$dest"
                if [ -e {self.run_dir}/built-$commit ]; then
                  cp {self.run_dir}/built-$commit "$dest/yulius"
                else
                  printf '#!/bin/sh\\necho "yulius 0.1.0 (%s)"\\n' "$commit" > "$dest/yulius"
                fi
                chmod +x "$dest/yulius" ;;
              *) echo "stub docker: $*" >&2; exit 1 ;;
            esac
            """,
        )

    def fake_yulius(self, sha: str, schema: int, mode: str = "ok") -> str:
        """A fake binary of one release: its version, its start's migration, its doctor."""
        run = self.run_dir
        db = self.root / "var" / "lib" / "yulius" / "yulius.db"
        return textwrap.dedent(
            f"""\
            #!/bin/sh
            # fake yulius {sha}
            case "$1" in
              version) echo "yulius 0.1.0 ({sha})" ;;
              version-json) echo '{{"commit":"{sha}","version":"0.1.0"}}' ;;
              serve-fake)
                [ "{mode}" != broken ] || exit 1
                python3 -I - {db} {schema} {mode} <<'EOF'
            import sqlite3, sys
            db, schema, mode = sys.argv[1], int(sys.argv[2]), sys.argv[3]
            c = sqlite3.connect(db)
            c.execute("PRAGMA journal_mode=WAL")
            have = c.execute("PRAGMA user_version").fetchone()[0]
            if have > schema:
                sys.exit(1)
            for n in range(have + 1, schema + 1):
                c.execute(f"CREATE TABLE IF NOT EXISTS added_{{n}} (x)")
            if mode == "lossy":
                c.execute("DELETE FROM tasks WHERE rowid = (SELECT min(rowid) FROM tasks)")
            if mode == "drops":
                c.execute("DROP TABLE notes")
            c.execute(f"PRAGMA user_version = {{schema}}")
            c.commit()
            c.close()
            EOF
                ;;
              doctor)
                if [ "${{2:-}}" = --docker-smoke ]; then
                  echo smoke >> {run}/smoke.log
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
        c = sqlite3.connect(self.root / "var" / "lib" / "yulius" / "yulius.db")
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
        c = sqlite3.connect(self.root / "var" / "lib" / "yulius" / "yulius.db")
        try:
            return c.execute(sql).fetchone()[0]
        finally:
            c.close()

    def running(self) -> str:
        return json.loads((self.run_dir / "version").read_text())["commit"]

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

    def deploy(self, sha: str) -> str:
        return self.ok("deploy.sh", "dev", YULIUS_CD_SHA=sha, YULIUS_CD_KIND="deploy")

    def verify(self, sha: str, kind: str = "deploy", **extra: str) -> subprocess.CompletedProcess[str]:
        if kind == "deploy":
            return self.cd("verify.sh", "dev", YULIUS_CD_SHA=sha, YULIUS_CD_KIND=kind, **extra)
        return self.cd("verify.sh", "dev", YULIUS_CD_RELEASE=sha, YULIUS_CD_KIND=kind, **extra)

    def rollback(self, sha: str) -> str:
        return self.ok("rollback.sh", "dev", YULIUS_CD_RELEASE=sha, YULIUS_CD_KIND="rollback")

    # --- the scripts as such -----------------------------------------------------------

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
        for host in ("-oProxyCommand=x", "root@", "vm.example:22x", "a b"):
            with self.subTest(host):
                out = self.fails("verify.sh", "dev", STAGING_VM_HOST=host)
                self.assertIn("STAGING_VM_HOST", out)

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

    # --- deploy --------------------------------------------------------------------------

    def test_the_build_is_the_commit_with_the_pinned_toolchain(self) -> None:
        shutil.copy2(REPO / "rust-toolchain.toml", self.repo / "rust-toolchain.toml")
        out = self.ok("build.sh", NEWER, str(self.tmp / "out"))
        self.assertIn(f"built yulius 0.1.0 ({NEWER})", out)
        log = (self.run_dir / "docker.log").read_text()
        self.assertIn("RUST_TOOLCHAIN=1.", log)
        self.assertIn(f"YULIUS_BUILD_COMMIT={NEWER}", log)
        self.assertIn("deploy/build/Dockerfile", log)

    def test_a_build_that_names_another_commit_is_refused(self) -> None:
        shutil.copy2(REPO / "rust-toolchain.toml", self.repo / "rust-toolchain.toml")
        (self.run_dir / "build-lies").touch()
        out = self.fails("build.sh", NEW, str(self.tmp / "out"))
        self.assertIn(f"not commit {NEW}", out)
        self.fails("build.sh", NEW[:12], str(self.tmp / "out"))

    def test_deploy_builds_a_release_it_has_not_built(self) -> None:
        shutil.copy2(REPO / "rust-toolchain.toml", self.repo / "rust-toolchain.toml")
        (self.run_dir / f"built-{NEWER}").write_text(self.fake_yulius(NEWER, 5))
        out = self.ok("deploy.sh", "dev", YULIUS_CD_SHA=NEWER, YULIUS_CD_KIND="deploy")
        self.assertIn("building release", out)
        self.assertEqual(self.running(), NEWER)
        self.assertTrue((self.cache / "releases" / NEWER / "yulius").is_file())
        out = self.ok("deploy.sh", "dev", YULIUS_CD_SHA=NEWER, YULIUS_CD_KIND="deploy")
        self.assertIn("built already", out)

    def test_deploy_installs_the_release_and_keeps_the_previous_one_with_its_store(self) -> None:
        self.release(NEW, schema=5)
        out = self.deploy(NEW)
        self.assertIn(f"::yulius-release {NEW}", out)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)
        releases = self.root / "var" / "lib" / "yulius-releases"
        self.assertEqual((releases / "current").read_text().strip(), NEW)
        # The replaced release's binary and the store it left, before the migration.
        self.assertTrue((releases / OLD / "yulius").is_file())
        snap = sqlite3.connect(releases / OLD / "store" / "yulius.db")
        self.assertEqual(snap.execute("PRAGMA user_version").fetchone()[0], 4)
        snap.close()
        # Only yulius.service was stopped and started.
        log = (self.run_dir / "systemctl.log").read_text().split()
        self.assertEqual([w for w in log if w.endswith(".service")], ["yulius.service"] * 2)
        self.assertEqual(self.verify(NEW).returncode, 0)

    def test_a_second_deploy_of_the_running_release_changes_nothing(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        before = (self.run_dir / "systemctl.log").read_text()
        out = self.deploy(NEW)
        self.assertIn("installed already", out)
        self.assertEqual((self.run_dir / "systemctl.log").read_text(), before)

    def test_a_release_that_never_reports_its_commit_is_undone(self) -> None:
        self.release(NEW, schema=5, mode="broken")
        db = self.root / "var" / "lib" / "yulius" / "yulius.db"
        out = self.fails("deploy.sh", "dev", YULIUS_CD_SHA=NEW, YULIUS_CD_KIND="deploy")
        self.assertIn("put back", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("PRAGMA user_version"), 4)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 3)
        self.assertTrue(db.exists())
        self.assertFalse((self.root / "var" / "lib" / "yulius-releases" / "current").exists())

    def test_deploy_waits_while_the_update_helper_runs(self) -> None:
        self.release(NEW, schema=5)
        (self.run_dir / "update-active").touch()
        out = self.fails("deploy.sh", "dev", YULIUS_CD_SHA=NEW, YULIUS_CD_KIND="deploy")
        self.assertIn("update helper", out)
        self.assertEqual(self.running(), OLD)

    def test_an_upload_with_another_digest_is_refused(self) -> None:
        releases = self.root / "var" / "lib" / "yulius-releases"
        releases.mkdir(parents=True)
        incoming = releases / f".incoming-{NEW}"
        incoming.write_text(self.fake_yulius(NEW, 5))
        out = subprocess.run(
            ["sh", str(self.repo / "scripts" / "cd" / "remote.sh"), "receive", NEW, "0" * 64],
            env={**self.env(), "YULIUS_CD_ROOT": str(self.root)},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("does not have SHA-256", out.stderr)
        self.assertFalse(incoming.exists())
        self.assertFalse((releases / NEW).exists())

    def test_the_last_ten_releases_are_kept(self) -> None:
        shas = [f"{n:x}" * 40 for n in range(1, 13)]
        for n, sha in enumerate(shas):
            self.release(sha[:40], schema=5)
            self.deploy(sha[:40])
        releases = self.root / "var" / "lib" / "yulius-releases"
        kept = sorted(p.name for p in releases.iterdir() if p.is_dir())
        self.assertEqual(len(kept), 10, kept)
        self.assertIn(shas[-1][:40], kept)
        self.assertNotIn(OLD, kept)

    # --- verify --------------------------------------------------------------------------

    def deployed(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)

    def test_verify_runs_the_docker_smoke_on_a_deploy_only(self) -> None:
        self.deployed()
        self.assertEqual(self.verify(NEW).returncode, 0)
        self.assertEqual((self.run_dir / "smoke.log").read_text().count("smoke"), 1)
        self.assertEqual(self.verify(NEW, kind="drill").returncode, 0)
        self.assertEqual((self.run_dir / "smoke.log").read_text().count("smoke"), 1)

    def test_verify_skips_the_smoke_where_sandboxes_get_no_docker(self) -> None:
        self.deployed()
        (self.run_dir / "smoke-rc").write_text("1")
        # The doctor's line no longer offers Docker: the smoke does not apply.
        doctor = self.root / "usr" / "local" / "bin" / "yulius"
        doctor.write_text(doctor.read_text().replace('echo "ok    sandbox Docker', 'echo "warn  sandbox Docker'))
        out = self.verify(NEW)
        self.assertEqual(out.returncode, 1, out.stdout + out.stderr)  # binary changed: not the release
        releases = self.root / "var" / "lib" / "yulius-releases"
        shutil.copy2(doctor, releases / NEW / "yulius")
        out = self.verify(NEW)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn("does not apply", out.stdout + out.stderr)

    def test_verify_fails_on_each_broken_promise(self) -> None:
        self.deployed()
        migration = self.repo / "crates" / "yulius-store" / "src" / "migrations" / "0006_m.sql"

        def then(change, sha=NEW):
            def case():
                change()
                return self.verify(sha)

            return case

        cases = {
            f"is current, not {NEWER}": then(lambda: None, NEWER),
            "the contained doctor": then(lambda: (self.run_dir / "doctor-rc").write_text("1")),
            "answers 000, not 200, over HTTPS": then(lambda: (self.run_dir / "https-down").touch()),
            "smoke test failed": then(lambda: (self.run_dir / "smoke-rc").write_text("1")),
            "schema 5, not 6": then(lambda: migration.write_text("")),
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
                self.start()

    def test_verify_fails_when_a_migration_lost_rows_or_a_table(self) -> None:
        for mode, word in (("lossy", "tasks: 3 rows before, 2 now"), ("drops", "notes: gone")):
            with self.subTest(mode):
                self.setUp()
                self.release(NEW, schema=5, mode=mode)
                self.deploy(NEW)
                out = self.verify(NEW)
                self.assertNotEqual(out.returncode, 0)
                self.assertIn(word, out.stdout + out.stderr)

    # --- rollback and the drill ---------------------------------------------------------

    def test_a_rollback_across_a_migration_restores_the_releases_store(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        sqlite3.connect(self.root / "var" / "lib" / "yulius" / "yulius.db").execute(
            "INSERT INTO tasks VALUES ('written by new')"
        ).connection.commit()
        out = self.rollback(OLD)
        self.assertIn("restored", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("PRAGMA user_version"), 4)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 3)
        self.assertEqual(self.verify(OLD, kind="rollback").returncode, 0)
        # The drill's step forward: the store the newer release left comes back.
        self.rollback(NEW)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 4)
        self.assertEqual(self.verify(NEW, kind="drill").returncode, 0)

    def test_a_rollback_without_a_schema_change_keeps_the_live_store(self) -> None:
        self.release(NEW, schema=4)
        self.deploy(NEW)
        sqlite3.connect(self.root / "var" / "lib" / "yulius" / "yulius.db").execute(
            "INSERT INTO tasks VALUES ('kept')"
        ).connection.commit()
        out = self.rollback(OLD)
        self.assertIn("kept", out)
        self.assertEqual(self.running(), OLD)
        self.assertEqual(self.live("SELECT count(*) FROM tasks"), 4)

    def test_a_rollback_uploads_from_the_cd_cache_when_the_vm_dropped_it(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        self.release(NEWER, schema=5)
        self.deploy(NEWER)
        shutil.rmtree(self.root / "var" / "lib" / "yulius-releases" / NEW)
        out = self.rollback(NEW)
        self.assertIn("copying release", out)
        self.assertEqual(self.running(), NEW)

    def test_a_rollback_to_a_release_kept_nowhere_fails_without_building(self) -> None:
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=NEWER, YULIUS_CD_KIND="rollback")
        self.assertIn("never builds", out)
        self.assertEqual(self.running(), OLD)

    def test_a_rollback_that_does_not_start_puts_back_what_ran(self) -> None:
        self.release(NEW, schema=5)
        self.deploy(NEW)
        releases = self.root / "var" / "lib" / "yulius-releases"
        broken = self.fake_yulius(OLD, 4, mode="broken")
        (releases / OLD / "yulius").write_text(broken)
        out = self.fails("rollback.sh", "dev", YULIUS_CD_RELEASE=OLD, YULIUS_CD_KIND="rollback")
        self.assertIn("put back", out)
        self.assertEqual(self.running(), NEW)
        self.assertEqual(self.live("PRAGMA user_version"), 5)


if __name__ == "__main__":
    unittest.main()
