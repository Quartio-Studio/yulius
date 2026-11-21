"""Build once for the two test matrix jobs sharing this CI run's lane."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from home_lock import exclusive


def record(path, state):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(state))
    temporary.replace(path)


def run_shard(home, cache, run_id, sha, index, build, run):
    # CI gives each job a private cache named for its checkout. Concurrent runs have
    # different lanes, and the protected definition and its changed twin have different keys.
    match = re.fullmatch(r"(test(?:\.change)?)(?:\.[12])?(@[0-9]+)?", cache.name)
    if not match:
        raise ValueError(f"unexpected test job cache {cache.name}")
    slot = home / ".cache/yulius-test-builds" / (match[1] + (match[2] or ""))
    lock = slot / "lock"
    metadata = slot / "state.json"
    shared = slot / "tests.tar.zst"
    private = cache / "tests.tar.zst"
    pending = cache / "building.tar.zst"
    cache.mkdir(parents=True, exist_ok=True)
    identity = [run_id, sha]
    print("::yulius-stage test-build handoff", flush=True)
    with exclusive(lock):
        state = json.loads(metadata.read_text()) if metadata.exists() else {}
        if state.get("identity") != identity:
            shared.unlink(missing_ok=True)
            state = {"identity": identity, "finished": []}
        if state.get("failure") and state["builder"] != index:
            state["cover_both"] = True
            record(metadata, state)
            print("Test partition skipped because the sibling's test build failed.", flush=True)
            return int(os.environ.get("YULIUS_CI_SKIP_CODE", "78"))
        cover_both = state.get("cover_both", False)
        if not shared.is_file():
            print("::yulius-stage test-build", flush=True)
            state = {"identity": identity, "builder": index, "finished": [], "cover_both": cover_both}
            record(metadata, state)
            pending.unlink(missing_ok=True)
            try:
                result = build(pending)
                if result:
                    state["failure"] = result
                    record(metadata, state)
                    return result
                pending.replace(shared)
            finally:
                pending.unlink(missing_ok=True)
        # The child keeps the exact archive it acquired, including across cancellation
        # and recovery. A hard link consumes no additional archive blocks.
        private.unlink(missing_ok=True)
        os.link(shared, private)

    try:
        return run(private, cover_both)
    finally:
        private.unlink(missing_ok=True)
        with exclusive(lock):
            state = json.loads(metadata.read_text())
            if state["identity"] == identity:
                state["finished"] = [1, 2] if cover_both else sorted(set(state["finished"]) | {index})
                record(metadata, state)
                if state["finished"] == [1, 2]:
                    shared.unlink(missing_ok=True)


def main():
    matrix = os.environ["YULIUS_CI_MATRIX"]
    if matrix not in ("1/2", "2/2"):
        raise ValueError(f"unexpected test partition {matrix}")

    def build(archive):
        return subprocess.run(["scripts/cargo", "nextest", "archive", "--locked",
                               "--workspace", "--all-features", "--profile", "ci",
                               "--zstd-level", "1", "--archive-file", archive]).returncode

    def run(archive, cover_both):
        env = dict(os.environ)
        if cover_both:
            print("Recovered test build: running the full suite including the skipped peer.", flush=True)
            env.pop("YULIUS_CI_MATRIX", None)
        return subprocess.run(["scripts/dev", "test", "--archive-file", archive], env=env).returncode

    return run_shard(Path.home(), Path(os.environ["YULIUS_CI_CACHE"]),
                     os.environ["YULIUS_CI_RUN"], os.environ["YULIUS_CI_SHA"],
                     int(matrix[0]), build, run)


if __name__ == "__main__":
    result = main()
    sys.exit(128 - result if result < 0 else result)
