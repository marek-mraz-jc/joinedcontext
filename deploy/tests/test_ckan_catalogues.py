"""A further open-data catalogue is values alone (T-4636).

Dev rendered with `global.catalogues: [example-city]` and components/ckan/values/example-city/
example-environment.yaml: the second CKAN is a release of its own whose objects, Secrets, PVCs,
databases and edge host share nothing with the default instance's, its pods are fenced to their own
Solr and Redis, its assistant seed comes from its block, and the render passes kubeconform. A bad
entry stops the render.
"""

import shutil
import subprocess

import pytest
import yaml

from conftest import PROJECT_ROOT, TREE_IGNORE, load_yaml_all, set_global

EXAMPLE = [{"name": "example-city", "subDomain": "mesto"}]
DEFAULT, OTHER = "ckan-catalogue", "ckan-example-city"


def render(tmp_path_factory, catalogues, env="dev") -> subprocess.CompletedProcess:
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    if not (PROJECT_ROOT / f"deployment/environments/{env}").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    tree = tmp_path_factory.mktemp("catalogues") / "repo"
    shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
    set_global(tree, "catalogues", catalogues)
    shutil.copy(tree / "components/ckan/values/example-city/example-environment.yaml",
                tree / f"deployment/environments/{env}/zz-example-city.yaml.gotmpl")
    out = tree / f"{env}.yaml"
    result = subprocess.run([str(tree / "scripts/render.sh"), env, str(out)], cwd=str(tree),
                            capture_output=True, text=True)
    result.out = out
    return result


@pytest.fixture(scope="module")
def rendered_file(tmp_path_factory):
    result = render(tmp_path_factory, EXAMPLE)
    assert result.returncode == 0, result.stderr[-3000:]
    return result.out


@pytest.fixture(scope="module")
def docs(rendered_file):
    return [d for d in load_yaml_all(rendered_file.read_text()) if isinstance(d, dict)]


def instance_of(doc):
    """The CKAN release an object belongs to, by its pod template's or its own instance label."""
    meta = doc.get("spec", {}).get("template", {}).get("metadata", doc["metadata"])
    return (meta.get("labels") or {}).get("app.kubernetes.io/instance")


def owned(docs, release):
    return [d for d in docs if instance_of(d) == release]


def pods(docs, release):
    return [d for d in owned(docs, release) if d["kind"] in ("Deployment", "StatefulSet")]


def secret_refs(workloads):
    names = set()
    for w in workloads:
        spec = w["spec"]["template"]["spec"]
        for c in spec.get("containers", []) + spec.get("initContainers", []):
            for e in c.get("env", []):
                ref = (e.get("valueFrom") or {}).get("secretKeyRef")
                if ref:
                    names.add(ref["name"])
        names |= {v["secret"]["secretName"] for v in spec.get("volumes", []) if "secret" in v}
    return names


def claims(docs, release):
    names = {d["metadata"]["name"] for d in owned(docs, release) if d["kind"] == "PersistentVolumeClaim"}
    for s in owned(docs, release):
        if s["kind"] == "StatefulSet":
            names |= {f"{t['metadata']['name']}-{s['metadata']['name']}-0"
                      for t in s["spec"].get("volumeClaimTemplates", [])}
    return names


def test_the_second_instance_is_a_release_of_its_own(docs):
    for release in (DEFAULT, OTHER):
        kinds = {d["kind"] for d in pods(docs, release)}
        assert kinds == {"Deployment", "StatefulSet"}, f"{release}: CKAN, Redis and Solr"
    default = {(d["kind"], d["metadata"]["name"]) for d in owned(docs, DEFAULT)}
    other = {(d["kind"], d["metadata"]["name"]) for d in owned(docs, OTHER)}
    assert other and not default & other
    assert all(name.startswith(OTHER) for _, name in other), sorted(other)


def test_secrets_and_claims_are_disjoint_and_rendered(docs):
    default, other = secret_refs(pods(docs, DEFAULT)), secret_refs(pods(docs, OTHER))
    assert {"db-ckan-example-city", "db-ckan-datastore-read-example-city",
            "ckan-session-example-city", "ckan-admin-credentials-example-city"} <= other
    assert not default & other, default & other
    secrets = {d["metadata"]["name"] for d in docs if d["kind"] == "Secret"}
    assert other - secrets == set(), "every Secret the instance reads is generated"
    assert claims(docs, DEFAULT) and claims(docs, OTHER)
    assert not claims(docs, DEFAULT) & claims(docs, OTHER)


def test_each_instance_has_its_own_databases_and_host(docs):
    env = {e["name"]: e.get("value") for d in pods(docs, OTHER) if d["metadata"]["name"] == OTHER
           for e in d["spec"]["template"]["spec"]["containers"][0].get("env", [])}
    assert env["CKAN_SITE_URL"] == "https://mesto.dev.joinedcontext.com"
    assert env["CKAN_SQLALCHEMY_URL"].endswith("/ckan_example_city")
    assert env["CKAN_DATASTORE_READ_URL"].startswith("postgresql://ckan_datastore_read_example_city:")
    assert f"//{OTHER}-solr." in env["CKAN_SOLR_URL"] and f"//{OTHER}-redis." in env["CKAN_REDIS_URL"]
    databases = {d["metadata"]["name"] for d in docs if d["kind"] == "Database"}
    assert {"ckan-example-city", "ckan-datastore-example-city"} <= databases
    hosts = {r["host"] for d in docs if d["kind"] == "Ingress" for r in d["spec"]["rules"]}
    assert {"opendata.dev.joinedcontext.com", "mesto.dev.joinedcontext.com"} <= hosts
    certificate = [d for d in docs if d["kind"] == "Certificate" and d["metadata"]["name"] == "apisix-edge"][0]
    assert "mesto.dev.joinedcontext.com" in certificate["spec"]["dnsNames"], "HTTP-01, one name each"


def test_its_pods_reach_their_own_solr_and_redis_alone(docs):
    """The copied policies select the instance's release; none lets one instance's pods reach the
    other's Solr or Redis."""
    policies = [d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].startswith(OTHER)]
    assert policies
    for p in policies:
        text = yaml.safe_dump(p["spec"])
        assert DEFAULT not in text, p["metadata"]["name"]
    solr = [p for p in policies if p["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/component") == "solr"]
    assert solr and all(p["spec"]["podSelector"]["matchLabels"]["app.kubernetes.io/instance"] == OTHER for p in solr)


def test_its_theme_and_assistant_come_from_its_block(docs):
    theme = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == f"{OTHER}-theme"][0]
    branding = yaml.safe_load([v for k, v in theme["data"].items() if "siteTitle" in v][0])
    assert branding["siteTitle"]["en"] == "Example City open data"
    assert branding["samples"]["en"] == ["Which car parks near the station have free spaces now?"]
    seeds = {k: v for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"].startswith("gitea-seed-")
             for k, v in (d.get("data") or {}).items()}
    assistant = [yaml.safe_load(v) for v in seeds.values() if "publicId: example-city-data" in v]
    assert len(assistant) == 1
    spec = assistant[0]["spec"]
    assert assistant[0]["metadata"]["namespace"] == "example-city"
    assert spec["allowedOrigins"] == ["https://mesto.dev.joinedcontext.com"]
    assert spec["scope"]["projects"] == [] and spec["sources"] == ["catalogue"], "its own spaces alone"
    assert spec["theme"]["title"] == {"en": "Example City open data", "sk": "Otvorené dáta Example City"}


def test_the_render_passes_kubeconform(rendered_file):
    if shutil.which("kubeconform") is None:
        pytest.skip("kubeconform not installed")
    result = subprocess.run(["kubeconform", "-strict", "-ignore-missing-schemas", "-summary", str(rendered_file)],
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stdout[-3000:] + result.stderr[-3000:]


def test_its_generated_secrets_meet_the_declared_lengths(rendered_file):
    """scripts/check-secrets.py, as CI runs it per environment, holds each further catalogue's
    Secrets to the lengths components/ckan/secrets.yaml declares for the default instance's."""
    result = subprocess.run(["python3", str(PROJECT_ROOT / "scripts/check-secrets.py"), str(rendered_file),
                             str(PROJECT_ROOT / "components")], capture_output=True, text=True)
    assert result.returncode == 0, result.stdout[-3000:]
    assert "0 problem(s)" in result.stdout


@pytest.mark.parametrize("catalogues, says", [
    ([{"name": "catalogue", "subDomain": "x"}], "not a catalogue name"),
    ([{"name": "Bad_Name", "subDomain": "x"}], "not a catalogue name"),
    ([{"name": "a", "subDomain": "x"}, {"name": "a", "subDomain": "y"}], "not a catalogue name"),
    ([{"name": "a", "subDomain": "x"}, {"name": "b", "subDomain": "x"}], "not a DNS label of its own"),
    ([{"name": "a"}], "not a DNS label of its own"),
    ([{"name": "a", "subDomain": "opendata"}], "is the default catalogue's"),
])
def test_a_bad_entry_stops_the_render(tmp_path_factory, catalogues, says):
    result = render(tmp_path_factory, catalogues)
    assert result.returncode != 0
    assert says in result.stderr, result.stderr[-2000:]
