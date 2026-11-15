"""AP-102 (T-2593): the Portal fetches every published build into a writable cache of its own.

The image's `/srv/apps` is read-only and keeps the bundles the image ships, so the builds the
package registry holds need another directory: an `emptyDir` with a size limit, mounted where
`JC_PORTAL_APPS_CACHE_DIR` points, while the root filesystem stays read-only.
"""

import pytest


def portal(docs):
    return next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")


@pytest.mark.parametrize("environment", ["dev", "local"])
def test_the_portal_fetches_builds_into_a_bounded_writable_cache(rendered, environment):
    pod = portal(rendered(environment))["spec"]["template"]["spec"]
    container = pod["containers"][0]
    env = {e["name"]: e.get("value") for e in container.get("env", [])}
    cache = env["JC_PORTAL_APPS_CACHE_DIR"]

    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == cache)
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    assert "sizeLimit" in volume["emptyDir"], "a full cache evicts the pod, so it is bounded"
    assert not mount.get("readOnly"), "the Portal writes the builds it fetches"
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert not cache.startswith("/srv/apps"), "the shipped bundles stay where the image put them"
