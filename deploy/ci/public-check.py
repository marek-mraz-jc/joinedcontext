#!/usr/bin/env python3
"""The public check over a tree, CI's second line (docs/Deployment/01-repository-layout.md §4).

    python3 deploy/ci/public-check.py <path>...

Reads every file git tracks under each path and refuses what may never be public: a global IPv4
address outside the allow-list (public-check.allow beside this file), an age recipient, a sops
value or mapping, a *.tfvars file, and in a .gitleaksignore a sha that is no commit of this
repository. The board's pre-push check (tasks/public-check.sh) applies the same rules to every
outgoing commit before it leaves, plus the private lists (node addresses, personal e-mails, the
old private history) and gitleaks; those lists never enter this repository, so this check has
only the public rules. A refusal names the file, the line and the rule, never the value.
Exit 0 clean, 1 refused, 2 when it cannot check.
"""
import ipaddress
import os
import re
import subprocess
import sys

# A dotted quad that is no part of a longer number (a version 1.2.3.4.5, a clause 4.5.1.2.3); one
# right after a clause word is a specification reference, never an address.
QUAD = re.compile(r"(?<!\w)(?<!\d\.)(\d{1,3}(?:\.\d{1,3}){3})(?!\w)(?!\.\d)")
SPEC = re.compile(r"(?i)(?:§|clause|section|annex|table|figure|rfc\s*\d+)\s*$")
AGE = re.compile(r"\bage1[0-9a-z]{58}\b")
SOPS = re.compile(r"ENC\[[A-Z0-9_]+,data:|^[\s+-]*sops:\s*$")
SHA = re.compile(r"\b[0-9a-f]{40}\b")
# public resolvers and GitHub Pages
ALLOW = ["1.1.1.1/32", "1.0.0.1/32", "8.8.8.8/32", "9.9.9.9/32", "185.199.108.0/22"]


def allow_list(path):
    nets = [ipaddress.ip_network(n) for n in ALLOW]
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            entry = line.split("#", 1)[0].strip()
            if entry:
                nets.append(ipaddress.ip_network(entry.split()[0]))
    return nets


def is_commit(sha):
    return subprocess.run(["git", "cat-file", "-e", f"{sha}^{{commit}}"], capture_output=True).returncode == 0


def verdicts(text, path, allow):
    """The rules a line breaks, as names; never the matched value."""
    found = []
    for m in QUAD.finditer(text):
        try:
            ip = ipaddress.ip_address(m.group(1))
        except ValueError:
            continue
        if ip.is_global and not ip.is_multicast and not any(ip in n for n in allow) and not SPEC.search(text[: m.start()]):
            found.append("a global IPv4 address (no address? add it to deploy/ci/public-check.allow with a reason)")
    if AGE.search(text):
        found.append("an age recipient")
    if SOPS.search(text):
        found.append("a sops value or mapping")
    if os.path.basename(path) == ".gitleaksignore":
        found += ["a .gitleaksignore sha that is no commit of this repository" for sha in SHA.findall(text) if not is_commit(sha)]
    return sorted(set(found))


def check(paths, allow):
    r = subprocess.run(["git", "ls-files", "-z", "--", *paths], capture_output=True)
    if r.returncode != 0:
        print(f"public-check: git ls-files failed: {r.stderr.decode(errors='replace').strip()[:300]}", file=sys.stderr)
        sys.exit(2)
    refusals = []
    for path in r.stdout.decode().split("\0"):
        if not path:
            continue
        if path.endswith(".tfvars"):
            refusals.append(f"{path}: a .tfvars file")
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                for n, line in enumerate(fh, 1):
                    refusals += [f"{path}:{n}: {v}" for v in verdicts(line, path, allow)]
        except (IsADirectoryError, FileNotFoundError):
            continue  # a submodule or a file deleted in the work tree
    return refusals


def main(argv):
    if not argv:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    allow = allow_list(os.path.join(os.path.dirname(os.path.abspath(__file__)), "public-check.allow"))
    refusals = check(argv, allow)
    for r in refusals:
        print(f"public-check: refused: {r}", file=sys.stderr)
    if not refusals:
        print(f"public-check: clean ({' '.join(argv)})", file=sys.stderr)
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
