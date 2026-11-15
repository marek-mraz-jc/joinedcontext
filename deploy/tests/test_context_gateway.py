"""The Context Gateway on dev, judged as the deployment renders it (T-0263, T-0277).

Three things are coupled across files and break silently when only one moves: the endpoint
slug in the seeded table and the audience the conformance client's token carries; the
ServiceAccount's name and the Keycloak client id the gateway resolves `azp` against
(`{project}-{name}`); and the split of grants between the anonymous caller (reads only) and
the service account (writes). A render that drifts on any of them still applies cleanly."""

import base64
import json
import shutil

import pytest
import yaml
from conftest import drop_component, seed_configmap
from pipeline_spec import outputs

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

WRITE_GROUPS = {"redirectionOps", "updateOps"}
WRITE_OPERATIONS = {
    "createEntity", "updateEntity", "appendAttrs", "updateAttrs", "deleteAttrs", "deleteEntity",
    "mergeEntity", "replaceEntity", "replaceAttrs", "purgeEntity",
    "createBatch", "upsertBatch", "updateBatch", "mergeBatch", "deleteBatch",
}


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


@pytest.fixture(scope="module")
def forge_seed(dev):
    """The files the forge bootstrap commits: the repository the gateway checks out (T-0278).

    A key's `__` is a `/` of the repository path."""
    configmap = seed_configmap(dev)
    return {key.replace("__", "/"): text for key, text in configmap["data"].items()}


@pytest.fixture(scope="module")
def seed(forge_seed):
    """The manifests of the conformance project, parsed.

    The seed also carries the data model's LinkML source and its generated artifacts, and the
    Bento configuration beside each Pipeline. The repository loader reads a file as a manifest
    only when it ends `.yaml` and not `.linkml.yaml` (jcctl loader.rs), and so does this
    fixture: parsing the Markdown reference as YAML is how the seed's own artifacts would break
    the whole module. A `pipelines/*/bento.yaml` is valid YAML with no `kind` at all, which is
    the other way in, so what parses without one is left out here too."""
    parsed = [
        yaml.safe_load(text)
        for path, text in forge_seed.items()
        if path.startswith("projects/banskabystrica/") and path.endswith(".yaml") and not path.endswith(".linkml.yaml")
    ]
    return {(m["kind"], m["metadata"]["name"]): m for m in parsed if isinstance(m, dict) and "kind" in m}


@pytest.fixture(scope="module")
def realm_clients(dev):
    secret = next(
        d for d in dev
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "keycloak-config-keycloak-config-cli-config-realms"
    )
    realm = json.loads(base64.b64decode(secret["data"]["dev.json"]))
    return {c["clientId"]: c for c in realm["clients"]}


@pytest.fixture(scope="module")
def gateway_pod(dev):
    deployment = next(d for d in dev if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-gateway")
    return deployment["spec"]["template"]["spec"]


@pytest.fixture(scope="module")
def gateway_env(gateway_pod):
    container = gateway_pod["containers"][0]
    return {e["name"]: e.get("value") for e in container["env"]}


def grants(policy):
    return set(policy["spec"]["operations"])


def space_of(manifest):
    """The one space a seeded manifest belongs to, or None when it belongs to no single space."""
    if manifest["kind"] == "ContextSpace":
        return manifest["metadata"]["name"]
    if manifest["kind"] == "ServiceAccount":
        scopes = {role["scope"]["contextSpace"] for role in manifest["spec"]["roles"]}
        return scopes.pop() if len(scopes) == 1 else None
    if manifest["kind"] == "App":
        # An App names its space in what it reads (AP-08), not in a contextSpaceRef of its own.
        needs = {need["contextSpaceRef"]["name"] for need in manifest["spec"].get("dataNeeds") or []}
        return needs.pop() if len(needs) == 1 else None
    ref = manifest["spec"].get("contextSpaceRef")
    if ref is None:
        return None
    return ref["name"] if isinstance(ref, dict) else ref


@requires_helmfile
def test_the_conformance_space_is_one_space_one_endpoint_and_the_policies_of_two_callers(seed):
    """The conformance space of the seed: one space, its model, one endpoint, a public reader
    and one writing account. The Helsinki demo (T-0478) and the city's own two spaces (T-2305)
    live beside it in the same project and are judged by their own tests, so this census is of
    `ovzdusie` and not of everything the project holds."""
    ovzdusie = {key: m for key, m in seed.items() if space_of(m) == "ovzdusie"}
    kinds = sorted(kind for kind, _ in ovzdusie)
    # No App reads it: the city's air-quality screen reads the live EEA readings in
    # banskabystrica-verejne, not the conformance suite's seeded stations (T-2916, T-2949). The
    # second Policy is the pipelines' write, which pipeline public-air and its expiry sweep need
    # (T-3120, PL-64). The conformance account writes in its own space (F-BNT1-002).
    assert kinds == ["ContextSpace", "DataModel", "Endpoint", "Policy", "Policy"], kinds


@requires_helmfile
def test_the_conformance_account_writes_only_where_nothing_is_published(seed):
    """F-BNT1-002: the smoke run and the ETSI suite create and delete fixtures. Their account is
    scoped to the `conformance` space alone, whose one Endpoint is neither public nor published
    and whose only other grant is the steward's read (T-2456)."""
    account = seed[("ServiceAccount", "conformance")]
    assert {role["scope"]["contextSpace"] for role in account["spec"]["roles"]} == {"conformance"}
    space = {key: m for key, m in seed.items() if space_of(m) == "conformance"}
    endpoints = [m for (kind, _), m in space.items() if kind == "Endpoint"]
    assert len(endpoints) == 1
    assert endpoints[0]["spec"]["audience"] != "public"
    assert "publish" not in endpoints[0]["spec"] and "catalog" not in endpoints[0]["spec"]
    policies = {name: m["spec"] for (kind, name), m in space.items() if kind == "Policy"}
    assert policies["conformance-write"]["assignee"] == {"kind": "serviceAccount", "id": "conformance"}
    assert policies["conformance-steward-read"]["operations"][0] == "retrieveOps"
    assert not {"createBatch", "upsertBatch", "deleteBatch", "redirectionOps"} & set(policies["conformance-steward-read"]["operations"])
    assert set(policies) == {"conformance-write", "conformance-steward-read"}, sorted(policies)
    # and no published space grants it anything
    for (kind, name), m in seed.items():
        if kind == "Policy" and m["spec"]["assignee"].get("id") == "conformance":
            assert space_of(m) == "conformance", name


@requires_helmfile
def test_every_seeded_manifest_of_the_city_belongs_to_one_of_its_spaces(seed):
    """Nothing in the project floats free of a space, and no manifest of it names a space the
    project does not seed (T-2305, T-2781)."""
    spaces = {name for (kind, name) in seed if kind == "ContextSpace"}
    # The city's own catalogue (T-5493) is the sixth, the conformance suite's own (F-BNT1-002) the
    # seventh, the MQTT demo sensors' (T-4857) the eighth.
    assert spaces == {"ovzdusie", "conformance", "banskabystrica-mesto", "banskabystrica-kpi", "banskabystrica-verejne",
                      "banskabystrica-otvorene-data", "catalog", "senzory"}
    # A DataSource is a fetch and a Project is the project: neither belongs to a space. A
    # Pipeline names its space through the Endpoint it writes through, which is the point. A
    # CkanInstance is the project's catalogue, which each space's Endpoint may publish to (T-2407).
    # A KnowledgeSource is a website the project's assistant answers from, and an
    # AssistantDeployment the assistant that answers from it: neither is a space's (T-3067). An
    # McpServer serves Endpoints by name, which may sit in different spaces (T-4141). An
    # MqttBroker is the project's, read by its pipelines whatever space they write (ADR-N-048).
    project_scoped = {
        "Project", "ServiceAccount", "DataSource", "Pipeline", "CkanInstance", "KnowledgeSource",
        "AssistantDeployment", "McpServer", "MqttBroker",
    }
    for (kind, name), manifest in seed.items():
        if kind in project_scoped:
            continue
        assert space_of(manifest) in spaces, f"{kind}/{name} names {space_of(manifest)}"

    endpoints = {
        f'urn:ngsi-ld:Endpoint:banskabystrica.sk:{m["spec"]["contextSpaceRef"]}:{name}'
        for (kind, name), m in seed.items()
        if kind == "Endpoint"
    }
    for (kind, name), manifest in seed.items():
        if kind == "Pipeline":
            for output in outputs(manifest["spec"]):
                assert output["targetEndpoint"] in endpoints, f"{name} writes nowhere the project serves"

    account = seed[("ServiceAccount", "pipelines")]
    scoped = {role["scope"]["contextSpace"] for role in account["spec"]["roles"]}
    # ovzdusie since T-3120: pipeline public-air writes the conformance space's open dataset;
    # banskabystrica-otvorene-data since T-4255: the generated catalogue-table pipelines. The
    # city's catalogue (T-5493) is the Portal reconciler's alone: no pipeline writes it.
    assert scoped == spaces - {"catalog", "conformance"}, scoped


@requires_helmfile
def test_the_seeded_slug_is_one_the_gateway_accepts(seed):
    """EP-02: 26+ characters of lowercase RFC 4648 base32 (a-z, 2-7). A slug with a `0`, `1`,
    `8` or `9` in it renders fine and is left out of the endpoint table at start-up, so the
    surface answers 404 everywhere; the first seed had exactly that."""
    slug = seed[("Endpoint", "public-air")]["spec"]["slug"]
    assert len(slug) >= 26, slug
    assert set(slug) <= set("abcdefghijklmnopqrstuvwxyz234567"), slug


@requires_helmfile
def test_the_anonymous_caller_can_read_and_never_write(seed):
    public = seed[("Policy", "public-read")]
    assert public["spec"]["assignee"] == {"kind": "role", "id": "public"}
    granted = grants(public)
    assert granted, "an empty grant is a policy that does nothing"
    assert not granted & WRITE_GROUPS, granted
    assert not granted & WRITE_OPERATIONS, granted


@requires_helmfile
def test_the_writes_are_bound_to_the_service_account_and_only_to_it(seed):
    write = seed[("Policy", "conformance-write")]
    account = seed[("ServiceAccount", "conformance")]
    assert write["spec"]["assignee"] == {"kind": "serviceAccount", "id": account["metadata"]["name"]}
    assert grants(write) & WRITE_GROUPS, grants(write)
    # The roundtrip (T-5914) reads its writes back as history (NGSI-LD temporal, OGC history, STA
    # Observations) and as a WebSocket notification: without these the dev record is red.
    assert {"retrieveTemporal", "queryTemporal", "createSubscription"} <= grants(write), grants(write)
    # Every other account-bound policy names an account the same project seeds, or the account
    # the runner derives for one of its Pipelines (pl-<pipeline>, PL-19; T-5320's pl-osm); nothing
    # grants a write to an account that does not exist.
    accounts = {(m["metadata"]["namespace"], name) for (kind, name), m in seed.items() if kind == "ServiceAccount"}
    accounts |= {(m["metadata"]["namespace"], f"pl-{name}") for (kind, name), m in seed.items() if kind == "Pipeline"}
    for (kind, name), m in seed.items():
        if kind == "Policy" and m["spec"]["assignee"]["kind"] == "serviceAccount":
            assert (m["metadata"]["namespace"], m["spec"]["assignee"]["id"]) in accounts, name


@requires_helmfile
def test_the_keycloak_client_is_the_account_the_gateway_resolves_azp_to(seed, realm_clients):
    """`azp` is the client id; the gateway looks it up as `{project}-{name}` (auth/accounts.rs)."""
    account = seed[("ServiceAccount", "conformance")]
    client_id = f'{account["metadata"]["namespace"]}-{account["metadata"]["name"]}'
    client = realm_clients[client_id]
    assert client["serviceAccountsEnabled"] is True
    assert client["standardFlowEnabled"] is False and client["directAccessGrantsEnabled"] is False
    assert client["publicClient"] is False and client["secret"].startswith("$(CLIENT_SECRET_")


@requires_helmfile
def test_the_token_names_the_seeded_endpoint_as_its_audience(seed, realm_clients):
    """The gateway accepts a token only when `aud` names the slug it is called on (PF-45)."""
    # The conformance account's door is its own space's Endpoint (F-BNT1-002).
    slug = seed[("Endpoint", "conformance")]["spec"]["slug"]
    mappers = realm_clients["banskabystrica-conformance"]["protocolMappers"]
    audiences = {m["config"].get("included.custom.audience") for m in mappers if m["protocolMapper"] == "oidc-audience-mapper"}
    assert slug in audiences, (slug, audiences)


@requires_helmfile
def test_the_proposer_account_holds_a_portal_token_and_no_endpoint_token(forge_seed, realm_clients):
    """T-2245 (PF-45, PF-49, PF-58): the seeded workload on the Portal's side derives its client
    `{project}-{name}`, whose token names the Portal's API and nothing the gateway serves, and the
    account's role proposes one kind and never approves."""
    account = yaml.safe_load(forge_seed["projects/helsinki/access/serviceaccounts/pipeline-proposer.yaml"])
    client = realm_clients[f"{account['metadata']['namespace']}-{account['metadata']['name']}"]
    assert client["serviceAccountsEnabled"] is True and client["publicClient"] is False
    assert client["standardFlowEnabled"] is False and client["directAccessGrantsEnabled"] is False
    audiences = [
        m["config"].get("included.client.audience") or m["config"].get("included.custom.audience")
        for m in client.get("protocolMappers", [])
        if m["protocolMapper"] == "oidc-audience-mapper"
    ]
    assert audiences == ["portal-api"], audiences
    roles = {binding["role"] for binding in account["spec"]["roles"]}
    for role in roles:
        rules = yaml.safe_load(forge_seed[f"users/roles/{role}.yaml"])["spec"]["rules"]
        assert all("approve" not in rule["verbs"] for rule in rules), (role, rules)
        assert {verb for rule in rules for verb in rule["verbs"]} == {"propose"}, (role, rules)


@requires_helmfile
def test_the_gateway_verifies_against_the_realm_it_is_told_about(gateway_env):
    """Both OIDC variables or neither; the JWKS in-cluster over plain http, which the binary insists on."""
    assert gateway_env["JC_OIDC_ISSUER"].startswith("https://idm.")
    assert gateway_env["JC_OIDC_JWKS_URL"].startswith("http://keycloak-app-keycloakx-http.")
    assert gateway_env["JC_OIDC_ISSUER"].rsplit("/realms/", 1)[1] == gateway_env["JC_OIDC_JWKS_URL"].split("/realms/")[1].split("/")[0]
    assert gateway_env["JC_GATEWAY_ORG_DOMAIN"] == "hel.fi"


@requires_helmfile
def test_the_gateway_asks_for_its_own_token_on_the_in_cluster_service(gateway_env):
    """T-1500: the gateway mints its own token with `client_credentials` to read the Portal's
    preview list. Derived from the issuer, that request leaves for the cluster's own ingress
    hostname, which a pod on a single node cannot dial: the poller logged `no token for the
    Portal's preview list` every ten seconds and no preview was ever served. The token endpoint is
    the same in-cluster Service as the JWKS, in the same realm."""
    token_url = gateway_env["JC_OIDC_TOKEN_URL"]
    assert token_url.startswith("http://keycloak-app-keycloakx-http.")
    assert token_url.endswith("/protocol/openid-connect/token")
    realm_of = lambda url: url.split("/realms/")[1].split("/")[0]  # noqa: E731
    assert realm_of(token_url) == realm_of(gateway_env["JC_OIDC_JWKS_URL"])
    assert realm_of(token_url) == gateway_env["JC_OIDC_ISSUER"].rsplit("/realms/", 1)[1]


@requires_helmfile
def test_the_gateway_reads_the_forge_checkout_git_sync_keeps(dev, gateway_pod, gateway_env):
    """T-0278: the endpoint table is the configuration repository out of the forge, not a
    ConfigMap seed. git-sync runs once before the gateway (so it never starts on an empty
    table) and beside it (so a merged Endpoint serves within one period); the checkout is a
    shared volume the gateway sees read-only; the token is the forge's read-only one."""
    assert not any(d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "context-gateway" for d in dev)
    assert gateway_env["JC_GATEWAY_REPO_DIR"] == "/repo/current"

    init = next(c for c in gateway_pod.get("initContainers", []) if c["name"] == "git-sync-init")
    sidecar = next(c for c in gateway_pod["containers"] if c["name"] == "git-sync")
    for container in (init, sidecar):
        assert "@sha256:" in container["image"], container["image"]
        env = {e["name"]: e for e in container["env"]}
        assert env["GITSYNC_REPO"]["value"].startswith("http://gitea-http.") and env["GITSYNC_REPO"]["value"].endswith("/configuration.git")
        assert env["GITSYNC_ROOT"]["value"] == "/repo" and env["GITSYNC_LINK"]["value"] == "current"
        # git-sync (65533) on an emptyDir the pod's fsGroup owns: git >= 2.35 refuses it as dubious
        # ownership unless /repo, and only /repo, is a safe directory (git-sync v4.8.0, 2026-11-09)
        assert env["GITSYNC_GIT_CONFIG"]["value"] == "safe.directory:/repo"
        assert env["GITSYNC_PASSWORD"]["valueFrom"]["secretKeyRef"] == {"name": "gitea-token-gateway", "key": "token"}
        assert container["securityContext"]["readOnlyRootFilesystem"] is True
        assert container["securityContext"]["runAsNonRoot"] is True
        mounts = {m["mountPath"]: m for m in container["volumeMounts"]}
        assert "/repo" in mounts and not mounts["/repo"].get("readOnly")
    assert {e["name"]: e.get("value") for e in init["env"]}["GITSYNC_ONE_TIME"] == "true"
    assert "GITSYNC_ONE_TIME" not in {e["name"] for e in sidecar["env"]}

    gateway = gateway_pod["containers"][0]
    repo = next(m for m in gateway["volumeMounts"] if m["mountPath"] == "/repo")
    assert repo["readOnly"] is True and repo["name"] == "repo"
    assert any(v["name"] == "repo" and "emptyDir" in v for v in gateway_pod["volumes"])
    # dev runs layout 2 since 2026-10-06 (T-2647): the checkouts keep every registered project
    # beside the organization's own checkout (CC-85, CC-86).
    names = {c["name"] for c in gateway_pod["containers"] + gateway_pod.get("initContainers", [])}
    assert {"checkouts", "checkouts-init"} <= names
    assert gateway_env["JC_GATEWAY_PROJECTS_DIR"] == "/projects"


@requires_helmfile
def test_the_gateway_reads_a_persons_connectors_from_the_portals_internal_listener(gateway_env):
    """T-4546: `/api/mcp/me/{name}` needs the Portal's connector list (EP-123), on the internal
    listener the previews come from, which admits the gateway and its token alone."""
    previews = gateway_env["JC_GATEWAY_PREVIEWS_URL"]
    assert gateway_env["JC_GATEWAY_CONNECTORS_URL"] == previews.replace("/internal/previews", "/internal/connectors")
    assert gateway_env["JC_GATEWAY_CONNECTORS_URL"] == "http://portal.dev.svc.cluster.local:9090/internal/connectors"


@requires_helmfile
def test_the_gateway_posts_its_usage_counts_to_the_portals_internal_listener(gateway_env):
    """T-5118: the publishers' Usage tab reads the counts each gateway replica posts to the
    internal listener the previews come from, with the gateway's own token."""
    previews = gateway_env["JC_GATEWAY_PREVIEWS_URL"]
    assert gateway_env["JC_GATEWAY_USAGE_URL"] == previews.replace("/internal/previews", "/internal/usage")


def test_the_gateway_reads_its_revocations_without_an_agent_runner(rendered_variant):
    """T-5869: a gateway with a realm and no revocations list refuses to start (F-SEC1-024), and
    the k3d variants run no agent runner. The URL and the Portal's internal Service port follow
    the Portal, not the agent runner."""
    def without_agent_runner(tree):
        # The assistant needs the agent runner (its model goes through the proxy).
        drop_component(tree, "dev", "assistant")
        drop_component(tree, "dev", "agent-runner")

    docs = rendered_variant("dev", without_agent_runner)
    gateway = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-gateway")
    env = {e["name"]: e.get("value") for e in gateway["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_OIDC_ISSUER"]
    assert env["JC_GATEWAY_REVOCATIONS_URL"] == "http://portal.dev.svc.cluster.local:9090/internal/revocations"
    assert "JC_GATEWAY_PREVIEWS_URL" not in env
    service = next(d for d in docs if d.get("kind") == "Service" and d["metadata"]["name"] == "portal")
    assert {p["port"] for p in service["spec"]["ports"]} == {8080, 9090}
    assert any(
        d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "portal-internal-from-context-gateway"
        for d in docs
    )


def test_a_forge_outage_after_the_first_clone_keeps_the_gateway_in_service(gateway_pod):
    """T-2978: git-sync exits on its first failed fetch by default, and a crash-looping sidecar
    makes the only gateway pod NotReady: a Postgres restart took Gitea and with it every endpoint
    for about 30 s. The sidecar keeps its last checkout and retries forever instead; the one-time
    init still fails, since a gateway with no checkout has nothing to serve."""
    init = next(c for c in gateway_pod.get("initContainers", []) if c["name"] == "git-sync-init")
    sidecar = next(c for c in gateway_pod["containers"] if c["name"] == "git-sync")
    assert {e["name"]: e.get("value") for e in sidecar["env"]}.get("GITSYNC_MAX_FAILURES") == "-1"
    assert "GITSYNC_MAX_FAILURES" not in {e["name"] for e in init["env"]}


@requires_helmfile
def test_layout_two_checks_out_every_project_beside_the_organization(rendered_variant):
    """CC-86, T-2646: in layout 2 `jcctl checkouts` runs once after git-sync's first clone and
    beside the gateway after, writing the projects the gateway only reads; its image is pinned by
    digest, it reads the forge's read-only token from a mounted file, and nothing of it runs as
    root or writes its root filesystem."""
    from conftest import set_global

    docs = rendered_variant("dev", lambda tree: set_global(tree, "configRepo.layout", 2))
    pod = next(
        d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-gateway"
    )["spec"]["template"]["spec"]
    gateway = pod["containers"][0]
    env = {e["name"]: e.get("value") for e in gateway["env"]}
    assert (env["JC_GATEWAY_PROJECTS_DIR"], env["JC_GATEWAY_ASSEMBLY_DIR"]) == ("/projects", "/assembly/current")
    mounts = {m["mountPath"]: m for m in gateway["volumeMounts"]}
    assert mounts["/projects"]["readOnly"] is True
    assert not mounts["/assembly"].get("readOnly")

    inits = [c["name"] for c in pod["initContainers"]]
    assert inits.index("git-sync-init") < inits.index("checkouts-init"), "the organization first"
    init = next(c for c in pod["initContainers"] if c["name"] == "checkouts-init")
    sidecar = next(c for c in pod["containers"] if c["name"] == "checkouts")
    for container in (init, sidecar):
        assert "/checkouts:" in container["image"] and "@sha256:" in container["image"], container["image"]
        args = container["args"]
        assert args[:4] == ["--org-dir", "/repo/current", "--projects-dir", "/projects"]
        assert args[args.index("--forge") + 1].startswith("http://gitea-http.")
        assert args[args.index("--token-file") + 1] == "/var/run/forge/token"
        assert container["securityContext"]["readOnlyRootFilesystem"] is True
        assert container["securityContext"]["runAsNonRoot"] is True
        own = {m["mountPath"]: m for m in container["volumeMounts"]}
        assert own["/repo"]["readOnly"] is True and own["/var/run/forge"]["readOnly"] is True
        assert not own["/projects"].get("readOnly")
    assert "--once" in init["args"] and "--once" not in sidecar["args"]
    assert "readinessProbe" in sidecar

    token = next(v for v in pod["volumes"] if v["name"] == "forge-token")
    assert token["secret"]["secretName"] == "gitea-token-gateway"


@requires_helmfile
def test_the_space_has_a_published_model_at_the_major_the_demo_prints(seed, forge_seed):
    """DM-22, EP-49: `schema/v{major}` answers from a model attached to the space. Without a
    seeded DataModel the endpoint has no models at all and every schema URL of DEMO.md step 4
    is a 404, whatever the endpoint enables; that is exactly how dev shipped (T-0380)."""
    model = seed[("DataModel", "bb-air-quality")]
    assert model["spec"]["version"].startswith("1."), "the demo prints schema/v1"
    assert model["spec"]["lifecycle"] == "published", "only a published model may be referenced (DM-26)"
    assert model["spec"]["classes"] == ["AirQualityObserved"]

    # A published model commits all four artifacts (DM-02), and the gateway opens them beside
    # the manifest: a path the seed does not commit reads nothing and silently degrades the
    # schema to the derived one.
    folder = "projects/banskabystrica/spaces/ovzdusie/datamodels/"
    for path in model["spec"]["artifacts"].values():
        assert folder + path.removeprefix("./") in forge_seed, path
    assert folder + model["spec"]["linkml"].removeprefix("./") in forge_seed


@requires_helmfile
def test_the_public_grant_hides_exactly_the_two_attributes_the_demo_shows_hidden(seed, forge_seed):
    """EP-47: the schema is a projection of the same grant the data path enforces, so the two
    attributes DEMO.md step 4 calls hidden have to be attributes the model declares and the
    public policy leaves out. A grant that names none of them hides nothing at all."""
    granted = set(seed[("Policy", "public-read")]["spec"]["information"][0]["propertyNames"])
    compiled = json.loads(forge_seed["projects/banskabystrica/spaces/ovzdusie/datamodels/bb-air-quality.v1.schema.json"])
    declared = set(compiled["definitions"]["AirQualityObserved"]["properties"])

    assert granted <= declared, granted - declared
    # `id` and `type` are the format rather than the data: no grant lists them and the
    # projection never removes them (handlers/schema.rs STRUCTURAL).
    hidden = declared - granted - {"id", "type"}
    assert hidden == {"reliability", "refDevice"}, hidden

    entities = seed[("Policy", "public-read")]["spec"]["information"][0]["entities"]
    assert [e["type"] for e in entities] == ["AirQualityObserved"]


@requires_helmfile
def test_the_gateway_holds_no_delivery_key_and_nothing_generates_one(dev, gateway_pod):
    """T-3925: the broker delivers a notification itself under the engine (plan 5.5), so the
    gateway seals no subscriber and dials no subscriber: no delivery key, no egress base, and
    no generated Secret left behind to hold a key nobody reads."""
    env = {e["name"] for e in gateway_pod["containers"][0]["env"]}
    assert not env & {"JC_GATEWAY_DELIVERY_KEY", "JC_GATEWAY_EGRESS_URL"}, env
    assert not [
        d for d in dev
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "context-gateway-delivery-key"
    ]


def test_production_caps_the_websocket_entity_table_like_dev(rendered, gateway_env):
    # T-4352: the fan-out keeps up to JC_GATEWAY_WS_ENTITIES full entities per Endpoint; unset,
    # it is 50,000 each. Production holds dev's value, and the env map it adds merges with the
    # base's, so the broker address and every other base variable stay.
    deployment = next(d for d in rendered("production")
                      if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-gateway")
    env = {e["name"]: e.get("value") for e in deployment["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_GATEWAY_WS_ENTITIES"] == gateway_env["JC_GATEWAY_WS_ENTITIES"] == "10000"
    assert env["JC_GATEWAY_BROKER_URL"].startswith("http://context-broker.")
