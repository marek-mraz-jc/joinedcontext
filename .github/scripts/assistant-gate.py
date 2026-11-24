#!/usr/bin/env python3
"""Which pre-production gate a jc range needs (T-7788), and whether a gate verdict admits it.

    assistant-gate.py classify < paths         # changes.sh: prints assistant=true|false
    assistant-gate.py need <from> <to>         # prod-deployer: prints full, needed or none
    assistant-gate.py admit <verdict.json> --digest <sha256:...> [--need full|needed|none]
    assistant-gate.py --selftest

`need` reads `git diff --name-only <from> <to>` (the last prod pin's jc commit to the candidate's)
against .github/full-gate-paths.txt and .github/assistant-gate-paths.txt: `full` when a path is in
the first list (every service's test, the jc-smoke judge run and the assistant gate), `needed`
when it is only in the second (the assistant gate), `none` otherwise. `admit` answers 0 only when
the range needs no gate or the verdict passed for exactly that image digest; it prints why not.
A `full` range also needs ci, every smoke shard and the jc-smoke judge run green on the digest:
that is the deployer's record, named in the refusal, not something this script can read.

A list line is a glob over the repository path: `**` any number of folders (none included), `*`
any characters within one name, a leading `!` takes back what earlier lines matched; `#` starts a
comment. The selftest holds every positive line to at least one tracked file, so a renamed crate
cannot silently drop out of the gate."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ASSISTANT = ROOT / ".github/assistant-gate-paths.txt"
FULL = ROOT / ".github/full-gate-paths.txt"


def pattern(glob: str) -> re.Pattern:
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out, i = out + "(?:[^/]+/)*", i + 3
        elif glob.startswith("**", i):
            out, i = out + ".*", i + 2
        elif glob[i] == "*":
            out, i = out + "[^/]*", i + 1
        else:
            out, i = out + re.escape(glob[i]), i + 1
    return re.compile(out + r"\Z")


def rules(path: Path) -> list[tuple[bool, str, re.Pattern]]:
    """(keep, glob, regex) per line, in order."""
    out = []
    for line in path.read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            keep = not line.startswith("!")
            glob = line.lstrip("!")
            out.append((keep, glob, pattern(glob)))
    return out


def matches(rs, path: str) -> bool:
    hit = False
    for keep, _, rx in rs:
        if rx.match(path):
            hit = keep
    return hit


def need(paths: list[str], full=None, assistant=None) -> str:
    full = rules(FULL) if full is None else full
    assistant = rules(ASSISTANT) if assistant is None else assistant
    if any(matches(full, p) for p in paths):
        return "full"
    if any(matches(assistant, p) for p in paths):
        return "needed"
    return "none"


def changed(base: str, head: str) -> list[str]:
    for ref in (base, head):
        if not re.fullmatch(r"[0-9a-f]{7,40}|[A-Za-z0-9._/~^-]{1,200}", ref) or ref.startswith("-"):
            raise SystemExit(f"not a commit: {ref!r}")
    out = subprocess.run(["git", "-C", str(ROOT), "diff", "--name-only", base, head, "--"],
                         capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"git diff {base} {head}: {out.stderr.strip()} (fetch both commits first)")
    return [p for p in out.stdout.splitlines() if p]


def admit(verdict: dict, digest: str, needs: str) -> list[str]:
    """Why the verdict does not admit the digest for a range that `needs` the gate; empty when it does."""
    if needs == "none":
        return []
    why = []
    if verdict.get("digest") != digest:
        why.append(f"the verdict is for image {verdict.get('digest')!r}, not {digest}: run the gate on this digest")
    if verdict.get("pass") is not True:
        why.append("the gate did not pass: " + "; ".join(verdict.get("why") or ["no reason recorded"]))
    return why


FULL_ALSO = ("a full range also needs ci, every smoke shard and the jc-smoke judge run green on this "
             "digest (0 new fails): confirm them in the deployer's record before prod")


def selftest() -> None:
    tracked = subprocess.run(["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True,
                             check=True).stdout.splitlines()
    for path in (ASSISTANT, FULL):
        for keep, glob, rx in rules(path):
            if keep and not any(rx.match(p) for p in tracked):
                raise SystemExit(f"FAIL: {path.name}: {glob} matches no tracked file")
    cases = {
        "vendor/joinedcontext-platform/crates/assistant/src/chat/routed.rs": "needed",
        "vendor/joinedcontext-platform/crates/assistant/tests/answer_evals_tests.rs": "none",
        "vendor/joinedcontext-platform/crates/assistant/README.md": "none",
        "vendor/joinedcontext-platform/crates/context-gateway/src/mcp/hub_lead.md": "full",
        "crates/antares/crates/antares-ql/src/primer.md": "needed",
        "bins/jc/src/modules/assistant.rs": "full",
        "bins/jc/src/setup.rs": "full",
        "crates/portal/ui/src/pages/knowledge/Deployments.tsx": "needed",
        "crates/portal/ui/src/pages/apps/Apps.tsx": "none",
        "crates/portal/migrations/0100_x.sql": "full",
        "crates/antares/Cargo.lock": "full",
        "Cargo.lock": "full",
        "vendor/joinedcontext-platform/crates/jcctl/fuzz/Cargo.lock": "none",
        "deploy/components/jc/images.yaml": "none",
        "deploy/components/jc/values/server/base-values.yaml.gotmpl": "full",
        "deploy/components/ai/bundle.lock": "full",
        "crates/kaisatsu/core/src/lib.rs": "full",
        "docs/README.md": "none",
        "bins/jc/smoke/assistant-gate/fixture.json": "needed",
    }
    for p, want in cases.items():
        got = need([p])
        if got != want:
            raise SystemExit(f"FAIL: {p} -> {got}, want {want}")
    if need([]) != "none" or need(["docs/a.md", "vendor/viz/site/x.js"]) != "needed":
        raise SystemExit("FAIL: a range is the strongest of its paths")
    if pattern("a/**/b").match("a/b") is None or pattern("a/*").match("a/b/c") is not None:
        raise SystemExit("FAIL: glob")
    ok = {"digest": "sha256:1", "pass": True}
    assert admit({}, "sha256:1", "none") == []
    assert admit(ok, "sha256:1", "needed") == []
    assert any("not sha256:2" in w for w in admit(ok, "sha256:2", "needed"))
    assert any("did not pass: slow" in w for w in admit({"digest": "sha256:1", "pass": False, "why": ["slow"]}, "sha256:1", "needed"))
    assert admit(ok, "sha256:1", "full") == []
    print("assistant-gate.py selftest ok")


def main(argv: list[str]) -> int:
    if argv[:1] == ["--selftest"]:
        selftest()
        return 0
    if argv[:1] == ["classify"]:
        paths = [ln.strip() for ln in sys.stdin if ln.strip()]
        print(f"assistant={'true' if need(paths) != 'none' else 'false'}")
        return 0
    if len(argv) == 3 and argv[0] == "need":
        print(need(changed(argv[1], argv[2])))
        return 0
    if len(argv) >= 4 and argv[0] == "admit" and "--digest" in argv:
        digest = argv[argv.index("--digest") + 1]
        needs = argv[argv.index("--need") + 1] if "--need" in argv else "full"
        if needs not in ("full", "needed", "none"):
            raise SystemExit(f"--need {needs!r}: full, needed or none")
        why = admit(json.loads(Path(argv[1]).read_text()), digest, needs)
        for w in why:
            print(f"refused: {w}")
        if not why:
            print(f"admitted ({needs})")
            if needs == "full":
                print(f"also: {FULL_ALSO}")
        return 1 if why else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
