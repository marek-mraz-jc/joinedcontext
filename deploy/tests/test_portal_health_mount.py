"""OPS-53 (T-2803): the Portal reads the validation checks' digests from an optional ConfigMap.

`scripts/publish-health.py` writes `jc-validation-results` after each check; the release never
owns it, so a sync does not wipe what was published, and a Portal starts before anything is.
"""

import pytest


def portal(docs):
    return next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")


@pytest.mark.parametrize("environment", ["dev", "local"])
def test_the_portal_mounts_the_published_results_read_only_and_optional(rendered, environment):
    docs = rendered(environment)
    pod = portal(docs)["spec"]["template"]["spec"]
    container = pod["containers"][0]
    env = {e["name"]: e.get("value") for e in container.get("env", [])}
    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == env["JC_HEALTH_DIR"])
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    assert mount.get("readOnly") is True
    assert volume["configMap"] == {"name": "jc-validation-results", "optional": True}
    assert not any(d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "jc-validation-results" for d in docs), \
        "the release must not own the ConfigMap the checks publish into"
