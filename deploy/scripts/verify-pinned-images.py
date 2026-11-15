#!/usr/bin/env python3
"""Refuse an apply whose render carries a platform image without our signature (T-5564, OPS-27).

`scripts/pin-images.py` checks the signature when it pins, and `scripts/validate-dev.py` checks the
running pods an hour later. A digest written into an images.yaml by hand passed neither before it
ran. This script sits between the render and `helmfile sync` in `just dev-apply` and
`just prod-apply`: every `ghcr.io/marek-mraz-jc/*@sha256:` reference anywhere in the rendered
environment (container images, init containers, env values, config) must pass `cosign verify` for
a keyless certificate our own GitHub workflows minted, and every `image:` of ours must be a digest.
Every problem is printed, so one run names each digest to fix; any problem exits 1 and the apply
stops before it changes the cluster.

  scripts/verify-pinned-images.py <environment>          render, then verify (deployment/ assembled)
  scripts/verify-pinned-images.py <env> --render FILE    verify an existing render
  scripts/verify-pinned-images.py <env> --render FILE --scoped   a render of some releases, which may name none

  scripts/verify-pinned-images.py <env> --scanned       also the scan record production needs (T-6154)

`--scanned` (the release gate and `just prod-apply`): every digest a repository in SCANNED published
must also carry the CycloneDX SBOM and the vuln-scan attestation of that repository's
image-scan.yml on main, which attests only after its HIGH/CRITICAL gate passed. Dev pins a digest
as soon as image.yml signed it; the scan runs beside, and production waits for it.

`--registry` and `--key` exist for the offline selftest (tests/test_verify_pinned_images.py): a
throwaway registry on localhost and a local key pair stand in for ghcr.io and the keyless identity.
"""

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OWNER = "marek-mraz-jc"
ISSUER = "https://token.actions.githubusercontent.com"
# The workflows that sign what an environment pins, read off the pinned digests' certificates
# (2026-10-26: image.yml of platform and portal, image-viz/image-ckan of deployment, Antares
# ship.yml on dev) plus the other signers of the same images (promote.yml, the deployment's
# image-gitops, image-patched (T-6114) and reusable-container-sign, Antares image-jc.yml and full.yml on a tag)
# and the monorepo joinedcontext's image.yml (components/jc, the one binary), image-embedder.yml
# and image-apps.yml (components/embedder, components/app-pods). image-scan.yml only attests. A preview.yml
# or any branch build signs too and is refused here (F-DEP2-005).
IDENTITY = (
    rf"^https://github\.com/{OWNER}/("
    r"(joinedcontext-platform|joinedcontext-portal)/\.github/workflows/(image|promote)\.yml@refs/heads/main"
    r"|joinedcontext/\.github/workflows/(image|image-embedder|image-apps)\.yml@refs/heads/main"
    r"|joinedcontext-deployment/\.github/workflows/(image-[a-z]+|reusable-container-sign)\.yml@refs/heads/main"
    r"|Antares-NGSI-LD-Context-Broker/\.github/workflows/(ship|image-jc)\.yml@refs/heads/dev"
    r"|Antares-NGSI-LD-Context-Broker/\.github/workflows/full\.yml@refs/tags/v[0-9][0-9a-z.+-]*"
    r")$"
)


# Image paths under OWNER (and their sub-paths) whose repository's image-scan.yml attests what
# its image.yml pushed (T-6154). Antares and the deployment's own images scan in their own lanes.
SCANNED = {
    "joinedcontext-platform": "joinedcontext-platform",  # with /checkouts and /model-tools
    "joinedcontext-portal": "joinedcontext-portal",
    "joinedcontext-portal-ui": "joinedcontext-portal",
    "joinedcontext-app-builder": "joinedcontext-portal",
    "joinedcontext-app-builder-rust": "joinedcontext-portal",
    "jc": "joinedcontext",  # the monorepo's one binary
}
VULN_SCAN = "https://joinedcontext.com/attestations/vuln-scan/v1"


def scanner(image: str, registry: str) -> str | None:
    """The repository whose image-scan.yml must have attested `image` (repo@digest), or None."""
    path = image.split("@", 1)[0].removeprefix(f"{registry}/{OWNER}/")
    return next((repo for prefix, repo in SCANNED.items() if path == prefix or path.startswith(prefix + "/")), None)


def references(render: str, registry: str):
    """(every pinned image of ours as `repo@digest`, every `image:` of ours that is not pinned)."""
    prefix = re.escape(f"{registry}/{OWNER}/")
    pinned = {
        f"{m.group(1)}@{m.group(2)}"
        for m in re.finditer(rf"({prefix}[a-z0-9._/-]+)(?::[\w.-]+)?@(sha256:[0-9a-f]{{64}})", render)
    }
    unpinned = {
        m.group(1)
        for m in re.finditer(rf"""^\s*-?\s*image:\s*["']?({prefix}[^\s"']+)""", render, re.MULTILINE)
        if "@sha256:" not in m.group(1)
    }
    return sorted(pinned), sorted(unpinned)


def cosign_verify(image: str, key: str | None, command=("verify",), identity: str | None = None) -> str | None:
    """None when the signature (or, with `command` verify-attestation, the attestation) holds,
    else the reason cosign gave. `identity` asks for that one signer instead of any of IDENTITY."""
    trust = ["--key", key, "--insecure-ignore-tlog=true"] if key else [
        "--certificate-oidc-issuer", ISSUER,
        *(["--certificate-identity", identity] if identity else ["--certificate-identity-regexp", IDENTITY])]
    for attempt in range(3):
        try:
            run = subprocess.run(["cosign", *command, *trust, image], check=False, capture_output=True, text=True, timeout=120)
        except subprocess.TimeoutExpired:
            return f"cosign {command[0]} did not answer within 120 s"
        if run.returncode == 0:
            return None
        # Parallel cosigns contend for the one local Sigstore trust cache; a locked cache says nothing about
        # the signature, so ask again (deployment ci 40962622414). Any other refusal stands at once.
        if "resource temporarily unavailable" not in run.stderr:
            break
        time.sleep(1 + attempt)
    lines = run.stderr.strip().splitlines()
    return lines[-1] if lines else f"cosign exited {run.returncode}"


def scan_problems(image: str, registry: str, key: str | None) -> list[str]:
    """What the scan record of `image` lacks; empty when its repository has no image-scan.yml."""
    repo = scanner(image, registry)
    if repo is None:
        return []
    identity = f"https://github.com/{OWNER}/{repo}/.github/workflows/image-scan.yml@refs/heads/main"
    found = []
    for kind in ("cyclonedx", VULN_SCAN):
        why = cosign_verify(image, key, ("verify-attestation", "--type", kind), identity)
        if why is not None:
            found.append(f"{image} has no {kind} attestation of {repo}'s image-scan.yml ({why}); production "
                         "takes only a digest that passed the scan: wait for or rerun image-scan.yml on its commit")
    return found


def problems(render: str, registry: str, key: str | None, scanned: bool = False) -> tuple[int, list[str]]:
    pinned, unpinned = references(render, registry)
    found = [f"{image} is not pinned by digest: pin it with scripts/pin-images.py" for image in unpinned]
    with ThreadPoolExecutor(max_workers=8) as pool:
        for image, why in zip(pinned, pool.map(lambda i: cosign_verify(i, key), pinned)):
            if why is not None:
                found.append(
                    f"{image} carries no signature from a {OWNER} workflow ({why}); "
                    "pin a digest the image workflow signed (scripts/pin-images.py)"
                )
        if scanned:
            for more in pool.map(lambda i: scan_problems(i, registry, key), pinned):
                found.extend(more)
    return len(pinned), found


def render(environment: str, out: Path):
    run = subprocess.run([str(ROOT / "scripts/render.sh"), environment, str(out), "--deployed"], check=False, cwd=ROOT)
    if run.returncode != 0:
        sys.exit(f"verify-pinned-images: rendering {environment} failed; run `just _dev-assemble` first")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("environment")
    parser.add_argument("--render", type=Path, help="an existing render of the environment")
    parser.add_argument("--registry", default="ghcr.io")
    parser.add_argument("--key", help="verify against this public key instead of the keyless identity")
    parser.add_argument("--scanned", action="store_true", help="also require the image-scan.yml attestations (production)")
    parser.add_argument("--scoped", action="store_true",
                        help="the render holds only some releases (just dev-apply-pins): naming none of our images is fine")
    args = parser.parse_args()
    if shutil.which("cosign") is None:
        print("verify-pinned-images: cosign is not installed, so no signature can be verified", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as scratch:
        path = args.render or Path(scratch) / f"{args.environment}.yaml"
        if args.render is None:
            render(args.environment, path)
        checked, found = problems(path.read_text(), args.registry, args.key, args.scanned)
    for problem in found:
        print(f"verify-pinned-images: {problem}", file=sys.stderr)
    if found:
        print(f"verify-pinned-images: {len(found)} problem(s) in {args.environment}; nothing was applied", file=sys.stderr)
        return 1
    if checked == 0 and not args.scoped:
        print(f"verify-pinned-images: the {args.environment} render names no {OWNER} image", file=sys.stderr)
        return 1
    print(f"verify-pinned-images: {checked} {OWNER} digest(s) of {args.environment} carry our signature")
    return 0


if __name__ == "__main__":
    sys.exit(main())
