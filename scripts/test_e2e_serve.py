"""The e2e suite's static file server keeps up with a page's burst of requests (SPEC-148, #354)."""
import http.client
import importlib.util
import socket
import tempfile
import threading
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location("e2e_serve", Path(__file__).resolve().parent.parent / "e2e" / "serve.py")
serve = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(serve)


class Serve(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        Path(self.root.name, "app.js").write_text("export const ok = true;\n")
        self.server = serve.make_server(0, self.root.name)
        self.port = self.server.server_address[1]
        self.sockets = []
        self.serving = False

    def tearDown(self):
        for sock in self.sockets:
            sock.close()
        if self.serving:
            self.server.shutdown()
        self.server.server_close()
        self.root.cleanup()

    def start(self):
        self.serving = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def test_a_burst_of_connections_waits_for_a_busy_server_instead_of_being_dropped(self):
        # The server accepts nothing yet, as when a loaded machine has not scheduled its accept
        # loop: the kernel completes each handshake into the listen queue, up to its length. A
        # connection beyond it has its SYN dropped and waits for the client's retransmission
        # (1 s, then 3 s): the stalls of #354. A page and Monaco open about 200 at once.
        for _ in range(64):
            sock = socket.create_connection(("127.0.0.1", self.port), timeout=0.5)
            self.sockets.append(sock)

    def test_one_connection_carries_several_requests(self):
        # Keep-alive: a page's modules do not each cost a new connection (HTTP/1.0 closes after one).
        self.start()
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            for _ in range(3):
                conn.request("GET", "/app.js")
                response = conn.getresponse()
                self.assertEqual(response.read(), b"export const ok = true;\n")
                self.assertEqual(response.version, 11)
                self.assertFalse(response.will_close)
        finally:
            conn.close()

    def test_a_missing_file_is_404_and_the_next_request_is_answered(self):
        self.start()
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            conn.request("GET", "/missing.js")
            response = conn.getresponse()
            response.read()
            self.assertEqual(response.status, 404)
            conn.request("GET", "/app.js")
            self.assertEqual(conn.getresponse().status, 200)
        finally:
            conn.close()


if __name__ == "__main__":
    unittest.main()
