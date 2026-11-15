"""T-4687: the BBSK demo is outside the platform's deployment and holds only BB and BBSK.

`demos/bbsk/` renders its own catalogue, database, edge and NetworkPolicies into namespace
`demo-bbsk` and nothing else; the platform's environments render none of it. Its catalogue harvests
only the city's and the region's sources, says DEMO on every page and keeps search engines out,
and its assistant reads the region's and the city's public sources through one McpServer (owner 2026-10-19T19:50Z).
"""

import shutil
import subprocess
from pathlib import Path

import pytest
import yaml
from conftest import load_yaml_all
from test_public_assistant_seed import deployment, general, manifests

ROOT = Path(__file__).resolve().parent.parent
HOSTS = {"dev": "bbsk-demo.apps.dev.joinedcontext.com", "smartcity": "bbsk.smartcity.joinedcontext.com"}
PUBLISHERS = {"banskabystrica", "bbsk"}


def render(env: str) -> list[dict]:
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    result = subprocess.run(
        ["helmfile", "-f", "demos/bbsk/helmfile.yaml.gotmpl", "-e", env, "template", "--skip-deps"],
        cwd=ROOT, capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    return [d for d in load_yaml_all(result.stdout) if isinstance(d, dict)]


@pytest.fixture(scope="module", params=sorted(HOSTS))
def demo(request):
    return request.param, render(request.param)


def one(docs, kind, name=None):
    found = [d for d in docs if d["kind"] == kind and (name is None or d["metadata"]["name"] == name)]
    assert len(found) == 1, (kind, name, len(found))
    return found[0]


def demo_objects(docs):
    return [d for d in docs if d["metadata"].get("namespace") == "demo-bbsk" or "demo-bbsk" in d["metadata"]["name"]]


def test_every_object_of_the_demo_is_in_its_own_namespace(demo):
    _, docs = demo
    assert docs
    assert {d["metadata"].get("namespace") for d in docs} == {"demo-bbsk"}


def test_the_platform_renders_no_object_of_the_demo(rendered):
    assert demo_objects(rendered("dev")) == []


def test_the_host_has_its_own_certificate_and_every_answer_says_noindex(demo):
    env, docs = demo
    host = HOSTS[env]
    certificate = one(docs, "Certificate")["spec"]
    assert certificate["dnsNames"] == [host] and certificate["issuerRef"]["kind"] == "ClusterIssuer"
    ingress = one(docs, "Ingress")
    assert [rule["host"] for rule in ingress["spec"]["rules"]] == [host]
    assert ingress["spec"]["tls"] == [{"hosts": [host], "secretName": certificate["secretName"]}]
    middleware = ingress["metadata"]["annotations"]["traefik.ingress.kubernetes.io/router.middlewares"]
    assert middleware == "demo-bbsk-demo-bbsk-headers@kubernetescrd"
    headers = one(docs, "Middleware", "demo-bbsk-headers")["spec"]["headers"]
    assert headers["customResponseHeaders"]["X-Robots-Tag"] == "noindex, nofollow"
    # T-4740: the host serves the landing page; the catalogue runs behind it until it is removed.
    assert one(docs, "Deployment", "ckan-demo-bbsk")
    backend = ingress["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]
    assert backend == {"name": "demo-bbsk-landing", "port": {"number": 8080}}


def test_the_namespace_is_closed_but_for_the_edge_and_public_https(demo):
    _, docs = demo
    policies = {d["metadata"]["name"]: d["spec"] for d in docs if d["kind"] == "NetworkPolicy"}
    default = policies["demo-bbsk-default"]
    assert default["podSelector"] == {} and default["policyTypes"] == ["Ingress", "Egress"]
    # Inside the namespace only: no rule of the default names another namespace but DNS.
    assert default["ingress"] == [{"from": [{"podSelector": {}}]}]
    harvest = policies["demo-bbsk-harvest"]["egress"][0]["to"][0]["ipBlock"]
    assert harvest["cidr"] == "0.0.0.0/0" and "10.0.0.0/8" in harvest["except"]
    assert policies["demo-bbsk-edge"]["ingress"][0]["ports"] == [{"protocol": "TCP", "port": 5000}]


def test_the_catalogue_runs_on_the_demos_own_database_and_secrets(demo):
    _, docs = demo
    db = one(docs, "StatefulSet", "demo-bbsk-db")["spec"]["template"]["spec"]["containers"][0]
    assert "@sha256:" in db["image"]
    secrets = {d["metadata"]["name"]: d for d in docs if d["kind"] == "Secret"}
    assert set(secrets) == {"db-ckan", "db-ckan-datastore-read", "ckan-session", "ckan-admin-credentials"}
    assert all(s["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep" for s in secrets.values())
    ckan = one(docs, "Deployment", "ckan-demo-bbsk")["spec"]["template"]["spec"]
    env = {e["name"]: e for c in ckan["containers"] for e in c.get("env", [])}
    assert "@demo-bbsk-db.demo-bbsk.svc.cluster.local:5432/ckan" in env["CKAN_SQLALCHEMY_URL"]["value"]


def test_the_catalogue_harvests_only_the_city_and_the_region(demo):
    _, docs = demo
    harvests = [d for d in docs if d["kind"] == "CronJob" and "harvest" in d["metadata"]["name"]]
    configs = []
    for job in harvests:
        container = job["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
        env = {e["name"]: e.get("value") for e in container["env"]}
        configs.append(yaml.safe_load(env["HARVEST_CONFIG"]))
    assert {c["organization"] for c in configs} == PUBLISHERS


def test_every_page_says_demo_and_frames_the_demo_assistant(demo):
    env, docs = demo
    branding = None
    for doc in docs:
        if doc["kind"] == "ConfigMap" and "branding.json" in doc.get("data", {}):
            branding = yaml.safe_load(doc["data"]["branding.json"])
    assert branding, "the theme's branding block is rendered"
    assert branding["ribbon"].startswith("DEMO") and branding["noindex"] is True
    assert branding["ribbon"] in branding["footerLines"]
    assert branding["siteTitle"].startswith("DEMO")
    # "Add to your AI assistant" names the demo assistant's own server, not the platform's hub.
    assert branding["mcpServer"] == "d/bbsk-demo"
    assert set(branding["languages"]["offered"]) == {"sk", "hu", "en"}
    rendered = yaml.safe_dump(docs)
    domain = HOSTS[env].split(".", 1)[1].removeprefix("apps.")
    assert f"https://assistant.{domain}/d/bbsk-demo/widget" in rendered
    # Another body's organization is never offered.
    for other in ("praha", "zilina", "hel-fi", "slovensko"):
        assert f'"{other}":' not in doc_text(docs, "organizations.json"), other


def doc_text(docs, key: str) -> str:
    return "".join(d.get("data", {}).get(key, "") for d in docs if d["kind"] == "ConfigMap")


def test_the_demo_assistant_reads_the_regions_and_the_citys_public_data_through_one_server():
    # Owner 2026-10-19T19:50Z (replacing 17:55Z's BBSK only): the region and the city, the two
    # publishers the demo catalogue harvests; T-4750: the data through one public McpServer that
    # selects both, never a connector list beside it.
    spec = deployment_named("bbsk-demo")["spec"]
    assert spec["channel"] == "public" and spec["publicId"] == "bbsk-demo"
    assert "connectors" not in spec and "select" not in spec
    assert spec["scope"] == {"projects": ["banskabystrica"]}
    own = ["web", "data-portal", "catalogue"]
    assert spec["sources"] == own + [f"banskabystrica/{n}" for n in own]
    for named in spec["sources"]:
        project, _, name = named.rpartition("/")
        assert (project or "bbsk") in PUBLISHERS, named
        assert manifests("KnowledgeSource", project or "bbsk")[name]["spec"]["visibility"] == "public", named
    assert spec["mcpServer"] == "bbsk/kraj-a-mesto"
    server = manifests("McpServer", "bbsk")["kraj-a-mesto"]["spec"]
    assert server["audience"] == "public" and "members" not in server
    assert server["select"]["accessLevel"] == ["public"]
    assert {s.split("/")[0] for s in server["select"]["spaces"]} == PUBLISHERS
    prompt = spec["systemPrompt"]
    assert prompt.startswith("Si ukážkový asistent (DEMO) nad otvorenými dátami Banskobystrického samosprávneho kraja a")
    assert "mesta Banská Bystrica" in prompt and "Táto ukážka obsahuje len dáta" in prompt
    for text in (*spec["theme"]["subtitle"].values(), spec["theme"]["greeting"], *spec["disclaimer"].values()):
        assert "Bystric" in text, text
    assert spec["allowedOrigins"] == [f"https://{HOSTS['smartcity']}", f"https://{HOSTS['dev']}"]
    # Owner 2026-10-19T13:49Z: two cards from www.bbsk.sk, then the open-data ones, only those proven 3 of 3.
    # The Zvolen schools card is off since the owner's 16:22Z run read Organization (supervisor 16:35Z);
    # the population since prod's 2026-10-20 run (T-4769); office hours back with dataset links (T-4793):
    # at most three, each language alike.
    assert len(spec["suggestions"]["sk"]) == len(spec["suggestions"]["en"]) in (2, 3)
    assert not any("Zvolen" in q for q in spec["suggestions"]["sk"] + spec["suggestions"]["en"])
    # T-4740: the panel is "AI asistent" in the landing page's blue, never a name its initials stand for.
    assert spec["theme"]["title"]["sk"] == "AI asistent" and spec["theme"]["primaryColor"] == "#264a96"


def test_the_demo_stays_inside_the_keys_five_dollars_with_the_public_assistants():
    bodies = ("banskabystrica", "bbsk", "zilina", "praha")
    total = sum(deployment(p)["spec"]["budget"]["costPerDayUsd"] for p in bodies)
    total += general()["spec"]["budget"]["costPerDayUsd"]
    assert total + deployment_named("bbsk-demo")["spec"]["budget"]["costPerDayUsd"] <= 5


def test_an_office_behind_one_address_is_paced_by_the_minute_not_a_share_of_the_day():
    # T-5928 (owner 2026-10-28): 9 questions spent one address's 10 % share of bbsk-demo's $1.
    # The trial deployments give one visitor the whole day; the per-minute and per-hour rates
    # stay the abuse bound, the day's cost the money bound.
    for spec in (deployment_named("bbsk-demo")["spec"], general()["spec"]):
        assert spec["budget"]["visitorSharePercent"] == 100
        rate = spec["rateLimit"]
        assert rate["perClientPerMinute"] <= 10 and rate["perClientPerHour"] <= 60
        assert rate["perClientPerDay"] >= 200


def deployment_named(public_id: str) -> dict:
    found = [d for d in manifests("AssistantDeployment", "bbsk").values() if d["spec"]["publicId"] == public_id]
    assert len(found) == 1, public_id
    return found[0]
