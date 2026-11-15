"""Who the edge thinks the caller is (T-0929, EP-20, SP-22).

APISIX is a ClusterIP behind the ingress controller, and the chart's own nginx config trusts
only `127.0.0.1` and reads `X-Real-IP`. Without the snippet this test holds, `$remote_addr` is
the ingress pod for every caller on the internet: one address in the access log, and one
`limit-count` bucket the first caller empties for everybody.
"""

import shutil

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

ENVIRONMENTS = ("local", "dev", "production")


def apisix_config(docs):
    for doc in docs:
        if doc.get("kind") == "ConfigMap" and doc["metadata"]["name"] == "apisix":
            return yaml.safe_load(doc["data"]["config.yaml"])
    raise AssertionError("no apisix ConfigMap in this environment")


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_edge_reads_the_caller_out_of_the_forwarded_chain(rendered, environment):
    config = apisix_config(rendered(environment))
    snippet = config["nginx_config"]["http_end_configuration_snippet"]
    # The hop in front is trusted by address range, and it is a private one: a public range
    # here would let anyone on the internet forge the address every limit is counted on.
    trusted = [line.strip() for line in snippet.splitlines() if line.strip().startswith("set_real_ip_from")]
    assert trusted, snippet
    for line in trusted:
        cidr = line.removeprefix("set_real_ip_from").strip().rstrip(";")
        assert cidr.startswith(("10.", "172.16.", "172.17.", "172.18.", "192.168.")), cidr
    # `set_real_ip_from` may be repeated; `real_ip_header` may not. A second one here is a
    # pod that crash-loops on start, which is how this was found.
    directives = [line.strip() for line in snippet.splitlines()
                  if line.strip() and not line.strip().startswith("#")]
    for once in ("real_ip_header", "real_ip_recursive"):
        assert not any(line.startswith(once) for line in directives), \
            f"nginx refuses a duplicate {once}: {directives}"
    assert config["nginx_config"]["http"]["real_ip_header"] == "X-Real-IP"
