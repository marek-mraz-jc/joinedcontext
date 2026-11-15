"""PF-97 (T-2716): the operator's bounds on an Organization reach the Portal as a mounted file.

`portal.organizationBounds` is rendered into the Portal's ConfigMap and named by
`JC_PORTAL_ORGANIZATION_BOUNDS_FILE`; nothing set renders an empty map, which the Portal reads
as the catalog's built-in bounds.
"""

import pytest
import yaml


def portal(docs):
    return next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")


def bounds_file(docs):
    pod = portal(docs)["spec"]["template"]["spec"]
    container = pod["containers"][0]
    env = {e["name"]: e.get("value") for e in container.get("env", [])}
    path = env["JC_PORTAL_ORGANIZATION_BOUNDS_FILE"]
    directory, name = path.rsplit("/", 1)
    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == directory)
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    config_map = next(
        d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == volume["configMap"]["name"]
    )
    return yaml.safe_load(config_map["data"][name])


DATA_API = "spec.limits.edge.requestsPerMinute.publicEndpoint"


@pytest.mark.parametrize("environment", ["dev", "local", "smartcity"])
def test_the_portal_reads_the_operators_bounds_from_a_mounted_file(rendered, environment):
    # T-4717: the one default bound holds the data APIs at 10 a second, the catalog's 5000
    # included; every other entry keeps its built-in bound.
    assert bounds_file(rendered(environment)) == {DATA_API: {"max": 600}}


def with_bounds(tree):
    path = tree / "components/portal/default-environment.yaml.gotmpl"
    text = path.read_text()
    line = f"      {DATA_API}: {{ max: 600 }}\n"
    assert line in text
    path.write_text(
        text.replace(
            line,
            line +
            "      spec.projects.quota.contextSpaces: { max: 20 }\n"
            "      spec.limits.signIn.sessionIdleMinutes: { min: 15, max: 120 }\n",
        )
    )


def test_the_bounds_the_operator_sets_are_the_file_the_portal_reads(rendered_variant):
    assert bounds_file(rendered_variant("dev", with_bounds)) == {
        DATA_API: {"max": 600},
        "spec.projects.quota.contextSpaces": {"max": 20},
        "spec.limits.signIn.sessionIdleMinutes": {"min": 15, "max": 120},
    }
