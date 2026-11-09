"""Smoke suite `image`: the jc image as the cluster runs it, through .github/scripts/image-smoke.sh.

   bins/jc/smoke/run image --base-url <url> --out <dir> [--image ref] [--max-bytes n]

image-smoke.sh starts the image nonroot on a read-only root filesystem with every capability
dropped and the deployment's config shape, then checks the admin /healthz, /metrics refused
without its token and served with it, one health or discovery route of each module and the
gateway's forge checkout in the state volume. Each of those is one row here: passed when the
script printed it or got past it, failed when the script stopped on it, skipped as not reached
after an earlier failure. Beside it: the image's size (gate item 6: under 1 GB) and the image's
user. --image defaults to $JC_IMAGE; without an image (or docker) every row is a named skip.

The rehearsal config (the image on a rehearsal's jc.toml, Postgres and state dir) needs up.sh to
start jc from an image: that file is jc-migrate's (deploy/rehearsal/up.sh), asked 2026-11-09; until
up.sh reads REHEARSAL_IMAGE the row `rehearsal` is a named skip."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
REPO = SMOKE.parent.parent.parent
sys.path.insert(0, str(SMOKE))
from common import FAIL, Results, loopback_only

SCRIPT = REPO / ".github" / "scripts" / "image-smoke.sh"
UP = REPO / "deploy" / "rehearsal" / "up.sh"

# image-smoke.sh's checks in the order it runs them: (row id, what, the text of its failure line).
STAGES = [
    ("admin.healthz", "admin /healthz answers", "no admin /healthz"),
    ("user.nonroot", "the image runs as nonroot:nonroot", "does not run as nonroot"),
    ("admin.metrics.anonymous", "admin /metrics without a token answers 401", "/metrics without a token"),
    ("admin.metrics.token", "admin /metrics with its token answers 200", "/metrics with its token"),
]
MODULES = re.compile(r"^check (/\S+) (.+)$", re.MULTILINE)
TAIL = [
    ("gateway.checkout", "the gateway's forge checkout lies in the state volume", "forge checkout is missing"),
    ("gateway.log", "no gateway checkout failed in the log", "a gateway checkout failed"),
]


def stages(script_text: str) -> list[tuple[str, str, str]]:
    """Every check of the script, in order; the module routes read from its `check` lines."""
    mods = [(f"module{path}", f"GET {path} answers with {needle.strip(chr(39))}", f"{path} did not answer")
            for path, needle in MODULES.findall(script_text)]
    return STAGES + mods + TAIL


def rows_from_output(res: Results, text: str, plan: list[tuple[str, str, str]], call: str) -> None:
    """One row per stage from the script's output (`ok <path>`, `SMOKE: <failure>`, `SMOKE: ok`)."""
    failed = re.search(r"^SMOKE: (?!ok$)(.+)$", text, re.MULTILINE)
    done = re.search(r"^SMOKE: ok$", text, re.MULTILINE) is not None
    reason = failed.group(1).strip() if failed else ""
    stop = next((i for i, (_, _, needle) in enumerate(plan) if failed and needle in reason), None)
    if failed and stop is None:
        res.check("script", "image", call, "every check of image-smoke.sh", reason, False, tail(text))
    for i, (id, what, _) in enumerate(plan):
        if done or (stop is not None and i < stop):
            res.check(id, "image", call, what, "ok", True)
        elif stop == i:
            res.check(id, "image", call, what, reason[:200], False, tail(text))
        elif failed:
            res.skip(id, "image", call, what, f"not reached: image-smoke.sh stopped earlier ({reason[:120]})")
        else:
            res.check(id, "image", call, what, "no verdict line from image-smoke.sh", False, tail(text))


def tail(text: str, n: int = 6) -> str:
    return " / ".join(ln for ln in text.strip().splitlines()[-n:])[:400]


def inspect(image: str, fmt: str) -> str | None:
    p = subprocess.run(["docker", "image", "inspect", image, "--format", fmt], capture_output=True, text=True,
                       check=False)
    return p.stdout.strip() if p.returncode == 0 else None


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run image", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True, help="unused but checked: the runner's common flag")
    p.add_argument("--out", required=True)
    p.add_argument("--image", default=os.environ.get("JC_IMAGE", ""))
    p.add_argument("--max-bytes", type=int, default=1_000_000_000, help="gate item 6: the image under 1 GB")
    p.add_argument("--timeout", type=float, default=900)
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    res = Results("image", a.base_url)
    plan = stages(SCRIPT.read_text())
    call = f"sh {SCRIPT.relative_to(REPO)} {a.image or '<image>'}"
    size = inspect(a.image, "{{.Size}}") if a.image else None
    if size is None:
        why = ("no --image (or $JC_IMAGE): name the jc image to check" if not a.image
               else f"docker has no image {a.image} (or no docker)")
        for id, what, _ in plan + [("size", "image size", ""), ("rehearsal", "the image on a rehearsal config", "")]:
            res.skip(id, "image", call, what, why)
        res.write(Path(a.out), {"image": a.image})
        return 0
    res.check("size", "image", f"docker image inspect {a.image}", f"<= {a.max_bytes} bytes", f"{size} bytes",
              int(size) <= a.max_bytes)
    try:
        sp = subprocess.run(["sh", str(SCRIPT), a.image], capture_output=True, text=True, timeout=a.timeout,
                            check=False)
        text = sp.stdout + sp.stderr
    except subprocess.TimeoutExpired as e:
        text = f"{e.stdout or ''}\nSMOKE: timeout after {a.timeout} s"
    (Path(a.out)).mkdir(parents=True, exist_ok=True)
    (Path(a.out) / "image-smoke.log").write_text(text)
    rows_from_output(res, text, plan, call)
    if "REHEARSAL_IMAGE" not in UP.read_text():
        res.skip("rehearsal", "image", "up.sh with REHEARSAL_IMAGE", "the image starts on a rehearsal config",
                 "deploy/rehearsal/up.sh starts JC_BIN only (no image mode; jc-migrate's file, asked 2026-11-09)")
    else:
        res.skip("rehearsal", "image", "up.sh with REHEARSAL_IMAGE", "the image starts on a rehearsal config",
                 "up.sh reads REHEARSAL_IMAGE: run `run restore` with REHEARSAL_IMAGE set to compare the image's copy")
    res.write(Path(a.out), {"image": a.image, "imageId": inspect(a.image, "{{.Id}}"),
                            "revision": inspect(a.image, '{{index .Config.Labels "org.opencontainers.image.revision"}}')})
    return 1 if res.totals()[FAIL] else 0


if __name__ == "__main__":
    sys.exit(main())
