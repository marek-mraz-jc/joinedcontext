"""The participation platform (ADR-N-065, T-6316): what the render proves before anyone applies it.

Decidim holds citizen accounts, so the properties that matter are decidable here: every secret
reaches the pods by reference, the images are pinned, sign-in goes through the realm alone, the
edge refuses the paths that would give an account a local password, and nothing but the edge,
web and Sidekiq reaches the workloads. The component renders in the addons environment until
T-6317 lists it on dev and smartcity.
"""

import base64
import csv
import json
import re
import shutil
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
COMPONENT = PROJECT_ROOT / "components/decidim"
HOST = "participacia.addons.example.org"

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def by_name(docs, kind, name):
    match = [d for d in docs if d and d.get("kind") == kind and d["metadata"]["name"] == name]
    assert match, f"{kind}/{name} is not in the render"
    return match[0]


def pod(docs, name):
    return by_name(docs, "Deployment", name)["spec"]["template"]["spec"]


def env_of(container):
    return {e["name"]: e for e in container.get("env", [])}


@requires_helmfile
def test_the_three_workloads_their_database_and_claim_render(rendered):
    docs = rendered("addons")
    for name in ("decidim-web", "decidim-sidekiq", "decidim-redis"):
        by_name(docs, "Deployment", name)
    by_name(docs, "Service", "decidim-web")
    by_name(docs, "Service", "decidim-redis")
    by_name(docs, "PersistentVolumeClaim", "decidim-web-files")
    assert by_name(docs, "Database", "decidim")["spec"]["owner"] == "decidim"


@requires_helmfile
def test_web_migrates_and_bootstraps_before_puma_starts(rendered):
    spec = pod(rendered("addons"), "decidim-web")
    (init,) = [c for c in spec["initContainers"] if c["name"] == "migrate"]
    assert init["command"][-1].startswith("mkdir -m 700 -p \"$TMPDIR\" && bin/rails db:migrate && bin/rails runner db/joinedcontext_bootstrap.rb && ")
    assert init["image"] == spec["containers"][0]["image"], "migrations of one release, puma of another"
    assert set(env_of(init)) == set(env_of(spec["containers"][0])), "the bootstrap reads what puma reads"


@requires_helmfile
def test_every_image_is_pinned_by_digest_and_web_and_sidekiq_run_one(rendered):
    docs = rendered("addons")
    images = {}
    for name in ("decidim-web", "decidim-sidekiq", "decidim-redis"):
        spec = pod(docs, name)
        for c in spec["containers"] + spec.get("initContainers", []):
            assert "@sha256:" in c["image"], f"{name}/{c['name']}: {c['image']}"
            images.setdefault(name, c["image"])
    assert images["decidim-web"] == images["decidim-sidekiq"]


@requires_helmfile
def test_secrets_reach_the_pods_by_reference_only(rendered):
    """A literal secret in a Deployment is in every `kubectl get -o yaml` and in Git's render."""
    docs = rendered("addons")
    expected = {
        "DATABASE_PASSWORD": ("db-decidim", "password"),
        "SECRET_KEY_BASE": ("decidim-secret-key-base", "secret"),
        "OIDC_CLIENT_SECRET": ("keycloak-client-decidim", "client-secret"),
    }
    for name in ("decidim-web", "decidim-sidekiq"):
        env = env_of(pod(docs, name)["containers"][0])
        for var, (secret, key) in expected.items():
            ref = env[var]["valueFrom"]["secretKeyRef"]
            assert (ref["name"], ref["key"]) == (secret, key), f"{name} {var}"
            assert "value" not in env[var]
        assert env["DATABASE_URL"]["value"].startswith("postgres://decidim@"), "a password in the URL"
        assert env["DATABASE_URL"]["value"].endswith("sslmode=require")
        assert env["RAILS_LOG_LEVEL"]["value"] == "warn", "request lines carry client addresses"
    for secret in ("db-decidim", "decidim-secret-key-base", "keycloak-client-decidim"):
        by_name(docs, "Secret", secret)


@requires_helmfile
def test_sign_in_goes_through_the_realm_on_https(rendered):
    env = env_of(pod(rendered("addons"), "decidim-web")["containers"][0])
    assert env["OIDC_ISSUER"]["value"] == "https://idm.addons.example.org/realms/addons"
    assert env["OIDC_CLIENT_ID"]["value"] == "decidim"
    assert env["OIDC_REDIRECT_URI"]["value"] == f"https://{HOST}/users/auth/keycloak/callback"
    assert env["DECIDIM_HOST"]["value"] == HOST


@requires_helmfile
def test_every_container_runs_non_root_read_only_without_capabilities(rendered):
    docs = rendered("addons")
    for name, uid in (("decidim-web", 10001), ("decidim-sidekiq", 10001), ("decidim-redis", 999)):
        spec = pod(docs, name)
        assert spec["securityContext"]["runAsNonRoot"] is True
        assert spec["securityContext"]["runAsUser"] == uid
        assert spec["automountServiceAccountToken"] is False
        for c in spec["containers"] + spec.get("initContainers", []):
            sc = c["securityContext"]
            assert sc["readOnlyRootFilesystem"] is True and sc["allowPrivilegeEscalation"] is False, f"{name}/{c['name']}"
            assert sc["capabilities"]["drop"] == ["ALL"], f"{name}/{c['name']}"
            assert c["resources"]["limits"]["memory"], f"{name}/{c['name']}"


def test_the_edge_refuses_the_paths_that_mint_a_local_password():
    plugins = yaml.safe_load((COMPONENT / "apisix-plugins.yaml").read_text())["decidim"]["plugins"]
    blocker = plugins["uri-blocker"]
    rules = [re.compile(r) for r in blocker["block_rules"]]
    refused = ["/system", "/system/organizations", "/users/password/new", "/users/password?x=1",
               "/users/invitation/accept?invitation_token=t", "/users/sign_up"]
    allowed = ["/", "/api", "/users/sign_in", "/users/auth/keycloak/callback", "/processes/x",
               "/systemic-change", "/users/passwords-are-not-a-route"]
    for path in refused:
        assert any(r.search(path) for r in rules), f"{path} reaches Decidim"
    for path in allowed:
        assert not any(r.search(path) for r in rules), f"{path} is refused"
    assert blocker["rejected_code"] == 404
    assert plugins["proxy-rewrite"]["headers"]["set"]["X-Forwarded-Proto"] == "https"


@requires_helmfile
def test_the_route_and_certificate_carry_the_participation_host(rendered):
    docs = rendered("addons")
    certs = [d for d in docs if d and d.get("kind") == "Certificate"]
    assert any(HOST in c["spec"]["dnsNames"] for c in certs), "no certificate names the host"
    assert not any(n.startswith("*") for c in certs for n in c["spec"]["dnsNames"]), "a wildcard"


@requires_helmfile
def test_only_web_sidekiq_and_sync_reach_redis_and_the_database_and_only_the_edge_reaches_web(rendered):
    docs = rendered("addons")
    policies = {d["metadata"]["name"]: d for d in docs if d and d.get("kind") == "NetworkPolicy"}

    def sources(policy):
        return sorted(
            p["podSelector"]["matchLabels"].get("app.kubernetes.io/name", "")
            for rule in policies[policy]["spec"].get("ingress", [])
            for p in rule.get("from", [])
        )

    assert sources("decidim-web") == ["apisix"]
    assert sources("decidim-redis") == ["decidim-sidekiq", "decidim-sync", "decidim-web"]
    assert sources("postgres-from-decidim") == ["decidim-sidekiq", "decidim-sync", "decidim-web"]
    assert policies["decidim-sidekiq"]["spec"].get("ingress", []) == [], "Sidekiq serves nothing"
    # Meshed: each plain rule has its 4143 half (T-5314).
    assert sources("decidim-web-allow-linkerd") == ["apisix"]
    assert sources("decidim-redis-allow-linkerd") == ["decidim-sidekiq", "decidim-sync", "decidim-web"]
    assert sources("postgres-linkerd-from-decidim") == ["decidim-sidekiq", "decidim-sync", "decidim-web"]
    for name in ("decidim-web", "decidim-sidekiq"):
        egress_ports = {
            port["port"] for rule in policies[name]["spec"]["egress"] for port in rule.get("ports", [])
        }
        assert {5432, 6379, 587}.issubset(egress_ports), name
    web_443 = [r for r in policies["decidim-web"]["spec"]["egress"] if {"port": 443, "protocol": "TCP"} in r["ports"]]
    assert web_443, "the web cannot reach the realm"


def test_the_image_drops_root_and_the_shipped_key():
    dockerfile = (PROJECT_ROOT / "images/decidim/Dockerfile").read_text()
    assert "rm -f config/master.key config/credentials.yml.enc" in dockerfile
    assert dockerfile.rstrip().splitlines()[-1].startswith("CMD")
    assert "USER 10001:10001" in dockerfile
    assert "gem install decidim --version 0.32.2 " in dockerfile
    # The runtime stage is the slim Ruby image: no compiler, no Node, pinned by digest.
    final = dockerfile.split("\nFROM ")[-1]
    assert final.startswith("ruby:3.4.12-slim-trixie@sha256:")
    assert "node" not in final.split("\n", 1)[0]


def test_the_layout_loads_the_launcher_from_the_assistant_host_alone():
    # The Dockerfile puts the partial where Decidim looks for it, and only the assistant host joins
    # the script and frame policy; both stay off without ASSISTANT_ORIGIN.
    image = PROJECT_ROOT / "images/decidim"
    assert "COPY assistant.html.erb app/views/layouts/decidim/_assistant.html.erb" in (image / "Dockerfile").read_text()
    assert 'render partial: "layouts/decidim/assistant"' in (image / "head-extra.html.erb").read_text()
    partial = (image / "assistant.html.erb").read_text()
    assert 'data: { lang: I18n.locale, color: current_organization.colors["primary"] }' in partial and 'if ENV["ASSISTANT_ORIGIN"].present?' in partial
    ruby = (image / "joinedcontext.rb").read_text()
    assert re.search(r'if ENV\["ASSISTANT_ORIGIN"\]\.present\?\n  Decidim\.content_security_policies_extra = \{\n'
                     r'    "script-src" => \[ENV\.fetch\("ASSISTANT_ORIGIN"\)\],\n    "frame-src" => \[ENV\.fetch\("ASSISTANT_ORIGIN"\)\]\n  \}', ruby)


def test_the_image_widens_only_the_rubyzip_line_and_ships_no_key():
    """T-6448: the one declared range the image moves is decidim-core's rubyzip, past
    CVE-2026-85396, and a Decidim release that rewrites the line stops the build."""
    dockerfile = (PROJECT_ROOT / "images/decidim/Dockerfile").read_text()
    assert dockerfile.count("sed -i") == 1
    assert "&& grep -q '%q<rubyzip>.freeze, \\[\"~> 3.4\".freeze\\]'" in dockerfile
    for gem in ("decidim-core", "doc2text", "graphql", "rubyzip"):
        assert f" {gem} " in dockerfile.split("bundle lock --update", 1)[1].split("bundle config", 1)[0]
    assert "rm -rf /usr/local/bundle/gems/*/spec /usr/local/bundle/gems/hexapdf-*/data/hexapdf/cert" in dockerfile
    workflow = (PROJECT_ROOT / ".github/workflows/image-decidim.yml").read_text()
    assert 'exit-code: "1"' in workflow and 'severity: "HIGH,CRITICAL"' in workflow


@requires_helmfile
@pytest.mark.parametrize("environment", ["stage", "smartcity"])
def test_the_realm_person_who_administers_decidim_holds_nothing_else(rendered, environment):
    """T-6448: a production realm seeds one person beside the owner: no Portal role, no group,
    the mfa-required role, a generated password, an id of its own, and that person is Decidim's admin."""
    docs = rendered(environment)
    secret = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"].endswith("config-cli-config-realms"))
    realm = json.loads(base64.b64decode(secret["data"][f"{environment}.json"]))
    person = next(u for u in realm["users"] if u["username"].startswith("participacia.steward@"))
    assert person["realmRoles"] == [f"default-roles-{environment}", "mfa-required"]
    assert person["groups"] == []
    # stage's script signs in with a seeded factor; on prod the person enrols their own device.
    assert {c["type"] for c in person["credentials"]} == ({"password"} if environment == "smartcity" else {"password", "otp"})
    assert all("$(USER_" in (c.get("value") or c["secretData"]) for c in person["credentials"])
    web = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "decidim-web")
    env = {e["name"]: e.get("value") for e in web["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["DECIDIM_ADMIN_EMAIL"] == person["email"]
    assert not any(
        "de300000-" in str(u.get("id")) for d in docs if d.get("kind") == "ConfigMap" for u in [d.get("data") or {}]
    )


@requires_helmfile
def test_each_page_names_the_assistant_of_its_own_environment(rendered):
    env = env_of(pod(rendered("addons"), "decidim-web")["containers"][0])
    assert env["ASSISTANT_ORIGIN"]["value"] == "https://assistant.addons.example.org"
    assert env["ASSISTANT_PUBLIC_ID"]["value"] == "participacia"


def forge_seed(docs):
    """The files of the forge seed ConfigMaps of one render, by their path with `__` for `/`."""
    return {k: v for d in docs if d and d.get("kind") == "ConfigMap" and re.fullmatch(r"gitea-seed-s\d+-.*", d["metadata"]["name"])
            for k, v in (d.get("data") or {}).items()}


@requires_helmfile
@pytest.mark.parametrize("environment, domain", [("stage", "stage.joinedcontext.com"), ("smartcity", "smartcity.joinedcontext.com")])
def test_the_forge_seed_carries_the_environments_own_decidim_host(rendered, environment, domain):
    files = forge_seed(rendered(environment))
    assistant = yaml.safe_load(files["projects__slovensko__assistant__deployments__participacia.yaml"])
    assert assistant["spec"]["allowedOrigins"] == [f"https://participacia.{domain}"]
    bento = files["projects__slovensko__pipelines__decidim-participacia__bento.yaml"]
    assert f"url: https://participacia.{domain}/api" in bento
    assert "dev.joinedcontext.com" not in bento


@requires_helmfile
def test_dev_runs_no_decidim_so_its_forge_holds_no_harvester_of_it(rendered):
    assert not [k for k in forge_seed(rendered("dev")) if "participacia" in k and "slovensko-participacia" not in k]


@requires_helmfile
def test_sidekiq_says_when_it_runs_though_it_serves_no_port(rendered):
    """validate-probes (T-6438): the process title is Sidekiq's own health signal, and it needs
    no port, no Redis client and no second Ruby in the pod."""
    container = pod(rendered("addons"), "decidim-sidekiq")["containers"][0]
    assert container["readinessProbe"]["exec"]["command"] == ["grep", "-qF", "busy]", "/proc/1/cmdline"]
    assert "livenessProbe" not in container


@requires_helmfile
def test_web_and_sidekiq_share_one_selinux_level(rendered):
    """Both mount decidim-web-files; a random level per pod relabels it and the other is refused."""
    docs = rendered("stage")
    pods = [d["spec"]["template"]["spec"] for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] in ("decidim-web", "decidim-sidekiq")]
    assert len(pods) == 2
    assert len({p["securityContext"]["seLinuxOptions"]["level"] for p in pods}) == 1


@requires_helmfile
def test_the_site_runs_after_the_bootstrap_only_where_an_environment_asks(rendered):
    """T-6453: the themed site is for stage until the owner has looked; prod keeps a plain Decidim."""
    for env, wanted in (("addons", "false"), ("stage", None), ("smartcity", "false")):
        spec = pod(rendered(env), "decidim-web")
        migrate = next(c for c in spec["initContainers"] if c["name"] == "migrate")
        values = {e["name"]: e.get("value") for e in migrate["env"]}
        if wanted:
            assert values["DECIDIM_SITE_CONTENT"] == wanted, env
            assert values["DECIDIM_SITE_DEMO_NOTICE"] == wanted, env
        else:
            # Stage may turn the site on, and then always with the demo notice.
            assert values["DECIDIM_SITE_DEMO_NOTICE"] == "true"
        assert values["DECIDIM_PORTAL_URL"].startswith("https://portal.")
        assert values["DECIDIM_CATALOGUE_URL"].startswith("https://opendata.")
        command = migrate["command"][-1]
        assert command.index("joinedcontext_bootstrap.rb") < command.index("joinedcontext_site.rb")
        # Attachments must land on the claim web serves them from, never in the init's own layer.
        assert {"name": "files", "mountPath": "/code/storage"} in migrate["volumeMounts"]


def test_ruby_gets_a_temporary_directory_it_accepts(rendered):
    """An emptyDir is 0777 without the sticky bit, so Ruby's Dir.tmpdir raises and Rails never boots."""
    docs = rendered("stage")
    web = pod(docs, "decidim-web")
    sidekiq = pod(docs, "decidim-sidekiq")
    migrate = next(c for c in web["initContainers"] if c["name"] == "migrate")
    for container in (migrate, web["containers"][0], sidekiq["containers"][0]):
        assert {"name": "TMPDIR", "value": "/tmp/app"} in container["env"], container["name"]
    assert migrate["command"][-1].startswith('mkdir -m 700 -p "$TMPDIR" && ')
    assert sidekiq["containers"][0]["command"][-1].startswith('mkdir -m 700 -p "$TMPDIR" && exec ')


def seed(name):
    return yaml.safe_load((COMPONENT / "seed" / name).read_text())


def publish_row(docs):
    return json.loads(env_of(pod(docs, "decidim-sidekiq")["containers"][0])["JOINEDCONTEXT_ENDPOINTS"]["value"])[0]


@requires_helmfile
def test_push_and_harvester_share_one_row_and_the_write_endpoint(rendered):
    row = publish_row(rendered("addons"))
    with (PROJECT_ROOT / "seeds/decidim/instances.csv").open(encoding="utf-8") as f:
        harvested = next(r for r in csv.DictReader(f) if r["name"] == "participacia")
    assert (row["instance"], row["scope"], row["locality"], row["provider"]) == (harvested["name"], harvested["scope"], harvested["locality"], harvested["provider"])
    endpoint = yaml.safe_load((PROJECT_ROOT / "components/context-gateway/seed/slovensko/slovensko-endpoint-participacia-zapis.yaml").read_text())
    assert row["url"] == f"https://api.addons.example.org/api/endpoint/{endpoint['spec']['slug']}"
    assert row["space"] == endpoint["spec"]["contextSpaceRef"]


@requires_helmfile
def test_only_sidekiq_holds_the_publishing_credential(rendered):
    docs = rendered("addons")
    side = env_of(pod(docs, "decidim-sidekiq")["containers"][0])
    assert side["JOINEDCONTEXT_CLIENT_SECRET"]["valueFrom"]["secretKeyRef"] == {"name": "keycloak-client-slovensko-decidim-publish", "key": "client-secret"}
    assert side["JOINEDCONTEXT_CLIENT_ID"]["value"] == "slovensko-decidim-publish"
    assert side["JOINEDCONTEXT_GATEWAY_HOST"]["value"] == "api.addons.example.org"
    web = env_of(pod(docs, "decidim-web")["containers"][0])
    assert "JOINEDCONTEXT_ENDPOINTS" in web, "the admin page lists the row"
    assert not {"JOINEDCONTEXT_CLIENT_SECRET", "JOINEDCONTEXT_CLIENT_ID", "JOINEDCONTEXT_TOKEN_URL"} & set(web)


def test_the_publishing_account_writes_its_five_classes_in_its_one_space():
    account, policy = seed("slovensko-serviceaccount-publish.yaml"), seed("slovensko-policy-publish.yaml")
    types = ["ParticipatoryProcess", "Proposal", "BudgetProject", "Result", "Meeting"]
    assert [r["scope"] for r in account["spec"]["roles"]] == [{"contextSpace": "slovensko-participacia"}]
    assert account["spec"]["roles"][0]["types"] == types
    assert policy["spec"]["assignee"] == {"kind": "serviceAccount", "id": account["metadata"]["name"]}
    assert policy["spec"]["contextSpaceRef"]["name"] == "slovensko-participacia"
    assert sorted(policy["spec"]["operations"]) == ["createBatch", "deleteBatch", "queryBatch", "upsertBatch"]
    assert [e["type"] for e in policy["spec"]["information"][0]["entities"]] == types
    assert "policyRef" not in yaml.safe_load((PROJECT_ROOT / "components/context-gateway/seed/slovensko/slovensko-endpoint-participacia-zapis.yaml").read_text())["spec"]


def test_the_publishing_client_opens_the_write_endpoint_and_no_browser_flow():
    client = yaml.safe_load((COMPONENT / "keycloak-clients.yaml").read_text())["slovensko-decidim-publish"]
    raw = client["rawValues"]
    assert client["componentNamespaces"] == ["decidim"]
    assert raw["serviceAccountsEnabled"] and not (raw["standardFlowEnabled"] or raw["directAccessGrantsEnabled"] or raw["implicitFlowEnabled"])
    audiences = [m["config"]["included.custom.audience"] for m in raw["protocolMappers"]]
    assert audiences == [yaml.safe_load((PROJECT_ROOT / "components/context-gateway/seed/slovensko/slovensko-endpoint-participacia-zapis.yaml").read_text())["spec"]["slug"]]


@requires_helmfile
def test_the_nightly_pass_queues_from_the_sidekiq_image_and_never_writes(rendered):
    docs = rendered("addons")
    cron = by_name(docs, "CronJob", "decidim-sync")
    job = cron["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    (container,) = job["containers"]
    assert "decidim_joinedcontext:sync_all" in container["command"][-1]
    assert container["image"] == pod(docs, "decidim-sidekiq")["containers"][0]["image"]
    env = {e["name"]: e for e in container["env"]}
    assert env["RAILS_MAX_THREADS"]["value"] == "1"
    assert "JOINEDCONTEXT_CLIENT_SECRET" not in env
    assert job["automountServiceAccountToken"] is False
    assert container["securityContext"]["readOnlyRootFilesystem"] is True


@requires_helmfile
def test_sidekiq_reaches_the_public_edge_and_the_sync_only_its_data(rendered):
    docs = rendered("addons")
    policies = [d for d in docs if d and d.get("kind") == "NetworkPolicy"]

    def ports(name):
        (policy,) = [p for p in policies if p["metadata"]["name"] == name]
        return {port["port"] for rule in policy["spec"]["egress"] for port in rule.get("ports", [])}

    assert 443 in ports("decidim-sidekiq")
    assert ports("decidim-sync") == {53, 5432, 6379}
