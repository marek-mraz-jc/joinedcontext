"""scripts/smoke-ws.py against a local server (T-4035): the anonymous upgrade, the subscribe
frame it sends, and every answer it must tell apart (201, a refusal before the upgrade, a close)."""

import importlib.util
import json
import socket
import struct
import threading
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/smoke-ws.py"
spec = importlib.util.spec_from_file_location("smoke_ws", SCRIPT)
smoke_ws = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke_ws)


def server_frame(opcode: int, payload: bytes) -> bytes:
    size = len(payload)
    head = bytes([0x80 | opcode]) + (bytes([size]) if size < 126 else bytes([126]) + struct.pack("!H", size))
    return head + payload


def serve(answer):
    """A one-connection server on localhost; `answer(conn, request_head)` plays the gateway."""
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    seen = {}

    def run():
        conn, _ = listener.accept()
        with conn:
            head = b""
            while b"\r\n\r\n" not in head:
                head += conn.recv(1)
            seen["head"] = head.decode()
            answer(conn, seen)
        listener.close()

    threading.Thread(target=run, daemon=True).start()
    return f"http://127.0.0.1:{listener.getsockname()[1]}/api/endpoint/abc/ws", seen


def upgraded(conn, seen, reply):
    conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                 b"Sec-WebSocket-Protocol: ngsi-ld-ws\r\n\r\n")
    opcode, payload = smoke_ws.read_message(conn)
    seen["sent"] = (opcode, json.loads(payload))
    reply(conn)


def test_an_anonymous_upgrade_and_a_created_subscription_pass(capsys):
    def reply(conn):
        # A notification of another subscription first: the client waits for its own x-id.
        conn.sendall(server_frame(1, b'{"metadata":{"x-op":"x-notification","x-seq":1}}'))
        conn.sendall(server_frame(1, json.dumps({"metadata": {"x-op": "x-result", "x-id": "smoke", "x-status": 201},
                                                 "body": {"id": "urn:ngsi-ld:Subscription:1"}}).encode()))

    url, seen = serve(lambda conn, seen: upgraded(conn, seen, reply))
    assert smoke_ws.main(["smoke-ws.py", url, "Vehicle", "5"]) == 0
    assert capsys.readouterr().out.strip() == "101 201"
    head = seen["head"].lower()
    assert "origin:" not in head and "authorization:" not in head and "bearer." not in head
    assert "sec-websocket-protocol: ngsi-ld-ws" in head
    opcode, message = seen["sent"]
    assert opcode == 1
    assert message["metadata"] == {"x-op": "x-subscribe", "x-id": "smoke"}
    assert message["body"]["entities"] == [{"type": "Vehicle"}]


def test_a_refused_subscription_fails_with_its_status(capsys):
    def reply(conn):
        conn.sendall(server_frame(1, b'{"metadata":{"x-op":"x-result","x-id":"smoke","x-status":403}}'))

    url, _ = serve(lambda conn, seen: upgraded(conn, seen, reply))
    assert smoke_ws.main(["smoke-ws.py", url, "Vehicle", "5"]) == 1
    assert capsys.readouterr().out.strip() == "101 403"


def test_a_refusal_before_the_upgrade_fails_with_the_http_status(capsys):
    url, _ = serve(lambda conn, seen: conn.sendall(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"))
    assert smoke_ws.main(["smoke-ws.py", url, "Vehicle", "5"]) == 1
    assert capsys.readouterr().out.strip() == "404 -"


def test_a_close_instead_of_a_result_fails_and_says_why(capsys):
    url, _ = serve(lambda conn, seen: upgraded(conn, seen, lambda c: c.sendall(server_frame(8, b"\x03\xe8going away"))))
    assert smoke_ws.main(["smoke-ws.py", url, "Vehicle", "5"]) == 1
    assert capsys.readouterr().out.strip() == "101 - (closed: going away)"


def test_a_client_frame_is_masked_and_a_long_one_carries_its_16_bit_length():
    data = smoke_ws.frame("x" * 300)
    assert data[0] == 0x81 and data[1] == 0x80 | 126
    assert struct.unpack("!H", data[2:4])[0] == 300
    mask, payload = data[4:8], data[8:]
    assert bytes(b ^ mask[i % 4] for i, b in enumerate(payload)) == b"x" * 300


def test_nobody_listening_fails_without_a_traceback(capsys):
    free = socket.socket()
    free.bind(("127.0.0.1", 0))
    port = free.getsockname()[1]
    free.close()
    assert smoke_ws.main(["smoke-ws.py", f"http://127.0.0.1:{port}/ws", "Vehicle", "2"]) == 1
    assert capsys.readouterr().out.startswith("0 - (")
