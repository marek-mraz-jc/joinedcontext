"""seeds/bbsk-events/check-sources.py fetches a source only from the host whose robots.txt it read,
and names an answer it would have cut short instead of reporting it as a changed format (F-R2BNT3-018)."""

import http.server
import importlib.util
import threading
import urllib.error
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "seeds/bbsk-events/check-sources.py"
spec = importlib.util.spec_from_file_location("check_sources", SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class Source(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/away":
            self.send_response(302)
            self.send_header("Location", "https://elsewhere.invalid/feed")
            self.end_headers()
        elif self.path == "/plain":
            self.send_response(302)
            self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}/ok")
            self.end_headers()
        else:
            body = b"x" * (check.MAX_BYTES + 1 if self.path == "/big" else 10)
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, *args):
        pass


@pytest.fixture(scope="module")
def base():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Source)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_port}"
    server.shutdown()


@pytest.mark.parametrize("path", ["/away", "/plain"])
def test_a_redirect_off_the_host_or_off_https_is_refused(base, path):
    with pytest.raises(urllib.error.HTTPError, match="redirect off the host"):
        check.fetch(base + path)


def test_an_answer_over_the_cap_fails_by_its_size(base):
    with pytest.raises(ValueError, match="larger than"):
        check.fetch(base + "/big")
    assert check.fetch(base + "/ok")[1] == "x" * 10
