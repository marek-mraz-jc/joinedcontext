"""T-1720: production is not dev with another name (CC-73, CC-75).

The attack diffs a production render against dev and looks for what only a workbench should
carry: the demo people and the demo cities' accounts and feeds, debug switches, CORS open to
any origin, verbose errors, the dev cluster's hosts, a placeholder domain, the `dev`
namespace and realm, and a model key written into the render. Each detector below is proved
twice: on a hand-made document that carries the defect, so it cannot pass by matching nothing,
and on the dev render, which carries most of them on purpose. The production render must carry
none. Single replicas are the production baseline's (test_production_baseline.py,
`test_stateless_services_run_at_least_two_replicas`).

The example `production` environment is what CI renders. An operator going live renders their
own instead: `JC_PRODUCTION_ENVIRONMENT=<name> pytest tests/test_production_is_not_dev.py`, which
is CHK-15 of the acceptance checklist a person signs (docs Deployment/08, section 5).
"""

import os
import re
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent

# The people and the cities of the demo: the realm users of the development profile and the
# projects the development seed creates. Read from the tree, so a new demo person or city is
# covered the day it is added.
_DEMO_USERS = yaml.safe_load((PROJECT_ROOT / "components/keycloak/demo-users.yaml").read_text())
DEMO_PEOPLE = sorted(_DEMO_USERS)
# The fixed Keycloak ids the seeds name the demo people by (PF-126).
DEMO_IDS = sorted(user["id"] for user in _DEMO_USERS.values())
DEMO_PROJECTS = sorted(p.name for p in (PROJECT_ROOT / "components/context-gateway/seed").iterdir() if p.is_dir())


@pytest.fixture(scope="module")
def production(rendered):
    return rendered(os.environ.get("JC_PRODUCTION_ENVIRONMENT", "production"))


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def strings(node, path=()):
    """Every string of a document, with where it sits."""
    if isinstance(node, dict):
        for key, value in node.items():
            yield from strings(value, (*path, str(key)))
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from strings(value, (*path, str(index)))
    elif isinstance(node, str):
        yield path, node


def name_of(doc) -> str:
    meta = doc.get("metadata") or {}
    return f"{doc.get('kind')}/{meta.get('namespace', '-')}/{meta.get('name')}"


def env_vars(doc):
    """Every container environment variable of a workload, as (name, value, valueFrom)."""
    spec = (doc.get("spec") or {}).get("template", {}).get("spec") or (doc.get("spec") or {}).get(
        "jobTemplate", {}
    ).get("spec", {}).get("template", {}).get("spec") or {}
    for container in (spec.get("containers") or []) + (spec.get("initContainers") or []):
        for var in container.get("env") or []:
            yield var.get("name", ""), var.get("value"), var.get("valueFrom")


def embedded(doc):
    """A ConfigMap's files read as YAML where they are YAML, so a config file is judged by its keys."""
    for value in (doc.get("data") or {}).values() if doc.get("kind") == "ConfigMap" else []:
        try:
            yield from (d for d in yaml.safe_load_all(value) if isinstance(d, (dict, list)))
        except yaml.YAMLError:
            continue


DEV_DOMAIN = "dev.joinedcontext.com"


def dev_hosts(docs):
    """The dev cluster's hosts: the node's sslip.io name it had, and dev.joinedcontext.com (T-2806)."""
    return sorted(
        {name_of(d) for d in docs for _, s in strings(d) if "sslip.io" in s or DEV_DOMAIN in s}
    )


def placeholder_domains(docs):
    """A reserved test domain (RFC 2606) or localhost standing in for the instance's own."""
    pattern = re.compile(r"(^|[/@.])[a-z0-9-]+\.(test|localhost)(?=$|[/:\"' ])")
    return sorted({f"{name_of(d)}: {s}" for d in docs for _, s in strings(d) if pattern.search(s)})


def demo_people(docs):
    return sorted(
        {f"{name_of(d)}: {person}" for d in docs for _, s in strings(d) for person in DEMO_PEOPLE if person in s}
        | {f"{name_of(d)}: {id_}" for d in docs for _, s in strings(d) for id_ in DEMO_IDS if id_ in s}
        | {
            name_of(d)
            for d in docs
            for person in DEMO_PEOPLE
            if (d.get("metadata") or {}).get("name") == f"keycloak-user-{person.replace('.', '-')}"
        }
    )


def demo_accounts_and_feeds(docs):
    """A resource, a client or a stream named for a demo city, and the demo feeds themselves."""
    named = re.compile(r"(^|[-/\"])(" + "|".join(map(re.escape, DEMO_PROJECTS)) + r")-")
    found = set()
    for d in docs:
        meta = d.get("metadata") or {}
        if named.search(meta.get("name") or ""):
            found.add(name_of(d))
        if (meta.get("labels") or {}).get("app.kubernetes.io/instance", "").startswith("demo-feeds"):
            found.add(name_of(d))
        for var, value, _ in env_vars(d):
            if isinstance(value, str) and named.search(value) and not value.startswith(("http:", "https:")):
                found.add(f"{name_of(d)}: {var}={value}")
    return sorted(found)


def debug_switches(docs):
    found = set()
    for d in docs:
        for var, value, _ in env_vars(d):
            if not isinstance(value, str):
                continue
            if ("LOG" in var.upper() and re.search(r"\b(debug|trace)\b", value, re.I)) or (
                var.upper() in {"RUST_BACKTRACE", "DEBUG"} and value.lower() not in {"", "0", "false"}
            ):
                found.add(f"{name_of(d)}: {var}={value}")
        for conf in embedded(d):
            for key, value in walk_items(conf):
                if key in {"enable_debug", "debug"} and value is True:
                    found.add(f"{name_of(d)}: {key}: true")
    return sorted(found)


def walk_items(node):
    if isinstance(node, dict):
        for key, value in node.items():
            yield key, value
            yield from walk_items(value)
    elif isinstance(node, list):
        for value in node:
            yield from walk_items(value)


def open_cors(docs):
    """CORS that admits any origin: a literal `*`, or an unanchored catch-all pattern."""
    found = set()
    for d in docs:
        for conf in [d, *embedded(d)]:
            for key, value in walk_items(conf):
                values = value if isinstance(value, list) else [value]
                if key in {"allow_origins", "allowed_origins", "allowOrigins"} and any(
                    v == "*" or v == "**" for v in values if isinstance(v, str)
                ):
                    found.add(f"{name_of(d)}: {key}={value}")
                if key == "allow_origins_by_regex" and any(
                    isinstance(v, str) and (not v.startswith("^") or not v.endswith("$") or ".*" in v)
                    for v in values
                ):
                    found.add(f"{name_of(d)}: {key}={value}")
        for var, value, _ in env_vars(d):
            if "CORS" in var.upper() and value == "*":
                found.add(f"{name_of(d)}: {var}=*")
    return sorted(found)


def verbose_errors(docs):
    found = set()
    for d in docs:
        for var, value, _ in env_vars(d):
            upper = var.upper()
            if any(word in upper for word in ("VERBOSE", "EXPOSE_ERROR", "SHOW_ERROR", "STACKTRACE")) and (
                isinstance(value, str) and value.lower() in {"1", "true", "yes", "on"}
            ):
                found.add(f"{name_of(d)}: {var}={value}")
        for conf in embedded(d):
            for key, value in walk_items(conf):
                if key == "show_upstream_status_in_response_header" and value is True:
                    found.add(f"{name_of(d)}: {key}: true")
    return sorted(found)


def dev_namespaces_and_realm(docs):
    found = set()
    for d in docs:
        namespace = (d.get("metadata") or {}).get("namespace") or ""
        if d.get("kind") == "Namespace":
            namespace = (d.get("metadata") or {}).get("name") or ""
        if re.fullmatch(r"dev|dev-.*|.*-dev", namespace):
            found.add(name_of(d))
        for _, s in strings(d):
            if re.search(r"/realms/dev\b|\.dev\.svc\b", s):
                found.add(name_of(d))
    return sorted(found)


def written_model_keys(docs):
    """The assistant's model key is a Secret reference; a value in the render is a shared key."""
    return sorted(
        f"{name_of(d)}: {var}"
        for d in docs
        for var, value, _ in env_vars(d)
        if var.upper().endswith(("MODEL_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY")) and value is not None
    )


DETECTORS = {
    "dev_hosts": dev_hosts,
    "placeholder_domains": placeholder_domains,
    "demo_people": demo_people,
    "demo_accounts_and_feeds": demo_accounts_and_feeds,
    "debug_switches": debug_switches,
    "open_cors": open_cors,
    "verbose_errors": verbose_errors,
    "dev_namespaces_and_realm": dev_namespaces_and_realm,
    "written_model_keys": written_model_keys,
}


def workload(env: dict, **extra) -> dict:
    return {
        "kind": "Deployment",
        "metadata": {"name": extra.pop("name", "app"), "namespace": extra.pop("namespace", "jc")},
        "spec": {"template": {"spec": {"containers": [{"name": "c", "env": [
            {"name": k, "value": v} for k, v in env.items()
        ]}]}}},
        **extra,
    }


def config_map(content: dict) -> dict:
    return {"kind": "ConfigMap", "metadata": {"name": "conf", "namespace": "jc"}, "data": {"c.yaml": yaml.safe_dump(content)}}


# One document per detector that carries its defect, the way a careless overlay would write it.
DEFECTS = {
    "dev_hosts": [
        workload({"JC_PUBLIC_URL": "https://portal.203.0.113.40.sslip.io"}),
        workload({"JC_PUBLIC_URL": f"https://portal.{DEV_DOMAIN}"}, name="other"),
    ],
    "placeholder_domains": [workload({"JC_PORTAL_ORG_DOMAIN": "joinedcontext.test"})],
    "demo_people": [workload({"JC_OWNER": f"{DEMO_PEOPLE[0]}@hel.fi"}), config_map({"subjects": [{"user": DEMO_IDS[0]}]})],
    "demo_accounts_and_feeds": [workload({"JC_CLIENT_ID": f"{DEMO_PROJECTS[0]}-pipelines"})],
    "debug_switches": [workload({"RUST_LOG": "info,jc_core=debug"}), config_map({"apisix": {"enable_debug": True}})],
    "open_cors": [config_map({"plugins": {"cors": {"allow_origins": "*"}}}), config_map({"cors": {"allow_origins_by_regex": [".*"]}})],
    "verbose_errors": [workload({"JC_EXPOSE_ERRORS": "true"})],
    "dev_namespaces_and_realm": [workload({}, namespace="dev")],
    "written_model_keys": [workload({"JC_MODEL_KEY": "sk-or-v1-0000"})],
}

# What the dev render carries on purpose, which is the second proof each of these detectors works.
ON_DEV = ["dev_hosts", "demo_people", "demo_accounts_and_feeds", "dev_namespaces_and_realm"]


@pytest.mark.parametrize("detector", sorted(DETECTORS))
def test_the_detector_finds_the_defect_it_is_named_for(detector):
    # CC-73: a detector that matches nothing proves nothing about a clean render.
    assert DETECTORS[detector](DEFECTS[detector]), f"{detector} missed its own defect"
    for defect in DEFECTS[detector]:
        assert DETECTORS[detector]([defect]), f"{detector} missed {defect}"
    assert DETECTORS[detector]([workload({"JC_LOG": "info"})]) == []


@pytest.mark.parametrize("detector", ON_DEV)
def test_the_dev_render_is_what_the_detector_is_for(detector, dev):
    # CC-73: the workbench carries these deliberately, so each finds something there.
    assert DETECTORS[detector](dev), f"{detector} finds nothing on dev; it would find nothing on production either"


@pytest.mark.parametrize("detector", sorted(DETECTORS))
def test_production_renders_nothing_of_dev(detector, production):
    # CC-73, CC-75: every one of these in a production render is dev under another name.
    found = DETECTORS[detector](production)
    assert found == [], f"the production render carries {detector}: {found}"
