#!/usr/bin/env python3
"""Asks a public assistant one cheap question every few seconds while a rollout runs (T-4809).

    scripts/assistant-probe.py BASE [--id data] [--every 15] [--most 40] 2>LOG

Each question earns or reuses a visitor token the way the widget does (a challenge and its proof
of work, API/05 §1.2.1) and is asked without an Origin. One JSON line per question goes to stderr;
on SIGTERM, SIGINT or after --most questions the probe prints one summary line:

    assistant probe: 12 asked, 12 ok, 0 model_unreachable, 0 other

A visitor reads MODEL_UNREACHABLE when the assistant cannot reach its model; that answer and every
non-200 are what a rollout must not cause. The probe never fails the caller: its exit code is 0
and the summary is the evidence."""

import argparse
import contextlib
import hashlib
import json
import signal
import sys
import time
import urllib.error
import urllib.request

QUESTION = "Dobrý deň"
# The sentence of crates/assistant/src/chat/model.rs MODEL_UNREACHABLE (platform).
MODEL_UNREACHABLE = "Asistent sa teraz nevie spojiť so svojím modelom"


def zero_bits(digest: bytes) -> int:
    bits = 0
    for byte in digest:
        if byte:
            return bits + 8 - byte.bit_length()
        bits += 8
    return bits


def solve(challenge: str, difficulty: int) -> str:
    nonce = 0
    while zero_bits(hashlib.sha256(f"{challenge}:{nonce}".encode()).digest()) < difficulty:
        nonce += 1
    return str(nonce)


def _json(url: str, body: dict | None = None) -> dict:
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if data else {}
    request = urllib.request.Request(url, data=data, headers=headers, method="POST" if data else "GET")
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def visitor_token(root: str) -> str:
    challenge = _json(f"{root}/challenge")
    nonce = solve(challenge["challenge"], int(challenge["difficulty"]))
    return _json(f"{root}/token", {"challenge": challenge["challenge"], "nonce": nonce})["token"]


def verdict(status: int, stream: str) -> str:
    """`ok`, `model_unreachable` or `other` for one answer's status and its event stream."""
    if MODEL_UNREACHABLE in stream:
        return "model_unreachable"
    if status != 200:
        return "other"
    names = [line[7:] for line in stream.splitlines() if line.startswith("event: ")]
    return "ok" if names and names[-1] == "done" and "error" not in names else "other"


def ask(root: str, token: str) -> tuple[int, str]:
    request = urllib.request.Request(
        f"{root}/chat",
        data=json.dumps({"message": QUESTION}).encode(),
        headers={"Content-Type": "application/json", "Accept": "text/event-stream", "X-JC-Visitor": token},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.status, response.read().decode(errors="replace")
    except urllib.error.HTTPError as err:
        return err.code, err.read().decode(errors="replace")


def probe(base: str, public_id: str, every: float, most: int, log, counts: dict, now=time.monotonic) -> dict:
    """Asks up to `most` questions, counting each verdict in `counts` as it comes, so a stop
    mid-probe still reports what was asked."""
    root = f"{base.rstrip('/')}/api/v1/d/{public_id}"
    token, earned = None, 0.0
    for n in range(most):
        started = now()
        try:
            # A token is good for an hour; a new one is earned after 50 minutes.
            if token is None or started - earned > 3_000:
                token, earned = visitor_token(root), started
            status, stream = ask(root, token)
            why = verdict(status, stream)
        except (OSError, ValueError, KeyError) as err:
            status, why = 0, "other"
            stream = f"{type(err).__name__}: {err}"
        counts[why] += 1
        line = {"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "status": status, "verdict": why}
        if why != "ok":
            line["detail"] = stream[:300]
        print(json.dumps(line, ensure_ascii=False), file=log, flush=True)
        if n + 1 < most:
            time.sleep(max(0.0, every - (now() - started)))
    return counts


def summary(counts: dict) -> str:
    asked = sum(counts.values())
    return (f"assistant probe: {asked} asked, {counts['ok']} ok, "
            f"{counts['model_unreachable']} model_unreachable, {counts['other']} other")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base", help="the assistant's origin, e.g. https://assistant.smartcity.joinedcontext.com")
    parser.add_argument("--id", default="data", help="the deployment's publicId")
    parser.add_argument("--every", type=float, default=15.0, help="seconds between questions")
    parser.add_argument("--most", type=int, default=40, help="the most questions asked (budget)")
    args = parser.parse_args()
    if not args.base.startswith(("https://", "http://")) or not 1 <= args.most <= 40 or args.every < 5:
        parser.error("BASE must be an http(s) origin, --most 1..40 and --every at least 5 seconds")
    counts = {"ok": 0, "model_unreachable": 0, "other": 0}

    def stop(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    with contextlib.suppress(KeyboardInterrupt):
        probe(args.base, args.id, args.every, args.most, sys.stderr, counts)
    print(summary(counts), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
