"""What the Portal may write on the cluster, and where (T-0411, AP-13, AP-15, AP-18).

The Portal's app reconciler applies a Deployment, a Service, a Secret and a NetworkPolicy per
app, in the project's apps namespace under the ClusterRole it binds there. That is the largest
privilege the platform hands any of its own workloads, so the grant is decided in the render and
asserted here: in its own namespace, which holds the platform's workloads, the pipeline runner's
restart patch and the Secrets it hands over (T-5094), and a token mount without which the
reconciler deploys nothing at all. Cluster-wide it may create a project's apps namespace and bind
that role in it, fenced by an admission policy (AP-116, AP-117).
"""

import shutil
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent

requires_helmfile = pytest.mark.skipif(
    shutil.which("helmfile") is None, reason="helmfile not installed"
)

# Exactly what an App compiles into, and nothing else (Architecture/16 section 5).
GRANTED = {
    ("apps", "deployments"),
    ("", "secrets"),
}


def portal_deployment(docs):
    return next(
        d
        for d in docs
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"
    )


def portal_env(docs):
    pod = portal_deployment(docs)["spec"]["template"]["spec"]
    container = next(c for c in pod["containers"] if c["name"].startswith("portal"))
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def portal_role(docs):
    return next(
        d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "portal"
    )


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_the_portal_may_roll_the_pipeline_runner_in_its_namespace_and_nothing_else(
    rendered, environment
):
    docs = rendered(environment)
    role = portal_role(docs)

    granted = {
        (group, resource)
        for rule in role["rules"]
        for group in rule["apiGroups"]
        for resource in rule["resources"]
    }
    assert granted == GRANTED, f"{environment}: the Portal's Role grants {granted}"
    # The release namespace holds the platform's workloads: the Portal may roll the pipeline
    # runner and touch no other Deployment, Service or NetworkPolicy there (T-5094).
    deployments = [rule for rule in role["rules"] if "deployments" in rule["resources"]]
    assert deployments == [
        {
            "apiGroups": ["apps"],
            "resources": ["deployments"],
            "resourceNames": ["pipeline-runner"],
            "verbs": ["get", "patch"],
        }
    ], f"{environment}: the Portal may write {deployments}"

    # Cluster-wide only what creates a project's apps namespace (AP-116, AP-117), asserted below.
    cluster_wide = {
        d["metadata"]["name"]
        for d in docs
        if d.get("kind") in {"ClusterRole", "ClusterRoleBinding"}
        and "portal" in d["metadata"]["name"]
    }
    slug = portal_env(docs)["JC_PORTAL_RELEASE"]
    assert cluster_wide == {f"{slug}-portal-apps", f"{slug}-portal-namespaces"}, (
        f"{environment}: the Portal was granted {cluster_wide}"
    )


# What the Portal may do cluster-wide: create and delete a project's apps namespace and bind the
# apps role in it (AP-116). RBAC cannot name a prefix, so the admission policy is the bound.
NAMESPACE_GRANT = {
    ("", "namespaces", ("create", "delete", "get", "patch"), ()),
    ("rbac.authorization.k8s.io", "rolebindings", ("create", "get", "patch"), ()),
    ("rbac.authorization.k8s.io", "clusterroles", ("bind",), ("{slug}-portal-apps",)),
}


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_the_portals_cluster_wide_grant_is_the_namespace_one_and_a_policy_fences_it(
    rendered, environment
):
    docs = rendered(environment)
    env = portal_env(docs)
    slug = env["JC_PORTAL_RELEASE"]
    by_name = {(d.get("kind"), d["metadata"]["name"]): d for d in docs}
    pod = portal_deployment(docs)["spec"]["template"]["spec"]
    assert env["JC_PORTAL_SERVICE_ACCOUNT"] == pod["serviceAccountName"]

    role = by_name[("ClusterRole", f"{slug}-portal-namespaces")]
    granted = {
        (
            group,
            resource,
            tuple(sorted(rule["verbs"])),
            tuple(rule.get("resourceNames", ())),
        )
        for rule in role["rules"]
        for group in rule["apiGroups"]
        for resource in rule["resources"]
    }
    expected = {
        (g, r, v, tuple(n.format(slug=slug) for n in names))
        for g, r, v, names in NAMESPACE_GRANT
    }
    assert granted == expected

    # The apps role is granted to nobody here; the Portal binds it inside each apps namespace.
    apps = by_name[("ClusterRole", f"{slug}-portal-apps")]
    assert {r for rule in apps["rules"] for r in rule["resources"]} == {
        "deployments",
        "services",
        "secrets",
        "networkpolicies",
    }
    bound = [
        d["roleRef"]["name"]
        for d in docs
        if d.get("kind") in {"ClusterRoleBinding", "RoleBinding"}
    ]
    assert f"{slug}-portal-apps" not in bound

    binding = by_name[("ClusterRoleBinding", f"{slug}-portal-namespaces")]
    namespace = portal_deployment(docs)["metadata"]["namespace"]
    assert binding["subjects"] == [
        {"kind": "ServiceAccount", "name": pod["serviceAccountName"], "namespace": namespace}
    ]

    policy = by_name[("ValidatingAdmissionPolicy", f"{slug}-portal-apps-namespaces")]
    assert policy["spec"]["failurePolicy"] == "Fail"
    assert (
        f'"system:serviceaccount:{namespace}:{pod["serviceAccountName"]}"'
        in policy["spec"]["matchConditions"][0]["expression"]
    )
    fence = by_name[("ValidatingAdmissionPolicyBinding", f"{slug}-portal-apps-namespaces")]
    assert fence["spec"] == {
        "policyName": f"{slug}-portal-apps-namespaces",
        "validationActions": ["Deny"],
    }


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_the_role_is_in_the_namespace_the_portal_is_told_to_deploy_into(
    rendered, environment
):
    """A grant in one namespace and a reconciler writing into another is a 403 per app, once a
    minute, forever. The two come from the same value, and this is what proves it."""
    docs = rendered(environment)
    role = portal_role(docs)
    env = portal_env(docs)

    assert env["JC_PORTAL_APPS_NAMESPACE"] == role["metadata"]["namespace"]

    binding = next(
        d
        for d in docs
        if d.get("kind") == "RoleBinding" and d["metadata"]["name"] == "portal"
    )
    assert binding["metadata"]["namespace"] == role["metadata"]["namespace"]
    assert binding["roleRef"]["kind"] == "Role"
    subject = binding["subjects"][0]
    pod = portal_deployment(docs)["spec"]["template"]["spec"]
    assert subject["name"] == pod["serviceAccountName"]


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_the_portal_carries_the_identity_and_the_settings_together(rendered, environment):
    """Fail-closed in both directions: no token means the client finds no ServiceAccount mount
    and deploys nothing, and no settings means the same. Half of either would be a Portal that
    tries and 403s."""
    docs = rendered(environment)
    pod = portal_deployment(docs)["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is True

    env = portal_env(docs)
    for name in [
        "JC_PORTAL_APPS_NAMESPACE",
        "JC_PORTAL_ORG_DOMAIN",
        "JC_PORTAL_RELEASE",
        "JC_PORTAL_SERVICE_ACCOUNT",
    ]:
        assert env.get(name), f"{environment}: {name} is not set"
    # AP-26: an app's login is the APISIX edge's openid-connect, so the Portal takes no sidecar image.
    assert "JC_PORTAL_APPS_OAUTH2_PROXY_IMAGE" not in env


# What the Portal's client does to a Secret: read one back by name, apply it (PATCH, and create
# for a new one), delete it. It never lists or watches one (portal src/apps/kube.rs).
SECRET_VERBS = {"get", "create", "patch", "delete"}


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_no_role_of_the_portal_lists_or_watches_secrets(rendered, environment):
    """T-2479, SEC: every Role and ClusterRole the Portal's ServiceAccount holds, or binds in a
    project's apps namespace, names Secrets only by name: no `list`, `watch` or `*`, so a
    compromised Portal cannot enumerate the instance's credentials."""
    docs = rendered(environment)
    slug = portal_env(docs)["JC_PORTAL_RELEASE"]
    roles = {
        (d["kind"], d["metadata"].get("namespace"), d["metadata"]["name"]): d
        for d in docs
        if d.get("kind") in {"Role", "ClusterRole"}
    }
    held = [roles[("ClusterRole", None, f"{slug}-portal-apps")]]
    for binding in docs:
        if binding.get("kind") not in {"RoleBinding", "ClusterRoleBinding"}:
            continue
        if not any(
            s.get("kind") == "ServiceAccount" and s.get("name") == "portal"
            for s in binding.get("subjects") or []
        ):
            continue
        ref = binding["roleRef"]
        namespace = binding["metadata"].get("namespace") if ref["kind"] == "Role" else None
        held.append(roles[(ref["kind"], namespace, ref["name"])])

    checked = 0
    for role in held:
        for rule in role.get("rules", []):
            if not {"secrets", "*"} & set(rule.get("resources", [])):
                continue
            checked += 1
            verbs = set(rule["verbs"])
            where = f"{environment}: {role['kind']} {role['metadata']['name']}"
            assert "*" not in verbs and not verbs & {"list", "watch"}, f"{where} lists Secrets: {sorted(verbs)}"
            assert verbs <= SECRET_VERBS | {"update"}, f"{where}: {sorted(verbs)}"
    assert checked >= 2, "the Portal's own Role and the apps role both name Secrets"


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_outside_an_apps_namespace_the_portal_reads_secrets_by_name_and_deletes_none(
    rendered, environment
):
    """T-4798: Role `portal` and `portal-build-pods` sit in the release namespace on a
    single-namespace installation, beside the database, Keycloak and forge credentials, whose
    names the charts publish. Every rule there that reads a Secret names it; none deletes one;
    the writes RBAC cannot name are bounded by the `{release}-portal-secrets` admission policy."""
    docs = rendered(environment)
    slug = portal_env(docs)["JC_PORTAL_RELEASE"]
    roles = [
        d
        for d in docs
        if d.get("kind") == "Role" and d["metadata"]["name"] in {"portal", "portal-build-pods"}
    ]
    assert "portal" in {r["metadata"]["name"] for r in roles}
    unnamed_writes = False
    for role in roles:
        for rule in role["rules"]:
            if "secrets" not in rule["resources"]:
                continue
            where = f"{environment}: Role {role['metadata']['name']}"
            verbs = set(rule["verbs"])
            assert "delete" not in verbs, f"{where} deletes Secrets"
            if "get" in verbs:
                assert rule.get("resourceNames"), f"{where} reads any Secret it can name"
            unnamed_writes |= not rule.get("resourceNames") and bool(verbs & {"create", "patch", "update"})
    read = {
        name
        for rule in portal_role(docs)["rules"]
        if "secrets" in rule["resources"] and "get" in rule["verbs"]
        for name in rule["resourceNames"]
    }
    assert read <= {"app-registry"}, f"{environment}: {read}"
    if unnamed_writes:
        by_name = {(d.get("kind"), d["metadata"]["name"]): d for d in docs}
        policy = by_name[("ValidatingAdmissionPolicy", f"{slug}-portal-secrets")]
        assert policy["spec"]["failurePolicy"] == "Fail"
        assert by_name[("ValidatingAdmissionPolicyBinding", f"{slug}-portal-secrets")]["spec"] == {
            "policyName": f"{slug}-portal-secrets",
            "validationActions": ["Deny"],
        }
