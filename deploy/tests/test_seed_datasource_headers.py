"""Every seed DataSource header is a name and a string value (T-2880).

In a YAML flow mapping a comma ends the value, so `{ Accept: application/geo+json, application/json }`
is two headers, the second a key with a null value. The runner refused six bbsk pipelines for it
("invalid data source spec: invalid type: null, expected a string") and they never ran on dev.
"""

from pathlib import Path

from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"


def test_every_seed_datasource_header_value_is_a_string():
    bad, seen = [], 0
    for path, doc in seed_documents():
        if path.name.endswith((".linkml.yaml", "-bento.yaml")) or path.name == "index.yaml":
            continue
        if not isinstance(doc, dict) or doc.get("kind") != "DataSource":
            continue
        for name, value in (((doc.get("spec") or {}).get("http") or {}).get("headers") or {}).items():
            seen += 1
            if not isinstance(value, str):
                bad.append(f"{path.relative_to(SEED)}: header {name!r} is {value!r}")
    assert seen > 10, f"only {seen} seed headers found: the seed moved"
    assert bad == [], "quote a header value that holds a comma:\n" + "\n".join(bad)
