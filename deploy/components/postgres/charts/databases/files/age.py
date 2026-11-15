#!/usr/bin/env python3
"""The age v1 file format (age-encryption.org/v1, X25519 recipients) over stdin and stdout, for
the nightly dump (T-5838, CC-06): the roleJob image has python3-cryptography and no `age`.
A file this writes opens with `age -d -i <identity>`; one `age -r` wrote opens here.

  age.py encrypt [--identity-file F] RECIPIENT...   seal stdin to each age1... recipient, and
                                                    to the public half of the hex key in F
  age.py decrypt --identity-file F                  open stdin with the hex X25519 key in F

The key file holds 64 hex digits (the secrets generator's `encoding: hex`). The payload streams
in 64 KiB chunks, so memory stays flat whatever the dump's size.
"""
import base64
import hashlib
import hmac
import os
import sys

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

INTRO = b"age-encryption.org/v1\n"
LABEL = b"age-encryption.org/v1/X25519"
CHUNK = 64 * 1024
BECH32 = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"


class Refused(Exception):
    pass


def hkdf(ikm, salt, info):
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=salt, info=info).derive(ikm)


def b64(raw):
    return base64.b64encode(raw).rstrip(b"=")


def unb64(text):
    if text.endswith(b"=") or b"\r" in text or b"\n" in text:
        raise Refused("non-canonical base64 in the header")
    return base64.b64decode(text + b"=" * (-len(text) % 4), validate=True)


def recipient_key(text, want="age"):
    """The 32 bytes of an age1... recipient (bech32, BIP 173); `want` "age-secret-key-" reads an
    age identity (its upper-case secret-key form) the same way."""
    text = text.strip().lower()
    hrp, _, data = text.rpartition("1")
    if hrp != want or len(data) < 6 or any(c not in BECH32 for c in data):
        raise Refused(f"{text[:4]}...: not an age1 recipient")
    values = [BECH32.index(c) for c in data]
    check = 1
    for v in [ord(c) >> 5 for c in hrp] + [0] + [ord(c) & 31 for c in hrp] + values:
        top = check >> 25
        check = (check & 0x1FFFFFF) << 5 ^ v
        for i, g in enumerate((0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3)):
            check ^= g if (top >> i) & 1 else 0
    if check != 1:
        raise Refused(f"{text[:4]}...: bech32 checksum fails")
    acc, bits, out = 0, 0, bytearray()
    for v in values[:-6]:
        acc, bits = acc << 5 | v, bits + 5
        if bits >= 8:
            bits -= 8
            out.append(acc >> bits & 0xFF)
    if len(out) != 32:
        raise Refused(f"{text[:4]}...: not a 32-byte key")
    return bytes(out)


def identity(path):
    raw = open(path, encoding="ascii").read().strip()
    if len(raw) != 64:
        raise Refused(f"{path}: the key must be 64 hex digits")
    return X25519PrivateKey.from_private_bytes(bytes.fromhex(raw))


def public_bytes(key):
    return key.public_bytes_raw()


def wrap(file_key, recipient):
    ephemeral = X25519PrivateKey.generate()
    share = public_bytes(ephemeral.public_key())
    shared = ephemeral.exchange(X25519PublicKey.from_public_bytes(recipient))
    body = ChaCha20Poly1305(hkdf(shared, share + recipient, LABEL)).encrypt(b"\0" * 12, file_key, None)
    return b"-> X25519 " + b64(share) + b"\n" + b64(body) + b"\n"


def read_exact(stream, n):
    buf = bytearray()
    while len(buf) < n:
        part = stream.read(n - len(buf))
        if not part:
            break
        buf += part
    return bytes(buf)


def encrypt(recipients, src, dst):
    file_key = os.urandom(16)
    header = INTRO + b"".join(wrap(file_key, r) for r in recipients) + b"---"
    mac = hmac.new(hkdf(file_key, b"", b"header"), header, hashlib.sha256).digest()
    nonce = os.urandom(16)
    dst.write(header + b" " + b64(mac) + b"\n" + nonce)
    aead = ChaCha20Poly1305(hkdf(file_key, nonce, b"payload"))
    counter, chunk = 0, read_exact(src, CHUNK)
    while True:
        following = read_exact(src, CHUNK) if len(chunk) == CHUNK else b""
        last = not following
        dst.write(aead.encrypt(counter.to_bytes(11, "big") + bytes([last]), chunk, None))
        if last:
            return
        counter, chunk = counter + 1, following


def decrypt(key, src, dst):
    if src.readline() != INTRO:
        raise Refused("not an age v1 file")
    header, file_key, ours = INTRO, None, public_bytes(key.public_key())
    while True:
        line = src.readline()
        if line.startswith(b"--- "):
            header += b"---"
            break
        if not line.startswith(b"-> "):
            raise Refused("a damaged header")
        header += line
        body = b""
        while True:  # a stanza body ends on its first line under 64 columns
            part = src.readline()
            header += part
            body += part.rstrip(b"\n")
            if len(part.rstrip(b"\n")) < 64:
                break
        args = line[3:].split()
        if file_key is not None or args[:1] != [b"X25519"] or len(args) != 2 or len(body) > 44:
            continue
        share = unb64(args[1])
        shared = key.exchange(X25519PublicKey.from_public_bytes(share))
        try:
            file_key = ChaCha20Poly1305(hkdf(shared, share + ours, LABEL)).decrypt(
                b"\0" * 12, unb64(body), None)
        except Exception:
            file_key = None
    if file_key is None:
        raise Refused("no recipient of this file is the key given")
    mac = hmac.new(hkdf(file_key, b"", b"header"), header, hashlib.sha256).digest()
    if not hmac.compare_digest(unb64(line[4:].rstrip(b"\n")), mac):
        raise Refused("the header MAC does not match")
    aead = ChaCha20Poly1305(hkdf(file_key, read_exact(src, 16), b"payload"))
    counter, chunk = 0, read_exact(src, CHUNK + 16)
    while True:
        following = read_exact(src, CHUNK + 16) if len(chunk) == CHUNK + 16 else b""
        last = not following
        try:
            dst.write(aead.decrypt(counter.to_bytes(11, "big") + bytes([last]), chunk, None))
        except Exception as e:
            raise Refused(f"chunk {counter} is damaged or the file is truncated") from e
        if last:
            return
        counter, chunk = counter + 1, following


def main(argv):
    args, key_file = list(argv[1:]), None
    if "--identity-file" in args:
        at = args.index("--identity-file")
        key_file = args[at + 1] if at + 1 < len(args) else None
        del args[at:at + 2]
    if not args or args[0] not in ("encrypt", "decrypt") or (args[0] == "decrypt" and not key_file):
        print(__doc__, file=sys.stderr)
        return 2
    try:
        if args[0] == "decrypt":
            decrypt(identity(key_file), sys.stdin.buffer, sys.stdout.buffer)
        else:
            recipients = [recipient_key(r) for r in args[1:]]
            if key_file:
                recipients.append(public_bytes(identity(key_file).public_key()))
            if not recipients:
                raise Refused("no recipient: the dump would be sealed to nobody")
            encrypt(recipients, sys.stdin.buffer, sys.stdout.buffer)
        sys.stdout.buffer.flush()
    except (Refused, ValueError, OSError) as e:
        print(f"age.py: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
