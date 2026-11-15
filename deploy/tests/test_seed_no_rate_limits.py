"""No seed endpoint carries a rate limit nobody chose (T-2775, EP-20).

An endpoint is not limited unless a person sets `spec.rateLimits`, and an App's endpoint unless
its author sets `spec.limits.requestsPerMinute`. The seed is nobody's choice, so it sets neither:
the demo endpoints answer without `RateLimit-*` fields, which is what a fresh endpoint does.
A project's `quotas.requestsPerMinute` stays: it is a ceiling on a limit somebody sets.
"""

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SEEDS = [ROOT / "components/context-gateway/seed", ROOT / "components/gitea/apps"]


def seed_manifests():
    for seed in SEEDS:
        for path in sorted(seed.rglob("*.yaml")):
            if path.name.endswith((".linkml.yaml", "-bento.yaml")) or path.name == "index.yaml":
                continue
            for doc in yaml.safe_load_all(path.read_text()):
                if isinstance(doc, dict) and doc.get("kind") in ("Endpoint", "App"):
                    yield path.relative_to(ROOT), doc


def test_no_seed_endpoint_or_app_sets_a_rate_limit():
    limited = []
    seen = 0
    for path, doc in seed_manifests():
        seen += 1
        spec = doc.get("spec") or {}
        if doc["kind"] == "Endpoint" and "rateLimits" in spec:
            limited.append(f"{path}: spec.rateLimits {spec['rateLimits']}")
        if doc["kind"] == "App" and "requestsPerMinute" in (spec.get("limits") or {}):
            limited.append(f"{path}: spec.limits.requestsPerMinute {spec['limits']['requestsPerMinute']}")
    assert seen > 20, f"only {seen} seed endpoints and apps found: the seed moved, fix SEEDS"
    assert limited == [], "a seed sets a limit nobody chose:\n" + "\n".join(limited)
