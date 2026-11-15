#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Check that every public App the seeds publish answers on its own host (T-5774).

    scripts/smoke-public-apps.py <domain> [--development]

Reads the App manifests the gitea seed commits: components/gitea/apps/*/app.yaml and every
`kind: App` under components/context-gateway/seed/, its sharing matrix test project only with
`--development` (the development profile alone seeds it). Keeps those with `visibility: public`
and `lifecycle: published`, and reads each one's `https://<name>.apps.<domain>/` anonymously.
ko: no answer (an App without an Ingress), a status other than 200, or the Portal's page for an
App host with nothing to serve, which an App without a built bundle answers (prod's ten
Gitea-built Apps, 2026-10-19 to 2026-10-26). Exits 1 on any ko.
"""

import importlib.util
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("smoke_catalogue", Path(__file__).with_name("smoke-catalogue.py"))
smoke = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(smoke)

# The page the Portal answers for an App host it has nothing to serve on (its apps/roles.rs).
NO_BUNDLE = b"This page does not exist in the application"


# The seed the development profile alone carries (values/seed/development-values.yaml.gotmpl).
DEVELOPMENT_ONLY = "components/context-gateway/seed/sharing"


def public_apps(root: Path = ROOT, development: bool = False) -> list[str]:
    """The names of the seeded Apps anybody may open, sorted."""
    files = [*root.glob("components/gitea/apps/*/app.yaml"), *root.glob("components/context-gateway/seed/**/*.yaml")]
    if not development:
        files = [f for f in files if not f.is_relative_to(root / DEVELOPMENT_ONLY)]
    names = set()
    for path in files:
        for doc in yaml.safe_load_all(path.read_text()):
            if not isinstance(doc, dict) or doc.get("kind") != "App":
                continue
            spec = doc.get("spec") or {}
            if spec.get("visibility") == "public" and spec.get("lifecycle") == "published":
                names.add(doc["metadata"]["name"])
    return sorted(names)


def verdict(status: int, body: bytes, error: str = "") -> str:
    if status == 0:
        return f"ko: no answer ({error}); is there an Ingress for the host?"
    if NO_BUNDLE in body:
        return f"ko: {status}, the App has no built bundle on this instance"
    if status != 200:
        return f"ko: {status}"
    return "ok"


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3) or argv[2:] not in ([], ["--development"]):
        print(__doc__, file=sys.stderr)
        return 2
    failed = 0
    for name in public_apps(development=argv[2:] == ["--development"]):
        url = f"https://{name}.apps.{argv[1]}/"
        answer = smoke.send("GET", url, {"Accept": "text/html"}, None)
        line = verdict(answer.status, answer.body, answer.error)
        failed += line != "ok"
        print(f"  {'ok  ' if line == 'ok' else 'FAIL'}  {name}: {line}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
