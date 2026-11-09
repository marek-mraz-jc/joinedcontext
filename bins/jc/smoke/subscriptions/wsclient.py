"""A WebSocket client (RFC 6455) in the stdlib, enough for the TS 104 243 envelope of the
Endpoint sockets (`{"metadata": {"x-op": ...}, "body": ...}`): text frames, ping/pong, close.
Loopback or a port-forward only: the suite checks the base before it opens anything."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import socket
import ssl
import struct
import time
import urllib.parse

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
MAX_MESSAGE = 4 << 20


class Refused(Exception):
    """The upgrade was answered with something other than 101."""

    def __init__(self, status: int, headers: dict, body: bytes):
        super().__init__(f"upgrade answered {status}")
        self.status, self.headers, self.body = status, headers, body


class Closed(Exception):
    """The server closed the socket (code, or 1006 without a close frame)."""

    def __init__(self, code: int, reason: str = ""):
        super().__init__(f"closed {code} {reason}".strip())
        self.code, self.reason = code, reason


def accept_of(key: str) -> str:
    return base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()


def frame(opcode: int, payload: bytes, mask: bool = True) -> bytes:
    """One final frame; a client masks every frame it sends."""
    head = bytes([0x80 | opcode])
    n = len(payload)
    bit = 0x80 if mask else 0
    if n < 126:
        head += bytes([bit | n])
    elif n < 1 << 16:
        head += bytes([bit | 126]) + struct.pack("!H", n)
    else:
        head += bytes([bit | 127]) + struct.pack("!Q", n)
    if not mask:
        return head + payload
    key = os.urandom(4)
    return head + key + bytes(b ^ key[i % 4] for i, b in enumerate(payload))


class Ws:
    def __init__(self, base: str, path: str, token: str | None = None, protocols=("ngsi-ld-ws",),
                 headers: dict | None = None, timeout: float = 10):
        u = urllib.parse.urlparse(base)
        tls = u.scheme in ("https", "wss")
        port = u.port or (443 if tls else 80)
        self.buf = b""
        sock = socket.create_connection((u.hostname, port), timeout=timeout)
        self.sock = ssl.create_default_context().wrap_socket(sock, server_hostname=u.hostname) if tls else sock
        key = base64.b64encode(os.urandom(16)).decode()
        lines = [f"GET {path} HTTP/1.1", f"Host: {u.netloc}", "Upgrade: websocket", "Connection: Upgrade",
                 f"Sec-WebSocket-Key: {key}", "Sec-WebSocket-Version: 13"]
        if protocols:
            lines.append("Sec-WebSocket-Protocol: " + ", ".join(protocols))
        if token:
            lines.append(f"Authorization: Bearer {token}")
        lines += [f"{k}: {v}" for k, v in (headers or {}).items()]
        self.sock.sendall(("\r\n".join(lines) + "\r\n\r\n").encode())
        status, hdrs = self._head()
        if status != 101:
            raise Refused(status, hdrs, self._body(hdrs))
        if hdrs.get("sec-websocket-accept") != accept_of(key):
            raise Refused(status, hdrs, b"wrong Sec-WebSocket-Accept")
        self.protocol = hdrs.get("sec-websocket-protocol", "")

    # ------------------------------------------------------------------ HTTP part

    def _read(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise Closed(1006, "connection ended")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def _head(self):
        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise Refused(0, {}, b"no answer to the upgrade")
            self.buf += chunk
            if len(self.buf) > 65536:
                raise Refused(0, {}, b"upgrade answer head too long")
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        first, *rest = head.decode("latin-1").split("\r\n")
        parts = first.split(" ", 2)
        status = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
        hdrs = {}
        for line in rest:
            k, _, v = line.partition(":")
            hdrs[k.strip().lower()] = v.strip()
        return status, hdrs

    def _body(self, hdrs) -> bytes:
        try:
            n = int(hdrs.get("content-length", "0"))
            return self._read(min(n, 65536)) if n else self.buf[:65536]
        except (Closed, OSError, ValueError):
            return self.buf[:65536]

    # ------------------------------------------------------------------ frames

    def send(self, obj) -> None:
        self.sock.sendall(frame(0x1, json.dumps(obj).encode()))

    def message(self, timeout: float):
        """The next text message as JSON; None when nothing came within `timeout`."""
        deadline = time.monotonic() + timeout
        parts: list[bytes] = []
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return None
            self.sock.settimeout(left)
            try:
                b0, b1 = self._read(2)
            except TimeoutError:
                return None
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack("!H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack("!Q", self._read(8))[0]
            if n > MAX_MESSAGE:
                raise Closed(1009, "message over the client's cap")
            key = self._read(4) if b1 & 0x80 else None
            self.sock.settimeout(30)
            payload = self._read(n)
            if key:
                payload = bytes(b ^ key[i % 4] for i, b in enumerate(payload))
            opcode = b0 & 0x0F
            if opcode == 0x8:
                code = struct.unpack("!H", payload[:2])[0] if len(payload) >= 2 else 1005
                raise Closed(code, payload[2:].decode(errors="replace"))
            if opcode == 0x9:
                self.sock.sendall(frame(0xA, payload))
                continue
            if opcode in (0x1, 0x0):
                parts.append(payload)
                if b0 & 0x80:
                    text = b"".join(parts)
                    parts = []
                    try:
                        return json.loads(text)
                    except ValueError:
                        return {"raw": text.decode(errors="replace")}

    def until(self, pred, timeout: float):
        """The first message `pred` accepts within `timeout`, else None; the others are dropped."""
        deadline = time.monotonic() + timeout
        while True:
            m = self.message(max(0.0, deadline - time.monotonic()))
            if m is None:
                return None
            if pred(m):
                return m

    def close(self) -> None:
        try:
            self.sock.sendall(frame(0x8, struct.pack("!H", 1000)))
        except OSError:
            pass
        try:
            self.sock.close()
        except OSError:
            pass


def op(m) -> str:
    return ((m or {}).get("metadata") or {}).get("x-op", "") if isinstance(m, dict) else ""
