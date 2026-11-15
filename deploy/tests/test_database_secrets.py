"""Every database a component declares has its password generated (T-3189).

`databases.yaml` names the Secret a database's role reads its password from; only a
`secrets.yaml` entry of the same component makes `components/secrets` generate it. jc-assistant
declared `db-assistant` without one, and its pod waited on a Secret nothing creates.
"""
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent


def managed(component: Path) -> set[str]:
    path = component / "secrets.yaml"
    if not path.exists():
        return set()
    names = set()
    for parts in (yaml.safe_load(path.read_text()) or {}).values():
        for secrets in (parts or {}).values():
            names.update(secrets or {})
    return names


def test_every_declared_database_secret_is_generated_by_its_component():
    missing = []
    for databases in sorted(ROOT.glob("components/*/databases.yaml")):
        component = databases.parent
        for name, db in (yaml.safe_load(databases.read_text()) or {}).items():
            secret = (db or {}).get("secret") or {}
            if secret.get("generate") and secret.get("name") not in managed(component):
                missing.append(f"{component.name}/{name}: {secret.get('name')}")
    assert not missing, f"declared but never generated: {missing}"


def test_every_role_password_secret_is_one_cloudnativepg_reloads(rendered):
    """T-4709: CloudNativePG applies a changed role password only from a Secret labelled
    `cnpg.io/reload`. Without the label, a rotated `db-portal` stayed the old password in
    Postgres, the restarted Portal was refused, and ci-full's rotation step timed out."""
    docs = rendered("local")
    roles = {
        (cluster["metadata"]["namespace"], role["passwordSecret"]["name"])
        for cluster in docs
        if cluster.get("kind") == "Cluster" and cluster.get("apiVersion", "").startswith("postgresql.cnpg.io/")
        for role in (cluster.get("spec", {}).get("managed") or {}).get("roles") or []
        if role.get("passwordSecret")
    }
    secrets = {
        (doc["metadata"]["namespace"], doc["metadata"]["name"]): doc["metadata"].get("labels") or {}
        for doc in docs
        if doc.get("kind") == "Secret"
    }
    assert roles, "no Cluster with managed roles rendered"
    unlabelled = sorted(f"{ns}/{name}" for ns, name in roles if secrets.get((ns, name), {}).get("cnpg.io/reload") != "true")
    assert unlabelled == [], f"role password Secrets CloudNativePG never reloads: {unlabelled}"
