"""An exclusive lock in the home, for installs that two runs sharing one home must not interleave
(Yulius CI's lanes share the sandbox's CI home)."""
from contextlib import contextmanager
import os


@contextmanager
def exclusive(path):
    """Holds an exclusive `flock` on `path` (its folder is created) until the block ends, also when it
    fails; a second holder waits. Without `fcntl` (Windows) it holds nothing."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as lock:
        if os.name != "nt":
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
        yield
