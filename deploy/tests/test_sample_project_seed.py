"""T-3234 (PF-109): one project of real open data is the sample the Portal offers newcomers.

The Portal lists a project as the sample when its Project manifest carries the label
`joinedcontext.com/sample: "true"`, to those who may read it and no one else. On dev that is
banskabystrica, which the demo people read through the `viewers` binding; no other seeded
project carries the label, so "Try it with sample data" always opens the same project.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
LABEL = "joinedcontext.com/sample"


def projects() -> dict[str, dict]:
    found = {}
    for path in sorted(SEED.glob("*/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Project":
                found[doc["metadata"]["name"]] = doc
    return found


def test_banskabystrica_alone_is_the_sample():
    labelled = {name for name, doc in projects().items() if (doc["metadata"].get("labels") or {}).get(LABEL) == "true"}
    assert labelled == {"banskabystrica"}


def test_the_sample_is_read_by_the_named_viewers_and_no_default_group():
    """The `viewers` binding names the demo people a viewer of the sample's organization, so they
    open it read-only. A newcomer is a member and reaches no project (ADR-N-067 2.1, T-6415):
    `platform-readers`, the group every account lands in, is bound nowhere."""
    organization = projects()["banskabystrica"]["spec"]["organizationRef"]
    bindings = [
        doc["spec"]
        for path in sorted(SEED.glob("*/*.yaml"))
        for doc in yaml.safe_load_all(path.read_text())
        if isinstance(doc, dict) and doc.get("kind") == "RoleBinding"
    ]
    assert any(
        spec["role"] == "viewer"
        and spec.get("scope", {}).get("organization") == organization
        and any("user" in subject for subject in spec["subjects"])
        for spec in bindings
    ), "nobody reads the sample: no viewer binding names a person at its organization"
    assert not [s for spec in bindings for s in spec["subjects"] if s.get("group") == "platform-readers"]
