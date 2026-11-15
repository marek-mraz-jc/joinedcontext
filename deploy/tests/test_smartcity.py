"""T-4678: the production instance smartcity carries dev's seeds and none of dev's test objects.

smartcity renders the production profile with dev's values files (`global.valuesProfile`), so it
holds the cities' projects, pipelines and Apps that `test_production_is_not_dev.py` reports as a
demo for the generic production example. This module runs that file's other detectors as they
are and replaces the three that a city seed trips by what they stand for: no demo account and no
binding to one, no eval or conformance object, no demo feed, and placeholder hosts only inside an
App's own test fixtures. It also proves the production guard of the recipes refuses dev.
"""

import base64
import json
import re
import shutil
import subprocess
from datetime import datetime, timezone

import pytest

from test_dev_recipes import DEV_SERVER, NODES, PROJECT_ROOT, guard_env
from test_keycloak_people import SMARTCITY_OWNER_EMAIL
from test_production_is_not_dev import DETECTORS, name_of, strings

PROD_SERVER = f"https://{NODES['smartcity']}:6443"
DOMAIN = "smartcity.joinedcontext.com"
# The detectors a city seed trips by design; each is replaced by a narrower test below.
REPLACED = {"demo_accounts_and_feeds", "demo_people", "placeholder_domains"}


@pytest.fixture(scope="module")
def smartcity(rendered):
    return rendered("smartcity")


def realm(docs) -> dict:
    """The realm keycloak-config-cli imports, from its Secret."""
    secret = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"].endswith("config-cli-config-realms"))
    return json.loads(base64.b64decode(secret["data"]["smartcity.json"]))


def config_maps(docs, prefix=""):
    return [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith(prefix)]


@pytest.mark.parametrize("detector", sorted(set(DETECTORS) - REPLACED))
def test_smartcity_renders_nothing_of_dev(detector, smartcity):
    assert DETECTORS[detector](smartcity) == []


def test_no_demo_account_exists_or_is_bound(smartcity):
    secrets = [name_of(d) for d in smartcity if (d.get("metadata") or {}).get("name", "").startswith("keycloak-user-demo")]
    bindings = sorted(
        f"{name_of(d)}: {key}"
        for d in config_maps(smartcity, "gitea-")
        for key, text in (d.get("data") or {}).items()
        if re.search(r"(?m)^\s*-\s*(user|username|email):\s*demo\.[a-z]+@", text)
    )
    realm_users = sorted(u["username"] for u in realm(smartcity)["users"] if u["username"].startswith("demo."))
    assert (secrets, bindings, realm_users) == ([], [], [])


def people_ids(docs):
    """username -> id, as the keycloak people Job creates them (charts/people)."""
    data = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "keycloak-user-ids")
    return {line.split()[1]: line.split()[0] for line in data["data"]["users"].splitlines() if line.strip()}


def test_the_owner_is_the_realm_account_and_holds_the_stewards_bindings(smartcity):
    owner = SMARTCITY_OWNER_EMAIL
    humans = [u for u in realm(smartcity)["users"] if not u["username"].startswith("service-account-")]
    # Beside the owner: the Decidim administrator, who holds no right (test_decidim_component,
    # T-6448), and the presenter (tests/test_presenter.py, T-6542).
    assert [u["username"] for u in humans] == ["participacia.steward@hel.fi", "presenter@hel.fi", owner]
    presenter = humans[1]
    humans = humans[2:]
    assert humans[0]["credentials"][0]["value"] == "$(USER_PASSWORD_INITIAL)"
    # keycloak-config-cli fails the whole import on a membership of a group the realm lacks.
    groups = {g["path"] for g in realm(smartcity)["groups"]}
    assert set(humans[0]["groups"]) <= groups and set(presenter["groups"]) <= groups
    admins = [
        text for d in config_maps(smartcity, "gitea-bootstrap")
        for key, text in (d.get("data") or {}).items() if key == "users__assignments__admins.yaml"
    ]
    # The seeds name the steward by id; on this instance that is the owner's (PF-126).
    owner_id = people_ids(smartcity)[owner]
    assert admins and all(owner_id in text for text in admins)


def test_no_eval_or_conformance_object_is_seeded(smartcity):
    keys = [key for d in config_maps(smartcity, "gitea-") for key in (d.get("data") or {})]
    tests = [k for k in keys if re.search(r"__(deployments|sources)__[^_]*eval[^/]*\.yaml$|__serviceaccounts__conformance\.yaml$|__policies__conformance-", k)]
    clients = [name_of(d) for d in smartcity for _, s in strings(d) if "banskabystrica-conformance" in s]
    feeds = [name_of(d) for d in smartcity
             if ((d.get("metadata") or {}).get("labels") or {}).get("app.kubernetes.io/instance", "").startswith("demo-feeds")]
    assert (tests, sorted(set(clients)), feeds) == ([], [], [])
    # The filter keeps the cities' own assistants and sources: it is not a blanket drop.
    assert any(k.endswith("__assistant__deployments__data.yaml") for k in keys)


def test_placeholder_hosts_only_inside_an_apps_own_test_fixtures(smartcity):
    outside = [d for d in smartcity if not name_of(d).startswith("ConfigMap/smartcity/gitea-sample-apps-")]
    assert DETECTORS["placeholder_domains"](outside) == []


def test_the_instance_is_its_own_host_on_traefik_with_production_certificates(smartcity):
    ingresses = [d for d in smartcity if d.get("kind") == "Ingress"]
    assert ingresses
    assert {d["spec"].get("ingressClassName") for d in ingresses} == {"traefik"}
    issuers = {d["spec"]["issuerRef"]["name"] for d in smartcity if d.get("kind") == "Certificate"}
    assert issuers == {"letsencrypt-prod"}
    hosts = {r["host"] for d in ingresses for r in d["spec"].get("rules", [])}
    assert {DOMAIN, f"idm.{DOMAIN}", f"opendata.{DOMAIN}", f"data.{DOMAIN}"} <= hosts
    assert all(h == DOMAIN or h.endswith("." + DOMAIN) for h in hosts)


def test_the_old_catalogue_host_answers_301_to_the_same_path(smartcity):
    routes = "\n".join(s for d in config_maps(smartcity, "apisix-standalone") for _, s in strings(d))
    assert "https://opendata.${DOMAIN}$request_uri" in routes or f"https://opendata.{DOMAIN}$request_uri" in routes


def test_security_txt_expires_in_the_future():
    text = (PROJECT_ROOT / "ci/example-deployments/environments/smartcity/global.yaml.gotmpl").read_text()
    expires = re.search(r'expires: "([^"]+)"', text).group(1)
    assert datetime.fromisoformat(expires.replace("Z", "+00:00")) > datetime.now(timezone.utc)


def run_prod_guard(tmp_path, server):
    env = guard_env(tmp_path, server)
    return subprocess.run(["just", "_prod-guard"], cwd=PROJECT_ROOT, env=env, capture_output=True, text=True, check=False)


@pytest.mark.skipif(shutil.which("just") is None or shutil.which("kubectl") is None, reason="just and kubectl")
@pytest.mark.parametrize("server", [DEV_SERVER, "https://203.0.113.2:6443", "https://1203.0.113.20:6443"])
def test_the_prod_guard_refuses_dev_and_lookalikes(tmp_path, server):
    result = run_prod_guard(tmp_path, server)
    assert result.returncode != 0 and "refusing" in result.stderr


@pytest.mark.skipif(shutil.which("just") is None or shutil.which("kubectl") is None, reason="just and kubectl")
def test_the_prod_guard_accepts_the_production_cluster(tmp_path):
    result = run_prod_guard(tmp_path, PROD_SERVER)
    assert result.returncode == 0, result.stderr


def test_postgres_is_dumped_nightly_and_the_dump_job_may_reach_it(smartcity):
    """No object store yet, so a nightly pg_dumpall onto the node is the backup (T-4678)."""
    jobs = [d for d in smartcity if d.get("kind") == "CronJob" and d["metadata"]["name"] == "postgres-cluster-local-backup"]
    assert len(jobs) == 1
    pod = jobs[0]["spec"]["jobTemplate"]["spec"]["template"]
    script = pod["spec"]["containers"][0]["command"][-1]
    assert "pg_dumpall" in script and "exit 1" in script
    labels = pod["metadata"]["labels"]
    selected = [
        d for d in smartcity if d.get("kind") == "NetworkPolicy"
        and "Egress" in d["spec"].get("policyTypes", [])
        and (d["spec"]["podSelector"].get("matchLabels") or {}).items() <= labels.items()
        and any(p.get("port") == 4143 for rule in d["spec"].get("egress", []) for p in rule.get("ports", []))
    ]
    assert selected, "no egress policy lets the dump job's proxy reach postgres on 4143"
    # local-path binds the volume only for a pod that mounts it: a Job of the apply does, or the
    # release waits on a Pending PVC until the night (prod apply 2026-10-19).
    now = [d for d in smartcity if d.get("kind") == "Job" and d["metadata"]["name"] == "postgres-cluster-local-backup-now"]
    assert len(now) == 1 and now[0]["spec"]["ttlSecondsAfterFinished"] > 0
    claims = {v["persistentVolumeClaim"]["claimName"] for v in now[0]["spec"]["template"]["spec"]["volumes"] if "persistentVolumeClaim" in v}
    assert claims == {"postgres-cluster-local-backup"}



def test_apps_db_is_dumped_nightly_and_only_its_dump_job_reaches_it(smartcity):
    """apps-db has no object store either: the same nightly dump, of apps-db, through a policy
    that names apps-db alone (F-DEP1-018)."""
    jobs = [d for d in smartcity if d.get("kind") == "CronJob" and d["metadata"]["name"] == "apps-db-local-backup"]
    assert len(jobs) == 1
    pod = jobs[0]["spec"]["jobTemplate"]["spec"]["template"]
    env = {e["name"]: e for e in pod["spec"]["containers"][0]["env"]}
    assert env["PGHOST"]["value"].startswith("apps-db-rw.")
    assert env["PGPASSWORD"]["valueFrom"]["secretKeyRef"]["name"] == "apps-db-superuser"
    labels = pod["metadata"]["labels"]
    policies = [d for d in smartcity if d.get("kind") == "NetworkPolicy"]

    def selects(d):
        return (d["spec"]["podSelector"].get("matchLabels") or {}).items() <= labels.items()

    egress = [peer["podSelector"]["matchLabels"]
              for d in policies if selects(d) and "Egress" in d["spec"].get("policyTypes", [])
              for rule in d["spec"].get("egress", []) for peer in rule.get("to", []) if "podSelector" in peer]
    assert {"cnpg.io/cluster": "apps-db"} in egress
    assert not any(peer.get("cnpg.io/cluster") == "postgres-cluster" for peer in egress), \
        "the apps-db dump job may reach the platform's cluster"
    admitted = [d for d in policies
                if d["spec"]["podSelector"].get("matchLabels") == {"cnpg.io/cluster": "apps-db"}
                and any(peer.get("podSelector", {}).get("matchLabels", {}).items() <= labels.items()
                        and peer.get("podSelector", {}).get("matchLabels")
                        for rule in d["spec"].get("ingress", []) for peer in rule.get("from", []))]
    assert admitted, "apps-db admits no connection from its dump job"
    now = [d for d in smartcity if d.get("kind") == "Job" and d["metadata"]["name"] == "apps-db-local-backup-now"]
    assert len(now) == 1


def test_the_portal_syncs_every_2_minutes(smartcity):
    """Each run republishes the catalogue through the public edge (T-4717), and the timer is the
    only sync on this instance (no forge webhook secret), so neither 60 s nor 15 min."""
    portal = next(d for d in smartcity if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    env = {e["name"]: e.get("value") for e in portal["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_PORTAL_SYNC_INTERVAL"] == "120"

# smartcity's copies of dev's environment files, which differ from dev's in the domain alone.
# global and portal carry the instance's own choices and are tested above instead; the files
# both hold byte for byte live once in environments/_layers/hosted (T-4956,
# tests/test_values_schemas.py).
COPIES = ["ckan"]


@pytest.mark.parametrize("name", COPIES)
def test_the_environment_copies_differ_from_dev_in_the_domain_alone(name):
    """A copy edited on one side drifts silently: T-4632's api.{domain} reached dev's assistant
    prompt and not prod's (found by scripts/parity.py, 2026-10-19)."""
    environments = PROJECT_ROOT / "ci/example-deployments/environments"
    dev = (environments / f"dev/{name}.yaml.gotmpl").read_text()
    copy = (environments / f"smartcity/{name}.yaml.gotmpl").read_text()
    expected = re.sub(r"data\.dev\b", "data.smartcity", dev.replace("dev.joinedcontext.com", DOMAIN))
    assert copy == expected, f"smartcity/{name}.yaml.gotmpl: regenerate it from dev's with the domain replaced"
