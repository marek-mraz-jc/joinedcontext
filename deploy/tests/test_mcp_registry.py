"""The MCP Registry listing of the open-data connector (T-4581).

`mcp-registry/server.json` is what `mcp-publisher publish` sends; the registry refuses a file its
schema rejects or a name outside the namespace the login proved (DNS proof for joinedcontext.com
grants `com.joinedcontext/*`).
"""

import json
from pathlib import Path
from urllib.parse import urlparse

import jsonschema

FOLDER = Path(__file__).resolve().parent.parent / "mcp-registry"
SERVER = json.loads((FOLDER / "server.json").read_text())
SCHEMA = json.loads((FOLDER / "server.schema.json").read_text())


def urls(doc):
    if isinstance(doc, dict):
        for key, value in doc.items():
            if key in ("url", "src", "websiteUrl") and isinstance(value, str):
                yield value
            else:
                yield from urls(value)
    elif isinstance(doc, list):
        for item in doc:
            yield from urls(item)


def test_server_json_passes_the_registry_schema():
    jsonschema.validate(SERVER, SCHEMA, format_checker=jsonschema.FormatChecker())
    assert SERVER["$schema"].endswith("/2025-12-11/server.schema.json")


def test_name_is_in_the_domain_namespace():
    assert SERVER["name"].split("/")[0] == "com.joinedcontext"


def test_every_url_is_https_on_our_domain():
    found = list(urls(SERVER))
    assert found
    for url in found:
        parts = urlparse(url)
        if parts.netloc == "github.com":
            continue
        assert parts.scheme == "https", url
        assert parts.hostname.endswith(".joinedcontext.com"), url
        assert ".dev." not in f".{parts.hostname}", url


def test_remote_is_anonymous_streamable_http():
    (remote,) = SERVER["remotes"]
    assert remote["type"] == "streamable-http"
    assert urlparse(remote["url"]).path == "/api/mcp"
    assert "headers" not in remote and "variables" not in remote


def test_description_says_public_and_read_only():
    text = SERVER["description"].lower()
    assert "public" in text and "read-only" in text


def test_a_schema_violation_is_caught():
    bad = dict(SERVER, name="no-slash")
    try:
        jsonschema.validate(bad, SCHEMA)
    except jsonschema.ValidationError:
        return
    raise AssertionError("a name without a namespace passed the schema")
