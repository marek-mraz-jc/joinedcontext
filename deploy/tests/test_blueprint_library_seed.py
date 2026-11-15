"""CC-26, CC-28 (T-1571): the dev instance follows the platform's blueprint library.

The library is `library/blueprints/` of the platform repository. A new version reaches this
instance as a merge request somebody reviews: the source never merges by itself and never deletes
what the library dropped.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/helsinki"


def test_the_seed_follows_the_platform_library_as_reviewed_merge_requests():
    """CC-26, CC-28: one SyncSource, on the platform's library folder, reviewed and never pruning."""
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    assert index["helsinki-syncsource-blueprint-library.yaml"] == "projects/helsinki/sync/blueprint-library.yaml"

    source = yaml.safe_load((SEED / "helsinki-syncsource-blueprint-library.yaml").read_text())
    assert source["kind"] == "SyncSource"
    assert source["metadata"]["namespace"] == "helsinki"
    git = source["spec"]["source"]["git"]
    assert git["url"] == "https://github.com/marek-mraz-jc/joinedcontext-platform.git"
    assert git["path"] == "library/blueprints"
    assert "secretRef" not in git, "the library is public; a credential here would be a secret nobody needs"
    assert source["spec"]["autoMerge"] is False
    assert source["spec"]["prune"] is False
    assert "webhook" not in source["spec"]["schedule"], "the platform's repository pushes to no instance"
