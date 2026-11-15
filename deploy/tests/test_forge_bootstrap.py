"""The configuration repository the whole platform assumes (T-0399, CC-02, CC-08, OPS-26).

A forge with no repository in it is a forge nothing uses: the Portal's change proposals have
nowhere to land and the Context Gateway has nothing to project its endpoint table from. The
bootstrap Job is what creates the organization, the repository and the two tokens that read and
write it, and these tests are about the three ways that goes wrong quietly — a Job that can read
every Secret in the namespace, a Portal that got three of its four forge variables, and a token
rendered into the output in the clear.

The Job's own behaviour against a live forge is not testable here without a Gitea; what is
testable is that its script is valid shell and that every branch of it is guarded, which is what
the last test reads.
"""

import json
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml
from conftest import demo_id, seed_configmap

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

PROJECT_ROOT = Path(__file__).resolve().parent.parent

FORGE_VARIABLES = {"JC_GITEA_URL", "JC_GITEA_OWNER", "JC_GITEA_REPO", "JC_GITEA_TOKEN"}
TOKEN_SECRETS = {
    "gitea-token-portal",
    "gitea-token-gateway",
    "gitea-runner-registration",
    "app-registry",
    # What the lane refresher writes JC_LANE_TOKEN with (T-2636).
    "gitea-token-lane-secret",
    # The applications' machine user's (PF-106, T-2856).
    "gitea-token-apps",
}


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


@pytest.fixture(scope="module")
def job(local):
    return next(
        d for d in local
        if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap"
    )


@pytest.fixture(scope="module")
def script(job):
    return job["spec"]["template"]["spec"]["containers"][0]["args"][0]


@requires_helmfile
def test_the_job_runs_the_image_the_forge_already_runs(local, job):
    """One digest to re-pin, and nothing extra for the node to pull (OPS-28)."""
    container = job["spec"]["template"]["spec"]["containers"][0]
    forge = next(
        d for d in local
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "gitea"
    )
    forge_image = forge["spec"]["template"]["spec"]["containers"][0]["image"]
    assert "@sha256:" in container["image"]
    assert container["image"].split("@")[1] == forge_image.split("@")[1]


@requires_helmfile
def test_the_job_may_touch_only_the_secrets_it_writes(local):
    """The forge namespace also holds the database credential and the administrator password.
    The Job writes the two forge tokens and the runner's registration token (ADR-N-028), and
    may name nothing else.

    A Role with `get` on secrets and no `resourceNames` would let this Job read both, and it
    authenticates with one of them."""
    role = next(
        d for d in local
        if d.get("kind") == "Role" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    for rule in role["rules"]:
        if rule["resources"] == ["deployments"]:
            # PF-106: what read a token at start is restarted when it is minted again; `patch`
            # on those names, nothing that reads or lists. The runners read their registration
            # token at start too and restart when it moves organization (T-2969).
            assert rule["apiGroups"] == ["apps"] and rule["verbs"] == ["patch"], rule
            allowed = {"portal", "context-gateway", "agent-proxy", "gitea-runner", "gitea-runner-rust"}
            assert set(rule["resourceNames"]) <= allowed, rule
            continue
        assert rule["resources"] == ["secrets"]
        if "create" in rule["verbs"]:
            # `create` cannot be limited by name; it must therefore be the only verb in its rule.
            assert rule["verbs"] == ["create"]
        else:
            assert set(rule.get("resourceNames", [])) == TOKEN_SECRETS
            assert "list" not in rule["verbs"] and "watch" not in rule["verbs"]

    binding = next(
        d for d in local
        if d.get("kind") == "RoleBinding" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    assert binding["roleRef"]["kind"] == "Role", "a ClusterRole here would be namespace-wide"
    assert [s["name"] for s in binding["subjects"]] == ["gitea-bootstrap"]

    # F-R2DEP2-005: `create` cannot name a Secret, so an admission policy holds it to the token
    # Secrets and to types the cluster fills with nothing (never service-account-token).
    policy = next(d for d in local if d.get("kind") == "ValidatingAdmissionPolicy"
                  and d["metadata"]["name"].endswith("-gitea-bootstrap-secrets"))
    namespace = binding["metadata"]["namespace"]
    assert policy["spec"]["failurePolicy"] == "Fail"
    assert policy["spec"]["matchConditions"][0]["expression"] == (
        f'request.userInfo.username == "system:serviceaccount:{namespace}:gitea-bootstrap"')
    check = policy["spec"]["validations"][0]["expression"]
    assert "object.metadata.name in " + json.dumps(sorted(TOKEN_SECRETS), separators=(",", ":")) in check, check
    assert 'object.type in ["Opaque", "kubernetes.io/dockerconfigjson"]' in check, check
    bound = next(d for d in local if d.get("kind") == "ValidatingAdmissionPolicyBinding"
                 and d["spec"]["policyName"] == policy["metadata"]["name"])
    assert bound["spec"]["validationActions"] == ["Deny"]


@requires_helmfile
def test_the_job_says_why_it_holds_an_api_token(job):
    """The runtime policy strips the token from a pod that does not justify wanting one."""
    annotations = job["spec"]["template"]["metadata"]["annotations"]
    reason = annotations.get("security.joinedcontext.com/api-access-reason", "")
    assert len(reason) > 40, "the annotation is the justification, not a checkbox"
    assert job["spec"]["template"]["spec"]["automountServiceAccountToken"] is True


@requires_helmfile
def test_the_portal_gets_all_four_forge_variables_or_none(local):
    """`GiteaClient::from_env` refuses a partial set, so three of four is a Portal that will not start."""
    portal = next(
        d for d in local
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"
    )
    env = portal["spec"]["template"]["spec"]["containers"][0]["env"]
    present = {e["name"] for e in env} & FORGE_VARIABLES
    assert present == FORGE_VARIABLES, f"missing {FORGE_VARIABLES - present}"

    token = next(e for e in env if e["name"] == "JC_GITEA_TOKEN")
    assert "value" not in token, "a token in the manifest is a token in Git"
    assert token["valueFrom"]["secretKeyRef"]["name"] == "gitea-token-portal"

    url = next(e for e in env if e["name"] == "JC_GITEA_URL")["value"]
    assert url.startswith("http://gitea-http."), "the in-cluster Service, not the public host"


@requires_helmfile
def test_the_forge_is_deployed_before_what_reads_its_tokens(local):
    """A `secretKeyRef` naming a Secret that does not exist yet holds the pod in ConfigError."""
    globals_file = PROJECT_ROOT / "defaults/environment/global.yaml"
    components = yaml.safe_load(globals_file.read_text())["components"]
    assert components.index("gitea") < components.index("portal")
    assert components.index("gitea") < components.index("context-gateway")


@requires_helmfile
def test_no_token_is_rendered_in_the_clear(local):
    """Nothing in the output carries a forty-character forge token, because none exists yet.

    The Job mints them at run time and writes them straight into a Secret, which is the whole
    reason it exists; a token appearing here would mean somebody had gone back to generating one."""
    text = yaml.safe_dump_all(local)
    for secret in TOKEN_SECRETS:
        assert not any(
            d.get("kind") == "Secret" and d["metadata"]["name"] == secret for d in local
        ), f"{secret} is written by the Job, never rendered"
    assert "ADMIN_PASSWORD" not in text or "valueFrom" in text


@requires_helmfile
def test_the_script_is_valid_shell(script):
    """`sh -n` on the inlined script.

    A shell script inside a Helm template inside a Job is three levels away from anything that
    would notice an unbalanced quote, and the first thing that does notice is a CrashLoopBackOff
    on the cluster."""
    result = subprocess.run(["sh", "-n"], input=script, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr


def test_a_seed_file_goes_to_the_forge_in_a_body_file_not_an_argument(script):
    """One argument is capped at 128 KiB (MAX_ARG_STRLEN), and the BBSK open-data schema is 336 kB,
    448 kB as base64: passed inline, `curl` died with "Argument list too long" and the Job with it,
    which stopped apply 86 (2026-10-16, T-4255). The body is written by the shell's own printf
    into a file and handed to curl as `-d @file`."""
    seed = script[script.index("seed_repo() {"):]
    seed = seed[: seed.index("5d.")]
    # One commit of the whole seed (T-4709), and its body in a file.
    assert seed.count("-d @$WORK/request") == 1, seed
    assert '\\"content\\":\\"$encoded' not in seed
    # The shell's printf takes a value past the cap (a builtin, never exec'd); curl would not.
    result = subprocess.run(
        ["sh", "-c", 'big=$(head -c 300000 /dev/zero | tr "\\0" x); printf \'{"content":"%s"}\' "$big" > /dev/null'],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr


@requires_helmfile
def test_every_step_is_idempotent(script):
    """The Job runs on every apply, so each step has to find its own work already done.

    Read as text rather than executed, because executing it needs a forge. The three markers are
    the three branches that make a second run a no-op."""
    # Every organization, the configuration's and (PF-106) the applications', through one function.
    assert 'status=$(forge_code GET "/api/v1/orgs/$1")' in script
    assert 'ensure_org "$ORG"' in script
    # Every repository, the organization's and (layout 2) each project's, through one function.
    assert 'status=$(forge_code GET "/api/v1/repos/$ORG/$1")' in script
    assert 'ensure_repo "$REPO"' in script
    # The token check has to use an endpoint the read-only token may reach, or the gateway's
    # token is re-minted on every apply and the pod holding it is left with a dead credential.
    # The pull token (AP-108) may read packages only and names its own check in CHECK_PATH.
    assert '"$GITEA_URL${CHECK_PATH:-/api/v1/repos/$ORG/$REPO}")" = 200 ]; then' in script
    code = "\n".join(line for line in script.splitlines() if not line.lstrip().startswith("#"))
    assert '"$GITEA_URL/api/v1/user"' not in code, (
        "checking /user re-mints the read-only token on every apply: it has no read:user scope"
    )


@requires_helmfile
def test_the_job_reaches_the_forge_and_the_api_server_and_nothing_else(local):
    policy = next(
        d for d in local
        if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    ports = {
        port["port"]
        for rule in policy["spec"]["egress"]
        for port in rule.get("ports", [])
    }
    assert ports == {53, 3000, 443, 6443}
    assert "ingress" not in policy["spec"] or not policy["spec"]["ingress"], (
        "nothing calls the Job"
    )


def _component_list(path):
    """The ordered `components:` list of an environment file, or None when it has none.

    Read as text and not as YAML because these files are Go templates: `domain: {{ env ... }}`
    a few lines below is not YAML, and the list itself never needs a template to be understood.
    """
    lines = path.read_text().splitlines()
    try:
        start = lines.index("components:")
    except ValueError:
        return None
    names = []
    for line in lines[start + 1:]:
        if line.startswith("  - "):
            names.append(line[4:].strip())
        elif line.strip() and not line.startswith("  #"):
            break
    return names


def test_every_environment_lists_gitea_before_the_components_that_mount_its_tokens():
    """A component ordered after gitea's bootstrap Job cannot start, and takes the apply with it.

    `secretKeyRef` on a Secret that does not exist yet holds the pod in
    CreateContainerConfigError, and `helmfile apply` waits on that rollout before it reaches the
    component that would create the Secret. Nothing recovers: the apply times out with the forge
    never installed. The default list in `defaults/environment/global.yaml` orders gitea first
    for this reason, and an environment that overrides the list has to keep that property.
    """
    consumers = sorted(
        component.name
        for component in (PROJECT_ROOT / "components").iterdir()
        if component.is_dir()
        and component.name != "gitea"
        and any(
            secret in values.read_text()
            for values in component.rglob("values/**/*.gotmpl")
            for secret in TOKEN_SECRETS
        )
    )
    assert consumers, "no component mounts a forge token; this test has stopped testing anything"

    environments = [
        *(PROJECT_ROOT / "ci/example-deployments/environments").glob("*/global.yaml*"),
        PROJECT_ROOT / "defaults/environment/global.yaml",
    ]
    for environment in environments:
        components = _component_list(environment)
        if components is None or "gitea" not in components:
            continue
        for consumer in consumers:
            if consumer not in components:
                continue
            assert components.index("gitea") < components.index(consumer), (
                f"{environment.relative_to(PROJECT_ROOT)} deploys {consumer} before gitea, "
                f"so {consumer} waits for a Secret the apply has not reached yet"
            )


@requires_helmfile
def test_a_signed_in_person_lands_in_a_forge_team_that_reads(local, job, script):
    """PF-79, PF-80: without the claim, the source and the team, a person with a Keycloak
    session meets a 404 on a private repository, which is what the forge is.

    Three parts have to agree, and each has broken on its own: the realm carries the group
    every person gets, the forge's OpenID Connect source maps that claim onto a team of the
    organization, and the bootstrap Job creates the team before the first login tries to
    join it."""
    init = next(
        d for d in local
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-init"
    )["stringData"]["configure_gitea.sh"]
    assert '--group-claim-name "groups"' in init
    assert '--group-team-map "{\\"/platform-readers\\":{\\"joinedcontext\\":[\\"readers\\"]}}"' in init

    teams = next(e for e in job["spec"]["template"]["spec"]["containers"][0]["env"]
                 if e["name"] == "TEAMS")
    assert teams["value"].split() == ["readers"], "every team the map names is created here"
    assert '"permission":\\"read\\"' in script or '\\"permission\\":\\"read\\"' in script, \
        "a forge team reads; merging stays the Portal's approval (PF-80)"
    assert '\\"includes_all_repositories\\":true' in script

    realm = next(
        d for d in local
        if d.get("kind") == "Secret"
        and d["metadata"]["name"].endswith("keycloak-config-cli-config-realms")
    )
    import base64
    import json

    parsed = json.loads(base64.b64decode(realm["data"]["local.json"]))
    assert {"name": "platform-readers", "path": "/platform-readers"} in parsed["groups"]
    assert "/platform-readers" in parsed["defaultGroups"], "anyone created later joins it too"
    # The App sharing matrix's people stay out of it on purpose (T-4911, `platformReaders: false`).
    outside = {name for name, person in yaml.safe_load(
        (Path(__file__).resolve().parent.parent / "components/keycloak/demo-users.yaml").read_text()
    ).items() if person.get("platformReaders") is False}
    assert outside, "the sharing matrix's people are seeded"
    for user in parsed["users"]:
        if "credentials" in user:
            assert ("/platform-readers" in user.get("groups", [])) != (user["username"].split("@")[0] in outside), \
                "a demo login is created by this same import, which defaultGroups does not reach"
    gitea = next(c for c in parsed["clients"] if c["clientId"] == "gitea")
    mapper = next(m for m in gitea["protocolMappers"]
                  if m["protocolMapper"] == "oidc-group-membership-mapper")
    assert mapper["config"]["claim.name"] == "groups"
    assert mapper["config"]["full.path"] == "true", "the team map names /{group} (ADR-N-067 2.2)"
    assert mapper["config"]["id.token.claim"] == "true"


@requires_helmfile
def test_project_visibility_is_a_setting_the_seed_spells_right(local):
    """PF-61, T-5607: `members` (on an Organization's `projects`, or on one Project) keeps the
    organization-wide reader out, so departments of one project stay apart. Helsinki stays
    `organization`: DEMO.md's demo.viewer reads every project. A misspelt value would be refused
    by the Portal and the whole seed with it."""
    docs = [
        doc
        for key, text in seed_configmap(local)["data"].items()
        if key.endswith((".yaml", ".yml"))
        for doc in yaml.safe_load_all(text)
        if isinstance(doc, dict)
    ]
    organizations = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "Organization"}
    assert organizations["hel"]["spec"].get("projects", {}).get("visibility", "organization") == "organization"
    for doc in docs:
        if doc.get("kind") == "Organization":
            value = doc["spec"].get("projects", {}).get("visibility", "organization")
        elif doc.get("kind") == "Project":
            value = doc["spec"].get("visibility", "organization")
        else:
            continue
        assert value in ("organization", "members"), f"{doc['kind']} {doc['metadata']['name']}: {value!r}"


@requires_helmfile
def test_the_seed_binds_every_signed_in_person_to_the_viewer_role(local):
    """PF-56, PF-59: reading is a verb, so a person with no binding reads nothing.

    Since ADR-N-067 (T-6415) the viewers are named one by one: the realm's default group binds
    no project, or every new account would read every project of the Organization again."""
    seed = seed_configmap(local)["data"]
    role = yaml.safe_load(seed["users__roles__viewer.yaml"])
    assert role["kind"] == "Role" and role["metadata"]["name"] == "viewer"
    assert [rule["verbs"] for rule in role["spec"]["rules"]] == [["read"]], "the viewer changes nothing"
    kinds = set(role["spec"]["rules"][0]["kinds"])
    steward = yaml.safe_load(seed["users__roles__steward.yaml"])
    assert kinds >= set(steward["spec"]["rules"][0]["kinds"]), "every kind a steward proposes is readable"

    binding = yaml.safe_load(seed["users__assignments__viewers.yaml"])
    assert binding["spec"]["role"] == "viewer"
    assert binding["spec"]["scope"] == {"organization": "hel"}
    assert all("user" in subject for subject in binding["spec"]["subjects"]), "no group is a viewer of every project"


@requires_helmfile
def test_the_seed_carries_the_organization_its_settings_and_a_project_manifest(local):
    """PF-02, PF-41, CC-70, MF-01: one repository, one Organization, and no project directory
    without its manifest.

    Nothing on dev held `org.yaml`, so the domain of every entity URN, the locales and the
    project rules had no home, and two of three project directories had no Project at all
    (T-0901, T-0902)."""
    seed = seed_configmap(local)["data"]

    assert "org.yaml" not in seed, "the Organizations live at organizations/{name}.yaml (PF-121)"
    organizations = {
        key: yaml.safe_load(text) for key, text in seed.items() if key.startswith("organizations__")
    }
    assert set(organizations) == {
        "organizations__hel.yaml", "organizations__zilina.yaml", "organizations__barcelona.yaml",
        "organizations__bratislava.yaml",
    }
    by_name = {o["metadata"]["name"]: o for o in organizations.values()}
    assert [n for n, o in by_name.items() if o["spec"].get("installation")] == ["hel"], (
        "exactly one installation Organization (PF-121)"
    )
    org = by_name["hel"]
    assert org["kind"] == "Organization"
    assert org["spec"]["domain"] == "hel.fi", "the {orgDomain} of every entity URN (PF-41)"
    for each in by_name.values():
        assert each["spec"]["defaultLocale"] in each["spec"]["locales"]

    settings = yaml.safe_load(seed["platform-settings.yaml"])
    assert settings["modelTools"]["generatorVersion"].startswith("linkml-")
    assert "@sha256:" in settings["modelTools"]["image"], "the image is pinned by digest (DM-19)"

    projects = {
        key.split("__")[1]
        for key in seed
        if key.startswith("projects__")
    }
    for project in sorted(projects):
        manifest = seed.get(f"projects__{project}__project.yaml")
        assert manifest, f"the project directory {project!r} carries no project.yaml (MF-01)"
        parsed = yaml.safe_load(manifest)
        assert parsed["kind"] == "Project" and parsed["metadata"]["name"] == project
        owner = by_name.get(parsed["spec"]["organizationRef"])
        assert owner, f"project {project!r} names an Organization the seed has (PF-122)"
        if not owner["spec"].get("installation"):
            domain = parsed["spec"].get("domain", owner["spec"]["domain"])
            assert domain == owner["spec"]["domain"] or domain.endswith("." + owner["spec"]["domain"]), (
                f"project {project!r} publishes under its Organization's domain (PF-122)"
            )
    for own in ("zilina", "barcelona", "bratislava"):
        assert yaml.safe_load(seed[f"projects__{own}__project.yaml"])["spec"]["organizationRef"] == own
    assert {
        yaml.safe_load(seed[f"projects__{p}__project.yaml"])["spec"]["organizationRef"]
        for p in projects - {"zilina", "barcelona", "bratislava"}
    } == {"hel"}, "the other demo projects stay under hel (ADR-N-058)"


@requires_helmfile
def test_every_project_of_the_seed_runs_under_a_quota_it_can_hold(local):
    """PF-73, PF-75, T-2244: a platform without limits has nothing to show for them.

    Neither the organization nor either project declared a quota, so every dimension of
    `GET /api/v1/projects/{project}` answered with no limit, the Quota card read "no limit"
    eight times and the pre-emptive warnings of T-0526 and T-1594 could not appear at all.
    The organization now carries the default every project starts from, and the two smaller
    projects carry their own, tighter one."""
    seed = seed_configmap(local)["data"]
    dimensions = {
        "contextSpaces", "residentPipelines", "publicEndpoints", "apps",
        "ingestEventsPerSecond", "agentRunsPerDay", "entitiesPerSpace", "requestsPerMinute",
    }

    org = yaml.safe_load(seed["organizations__hel.yaml"])
    default = org["spec"]["projects"]["quota"]
    assert set(default) == dimensions, "the organization declares every dimension the card lists"
    assert all(isinstance(v, int) and v >= 1 for v in default.values()), default

    for key, text in seed.items():
        if not key.startswith("projects__") or not key.endswith("__project.yaml"):
            continue
        project = key.split("__")[1]
        # `mqtt` is the MQTT allowance, no PF-73 dimension (portal src/quotas.rs effective()).
        own = {k: v for k, v in (yaml.safe_load(text)["spec"].get("quotas") or {}).items() if k != "mqtt"}
        if own:
            assert set(own) == dimensions, f"{project} declares every dimension or none"
            assert all(isinstance(v, int) and v >= 1 for v in own.values()), own
        quota = own or default
        # A quota below what the seed already publishes would refuse the seed's own endpoint
        # the next time anybody proposes it (portal src/quotas.rs, PF-74).
        asked = [
            yaml.safe_load(t)["spec"].get("rateLimits", {}).get("requestsPerMinute", 0)
            for k, t in seed.items()
            if k.startswith(f"projects__{project}__") and k.endswith(".yaml")
            and not k.endswith(".linkml.yaml")
            and (yaml.safe_load(t) or {}).get("kind") == "Endpoint"
        ]
        if asked:
            assert quota["requestsPerMinute"] >= max(asked), (
                f"{project}: an endpoint asks {max(asked)}/min and the quota allows "
                f"{quota['requestsPerMinute']}"
            )


@requires_helmfile
def test_the_seed_carries_a_hub_whose_two_members_are_inside_its_own_project(local):
    """T-1209, EP-70, EP-71, PF-48: a space of registrations, one Endpoint over it, and two
    members the hub's broker may actually read.

    The federated suites skipped themselves because there was no hub on the cluster to point
    them at. What makes this one a federation rather than one tenant with two types in it is
    that its members are two different spaces: the live context and the indicator space beside
    it. PF-48 refuses a member outside the hub's project, so both are `helsinki`."""
    seed = seed_configmap(local)["data"]
    hub = "projects__helsinki__spaces__helsinki-hub"

    space = yaml.safe_load(seed[f"{hub}__space.yaml"])
    assert space["kind"] == "ContextSpace" and space["metadata"]["namespace"] == "helsinki"

    endpoint = yaml.safe_load(seed[f"{hub}__endpoints__helsinki-hub.yaml"])
    assert endpoint["spec"]["contextSpaceRef"] == "helsinki-hub"
    assert endpoint["spec"]["audience"] == "public"
    assert "mcp" in endpoint["spec"]["enabledRepresentations"], (
        "the hub's MCP surface is what a connector reads as one federated dataset (AG-30)"
    )

    registrations = {
        key.rsplit("__", 1)[1]: yaml.safe_load(value)
        for key, value in seed.items()
        if key.startswith(f"{hub}__registrations__")
    }
    assert set(registrations) == {"transport.yaml", "indicators.yaml"}, registrations.keys()

    spaces = set()
    for name, csr in registrations.items():
        assert csr["spec"]["contextSpaceRef"] == "helsinki-hub", name
        # Never an address: a hub answer that leaked a member's internal URL would tell a
        # public caller where to knock next (EP-71).
        assert "endpoint" not in csr["spec"], f"{name} names an address"
        member = csr["spec"]["endpointRef"]["name"]
        assert csr["spec"]["federation"]["identity"] == "serviceAccount", name
        assert csr["spec"]["federation"]["serviceAccountRef"]["name"] == "hub-reader", name
        assert csr["spec"]["information"], name
        target = yaml.safe_load(
            next(
                value for key, value in seed.items()
                if key.endswith(f"__endpoints__{member}.yaml")
            )
        )
        assert target["metadata"]["namespace"] == "helsinki", (
            f"{name} federates {member}, which is outside the hub's project (PF-48)"
        )
        spaces.add(target["spec"]["contextSpaceRef"])
    assert len(spaces) == 2, f"both members answer from one space, which federates nothing: {spaces}"

    # The forward's identity reads and never writes: a hub that forwarded as an account able to
    # write would make every reader of the hub a writer of the spaces behind it.
    reader = yaml.safe_load(seed["projects__helsinki__access__serviceaccounts__hub-reader.yaml"])
    assert {role["role"] for role in reader["spec"]["roles"]} == {"space-reader"}
    assert {role["scope"]["contextSpace"] for role in reader["spec"]["roles"]} == {
        "helsinki",
        "helsinki-kpi",
    }


@requires_helmfile
def test_a_role_that_may_act_on_an_endpoint_may_act_on_its_projection(local):
    """T-1222, MP-01: an Endpoint that names a projection carries a `ModelProjection`, and the
    Portal proposes the two as one import — the endpoint form's Check posts the bundle to
    `/import?dryRun=All`, which holds every kind in it to `propose`.

    The Helsinki seed granted `Endpoint` and not `ModelProjection`, so the demo steward could
    not check their own endpoint proposal: `403 no role grants propose on ModelProjection in
    project helsinki (PF-50)`, found by the share recording take on dev (2026-09-18)."""
    seed = seed_configmap(local)["data"]
    roles = {
        key.rsplit("__", 1)[1].removesuffix(".yaml"): yaml.safe_load(value)
        for key, value in seed.items()
        if key.startswith("users__roles__")
    }
    assert roles, "the seed carries no roles"

    for name, role in sorted(roles.items()):
        for rule in role["spec"]["rules"]:
            kinds = set(rule.get("kinds", []))
            if "Endpoint" not in kinds:
                continue
            assert "ModelProjection" in kinds, (
                f"role '{name}' may {rule.get('verbs')} an Endpoint but not the projection it "
                "carries, so its own proposal cannot be checked (MP-01)"
            )


@requires_helmfile
def test_one_named_keycloak_group_administers_the_forge_and_no_team_writes(local, script):
    """T-1421: `--admin-group` names one Keycloak group, so the forge is administered without
    the local `gitea_admin` password; every mapped team still only reads, so a push to `main`
    is refused and a Change stays the one way in (CC-41, PF-80)."""
    init = next(
        d for d in local
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-init"
    )["stringData"]["configure_gitea.sh"]
    # Once on `add-oauth`, once on `update-oauth`: both name the same one group.
    assert init.count("--admin-group") == init.count('--admin-group "/forge-admins"') == 2
    assert '\\"permission\\":\\"write\\"' not in script and '\\"permission\\":\\"admin\\"' not in script


def _forge_init(docs):
    return next(
        d for d in docs
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-init"
    )["stringData"]["configure_gitea.sh"]


@requires_helmfile
def test_a_listed_projects_groups_land_in_its_own_teams_and_leaving_one_leaves_the_team(
    local, job, rendered_variant
):
    """PF-87, T-2655: each project the environment lists maps `{slug}-readers` and
    `{slug}-writers` onto the teams of the same names, and nothing else changes; the bootstrap
    does not create those teams, the Portal's reconciler does, with its own description. The
    removal flag rides on both the add and the update, so a person whose binding went leaves the
    team at their next sign-in."""
    init = _forge_init(local)
    assert init.count('--group-team-map-removal "true"') == 2, init

    def listed(tree):
        path = tree / "components/gitea/default-environment.yaml.gotmpl"
        text = path.read_text()
        assert "    projectTeams: []\n" in text
        path.write_text(text.replace("    projectTeams: []\n", "    projectTeams: [doprava]\n"))

    docs = rendered_variant("local", listed)
    init = _forge_init(docs)
    owner = "joinedcontext"
    expected = (
        '{\\"/doprava-readers\\":{\\"%s\\":[\\"doprava-readers\\"]},'
        '\\"/doprava-writers\\":{\\"%s\\":[\\"doprava-writers\\"]},'
        '\\"/platform-readers\\":{\\"%s\\":[\\"readers\\"]}}' % (owner, owner, owner)
    )
    assert f'--group-team-map "{expected}"' in init, init
    variant_job = next(
        d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    teams = next(e for e in variant_job["spec"]["template"]["spec"]["containers"][0]["env"]
                 if e["name"] == "TEAMS")
    assert teams["value"].split() == ["readers"], "the project teams are the Portal's to create"


def test_a_listed_project_has_its_repository_before_its_teams_are_given_it(script):
    """T-3181: Zilina was listed in projectTeams before its seed had ever been pushed, so the
    team grant met a 404 and every apply stopped at the bootstrap. The repository is ensured
    (created at layout 2 with its `.jc/layout`) before the first grant to it."""
    loop = script[script.index('for slug in ${PROJECT_TEAMS:-}; do'):]
    loop = loop[: loop.index("done")]
    assert loop.index('project_repo "$slug"') < loop.index('project_team "$slug" readers read'), loop
    assert script.index("project_repo() {") < script.index('for slug in ${PROJECT_TEAMS:-}; do')


@requires_helmfile
def test_a_project_repository_grants_its_machine_users_before_its_rule_names_one(script):
    """T-6438: the forge drops a whitelisted user without write access from a branch rule, with
    no error. A rule set before the grant left a new repository with nobody who may push, so
    the seed answered 403 and each retry repaired one project: every clean install failed
    while dev, whose repositories already existed, never met it."""
    step = script[script.index("project_repo() {"):]
    step = step[: step.index("}")]
    assert step.index("ensure_repo") < step.index('grant "$PORTAL_USER" write') < step.index("protect"), step
    assert script.count('protect "$') == 2, "every project goes through project_repo, the organization repository alone"
    assert script.index('grant "$PORTAL_USER" write "$REPO"') < script.index('protect "$REPO"')


@requires_helmfile
def test_a_failed_job_leaves_a_pod_to_read_and_no_secret_in_its_log(job, script):
    """T-6438: under OnFailure the Job controller deleted the pod at the backoff limit, so eight
    variants failed in ci-full with no log of why. The diagnostics print what a failed pod
    said, so the script prints no credential."""
    assert job["spec"]["template"]["spec"]["restartPolicy"] == "Never"
    assert "set -x" not in script
    printed = [line for line in script.splitlines() if re.search(r"\b(echo|printf)\b", line) and "$" in line]
    leaks = [line for line in printed if re.search(r"\$\{?\w*(TOKEN|PASSWORD)\b", line)]
    assert not leaks, leaks


@requires_helmfile
def test_each_organization_has_its_keycloak_subtree_and_administrator(local):
    """ADR-N-058 2.5 (T-5242): keycloak-config-cli makes the root of Žilina's group subtree, its
    readers' binding names that group, and its administrator is a demo person in it."""
    import base64
    import json

    realm = next(
        d for d in local
        if d.get("kind") == "Secret"
        and d["metadata"]["name"].endswith("keycloak-config-cli-config-realms")
    )
    parsed = json.loads(base64.b64decode(realm["data"]["local.json"]))
    seed = seed_configmap(local)["data"]
    # Barcelona's (T-5260) and Bratislava's (T-5271) the same way.
    for name in ("zilina", "barcelona", "bratislava"):
        assert {"name": name, "path": f"/{name}"} in parsed["groups"]
        admin = next(u for u in parsed["users"] if u["username"].startswith(f"demo.{name}@"))
        assert f"/{name}" in admin["groups"]
        assert "portal-approver" in admin["realmRoles"]
        assert any(c["type"] == "otp" and c.get("id") for c in admin["credentials"]), "seeded OTP keeps an id (T-5214)"

        viewers = yaml.safe_load(seed[f"users__assignments__{name}-viewers.yaml"])
        assert viewers["spec"]["subjects"] == [{"group": name, "source": "provider"}]
        admins = yaml.safe_load(seed[f"users__assignments__{name}-admins.yaml"])
        assert admins["spec"]["subjects"] == [{"user": demo_id(f"demo.{name}")}], "the dev realm's id of the person"
        assert admins["spec"]["scope"] == {"organization": name}
