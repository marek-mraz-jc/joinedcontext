"""T-5113, T-5282: memory requests from measured use on dev and prod, limits never removed.

dev and smartcity (prod) both render the development profile, so one value sizes both
clusters; the request is the higher of the two measurements (the table is in T-5113 and in
docs Operations/02-capacity-and-scaling.md section 4).
"""

from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent

# (kind, workload, container): (memory request, memory limit)
SIZES = {
    ("Deployment", "model-tools", "model-tools-tools"): ("640Mi", "1Gi"),
    ("Deployment", "ckan", "ckan"): ("448Mi", "2Gi"),
    ("StatefulSet", "ckan-solr", "solr"): ("768Mi", "1Gi"),
    ("Deployment", "portal", "portal-portal"): ("576Mi", "1536Mi"),
    ("Deployment", "jc-assistant", "assistant-worker"): ("768Mi", "1Gi"),
    ("StatefulSet", "keycloak-app-keycloakx", "keycloak"): ("928Mi", "2048Mi"),
    ("Deployment", "jc-wasm-host-0", "wasm-host-shard"): ("64Mi", "1Gi"),
    ("Deployment", "context-gateway", "git-sync"): ("128Mi", "256Mi"),
    ("Deployment", "context-gateway", "checkouts"): ("128Mi", "256Mi"),
    ("Deployment", "jc-assistant", "git-sync"): ("64Mi", "256Mi"),
    ("Deployment", "jc-assistant", "checkouts"): ("64Mi", "256Mi"),
    # T-5926: RustFS 1.0.4 has no setting that bounds what grows (anon memory per stored object);
    # the request covers dev's measured working set, the limit is the headroom.
    ("StatefulSet", "artifact-store", "rustfs"): ("832Mi", "1024Mi"),
}

UNITS = {"Ki": 2**10, "Mi": 2**20, "Gi": 2**30}


def mebibytes(quantity: str) -> float:
    for suffix, factor in UNITS.items():
        if quantity.endswith(suffix):
            return float(quantity[: -len(suffix)]) * factor / 2**20
    return float(quantity) / 2**20


def containers(docs: list[dict]):
    for d in docs:
        if d.get("kind") in ("Deployment", "StatefulSet"):
            for c in d["spec"]["template"]["spec"]["containers"]:
                yield d, c


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_measured_requests_and_limits(rendered, env):
    found = {(d["kind"], d["metadata"]["name"], c["name"]): c for d, c in containers(rendered(env))}
    for key, (request, limit) in SIZES.items():
        memory = found[key]["resources"]
        assert (memory["requests"]["memory"], memory["limits"]["memory"]) == (request, limit), key


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_every_container_keeps_a_memory_limit_above_its_request(rendered, env):
    for d, c in containers(rendered(env)):
        memory = c.get("resources", {})
        name = f'{d["metadata"]["name"]}/{c["name"]}'
        assert "memory" in memory.get("limits", {}), name
        assert mebibytes(memory["requests"]["memory"]) <= mebibytes(memory["limits"]["memory"]), name


def test_helm_keeps_three_revisions():
    # 314 releases x helm's default 10 revisions were 3,125 Secrets (133 MiB) on dev, all
    # held in the k3s datastore and the API server's watch cache; a rollback needs the previous one.
    defaults = yaml.safe_load((ROOT / "defaults/helm-defaults.yaml").read_text())["helmDefaults"]
    assert defaults["historyMax"] == 3


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_keycloak_heap_is_capped_below_the_kc_sh_container_default(rendered, env):
    """T-5892: kc.sh's container default (70 % max, 50 % initial) held 1021Mi on a 97 % node."""
    found = {(d["kind"], d["metadata"]["name"], c["name"]): c for d, c in containers(rendered(env))}
    env_vars = {e["name"]: e.get("value") for e in found[("StatefulSet", "keycloak-app-keycloakx", "keycloak")]["env"]}
    assert env_vars["JAVA_OPTS_KC_HEAP"] == "-XX:MaxRAMPercentage=25 -XX:InitialRAMPercentage=5"
