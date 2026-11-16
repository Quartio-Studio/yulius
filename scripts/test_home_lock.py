import multiprocessing
import tempfile
import threading
import time
import unittest
from pathlib import Path

from home_lock import exclusive


def hold(path, ready, release):
    with exclusive(path):
        ready.set()
        release.wait(30)


class HomeLockTests(unittest.TestCase):
    def test_second_holder_waits_until_the_first_leaves(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "cache" / ".rustup.lock"
            ready, release = multiprocessing.Event(), multiprocessing.Event()
            holder = multiprocessing.Process(target=hold, args=(path, ready, release))
            holder.start()
            try:
                self.assertTrue(ready.wait(10))
                threading.Timer(1, release.set).start()
                started = time.monotonic()
                with exclusive(path):
                    waited = time.monotonic() - started
                self.assertGreaterEqual(waited, 0.8)
            finally:
                release.set()
                holder.join(10)

    def test_lock_is_released_when_the_body_fails(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / ".l"
            with self.assertRaises(RuntimeError):
                with exclusive(path):
                    raise RuntimeError("install failed")
            with exclusive(path):
                pass


if __name__ == "__main__":
    unittest.main()
