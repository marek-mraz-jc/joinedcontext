#!/usr/bin/env python3
"""Pin the images of the newest commit a source repository's `main` built (ADR-N-056 §2.4, OPS-57).

    python3 scripts/pin-images.py platform     # or: portal
    python3 scripts/pin-images.py platform --tier production
    PIN_COMMIT=<40 hex> python3 scripts/pin-images.py platform   # a green commit behind the head

For the commit of the newest green `image.yml` run on `main` of the source repository (the lane
builds once an hour, not per push, so the head of `main` usually has no images yet; T-4696) it pins, in every
`components/*/images.yaml`, the digest each image of that commit was published under, but only
when the repository's `ci` and `image.yml` runs on the commit are green and `cosign verify` accepts the signature
the repository's own `image.yml` made on `main`. It writes the files and prints the commit
message; it commits nothing (`.github/workflows/pin-images.yml` renders, commits and pushes).
Exit 0 with nothing written when the pins are current or the commit is not ready yet.
A branch preview image (tag pv-<slug>-<sha12>, T-5547) is never pinned: preview.yml builds and
signs it, and this script reads only image.yml runs, the commit's own tag and image.yml's identity
(tests/test_pins.py, "pv_tags_are_never_pinned").

Tiers (T-4721): `dev` (the default) pins the `dev-<N>` images ship publishes, built and
smoke-tested. `production` pins only a digest that also carries the `full-tests/v1` attestation
the repository's `promote.yml` adds when ci-full passed on it, and refuses any other.

The Portal's commit goes through scripts/pin-portal.sh, which also checks the app builders
against the workflows the seeded applications carry.

    python3 scripts/pin-images.py antares           # pin the newest green ship.yml image of Antares dev
    python3 scripts/pin-images.py antares --judge   # only judge the range; exit 3 names why it is a hand pin

Antares (T-6330) is pinned by machine only when the range from the pinned `commit:` to the commit
of the newest green ship.yml run on dev adds or changes no file under crates/antares-sql/migrations
and adds or drops no ANTARES_ key in docs/src/configuration.md (Antares dev/check-env-docs.sh holds
every key the code reads to a row there). Any other range, or one the GitHub API does not show whole,
stays the deployer's hand pin: each of those records the migrations and keys it brings.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OWNER = "marek-mraz-jc"
ISSUER = "https://token.actions.githubusercontent.com"
# Per source: its repository and the image paths its image.yml publishes under the commit's tag;
# the first is required, the others are pinned when that commit built them. The Portal's commit may
# have built only its UI bundle (a ui/-only push, T-6158): pin-portal.sh then keeps the binary of
# the newest commit before it that built one, when only ui/ changed since.
SOURCES = {
    "platform": ("joinedcontext-platform", ["joinedcontext-platform", "joinedcontext-platform/checkouts",
                                            "joinedcontext-platform/model-tools"]),
    "portal": ("joinedcontext-portal", ["joinedcontext-portal", "joinedcontext-portal-ui", "joinedcontext-app-builder",
                                        "joinedcontext-app-builder-rust"]),
}
DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")


def run(*args, check=True):
    return subprocess.run(args, capture_output=True, text=True, check=check)


def built(repository):
    """The commit of the newest green image.yml run on main."""
    out = run("gh", "run", "list", "-R", f"{OWNER}/{repository}", "--workflow", "image.yml", "--branch", "main",
              "--status", "success", "--json", "headSha", "--limit", "1").stdout
    runs = json.loads(out or "[]")
    commit = runs[0].get("headSha", "") if runs else ""
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        sys.exit(f"{repository}: no green image.yml run on main ({out!r})")
    return commit


def green(repository, workflow, commit):
    """The workflow's newest run on the commit that did not skip finished green.

    A workflow_run trigger from another workflow leaves a skipped run on top of the real one."""
    out = run("gh", "run", "list", "-R", f"{OWNER}/{repository}", "--workflow", workflow, "--commit", commit,
              "--json", "status,conclusion", "--limit", "10").stdout
    runs = [r for r in json.loads(out or "[]") if r["conclusion"] != "skipped"]
    return bool(runs) and runs[0]["status"] == "completed" and runs[0]["conclusion"] == "success"


def published(image, commit):
    """The digest of `image:commit` (or of another tag), or None while there is no such image."""
    result = run("docker", "buildx", "imagetools", "inspect", f"ghcr.io/{OWNER}/{image}:{commit}", check=False)
    digest = next((line.split()[1] for line in result.stdout.splitlines() if line.startswith("Digest:")), None)
    return digest if digest and DIGEST.match(digest) else None


FULL_TESTS = "https://joinedcontext.com/attestations/full-tests/v1"


def attested(repository, image, digest):
    """The digest carries the full-tests attestation of the repository's promote.yml on main."""
    identity = f"https://github.com/{OWNER}/{repository}/.github/workflows/promote.yml@refs/heads/main"
    result = run("cosign", "verify-attestation", "--type", FULL_TESTS, "--certificate-oidc-issuer", ISSUER,
                 "--certificate-identity", identity, f"ghcr.io/{OWNER}/{image}@{digest}", check=False)
    return result.returncode == 0


def signed(repository, image, digest):
    identity = f"https://github.com/{OWNER}/{repository}/.github/workflows/image.yml@refs/heads/main"
    result = run("cosign", "verify", "--certificate-oidc-issuer", ISSUER, "--certificate-identity", identity,
                 f"ghcr.io/{OWNER}/{image}@{digest}", check=False)
    return result.returncode == 0


def pin(text, image, digest, commit=None):
    """`text` with every block naming `ghcr.io/OWNER/image` pinned to `digest`, and how many. A
    block that records its source commit (`commit:`, read by scripts/check-docs-pin.sh) moves it
    to `commit` with the digest."""
    block = rf"(repository: 'ghcr\.io/{OWNER}/{re.escape(image)}'\n(?:    .*\n)*?    "
    text, n = re.subn(block + r"digest: )'sha256:[0-9a-f]{64}'", rf"\g<1>'{digest}'", text)
    if commit:
        text, _ = re.subn(block + r"commit: )'[0-9a-f]{40}'", rf"\g<1>'{commit}'", text)
    return text, n


def written(text, image):
    """The digest the block naming `ghcr.io/OWNER/image` pins in `text`, or None."""
    found = re.search(rf"repository: 'ghcr\.io/{OWNER}/{re.escape(image)}'\n(?:    .*\n)*?    digest: '(sha256:[0-9a-f]{{64}})'",
                      text)
    return found.group(1) if found else None


def pin_files(pins, commit=None):
    """Pin every images.yaml; the paths that changed."""
    changed = []
    for path in sorted(ROOT.glob("components/*/images.yaml")):
        text = original = path.read_text()
        for image, digest in pins.items():
            text, _ = pin(text, image, digest, commit)
        if text != original:
            path.write_text(text)
            changed.append(path)
    return changed


def main(source, tier="dev"):
    repository, images = SOURCES[source]
    # PIN_COMMIT pins another green commit (one the lane built behind the newest); the
    # signature check below still requires the image.yml of `main` to have signed its images.
    commit = os.environ.get("PIN_COMMIT") or built(repository)
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        sys.exit(f"{source}: PIN_COMMIT must be a full 40-hex commit, not {commit!r}")
    if not green(repository, "ci.yml", commit):
        print(f"{source} {commit[:7]}: ci is not green yet; nothing pinned", file=sys.stderr)
        return
    # A red or unfinished image.yml may have published some images and not others: wait for it
    # like an unfinished ci, never pin half a commit.
    if not green(repository, "image.yml", commit):
        print(f"{source} {commit[:7]}: image.yml is not green (red or still running); nothing pinned", file=sys.stderr)
        return
    pins = {}
    for n, image in enumerate(images):
        digest = published(image, commit)
        if digest is None:
            if n == 0 and source != "portal":
                print(f"{source} {commit[:7]}: {image} is not published yet; nothing pinned", file=sys.stderr)
                return
            continue
        if not signed(repository, image, digest):
            sys.exit(f"{source} {commit[:7]}: {image}@{digest} carries no signature of {repository}'s image.yml on main")
        if tier == "production" and not attested(repository, image, digest):
            sys.exit(f"{source} {commit[:7]}: {image}@{digest} has not passed the full tests (no full-tests "
                     f"attestation of {repository}'s promote.yml); a production tier pins only promoted images")
        pins[image] = digest
    if source == "portal" and not pins.keys() & {"joinedcontext-portal", "joinedcontext-portal-ui"}:
        print(f"{source} {commit[:7]}: neither the portal nor its UI bundle is published yet; nothing pinned",
              file=sys.stderr)
        return
    if source == "portal":
        # The builder contract check and the builders' own comment line are pin-portal.sh's.
        refused = subprocess.run([str(ROOT / "scripts/pin-portal.sh"), commit], cwd=ROOT, check=False)
        if refused.returncode != 0:
            # Whatever it wrote before it stopped goes back: a pin is all three images or none.
            subprocess.run(["git", "checkout", "--", "components"], cwd=ROOT, check=False)
            print(f"{source} {commit[:7]}: pin-portal.sh refused (exit {refused.returncode}); nothing pinned")
            return
        # pin-portal.sh may take the binary or the bundle of an earlier commit than the one checked
        # above (T-6050, T-6158): whatever it wrote passes the same signature and tier checks.
        text = (ROOT / "components/portal/images.yaml").read_text()
        for image in ("joinedcontext-portal", "joinedcontext-portal-ui"):
            digest = written(text, image)
            problem = None
            if digest is None:
                problem = "no digest pinned"
            elif not signed(repository, image, digest):
                problem = f"{image}@{digest} carries no signature of {repository}'s image.yml on main"
            elif tier == "production" and not attested(repository, image, digest):
                problem = f"{image}@{digest} has not passed the full tests (no full-tests attestation)"
            if problem:
                subprocess.run(["git", "checkout", "--", "components"], cwd=ROOT, check=False)
                sys.exit(f"{source} {commit[:7]}: {problem}; nothing pinned")
    elif not pin_files(pins, commit):
        print(f"{source} {commit[:7]}: already pinned", file=sys.stderr)
        return
    print(f"pin {source} {commit[:7]} (ci green, signed by {repository} image.yml on main)")


ANTARES = "Antares-NGSI-LD-Context-Broker"
ANTARES_IMAGE = "antares-broker-jc"
ANTARES_SIGNER = f"https://github.com/{OWNER}/{ANTARES}/.github/workflows/ship.yml@refs/heads/dev"
MIGRATIONS = "crates/antares-sql/migrations/"
KEY_DOC = "docs/src/configuration.md"
KEY = re.compile(r"ANTARES_[A-Z0-9_]*[A-Z0-9]")
COMPARE_FILES_CAP = 300  # the compare API lists at most 300 files: a range that long is not read whole


def judge(compare, keys_before, keys_after):
    """Why the Antares range in `compare` (the GitHub compare answer) needs a hand pin, or None.
    `keys_*` are the ANTARES_ keys documented at either end, None when that text could not be read."""
    if not isinstance(compare, dict) or compare.get("status") != "ahead":
        return f"dev is not ahead of the pinned commit (compare status {compare.get('status') if isinstance(compare, dict) else '?'})"
    files = compare.get("files")
    if not isinstance(files, list) or len(files) >= COMPARE_FILES_CAP:
        return "the range is too long to read whole"
    names = [f.get("filename", "") + "\0" + f.get("previous_filename", "") for f in files if isinstance(f, dict)]
    if len(names) != len(files):
        return "the range lists a file it does not name"
    migrations = sorted({n for pair in names for n in pair.split("\0") if n.startswith(MIGRATIONS)})
    if migrations:
        return "a migration: " + ", ".join(m.removeprefix(MIGRATIONS) for m in migrations)
    if keys_before is None or keys_after is None:
        return f"{KEY_DOC} could not be read at both ends"
    added, dropped = sorted(keys_after - keys_before), sorted(keys_before - keys_after)
    if added or dropped:
        return "runtime keys: " + ", ".join([f"+{k}" for k in added] + [f"-{k}" for k in dropped])
    return None


def antares_keys(commit):
    result = run("gh", "api", f"repos/{OWNER}/{ANTARES}/contents/{KEY_DOC}?ref={commit}",
                 "-H", "Accept: application/vnd.github.raw", check=False)
    return set(KEY.findall(result.stdout)) if result.returncode == 0 and result.stdout else None


def antares_shipped():
    """(commit, run id, run number) of the newest green ship.yml run on Antares dev."""
    out = run("gh", "run", "list", "-R", f"{OWNER}/{ANTARES}", "--workflow", "ship.yml", "--branch", "dev",
              "--status", "success", "--json", "headSha,databaseId,number", "--limit", "1").stdout
    runs = json.loads(out or "[]")
    shipped = runs[0] if runs else {}
    commit, run_id, number = shipped.get("headSha", ""), shipped.get("databaseId"), shipped.get("number")
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or not isinstance(run_id, int) or not isinstance(number, int):
        sys.exit(f"antares: no green ship.yml run on dev ({out!r})")
    return commit, run_id, number


def antares_pinned(text):
    """(commit, tag) the broker block of components/context-broker/images.yaml records, or Nones."""
    block = rf"repository: 'ghcr\.io/{OWNER}/{ANTARES_IMAGE}'\n(?:    .*\n)*?    "
    commit = re.search(block + r"commit: '([0-9a-f]{40})'", text)
    tag = re.search(block + r"tag: '([\w.-]+)'", text)
    return (commit.group(1) if commit else None), (tag.group(1) if tag else None)


def antares_signed(digest, commit):
    """ship.yml on dev signed `digest`, and signed it in the run of `commit`."""
    result = run("cosign", "verify", "--certificate-oidc-issuer", ISSUER, "--certificate-identity", ANTARES_SIGNER,
                 "--certificate-github-workflow-sha", commit, f"ghcr.io/{OWNER}/{ANTARES_IMAGE}@{digest}", check=False)
    return result.returncode == 0


def antares(judge_only=False):
    path = ROOT / "components/context-broker/images.yaml"
    text = path.read_text()
    old, old_tag = antares_pinned(text)
    commit, run_id, number = antares_shipped()
    short = commit[:7]
    if old is None or old_tag is None:
        print(f"antares {short}: hand pin: the broker block records no commit: or tag:", file=sys.stderr)
        if judge_only:
            sys.exit(3)
        return
    if old == commit:
        print(f"antares {short}: already pinned", file=sys.stderr)
        return
    result = run("gh", "api", f"repos/{OWNER}/{ANTARES}/compare/{old}...{commit}", check=False)
    try:
        compare = json.loads(result.stdout) if result.returncode == 0 else None
    except json.JSONDecodeError:
        compare = None
    reason = judge(compare, antares_keys(old), antares_keys(commit))
    if reason:
        print(f"antares {old[:7]}..{short} (ship.yml run {run_id}): hand pin: {reason}", file=sys.stderr)
        if judge_only:
            sys.exit(3)
        return
    if judge_only:
        print(f"antares {old[:7]}..{short}: no migration, no new key")
        return
    tag = f"dev-{number}"
    digest = published(ANTARES_IMAGE, tag)
    if digest is None:
        print(f"antares {short}: {tag} is not published yet; nothing pinned", file=sys.stderr)
        return
    if not antares_signed(digest, commit):
        sys.exit(f"antares {short}: {ANTARES_IMAGE}@{digest} carries no signature of ship.yml on dev at {short}")
    tasks = sorted({t for c in compare.get("commits", []) if isinstance(c, dict)
                    for t in re.findall(r"\bT-\d{4}\b", (c.get("commit") or {}).get("message", ""))},
                   key=lambda t: int(t[2:]))
    done = ", ".join(tasks) or f"{compare.get('ahead_by', '?')} commits"
    note = (f"    # Antares dev {short} ({tag}, over {old[:7]}: {done}). No migration, no new runtime key\n"
            f"    # (pinned by pin-images, T-6330). Rollback: {old_tag}. Signed by ship.yml on dev at {short} (run {run_id}).\n")
    block = rf"(repository: '{re.escape(f'ghcr.io/{OWNER}/{ANTARES_IMAGE}')}'\n(?:    .*\n)*?)(    tag: )'[\w.-]+'"
    text, n = re.subn(block, lambda m: f"{m.group(1)}{note}{m.group(2)}'{tag}'", text, count=1)
    text, _ = pin(text, ANTARES_IMAGE, digest, commit)
    if n != 1 or written(text, ANTARES_IMAGE) != digest or antares_pinned(text) != (commit, tag):
        sys.exit(f"antares {short}: the broker block did not take the pin; nothing pinned")
    path.write_text(text)
    print(f"pin Antares {short} ({tag}: {done}; no migration, no new key; signed by ship.yml run {run_id}, "
          f"cosign verified)")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["antares"] and args[1:] in ([], ["--judge"]):
        antares(judge_only=args[1:] == ["--judge"])
        sys.exit(0)
    if args[1:2] == ["--tier"] and len(args) == 3 and args[2] in ("dev", "production"):
        tier = args.pop()
        args.pop()
    else:
        tier = "dev"
    if len(args) != 1 or args[0] not in SOURCES:
        sys.exit(f"usage: pin-images.py {{{'|'.join(SOURCES)}}} [--tier dev|production] | antares [--judge]")
    main(args[0], tier)
