"""T-5126: the price table of Administration → Costs reaches the Portal as a mounted file.

`portal.costPrices` is rendered into the Portal's ConfigMap and named by
`JC_PORTAL_COST_PRICES_FILE`; nothing set renders an empty map, which the Portal reads as no
table (the page says so) rather than as prices of 0.
"""

import pytest
import yaml


def portal(docs):
    return next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")


def prices_file(docs):
    pod = portal(docs)["spec"]["template"]["spec"]
    container = pod["containers"][0]
    env = {e["name"]: e.get("value") for e in container.get("env", [])}
    directory, name = env["JC_PORTAL_COST_PRICES_FILE"].rsplit("/", 1)
    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == directory)
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    config_map = next(
        d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == volume["configMap"]["name"]
    )
    return yaml.safe_load(config_map["data"][name])


@pytest.mark.parametrize("environment", ["dev", "local", "smartcity"])
def test_no_price_table_renders_an_empty_map(rendered, environment):
    assert prices_file(rendered(environment)) == {}


def with_prices(tree):
    path = tree / "components/portal/default-environment.yaml.gotmpl"
    text = path.read_text()
    line = "    costPrices: {}\n"
    assert line in text
    path.write_text(
        text.replace(
            line,
            "    costPrices:\n"
            "      currency: EUR\n"
            "      perUsd: 0.86\n"
            "      nodes: [{ name: cpx42 nbg1, monthly: 30.0 }]\n",
        )
    )


def test_the_table_the_operator_sets_is_the_file_the_portal_reads(rendered_variant):
    assert prices_file(rendered_variant("dev", with_prices)) == {
        "currency": "EUR",
        "perUsd": 0.86,
        "nodes": [{"name": "cpx42 nbg1", "monthly": 30.0}],
    }
