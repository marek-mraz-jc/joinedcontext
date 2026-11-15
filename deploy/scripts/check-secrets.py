#!/usr/bin/env python3
"""Verify the generated bootstrap Secrets in a rendered manifest (T-0018, OPS-37).

    scripts/check-secrets.py <rendered.yaml> [components-dir]

Every Secret the in-cluster generator produces must carry `helm.sh/resource-policy: keep`
(so a `helm uninstall` cannot take the credentials with it) and every generated value must
be at least 32 characters long, which is what the component's own `secrets.yaml` declares.
The check reads those declarations rather than trusting the render, so shortening a
password in a component file fails here instead of quietly weakening a credential.
"""

import base64
import json
import re
import sys
from pathlib import Path

import yaml

MIN_LENGTH = 32
# Confidential Keycloak client secrets are not declared in any component `secrets.yaml`;
# defaults/environment/secrets.yaml.gotmpl derives them from `keycloak-clients.yaml`.
DERIVED_PREFIX = "keycloak-client-"
DERIVED_KEYS = {"client-secret": 32}
# The demo realm logins are derived the same way, from `components/keycloak/demo-users.yaml`
# (development profile only), so their declaration is read from that file rather than from a
# component `secrets.yaml`. The names are read, not matched by prefix: a generated
# `keycloak-user-*` Secret for a user that file does not name still fails.
DEMO_USER_KEYS = {"password": 32}
# A demo person whose realm role carries `mfa-required` has a TOTP seed beside it (T-5120). The
# roles are read from the rendered realm (dev's approvers carry none, T-5776); these when it has none.
MFA_ROLES = {"portal-approver", "platform-admin"}
MFA_SEED_KEYS = {"totp": 32}
# The owner's login of a production realm with dev's seeds (T-4678): declared in
# defaults/environment/secrets.yaml.gotmpl, rendered by smartcity alone.
OWNER_ACCOUNT = {"keycloak-user-initial": {"password": 32}}
# The Decidim administrator of a production realm (T-6448): global.participationStewardId, declared
# in defaults/environment/secrets.yaml.gotmpl, with a TOTP seed where the realm reads one.
PARTICIPATION_STEWARD = "keycloak-user-participacia-steward"
# Every further catalogue (global.catalogues, T-4636) gets the default instance's CKAN Secrets
# under its own name, `<family>-<catalogue>` (defaults/environment/secrets.yaml.gotmpl), so each is
# held to the length components/ckan/secrets.yaml declares for its family.
CATALOGUE_FAMILIES = ("db-ckan-datastore-read", "db-ckan", "ckan-session", "ckan-admin-credentials")
CATALOGUE_NAME = re.compile(r"[a-z]([a-z0-9-]{0,14}[a-z0-9])?")
# Kept Secrets that hold no generated value: the APISIX rule file, which helm seeds with the base
# and the Portal's reconciler then owns (ADR-N-030, AP-112). `keep` stops a sync deleting it once
# helm no longer renders it, not a credential rotation; check-rendered-secrets.py and gitleaks
# still read its content.
COMPOSED = {"apisix-standalone-config"}


def declarations(components_dir: Path) -> dict[str, dict[str, int]]:
    """secret name -> {key: declared length} for every generated key."""
    out: dict[str, dict[str, int]] = {}
    for path in sorted(components_dir.glob("*/secrets.yaml")):
        doc = yaml.safe_load(path.read_text()) or {}
        for parts in doc.values():
            for secrets in parts.values():
                for name, keys in secrets.items():
                    for key, spec in keys.items():
                        if key == "componentNamespaces" or not isinstance(spec, dict):
                            continue
                        if spec.get("generate") and "length" in spec:
                            out.setdefault(name, {})[key] = int(spec["length"])
    return out


def mfa_roles(docs: list[dict]) -> set[str]:
    """The realm roles carrying `mfa-required` in the rendered keycloak-config-cli realm."""
    for doc in docs:
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            for name, text in (doc.get("data") or {}).items():
                if name == "master.json":
                    continue
                roles = json.loads(base64.b64decode(text)).get("roles", {}).get("realm", [])
                return {r["name"] for r in roles
                        if "mfa-required" in ((r.get("composites") or {}).get("realm") or [])}
    return MFA_ROLES


def demo_user_declarations(components_dir: Path, roles: set[str] = MFA_ROLES) -> dict[str, dict[str, int]]:
    """secret name -> {key: declared length} for the demo realm logins."""
    path = components_dir / "keycloak" / "demo-users.yaml"
    if not path.exists():
        return {}
    users = yaml.safe_load(path.read_text()) or {}
    return {
        f"keycloak-user-{name.replace('.', '-')}": DEMO_USER_KEYS
        | (MFA_SEED_KEYS if roles & set((user or {}).get("realmRoles") or []) else {})
        for name, user in users.items()
    }


def main() -> int:
    rendered = Path(sys.argv[1])
    components = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("components")
    docs = [d for d in yaml.safe_load_all(rendered.read_text()) if isinstance(d, dict)]
    steward_keys = DEMO_USER_KEYS | (MFA_SEED_KEYS if "USER_TOTP_PARTICIPACIA_STEWARD" in rendered.read_text() else {})
    declared = (declarations(components) | demo_user_declarations(components, mfa_roles(docs)) | OWNER_ACCOUNT
                | {PARTICIPATION_STEWARD: steward_keys})
    generated = [
        d for d in docs
        if d.get("kind") == "Secret"
        and (d.get("metadata", {}).get("annotations") or {}).get("helm.sh/resource-policy") == "keep"
        and d["metadata"]["name"] not in COMPOSED
    ]

    problems: list[str] = []
    checked = 0
    for secret in generated:
        meta = secret["metadata"]
        where = f"{meta.get('namespace', '-')}/{meta['name']}"
        expected = declared.get(meta["name"])
        if expected is None and meta["name"].startswith(DERIVED_PREFIX):
            expected = DERIVED_KEYS
        if expected is None:
            expected = next((declared.get(family) for family in CATALOGUE_FAMILIES
                             if meta["name"].startswith(family + "-")
                             and CATALOGUE_NAME.fullmatch(meta["name"][len(family) + 1:])), None)
        if expected is None:
            problems.append(f"{where}: generated Secret with no declaration in any components/*/secrets.yaml")
            continue
        data = secret.get("data") or {}
        for key, length in expected.items():
            if key not in data:
                problems.append(f"{where}: declared key '{key}' is missing from the rendered Secret")
                continue
            value = base64.b64decode(data[key])
            checked += 1
            if length < MIN_LENGTH:
                problems.append(f"{where}: key '{key}' is declared {length} characters, below the {MIN_LENGTH} minimum")
            if len(value) != length:
                problems.append(f"{where}: key '{key}' decodes to {len(value)} characters, declared {length}")

    if not generated:
        problems.append(f"{rendered}: no generated Secret found — the secrets component did not render")

    for line in problems:
        print(f"FAIL {line}")
    print(f"{len(generated)} generated Secret(s), {checked} value(s) checked, {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
