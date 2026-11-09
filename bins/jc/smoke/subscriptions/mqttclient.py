"""An MQTT 3.1.1 client in the stdlib, enough for the edge brokers' checks: CONNECT with a
username and a token as password, SUBSCRIBE, PUBLISH at QoS 1 and the PUBLISH a subscriber gets.
`mqtt://host:port` plain (a port-forward to a broker's listener), `mqtts://` over TLS."""

from __future__ import annotations

import socket
import ssl
import struct
import time
import urllib.parse

CONNECT, CONNACK, PUBLISH, PUBACK, SUBSCRIBE, SUBACK, DISCONNECT = 1, 2, 3, 4, 8, 9, 14


class Gone(Exception):
    """The broker closed the connection (a refused CONNECT or a refused publish)."""


def remaining(n: int) -> bytes:
    out = b""
    while True:
        b, n = n % 128, n // 128
        out += bytes([b | (0x80 if n else 0)])
        if not n:
            return out


def text(s: str) -> bytes:
    raw = s.encode()
    return struct.pack("!H", len(raw)) + raw


def packet(kind: int, flags: int, body: bytes) -> bytes:
    return bytes([(kind << 4) | flags]) + remaining(len(body)) + body


def connect_packet(client_id: str, username: str | None, password: str | None, keepalive: int = 30) -> bytes:
    flags = 0x02  # clean session
    payload = text(client_id)
    if username is not None:
        flags |= 0x80
        payload += text(username)
    if password is not None:
        flags |= 0x40
        payload += text(password)
    return packet(CONNECT, 0, text("MQTT") + bytes([4, flags]) + struct.pack("!H", keepalive) + payload)


class Mqtt:
    def __init__(self, url: str, timeout: float = 10):
        u = urllib.parse.urlparse(url)
        tls = u.scheme == "mqtts"
        sock = socket.create_connection((u.hostname, u.port or (8883 if tls else 1883)), timeout=timeout)
        self.sock = ssl.create_default_context().wrap_socket(sock, server_hostname=u.hostname) if tls else sock
        self.buf = b""
        self.pid = 0
        self.inbox: list[tuple[str, bytes]] = []

    def _read(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise Gone("the broker closed the connection")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def read(self, timeout: float):
        """The next packet as (type, flags, body); None when nothing came within `timeout`."""
        self.sock.settimeout(max(timeout, 0.01))
        try:
            first = self._read(1)[0]
        except TimeoutError:
            return None
        except (ConnectionError, OSError) as e:
            raise Gone(str(e)) from e
        self.sock.settimeout(10)
        n, mult = 0, 1
        while True:
            b = self._read(1)[0]
            n += (b & 0x7F) * mult
            mult *= 128
            if not b & 0x80:
                break
        return first >> 4, first & 0x0F, self._read(n)

    def expect(self, kind: int, timeout: float):
        """The next packet of `kind`; PUBLISHes on the way go to the inbox. None on timeout."""
        deadline = time.monotonic() + timeout
        while True:
            p = self.read(deadline - time.monotonic())
            if p is None:
                return None
            if p[0] == PUBLISH:
                self._inbound(p)
                continue
            if p[0] == kind:
                return p

    def _inbound(self, p) -> None:
        _, flags, body = p
        n = struct.unpack("!H", body[:2])[0]
        topic = body[2:2 + n].decode(errors="replace")
        rest = body[2 + n:]
        if (flags >> 1) & 0x03:
            pid, rest = rest[:2], rest[2:]
            self.sock.sendall(packet(PUBACK, 0, pid))
        self.inbox.append((topic, rest))

    def connect(self, client_id: str, username: str | None, password: str | None, timeout: float = 10) -> int:
        """The CONNACK return code (0 accepted); -1 when the broker closed or never answered."""
        self.sock.sendall(connect_packet(client_id, username, password))
        try:
            p = self.expect(CONNACK, timeout)
        except Gone:
            return -1
        return p[2][1] if p and len(p[2]) >= 2 else -1

    def _next_pid(self) -> int:
        self.pid = self.pid % 65535 + 1
        return self.pid

    def publish(self, topic: str, payload: bytes, timeout: float = 10) -> bool:
        """QoS 1: True when the broker acknowledged it, False when it refused (closed, no PUBACK)."""
        pid = self._next_pid()
        try:
            self.sock.sendall(packet(PUBLISH, 0x02, text(topic) + struct.pack("!H", pid) + payload))
            p = self.expect(PUBACK, timeout)
        except (Gone, OSError):
            return False
        return bool(p) and struct.unpack("!H", p[2][:2])[0] == pid

    def subscribe(self, topic: str, timeout: float = 10) -> int:
        """The SUBACK return code: 0..2 granted, 0x80 refused, -1 closed or no answer."""
        pid = self._next_pid()
        try:
            self.sock.sendall(packet(SUBSCRIBE, 0x02, struct.pack("!H", pid) + text(topic) + bytes([1])))
            p = self.expect(SUBACK, timeout)
        except (Gone, OSError):
            return -1
        return p[2][2] if p and len(p[2]) >= 3 else -1

    def received(self, topic: str, payload: bytes, timeout: float) -> bool:
        """Whether a PUBLISH of `payload` on `topic` arrives within `timeout`."""
        deadline = time.monotonic() + timeout
        while True:
            if (topic, payload) in self.inbox:
                return True
            left = deadline - time.monotonic()
            if left <= 0:
                return False
            try:
                p = self.read(left)
            except Gone:
                return False
            if p and p[0] == PUBLISH:
                self._inbound(p)

    def close(self) -> None:
        try:
            self.sock.sendall(packet(DISCONNECT, 0, b""))
            self.sock.close()
        except OSError:
            pass
