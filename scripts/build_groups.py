"""Identify process groups that predate a build on hosts without subreapers."""
import ctypes
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


if sys.platform == "darwin":
    _proc_pidinfo = ctypes.CDLL("/usr/lib/libproc.dylib").proc_pidinfo
    _proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                             ctypes.c_void_p, ctypes.c_int]


def darwin_process_id(pid, group):
    # XNU's PROC_PIDUNIQIDENTIFIERINFO returns a 16-byte executable UUID
    # followed by the kernel's 64-bit process identifier, stable across reparenting.
    info = ctypes.create_string_buffer(56)
    size = _proc_pidinfo(pid, 17, ctypes.c_uint64(0), info, len(info))
    if size == len(info) and os.getpgid(pid) == group:
        return struct.unpack_from("=Q", info, 16)[0]
    return None


def linux_process_id(pid, group):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    if int(fields[2]) == group:
        return f"{pid}/{fields[19]}"
    return None


def group_snapshot():
    identity = darwin_process_id if sys.platform == "darwin" else linux_process_id
    # Capture PGID in ps's process table snapshot. A PID that exits before
    # its identity is read still contributes a group with unknown membership.
    args = ["/bin/ps", "-A", "-o", "pid=,pgid=,stat="]
    with tempfile.TemporaryFile() as listing:
        subprocess.run(args, stdout=listing, check=True, timeout=1)
        if listing.tell() > 16 * 1024 * 1024:
            raise RuntimeError("process group snapshot exceeded 16 MiB")
        listing.seek(0)
        groups = {}
        for row in listing:
            pid, group, state = row.split()
            if state.startswith(b"Z"):
                continue
            group = int(group)
            if group == 0:  # Kernel tasks cannot belong to a user build.
                continue
            members = groups.setdefault(group, [])
            try:
                ident = identity(int(pid), group)
            except (ProcessLookupError, FileNotFoundError, PermissionError):
                continue
            if ident is not None:
                members.append(str(ident))
    if not groups:
        raise RuntimeError("process group snapshot is empty")
    return groups


if __name__ == "__main__":
    print(json.dumps(group_snapshot()))
