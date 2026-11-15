#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Open an Endpoint's WebSocket as an anonymous client and subscribe to one type (T-4035).

    scripts/smoke-ws.py <https://host/api/endpoint/<slug>/ws> <Type> [timeout]

Prints `<upgrade status> <x-status of the subscribe>` (`101 201` when both hold) and exits 0 only
then. The client sends no Origin and no token, as websocat or a service would (EP-106/107), and
speaks the `ngsi-ld-ws` subprotocol of API/07. Standard library only: the smoke host has no
WebSocket package, and curl cannot send a frame.
"""

import base64
import json
import os
import socket
import ssl
import struct
import sys
from urllib.parse import urlsplit


def frame(text: str) -> bytes:
    """One masked text frame, as RFC 6455 5.3 requires of a client."""
    payload = text.encode()
    head = bytes([0x81])
    size = len(payload)
    if size < 126:
        head += bytes([0x80 | size])
    elif size < 1 << 16:
        head += bytes([0x80 | 126]) + struct.pack("!H", size)
    else:
        head += bytes([0x80 | 127]) + struct.pack("!Q", size)
    mask = os.urandom(4)
    return head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))


def exactly(sock: socket.socket, n: int) -> bytes:
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ConnectionError("the server closed the connection")
        data += chunk
    return data


def read_message(sock: socket.socket) -> tuple[int, bytes]:
    """The next unfragmented frame from the server: its opcode and payload (servers do not mask)."""
    first, second = exactly(sock, 2)
    size = second & 0x7F
    if size == 126:
        (size,) = struct.unpack("!H", exactly(sock, 2))
    elif size == 127:
        (size,) = struct.unpack("!Q", exactly(sock, 8))
    if size > 1 << 20:
        raise ValueError(f"a frame of {size} bytes")
    mask = exactly(sock, 4) if second & 0x80 else b""
    payload = exactly(sock, size)
    if mask:
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    return first & 0x0F, payload


def connect(url: str, timeout: float) -> tuple[socket.socket, int]:
    """The upgraded socket and the status line's code; the socket is open only on a 101."""
    parts = urlsplit(url)
    secure = parts.scheme in ("https", "wss")
    port = parts.port or (443 if secure else 80)
    sock = socket.create_connection((parts.hostname, port), timeout=timeout)
    if secure:
        sock = ssl.create_default_context().wrap_socket(sock, server_hostname=parts.hostname)
    key = base64.b64encode(os.urandom(16)).decode()
    request = (
        f"GET {parts.path or '/'} HTTP/1.1\r\nHost: {parts.netloc}\r\nUpgrade: websocket\r\n"
        f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
        "Sec-WebSocket-Protocol: ngsi-ld-ws\r\n\r\n"
    )
    sock.sendall(request.encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = sock.recv(1)
        if not chunk:
            break
        head += chunk
        if len(head) > 16384:
            break
    status_line = head.split(b"\r\n", 1)[0].split()
    status = int(status_line[1]) if len(status_line) > 1 and status_line[1].isdigit() else 0
    return sock, status


def subscribe(sock: socket.socket, entity_type: str) -> int:
    """The `x-status` of the `x-result` that answers one `x-subscribe` (API/07 §2)."""
    message = {
        "metadata": {"x-op": "x-subscribe", "x-id": "smoke"},
        "body": {"type": "Subscription", "entities": [{"type": entity_type}]},
    }
    sock.sendall(frame(json.dumps(message)))
    while True:
        opcode, payload = read_message(sock)
        if opcode == 0x8:
            raise ConnectionError(f"closed: {payload[2:].decode(errors='replace')}")
        if opcode != 0x1:
            continue
        meta = json.loads(payload).get("metadata", {})
        if meta.get("x-op") == "x-result" and meta.get("x-id") == "smoke":
            return int(meta.get("x-status", 0))


def main(argv: list[str]) -> int:
    if len(argv) not in (3, 4):
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    url, entity_type = argv[1], argv[2]
    timeout = float(argv[3]) if len(argv) == 4 else 20.0
    try:
        sock, status = connect(url, timeout)
    except OSError as error:
        print(f"0 - ({error})")
        return 1
    with sock:
        if status != 101:
            print(f"{status} -")
            return 1
        try:
            result = subscribe(sock, entity_type)
        except (OSError, ValueError) as error:
            print(f"101 - ({error})")
            return 1
    print(f"101 {result}")
    return 0 if result == 201 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
