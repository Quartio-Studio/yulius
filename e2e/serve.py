"""Static file server for the e2e suite: `python3 -I serve.py <port> <directory>` (SPEC-148).

`python3 -m http.server` as the suite ran it before kept up only on a quiet machine (#354):
- its listen queue holds 5 connections (`socketserver`'s default). The kernel drops a SYN beyond
  it, and the browser waits 1 s, then 3 s, before trying again. On a loaded machine the accept
  loop is not scheduled for a while, while every worker's page loads about 75 modules (Monaco
  about 120 more files), six connections at a time each, so first loads stalled past the tests'
  5 s waits;
- it speaks HTTP/1.0, which closes the connection after each answer, so every file costs a new
  connection, and another place in that queue.
This one queues 1024 connections and keeps them alive, as any production server does. It serves
the same files with the same answers otherwise.

Every answer carries the server's own `Content-Security-Policy` (the file `yulius serve` sends,
SPEC-483 W1b, ADR-0338), so the suite runs the UI under it; `instrument()` in tests/support.ts
fails a test on any load it refuses. The one addition: a brand lab page (`?lab=`, mock only, never
shipped) may load the Google Fonts families it previews.
"""
import functools
import http.server
import pathlib
import sys
import urllib.parse

POLICY_FILE = (
    pathlib.Path(__file__).resolve().parent.parent
    / "crates" / "yulius-server" / "src" / "content-security-policy.txt"
)
POLICY = POLICY_FILE.read_text(encoding="utf-8").strip()
LAB_FONTS = {"style-src": "https://fonts.googleapis.com", "font-src": "https://fonts.gstatic.com"}


def policy_for(path):
    """The server's policy; a lab page's (`?lab=`) adds the Google Fonts hosts."""
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(path).query, keep_blank_values=True)
    if "lab" not in query:
        return POLICY
    directives = []
    for directive in POLICY.split("; "):
        name = directive.split(" ", 1)[0]
        directives.append(f"{directive} {LAB_FONTS[name]}" if name in LAB_FONTS else directive)
    return "; ".join(directives)


class Handler(http.server.SimpleHTTPRequestHandler):
    # Keep-alive: every answer of SimpleHTTPRequestHandler (files, listings, errors) has a length.
    protocol_version = "HTTP/1.1"

    def end_headers(self):
        self.send_header("Content-Security-Policy", policy_for(self.path))
        super().end_headers()


class Server(http.server.ThreadingHTTPServer):
    # Room for several workers' pages and Monaco at once; the kernel caps it at net.core.somaxconn.
    request_queue_size = 1024


def make_server(port, directory):
    return Server(("127.0.0.1", port), functools.partial(Handler, directory=directory))


if __name__ == "__main__":
    make_server(int(sys.argv[1]), sys.argv[2]).serve_forever()
