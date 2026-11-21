"""The tables each store migration deletes rows from by design (ADR-0430).

Usage: python3 scripts/cd/migration_deletes.py [<repo>]
Prints `<schema>:<table>` pairs, comma separated (`-` for none): the schema version a migration
brings the store to (its position in crates/yulius-store's MIGRATIONS, not its file number) and
each table its own statements `DELETE FROM`. A trigger's body runs later, not in the migration,
so it does not count. remote.sh's migration check lets only those tables lose rows, and only
when the deploy applies that migration.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


def entries(lib: str) -> list[str]:
    """The migration file of each MIGRATIONS entry, in order."""
    consts = dict(re.findall(r'^const (\w+): &str = include_str!\("([^"]+)"\);', lib, re.M))
    body = lib.split("const MIGRATIONS: &[&str] = &[", 1)[1].split("\n];", 1)[0]
    files = []
    for line in body.splitlines():
        line = line.strip()
        if line.startswith("//") or not line:
            continue
        m = re.match(r'include_str!\("([^"]+)"\),?$', line)
        if m:
            files.append(m.group(1))
        elif line.rstrip(",") in consts:
            files.append(consts[line.rstrip(",")])
        else:
            raise SystemExit(f"migration_deletes: cannot read the MIGRATIONS entry {line!r}")
    return files


def deletes(sql: str) -> list[str]:
    """The tables the statements of one migration delete from (comments and triggers aside)."""
    sql = re.sub(r"--[^\n]*", "", sql)
    sql = re.sub(r"\bCREATE\s+TRIGGER\b.*?\bEND\s*;", "", sql, flags=re.S | re.I)
    return sorted(set(re.findall(r"\bDELETE\s+FROM\s+([A-Za-z_][A-Za-z0-9_]*)", sql, re.I)))


def main() -> None:
    repo = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parents[2])
    src = repo / "crates" / "yulius-store" / "src"
    lib = src / "lib.rs"
    if not lib.exists():
        print("-")
        return
    pairs = [
        f"{n}:{table.lower()}"
        for n, name in enumerate(entries(lib.read_text()), start=1)
        for table in deletes((src / name).read_text())
    ]
    print(",".join(pairs) or "-")


if __name__ == "__main__":
    main()
