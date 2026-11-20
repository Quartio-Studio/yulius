"""Keep common Git insertion points ahead of new ADRs in this repository."""
import argparse
from pathlib import Path
import re
import subprocess

FIRST = 401  # The first number after the index layout changed.
BLOCK = 32
ENTRY = re.compile(r"^- \[(\d{4})\]\(")
MARKER = re.compile(r"<!-- ADR (\d{4}) -->")
ROOT = Path(__file__).resolve().parent.parent
INDEX = ROOT / "docs/decisions/README.md"


def reserve_end(number):
    """Keep the rest of this block and the whole next block, within the four-digit series."""
    block = (max(FIRST, number) - FIRST) // BLOCK
    return min(9999, FIRST - 1 + (block + 2) * BLOCK)


def marker(number):
    return f"<!-- ADR {number:04} -->"


def slot_errors(index):
    lines = index.splitlines()
    markers = [int(match[1]) for line in lines if (match := MARKER.fullmatch(line))]
    highest = FIRST - 1
    errors = []
    for i, line in enumerate(lines):
        match = ENTRY.match(line)
        if match and int(match[1]) >= FIRST:
            number = int(match[1])
            highest = max(highest, number)
            if i + 1 == len(lines) or lines[i + 1] != marker(number):
                errors.append(f"place entry {number:04} immediately before {marker(number)}")
    if markers or highest >= FIRST:
        if markers != list(range(FIRST, max(markers, default=FIRST - 1) + 1)):
            errors.append(f"keep each ADR slot in order, starting with {FIRST:04}")
        if max(markers, default=FIRST - 1) < reserve_end(highest):
            errors.append("replenish the ADR slots with python3 scripts/decision_slots.py")
    return errors


def shared_slot_errors(index, base):
    """0401 and 0402 predate this migration, and 0403 records the migration itself."""
    used = {int(m[1]) for line in index.splitlines() if (m := ENTRY.match(line)) and int(m[1]) > 403}
    shared = {int(m[1]) for line in base.splitlines() if (m := MARKER.fullmatch(line))}
    return [f"ADR {n:04} needs its empty slot on dev before a branch fills it"
            for n in sorted(used - shared)]


def integration_slot_errors(root, index):
    """Check the common base, even when origin/dev acquired the slot after this branch forked."""
    relative = "docs/decisions/README.md"
    bases = subprocess.run(
        ["git", "merge-base", "--all", "HEAD", "refs/remotes/origin/dev"],
        cwd=root, text=True, capture_output=True,
    )
    if bases.returncode:
        return ["cannot check shared ADR slots: fetch origin/dev and its common history with HEAD"]
    errors = []
    for base in bases.stdout.splitlines():
        result = subprocess.run(["git", "show", f"{base}:{relative}"], cwd=root, text=True, capture_output=True)
        if result.returncode:
            return [f"cannot read {relative} at integration merge base {base}"]
        errors.extend(shared_slot_errors(index, result.stdout))
    return errors


def extend(index, through=FIRST):
    """Append only empty markers, in identical blocks on branches that cross a boundary."""
    lines = index.splitlines()
    last = max((int(match[1]) for line in lines if (match := MARKER.fullmatch(line))), default=FIRST - 1)
    highest = max((int(match[1]) for line in lines if (match := ENTRY.match(line))), default=FIRST - 1)
    added = "".join(f"{marker(n)}\n" for n in range(last + 1, reserve_end(max(through, highest)) + 1))
    if added and index and not index.endswith("\n"):
        index += "\n"
    return index + added


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("through", type=int, nargs="?", default=FIRST, help="reserve ahead of this number")
    through = parser.parse_args().through
    if not FIRST <= through <= 9999:
        parser.error(f"number must be between {FIRST} and 9999")
    index = INDEX.read_text()
    updated = extend(index, through)
    errors = slot_errors(updated) + integration_slot_errors(ROOT, updated)
    if errors:
        raise SystemExit("\n".join(errors))
    if updated != index:
        INDEX.write_text(updated)
