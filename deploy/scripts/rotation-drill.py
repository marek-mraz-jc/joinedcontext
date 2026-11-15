#!/usr/bin/env python3
"""The quarterly secrets rotation drill on dev (T-5106, OPS-45; Operations/01 Runbook 5).

    scripts/rotation-drill.py --instance dev --domain dev.joinedcontext.com --realm dev \
        --out <summary.json> [--model-key-file <file>] [--dry-run]

One credential of each kind the platform holds is rotated with `scripts/rotate-secret.sh`,
one after the other: a Keycloak client secret, a database role password, a forge token, the
catalogue's API token and, when the owner hands a new key over in a file, the model key. While
each rotation runs, every probe URL is read every two seconds; a probe that does not answer
2xx or 3xx is a failure, and the drill passes only with none.

The summary is the one `scripts/publish-health.py` publishes (check `secret-rotation`), so the
Portal's Health tab shows the last drill and marks it stale once no drill has been published
for two cadences. `--dry-run` passes `--dry-run` to every rotation, prints each plan and
starts no probe: nothing in the cluster changes.

No credential passes through this process: rotate-secret.sh reads and writes the values and
prints none. Exit 0 when nothing failed, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import threading
import time
import urllib.error
import urllib.request
from datetime import UTC, datetime
from pathlib import Path
from typing import Self

ROTATE = Path(__file__).resolve().parent / "rotate-secret.sh"
INTERVAL_S = 2.0
TIMEOUT_S = 10


def result(key: str, verdict: str, title: str, detail: str = "") -> dict:
    return {"key": key, "verdict": verdict, "title": title, "detail": detail, "evidence": ""}


def rotations(args: argparse.Namespace) -> list[tuple[str, list[str]]]:
    """(secret, extra arguments) per kind, in the order the drill runs them."""
    domain = args.domain
    plan = [
        ("keycloak-client-activity-ingest", ["--idm", f"https://idm.{domain}", "--realm", args.realm]),
        ("db-portal", []),
        ("gitea-token-gateway", ["--forge", f"https://{domain}/git"]),
        ("ckan-api-token", ["--catalogue", f"https://opendata.{domain}"]),
    ]
    if args.model_key_file:
        plan.append(("agent-runner-model-key",
                     ["--from-file", str(args.model_key_file), "--model-base", args.model_base]))
    return plan


class Probes:
    """Reads every URL every INTERVAL_S seconds on its own thread until stopped."""

    def __init__(self, urls: list[str]):
        self.urls = urls
        self.reads = {url: 0 for url in urls}
        self.failures: dict[str, list[str]] = {url: [] for url in urls}
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _read(self, url: str) -> str | None:
        """None when the URL answered 2xx or 3xx, else what it answered."""
        request = urllib.request.Request(url, headers={"User-Agent": "jc-rotation-drill"})
        try:
            with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
                return None if response.status < 400 else str(response.status)
        except urllib.error.HTTPError as error:
            return str(error.code)
        except (urllib.error.URLError, OSError) as error:
            return type(error).__name__

    def _run(self) -> None:
        while not self._stop.is_set():
            for url in self.urls:
                answer = self._read(url)
                self.reads[url] += 1
                if answer is not None:
                    stamp = datetime.now(UTC).strftime("%H:%M:%SZ")
                    self.failures[url].append(f"{stamp} {answer}")
            self._stop.wait(INTERVAL_S)

    def __enter__(self) -> Self:
        self._thread.start()
        return self

    def __exit__(self, *_exc: object) -> None:
        self._stop.set()
        self._thread.join()


def rotate(instance: str, secret: str, extra: list[str], dry_run: bool) -> dict:
    command = [str(ROTATE), "--instance", instance, "--secret", secret, *extra]
    if dry_run:
        command.append("--dry-run")
    started = time.monotonic()
    done = subprocess.run(command, capture_output=True, text=True, check=False)
    seconds = round(time.monotonic() - started)
    lines = [line for line in (done.stdout + done.stderr).splitlines() if line.strip()]
    last = lines[-1] if lines else "rotate-secret.sh printed nothing"
    print("\n".join(lines))
    verdict = "pass" if done.returncode == 0 else "fail"
    return result(f"rotate/{secret}", verdict, f"{secret}: {last} ({seconds} s)", "\n".join(lines[-20:]))


def drill(args: argparse.Namespace) -> list[dict]:
    results: list[dict] = []
    urls = args.probe or [
        f"https://portal.{args.domain}/api/v1/health",
        f"https://idm.{args.domain}/realms/{args.realm}/.well-known/openid-configuration",
        f"https://opendata.{args.domain}/api/3/action/status_show",
    ]
    if args.dry_run:
        for secret, extra in rotations(args):
            results.append(rotate(args.instance, secret, extra, True))
        results.append(result("probes", "skip", "dry run: no probe was read"))
    else:
        with Probes(urls) as probes:
            for secret, extra in rotations(args):
                results.append(rotate(args.instance, secret, extra, False))
        for url in urls:
            failed = probes.failures[url]
            verdict = "pass" if probes.reads[url] and not failed else "fail"
            results.append(result(f"probes/{url}", verdict,
                                  f"{url}: {len(failed)} of {probes.reads[url]} reads failed during the rotations",
                                  "\n".join(failed[:50])))
    if not args.model_key_file:
        results.append(result("rotate/agent-runner-model-key", "skip",
                              "model key: no new key supplied (the owner creates one in the provider's account)"))
    return results


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--instance", required=True, help="the helmfile environment, e.g. dev")
    parser.add_argument("--domain", required=True, help="the instance's domain, e.g. dev.joinedcontext.com")
    parser.add_argument("--realm", required=True, help="the Keycloak realm, e.g. dev")
    parser.add_argument("--out", type=Path, required=True, help="where the summary JSON goes")
    parser.add_argument("--model-key-file", type=Path, help="a new model key from the provider, one line")
    parser.add_argument("--model-base", default="https://openrouter.ai/api/v1", help="the provider's API base")
    parser.add_argument("--probe", action="append", help="a URL to read during the rotations (repeatable)")
    parser.add_argument("--dry-run", action="store_true", help="print each rotation's plan, change nothing")
    args = parser.parse_args(argv)
    for url in args.probe or []:
        if not url.startswith("https://"):
            parser.error(f"--probe {url}: only https URLs are probed")
    results = drill(args)
    summary = {"check": "secret-rotation", "repo": "joinedcontext-deployment",
               "run": datetime.now(UTC).strftime("%Y-%m-%dT%H:%MZ"), "requirements": ["OPS-45"],
               "results": results}
    args.out.write_text(json.dumps(summary, indent=2) + "\n")
    for r in results:
        print(f"{r['verdict']:5} {r['title']}")
    return 1 if any(r["verdict"] in ("fail", "error") for r in results) else 0


if __name__ == "__main__":
    raise SystemExit(main())
