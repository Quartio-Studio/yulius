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


def without_triggers(sql: str) -> str:
    """`sql` with each `CREATE TRIGGER … BEGIN … END` left out: its `END` is the one that closes
    no `CASE` of the body."""
    out, pos = [], 0
    for start in re.finditer(r"\bCREATE\s+(?:TEMP\s+|TEMPORARY\s+)?TRIGGER\b", sql, re.I):
        if start.start() < pos:
            continue
        out.append(sql[pos : start.start()])
        begin = re.compile(r"\bBEGIN\b", re.I).search(sql, start.end())
        if begin is None:
            return "".join(out)
        depth, end = 0, len(sql)
        for word in re.finditer(r"\b(CASE|END)\b", sql[begin.end() :], re.I):
            if word.group(1).upper() == "CASE":
                depth += 1
            elif depth:
                depth -= 1
            else:
                end = begin.end() + word.end()
                break
        pos = end
    out.append(sql[pos:])
    return "".join(out)


def code_only(sql: str) -> str:
    """`sql` with its comments (`--`, `/* */`) and the text of its quoted strings left out, in one
    pass, so a quote in a comment or a comment marker in a string is read as written."""
    out, i, n = [], 0, len(sql)
    while i < n:
        if sql.startswith("--", i):
            end = sql.find("\n", i)
            i = n if end < 0 else end
        elif sql.startswith("/*", i):
            end = sql.find("*/", i + 2)
            i = n if end < 0 else end + 2
            out.append(" ")
        elif sql[i] == "'":
            i += 1
            while i < n:
                if sql[i] == "'" and sql.startswith("''", i):
                    i += 2
                elif sql[i] == "'":
                    i += 1
                    break
                else:
                    i += 1
            out.append("''")
        else:
            out.append(sql[i])
            i += 1
    return "".join(out)


def deletes(sql: str) -> list[str]:
    """The tables the statements of one migration delete from (comments, strings and triggers
    aside), lowercase."""
    sql = without_triggers(code_only(sql))
    return sorted({t.lower() for t in re.findall(r"\bDELETE\s+FROM\s+([A-Za-z_][A-Za-z0-9_]*)", sql, re.I)})


def main() -> None:
    repo = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parents[2])
    src = repo / "crates" / "yulius-store" / "src"
    lib = src / "lib.rs"
    if not lib.exists():
        print("-")
        return
    pairs = [
        f"{n}:{table}"
        for n, name in enumerate(entries(lib.read_text()), start=1)
        for table in deletes((src / name).read_text())
    ]
    print(",".join(pairs) or "-")


if __name__ == "__main__":
    main()
