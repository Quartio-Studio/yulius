"""Static file server for the e2e suite: `python3 -I serve.py <port> <directory>` (SPEC-148).

`python3 -m http.server` as the suite ran it before kept up only on a quiet machine (#354):
- its listen queue holds 5 connections (`socketserver`'s default). The kernel drops a SYN beyond
  it, and the browser waits 1 s, then 3 s, before trying again. On a loaded machine the accept
  loop is not scheduled for a while, and a page opens about 75 modules at once (Monaco about 120
  more), so first loads stalled past the tests' 5 s waits;
- it speaks HTTP/1.0, which closes the connection after each answer, so every module costs a new
  connection, and another place in that queue.
This one queues 1024 connections and keeps connections alive, as any production server does. It serves the same files with the same answers otherwise.
"""
import functools
import http.server
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    # Keep-alive: every answer of SimpleHTTPRequestHandler (files, listings, errors) has a length.
    protocol_version = "HTTP/1.1"


class Server(http.server.ThreadingHTTPServer):
    # Room for several workers' pages and Monaco at once; the kernel caps it at net.core.somaxconn.
    request_queue_size = 1024


def make_server(port, directory):
    return Server(("127.0.0.1", port), functools.partial(Handler, directory=directory))


if __name__ == "__main__":
    make_server(int(sys.argv[1]), sys.argv[2]).serve_forever()
