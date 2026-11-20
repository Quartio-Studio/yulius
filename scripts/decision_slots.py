"""Keep common Git insertion points ahead of new ADRs in this repository."""
import argparse
from pathlib import Path
import re

FIRST = 401  # The first number after the index layout changed.
BLOCK = 32
ENTRY = re.compile(r"^- \[(\d{4})\]\(")
MARKER = re.compile(r"<!-- ADR (\d{4}) -->")
INDEX = Path(__file__).resolve().parent.parent / "docs/decisions/README.md"


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
        if markers != list(range(FIRST, max(markers, default=FIRST-1) + 1)):
            errors.append(f"keep each ADR slot in order, starting with {FIRST:04}")
        if max(markers, default=FIRST-1) < reserve_end(highest):
            errors.append("replenish the ADR slots with python3 scripts/decision_slots.py")
    return errors


def extend(index, through=FIRST):
    """Append only empty markers, in identical blocks on branches that cross a boundary."""
    lines = index.splitlines()
    last = max((int(match[1]) for line in lines if (match := MARKER.fullmatch(line))), default=FIRST-1)
    highest = max((int(match[1]) for line in lines if (match := ENTRY.match(line))), default=FIRST-1)
    return index + "".join(f"{marker(n)}\n" for n in range(last + 1, reserve_end(max(through, highest)) + 1))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("through", type=int, nargs="?", default=FIRST, help="reserve ahead of this number")
    through = parser.parse_args().through
    if not FIRST <= through <= 9999:
        parser.error(f"number must be between {FIRST} and 9999")
    INDEX.write_text(extend(INDEX.read_text(), through))
