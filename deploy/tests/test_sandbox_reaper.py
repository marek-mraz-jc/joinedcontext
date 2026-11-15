"""T-0050: the sandbox reaper deletes a namespace past its Time-To-Live and nothing else.

The script under test is read out of the rendered ConfigMap, not out of the repository, so
what these tests run is the text that reaches the cluster. `kubectl` is replaced by a stub
that answers the listing from a fixture and records every deletion, which is the whole of
what the reaper does to a cluster (OPS-44, PF-19, CC-67).
"""

import datetime
import os
import stat
import subprocess
import textwrap
from pathlib import Path

import pytest

LABEL = "sandbox.joinedcontext.com/lifecycle"
TTL = "sandbox.joinedcontext.com/ttl"
DAY = 24 * 60 * 60


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


def one(docs: list[dict], kind: str, name: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


@pytest.fixture(scope="module")
def script(local) -> str:
    return one(local, "ConfigMap", "sandbox-reaper-reaper")["data"]["reap.sh"]


def ago(days: float) -> str:
    when = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=days)
    return when.strftime("%Y-%m-%dT%H:%M:%SZ")


def run(script: str, tmp_path: Path, namespaces: list[tuple[str, str, str]], **env) -> tuple[str, list[str]]:
    """Runs the reaper with a stub kubectl. Returns its output and the namespaces it deleted.

    Each namespace is (name, creationTimestamp, ttl-annotation-or-dash), already in the shape
    the script's own go-template produces, because the template is evaluated by kubectl and
    there is no kubectl here.
    """
    listing = "".join(f"{n} {c} {t}\n" for n, c, t in namespaces)
    (tmp_path / "listing").write_text(listing)
    stub = tmp_path / "kubectl"
    stub.write_text(textwrap.dedent(f"""\
        #!/bin/sh
        case "$1" in
          get) cat {tmp_path / "listing"} ;;
          delete) echo "$3" >> {tmp_path / "deleted"} ;;
          *) echo "stub kubectl: unexpected $*" >&2; exit 64 ;;
        esac
        """))
    stub.chmod(stub.stat().st_mode | stat.S_IEXEC)

    result = subprocess.run(
        ["sh", "-"], input=script, text=True, capture_output=True,
        env={**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}", **env},
    )
    assert result.returncode == 0, result.stderr
    deleted_file = tmp_path / "deleted"
    deleted = deleted_file.read_text().split() if deleted_file.exists() else []
    return result.stdout, deleted


def test_an_expired_sandbox_is_deleted_and_a_young_one_is_kept(script, tmp_path):
    """The whole point, and the two cases that must never swap places."""
    out, deleted = run(script, tmp_path, [
        ("sandbox-old", ago(15), "-"),
        ("sandbox-young", ago(5), "-"),
    ])
    assert deleted == ["sandbox-old"], out
    assert "keep   sandbox-young" in out


def test_a_sandbox_exactly_at_the_ceiling_is_still_kept(script, tmp_path):
    """OPS-44 caps the Time-To-Live at 14 calendar days, so day 14 is inside the limit and
    day 14 plus a minute is not. An off-by-one here deletes a sandbox a day early."""
    _, deleted = run(script, tmp_path, [
        ("sandbox-at-limit", ago(13.99), "-"),
        ("sandbox-past-limit", ago(14.01), "-"),
    ])
    assert deleted == ["sandbox-past-limit"]


def test_a_shorter_ttl_annotation_shortens_the_life(script, tmp_path):
    """A three day sandbox is expired on day five; without the annotation it would have
    eleven days left."""
    out, deleted = run(script, tmp_path, [("sandbox-short", ago(5), "72h")])
    assert deleted == ["sandbox-short"], out


def test_a_longer_ttl_annotation_is_clamped_to_the_ceiling(script, tmp_path):
    """The annotation is a request, and an occupant of a sandbox can write it. If it were
    honoured upward, a sandbox could opt out of OPS-44 by annotating itself."""
    out, deleted = run(script, tmp_path, [
        ("sandbox-greedy", ago(15), "8760h"),
        ("sandbox-greedy-young", ago(5), "8760h"),
    ])
    assert deleted == ["sandbox-greedy"], out
    assert "clamped" in out


@pytest.mark.parametrize("ttl", ["last tuesday", "72", "-3h", "0h", ""])
def test_an_unreadable_ttl_falls_back_to_the_ceiling(script, tmp_path, ttl):
    """Neither direction of failure is acceptable: a nonsense annotation must not grant an
    unlimited sandbox, and must not delete one that is a day old either."""
    out, deleted = run(script, tmp_path, [
        ("sandbox-nonsense-old", ago(15), ttl or "-"),
        ("sandbox-nonsense-young", ago(1), ttl or "-"),
    ])
    assert deleted == ["sandbox-nonsense-old"], out


def test_a_namespace_whose_timestamp_does_not_parse_is_kept(script, tmp_path):
    """An age that cannot be computed is not an age past the limit. The reaper deletes
    namespaces, so the unknown case has to fall on the side that keeps data."""
    out, deleted = run(script, tmp_path, [("sandbox-broken", "not-a-timestamp", "-")])
    assert deleted == []
    assert "did not parse" in out


def test_dry_run_deletes_nothing(script, tmp_path):
    out, deleted = run(script, tmp_path, [("sandbox-old", ago(30), "-")], DRY_RUN="true")
    assert deleted == []
    assert "would  sandbox-old" in out


def test_the_listing_is_restricted_to_labelled_namespaces(script):
    """A permanent project namespace is not filtered out further down, it is never fetched.
    The selector in the script is the only thing standing between this CronJob and every
    namespace on the cluster."""
    assert f'--selector "$LABEL in (unmanaged,preview)"' in script
    assert 'LABEL="${LABEL:-sandbox.joinedcontext.com/lifecycle}"' in script


def test_the_deletion_cascades(script):
    """OPS-44 wants deletion cascading cleanly to all provisioned resources; a background
    cascade returns before the Secrets and ServiceAccount tokens inside are gone."""
    assert "--cascade=foreground" in script


def test_the_cronjob_runs_unprivileged_and_read_only(local):
    job = one(local, "CronJob", "sandbox-reaper-reaper")["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    assert job["securityContext"]["runAsNonRoot"] is True
    container = job["containers"][0]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["securityContext"]["allowPrivilegeEscalation"] is False
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    assert "@sha256:" in container["image"]


def test_the_reaper_may_only_list_and_delete_namespaces(local):
    """The one credential in this component is its ServiceAccount, and this is its whole
    reach. `get` on secrets, or any verb on workloads, would make an abandoned sandbox a
    smaller problem than the thing cleaning it up."""
    rules = one(local, "ClusterRole", "sandbox-reaper-reaper")["rules"]
    assert rules == [{"apiGroups": [""], "resources": ["namespaces"], "verbs": ["list", "delete"]}]


def test_two_runs_never_overlap(local):
    """Two concurrent runs would issue the same deletions from two pods."""
    spec = one(local, "CronJob", "sandbox-reaper-reaper")["spec"]
    assert spec["concurrencyPolicy"] == "Forbid"
    assert spec["timeZone"] == "UTC"


def test_the_chart_refuses_a_ceiling_above_the_requirement(tmp_path):
    """15 days is not a configuration choice, it is a breach of OPS-44 and PF-19, and it is
    refused while it is still text."""
    import shutil
    import subprocess as sp
    root = Path(__file__).resolve().parent.parent
    if shutil.which("helm") is None:
        pytest.skip("helm not installed")
    result = sp.run(
        ["helm", "template", "sandbox-reaper", "components/sandbox-reaper/charts/reaper",
         "--set", "ceilingDays=15", "--set", "image.repository=x", "--set", "image.tag=y"],
        cwd=str(root), capture_output=True, text=True,
    )
    assert result.returncode != 0, result.stdout
    assert "14 calendar days" in result.stderr, result.stderr
