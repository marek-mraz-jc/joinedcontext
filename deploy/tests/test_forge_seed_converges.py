"""The forge seed converges: it adds, it updates, and it removes what it no longer owns (T-2392).

The Job used to only ever `GET` a path and then `PUT` or `POST` it. Correcting one line of a seed
index — a share's path from `projects/bbsk/shares/` to the `shared/` its kind declares — therefore
left the file at both paths, two manifests then declared one `SharedSpaceReference bbsk/mesto-kpi`,
and the gateway refuses the **whole** manifest repository on a duplicate identity. Every endpoint
on `dev` stopped being served because one seed file had been renamed, and a person had to delete a
blob through the forge API by hand.

These tests run the Job's own script — the rendered one, not a copy — against `tests/fake_forge.py`
on PATH as `curl`, so the order of its steps and the shape of its requests are what is exercised.
"""

import base64
import json
import os
import re
import shutil
import stat
import subprocess
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
FAKE_FORGE = Path(__file__).resolve().parent / "fake_forge.py"

requires_helmfile = pytest.mark.skipif(
    shutil.which("helmfile") is None, reason="helmfile not installed"
)

ORG = "joinedcontext"
REPO = "configuration"
MANIFEST = ".jc/seed-manifest.txt"


def recorded_paths(record: str) -> list[str]:
    """The paths of the seed's record, one `path blob` line each (T-6711)."""
    lines = record.splitlines()
    assert all(re.fullmatch(r"\S+ [0-9a-f]{40}", line) for line in lines), lines
    return [line.split(" ")[0] for line in lines]
COMMIT = f"POST http://forge.test/api/v1/repos/{ORG}/{REPO}/contents"

SHARE = """apiVersion: joinedcontext.com/v1alpha1
kind: SharedSpaceReference
metadata:
  name: mesto-kpi
  namespace: bbsk
spec:
  endpointRef: { kind: Endpoint, name: bb-kpi }
"""

PROJECT = """apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: bbsk
  namespace: bbsk
spec:
  title: The region
"""


@pytest.fixture(scope="module")
def script(rendered):
    job = next(
        d
        for d in rendered("local")
        if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    return job["spec"]["template"]["spec"]["containers"][0]["args"][0]


class Forge:
    """One run of the Job's script against a repository that is a dict of paths."""

    def __init__(self, tmp_path: Path, contents: dict[str, str]):
        self.root = tmp_path
        self.state = tmp_path / "state.json"
        self.calls = tmp_path / "calls.log"
        self.state.write_text(
            json.dumps({"contents": dict(contents), "secrets": {}, "calls": []})
        )
        self.calls.write_text("")
        self.seed = tmp_path / "seed"
        self.seed.mkdir()
        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        shim = bin_dir / "curl"
        shim.write_text(f'#!/bin/sh\nexec python3 {FAKE_FORGE} "$@"\n')
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
        self.bin = bin_dir
        sa = tmp_path / "sa"
        sa.mkdir()
        (sa / "token").write_text("a.service.account.token")
        (sa / "ca.crt").write_text("-----BEGIN CERTIFICATE-----\n")
        self.sa = sa
        work = tmp_path / "work"
        work.mkdir()
        self.work = work

    def put(self, path: str, text: str) -> None:
        """One seed file, under the repository path it is committed to."""
        (self.seed / path.replace("/", "__")).write_text(text)

    def run(self, script: str, **overrides: str) -> subprocess.CompletedProcess:
        environment = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "GITEA_URL": "http://forge.test",
            "KAPI": "http://kube.test",
            "SA": str(self.sa),
            "SEED_DIR": str(self.seed),
            "WORK": str(self.work),
            "ORG": ORG,
            "REPO": REPO,
            "BRANCH": "main",
            "LAYOUT": "1",
            "TEAMS": "viewers",
            "NAMESPACE": "dev",
            "ADMIN_USER": "forge-admin",
            "ADMIN_PASSWORD": "not-a-real-password",
            "PORTAL_SECRET": "gitea-token-portal",
            "PORTAL_SCOPES": '["write:repository"]',
            "PORTAL_NAMESPACES": "dev",
            "GATEWAY_SECRET": "gitea-token-gateway",
            "GATEWAY_SCOPES": '["read:repository"]',
            "GATEWAY_NAMESPACES": "dev",
            "PORTAL_USER": "jc-portal",
            "PORTAL_RESTART": "dev/portal",
            "GATEWAY_USER": "jc-gateway",
            "GATEWAY_RESTART": "dev/context-gateway",
            "APPS_ORG": "",
            "APPS_USER": "jc-apps",
            "APPS_SECRET": "gitea-token-apps",
            "APPS_SCOPES": '["write:repository"]',
            "JC_FAKE_FORGE_STATE": str(self.state),
            "JC_FAKE_FORGE_CALLS": str(self.calls),
            **overrides,
        }
        return subprocess.run(
            ["sh", "-c", script], env=environment, capture_output=True, text=True
        )

    @property
    def contents(self) -> dict[str, str]:
        return json.loads(self.state.read_text())["contents"]

    @property
    def secrets(self) -> dict[str, dict]:
        return json.loads(self.state.read_text())["secrets"]

    @property
    def log(self) -> list[str]:
        return [line for line in self.calls.read_text().splitlines() if line]


@requires_helmfile
def test_a_renamed_seed_path_leaves_exactly_one_file(script, tmp_path):
    """The failure of 2026-09-20, as a test: the old path goes when the new one arrives.

    Run once at the old path, then again with the index corrected. What made the cluster refuse
    the repository was that both files were there after the second run."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/shares/mesto-kpi.yaml", SHARE)
    forge.put("projects/bbsk/project.yaml", PROJECT)
    first = forge.run(script)
    assert first.returncode == 0, first.stderr
    assert "projects/bbsk/shares/mesto-kpi.yaml" in forge.contents
    assert recorded_paths(forge.contents[MANIFEST]) == [
        "projects/bbsk/project.yaml",
        "projects/bbsk/shares/mesto-kpi.yaml",
    ]

    # The correction: the same file, at the path its kind declares.
    (forge.seed / "projects__bbsk__shares__mesto-kpi.yaml").unlink()
    forge.put("projects/bbsk/shared/mesto-kpi.yaml", SHARE)
    second = forge.run(script)
    assert second.returncode == 0, second.stderr

    assert "projects/bbsk/shared/mesto-kpi.yaml" in forge.contents
    assert "projects/bbsk/shares/mesto-kpi.yaml" not in forge.contents, (
        "the old path stayed, which is the duplicate identity the gateway refuses on"
    )
    assert "seed: removed projects/bbsk/shares/mesto-kpi.yaml" in second.stdout
    assert forge.log.count(COMMIT) == 2, "each run that changed something is one commit"
    assert recorded_paths(forge.contents[MANIFEST]) == [
        "projects/bbsk/project.yaml",
        "projects/bbsk/shared/mesto-kpi.yaml",
    ]

    # And a third run, with nothing changed, removes nothing and writes nothing.
    third = forge.run(script)
    assert third.returncode == 0, third.stderr
    assert "seed: 0 file(s) removed" in third.stdout
    assert "seed: 0 file(s) written" in third.stdout
    assert forge.log.count(COMMIT) == 2, "a run with nothing to change committed"


@requires_helmfile
def test_a_file_the_seed_never_wrote_is_left_alone(script, tmp_path):
    """The record is what makes a deletion decidable, and it only ever holds the seed's own paths.

    A person's manifest, a Portal merge request's file and the seed's own removed path all sit in
    one repository; only the last is the Job's to remove."""
    theirs = "projects/bbsk/spaces/bbsk-kpi/endpoints/written-by-a-person.yaml"
    forge = Forge(tmp_path, {theirs: "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\n"})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    first = forge.run(script)
    assert first.returncode == 0, first.stderr

    # The seed shrinks to nothing: still not the person's file to lose.
    (forge.seed / "projects__bbsk__project.yaml").unlink()
    forge.put("projects/bbsk/other.yaml", PROJECT.replace("name: bbsk", "name: bbsk-other"))
    second = forge.run(script)
    assert second.returncode == 0, second.stderr

    assert theirs in forge.contents, "a file the seed never wrote was deleted"
    assert "projects/bbsk/project.yaml" not in forge.contents
    assert not any(theirs in call for call in forge.log if call.startswith("DELETE")), (
        "the Job asked the forge to delete a file that was never its own"
    )



def edit(forge: Forge, path: str, text: str) -> None:
    """A merged Change (or a person) writes the file on the branch between two applies."""
    state = json.loads(forge.state.read_text())
    state["contents"][path] = text
    forge.state.write_text(json.dumps(state))


@requires_helmfile
def test_an_apply_never_reverts_an_edit_merged_since_the_seed_wrote_the_file(script, tmp_path):
    """T-6711, P-11: the seed is a post-upgrade hook, so every helm apply ran it, and it put its own
    version back over an edit approved in the Portal. It changes a file only while the branch
    still holds what the seed wrote there; an edited file is kept and reported, and is the
    person's from then on, even once the seed drops the path."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    forge.put("projects/bbsk/shared/mesto-kpi.yaml", SHARE)
    assert forge.run(script).returncode == 0
    edited = SHARE.replace("mesto-kpi", "mesto-kpi") + "# approved in the Portal\n"
    edit(forge, "projects/bbsk/shared/mesto-kpi.yaml", edited)

    # The seed's own version moves on too: still not the seed's to write.
    forge.put("projects/bbsk/shared/mesto-kpi.yaml", SHARE + "# the seed's next version\n")
    forge.put("projects/bbsk/project.yaml", PROJECT + "# the seed's next version\n")
    second = forge.run(script)
    assert second.returncode == 0, second.stderr
    assert forge.contents["projects/bbsk/shared/mesto-kpi.yaml"] == edited, "the apply reverted the merged edit"
    assert "seed: kept projects/bbsk/shared/mesto-kpi.yaml" in second.stdout
    assert forge.contents["projects/bbsk/project.yaml"].endswith("# the seed's next version\n"), (
        "a file nobody edited still converges on the seed"
    )
    assert recorded_paths(forge.contents[MANIFEST]) == ["projects/bbsk/project.yaml"]

    # A third run over the same state writes nothing, and a seed without the path deletes nothing.
    commits = forge.log.count(COMMIT)
    third = forge.run(script)
    assert third.returncode == 0 and forge.log.count(COMMIT) == commits, third.stdout
    (forge.seed / "projects__bbsk__shared__mesto-kpi.yaml").unlink()
    fourth = forge.run(script)
    assert fourth.returncode == 0, fourth.stderr
    assert forge.contents["projects/bbsk/shared/mesto-kpi.yaml"] == edited
    assert not any("mesto-kpi" in call for call in forge.log if call.startswith("DELETE"))


@requires_helmfile
def test_a_dropped_path_the_seed_wrote_and_nobody_edited_still_goes(script, tmp_path):
    """The rename of T-2392 keeps working under P-11's rule: the old path is unedited, so it goes."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    forge.put("projects/bbsk/shares/mesto-kpi.yaml", SHARE)
    assert forge.run(script).returncode == 0
    (forge.seed / "projects__bbsk__shares__mesto-kpi.yaml").unlink()
    forge.put("projects/bbsk/shared/mesto-kpi.yaml", SHARE)
    second = forge.run(script)
    assert second.returncode == 0, second.stderr
    assert "projects/bbsk/shares/mesto-kpi.yaml" not in forge.contents
    assert "seed: removed projects/bbsk/shares/mesto-kpi.yaml" in second.stdout


@requires_helmfile
def test_a_record_of_before_names_no_blob_so_a_differing_file_is_kept(script, tmp_path):
    """A repository seeded before T-6711 holds a record of paths alone: whether a differing file
    is the seed's old version or a person's edit cannot be told, so it is kept, never guessed."""
    forge = Forge(tmp_path, {
        "projects/bbsk/project.yaml": PROJECT + "# someone's line\n",
        MANIFEST: "projects/bbsk/project.yaml\n",
    })
    forge.put("projects/bbsk/project.yaml", PROJECT)
    result = forge.run(script)
    assert result.returncode == 0, result.stderr
    assert forge.contents["projects/bbsk/project.yaml"] == PROJECT + "# someone's line\n"
    assert "seed: kept projects/bbsk/project.yaml" in result.stdout
    assert recorded_paths(forge.contents[MANIFEST]) == []

@requires_helmfile
def test_two_seed_files_of_one_identity_fail_the_job_before_anything_is_written(script, tmp_path):
    """The gateway refuses the repository on a duplicate; the Job refuses the seed on one.

    Both paths are named, because the person reading this output is the one who has to pick which
    of the two files stays."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/shares/mesto-kpi.yaml", SHARE)
    forge.put("projects/bbsk/shared/mesto-kpi.yaml", SHARE)
    result = forge.run(script)

    assert result.returncode != 0, result.stdout
    assert "SharedSpaceReference/bbsk/mesto-kpi" in result.stderr
    for path in ("projects/bbsk/shares/mesto-kpi.yaml", "projects/bbsk/shared/mesto-kpi.yaml"):
        assert path in result.stderr, result.stderr
    assert forge.contents == {}, "the Job wrote before it refused"
    assert not any(call.startswith(("PUT", "POST", "DELETE")) and "/contents" in call for call in forge.log)


@requires_helmfile
def test_a_file_that_is_not_a_manifest_takes_part_in_no_clash(script, tmp_path):
    """A LinkML schema, a JSON-LD context and a Markdown page are seeded beside the manifests.

    None of them has a top-level `kind:`, so none of them has an identity to clash over — and two
    of them under one name must not stop the Job."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    forge.put("projects/bbsk/datamodels/observation.linkml.yaml", "id: https://example.test\nname: obs\n")
    forge.put("projects/bbsk/datamodels/observation.v1.context.jsonld", '{"@context":{}}\n')
    forge.put("projects/bbsk/datamodels/observation.v1.md", "# Observation\n\nname: obs\n")
    result = forge.run(script)

    assert result.returncode == 0, result.stderr
    assert len(forge.contents) == 5, forge.contents  # four files and the record
    assert recorded_paths(forge.contents[MANIFEST]) == [
        "projects/bbsk/datamodels/observation.linkml.yaml",
        "projects/bbsk/datamodels/observation.v1.context.jsonld",
        "projects/bbsk/datamodels/observation.v1.md",
        "projects/bbsk/project.yaml",
    ]


@requires_helmfile
def test_the_record_is_committed_with_what_it_records_and_names_no_secret(script, tmp_path):
    """A run that dies before its commit leaves the older record beside the older files, and
    repeats itself: the record and the files it names land in one commit or not at all.

    The record is also a file in the configuration repository, so it holds paths and nothing else
    — no token, no admin password, nothing the Job read out of a Secret."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    assert forge.run(script).returncode == 0

    writes = [call for call in forge.log if "/contents" in call and not call.startswith("GET")]
    assert writes == [COMMIT], writes
    record = forge.contents[MANIFEST]
    assert "not-a-real-password" not in record
    for secret in forge.secrets.values():
        token = base64.b64decode(secret["data"]["token"]).decode()
        assert token not in record
    assert recorded_paths(record) == ["projects/bbsk/project.yaml"]


@requires_helmfile
def test_a_fresh_seed_of_many_files_is_one_commit(script, tmp_path):
    """T-4709: the seed grew to 1,600 files, and a commit per file (a clone of the repository in
    the forge each time) kept the Job past helm's 900 s on every fresh k3d cluster of ci-full.
    However many files, a run is one commit, and a file is judged by its blob id, not read back
    one request at a time."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    for index in range(40):
        forge.put(f"projects/bbsk/datamodels/m{index}.md", f"# Model {index}\n")
    first = forge.run(script)
    assert first.returncode == 0, first.stderr
    assert "seed: 41 file(s) written" in first.stdout
    assert len(forge.contents) == 42, forge.contents  # the files and the record
    assert forge.log.count(COMMIT) == 1, forge.log
    assert not [call for call in forge.log if "/contents/projects/" in call], (
        "a seed file was read or written on its own"
    )

    forge.put("projects/bbsk/datamodels/m0.md", "# Model zero\n")
    second = forge.run(script)
    assert second.returncode == 0, second.stderr
    assert "seed: updated projects/bbsk/datamodels/m0.md" in second.stdout
    assert "seed: 1 file(s) written" in second.stdout
    assert forge.contents["projects/bbsk/datamodels/m0.md"] == "# Model zero\n"
    assert forge.log.count(COMMIT) == 2, forge.log


@requires_helmfile
def test_a_token_is_minted_again_when_its_scopes_change_and_only_then(script, tmp_path):
    """AP-75 (T-2468): the Portal's token gains `write:organization` to create an application's
    repository. A token that still authenticates used to be kept whatever it was minted with, so
    a scope added to the values never reached a running forge. The Secret now records the scopes;
    a different record mints again, the same record keeps the token a pod already holds."""

    forge = Forge(tmp_path, {})
    minted = lambda: [call for call in forge.log if call.endswith("/tokens")]  # noqa: E731

    first = forge.run(script)
    assert first.returncode == 0, first.stderr
    assert len(minted()) == 2, forge.log
    recorded = base64.b64decode(forge.secrets["gitea-token-portal"]["data"]["scopes"]).decode()
    assert recorded == '["write:repository"]'

    second = forge.run(script)
    assert second.returncode == 0, second.stderr
    assert len(minted()) == 2, "a token minted with the same scopes was minted again"
    assert second.stdout.count("still authenticates") == 2

    wider = '["write:repository","read:user","write:organization"]'
    third = forge.run(script, PORTAL_SCOPES=wider)
    assert third.returncode == 0, third.stderr
    assert len(minted()) == 3, "the Portal's token was not minted with its new scope"
    assert "DELETE http://forge.test/api/v1/users/forge-admin/tokens/jc-portal" in forge.log
    assert "the token in gitea-token-gateway still authenticates" in third.stdout
    recorded = base64.b64decode(forge.secrets["gitea-token-portal"]["data"]["scopes"]).decode()
    assert recorded == wider


HELSINKI = """apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: helsinki
  namespace: org
spec:
  organizationRef: hel
"""

ORG_MANIFEST = """apiVersion: joinedcontext.com/v1alpha1
kind: Organization
metadata:
  name: hel
spec:
  domain: hel.fi
"""


def layout2(forge: Forge) -> None:
    forge.put("org.yaml", ORG_MANIFEST)
    forge.put("projects/helsinki/project.yaml", HELSINKI)
    forge.put("projects/helsinki/shared/mesto-kpi.yaml", SHARE)
    forge.put("projects/bbsk/project.yaml", PROJECT.replace("spec:\n", "spec:\n  organizationRef: hel\n"))


@requires_helmfile
def test_layout_2_seeds_the_organization_and_one_repository_per_project(script, tmp_path):
    """A fresh installation in layout 2 (CC-85): the same cut `jcctl migrate` makes of a
    layout 1 repository, and a second run writes nothing. The fake forge always has the
    organization repository, so it starts as one this Job made: `.jc/layout` already 2."""
    forge = Forge(tmp_path, {".jc/layout": "2\n"})
    layout2(forge)
    first = forge.run(script, LAYOUT="2")
    assert first.returncode == 0, first.stderr

    org = forge.contents
    assert org[".jc/layout"] == "2\n"
    assert org["org.yaml"] == ORG_MANIFEST
    assert not [path for path in org if path.startswith("projects/helsinki/")], org.keys()
    assert org["projects/helsinki.yaml"] == (
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: helsinki\n"
        "  namespace: org\nspec:\n  organizationRef: hel\n  repository:\n    name: helsinki\n"
        "  ref: main\n"
    )
    assert "projects/bbsk.yaml" in org

    repos = json.loads(forge.state.read_text())["repos"]
    helsinki = repos["helsinki"]["files"]
    assert helsinki["project.yaml"] == HELSINKI
    assert helsinki["shared/mesto-kpi.yaml"] == SHARE
    assert helsinki[".jc/layout"] == "2\n"
    assert recorded_paths(helsinki[MANIFEST]) == [".jc/layout", "project.yaml", "shared/mesto-kpi.yaml"]
    assert repos["helsinki"]["private"] is True
    assert set(repos) == {"helsinki", "bbsk"}
    # F-R2DEP2-001: `{slug}-writers` write a project repository, so its `main` carries the
    # organization repository's rule: only the Portal pushes or merges there.
    rules = json.loads(forge.state.read_text())["rules"]
    for slug in ("helsinki", "bbsk"):
        assert rules[f"{slug}/main"]["push_whitelist_usernames"] == ["jc-portal"], slug
        assert rules[f"{slug}/main"] == {**rules["configuration/main"], "rule_name": "main"}, slug

    second = forge.run(script, LAYOUT="2")
    assert second.returncode == 0, second.stderr
    assert "created" not in second.stdout, second.stdout
    assert second.stdout.count("seed: 0 file(s) written") == 3, second.stdout


@requires_helmfile
def test_layout_2_refuses_a_layout_1_repository_instead_of_splitting_it(script, tmp_path):
    """Switching the value on an installation that was never migrated would move every project
    file out of the organization repository without its history; the Job stops instead."""
    forge = Forge(tmp_path, {"projects/helsinki/project.yaml": HELSINKI})
    layout2(forge)
    run = forge.run(script, LAYOUT="2")
    assert run.returncode != 0
    assert "not a layout 2 repository; migrate it first" in run.stderr
    assert forge.contents == {"projects/helsinki/project.yaml": HELSINKI}
    assert not json.loads(forge.state.read_text()).get("repos")


@requires_helmfile
def test_layout_2_does_not_adopt_a_project_repository_it_did_not_make(script, tmp_path):
    forge = Forge(tmp_path, {".jc/layout": "2\n"})
    state = json.loads(forge.state.read_text())
    state["repos"] = {"helsinki": {"files": {"README.md": "somebody else's\n"}, "private": True, "head": "0" * 40}}
    forge.state.write_text(json.dumps(state))
    layout2(forge)
    run = forge.run(script, LAYOUT="2")
    assert run.returncode != 0
    assert "joinedcontext/helsinki is not a layout 2 repository" in run.stderr
    assert json.loads(forge.state.read_text())["repos"]["helsinki"]["files"] == {"README.md": "somebody else's\n"}


@requires_helmfile
def test_layout_2_refuses_a_project_without_its_organization(script, tmp_path):
    forge = Forge(tmp_path, {".jc/layout": "2\n"})
    forge.put("projects/bbsk/project.yaml", PROJECT)
    run = forge.run(script, LAYOUT="2")
    assert run.returncode != 0
    assert "projects/bbsk/project.yaml names no apiVersion or spec.organizationRef" in run.stderr


ZILINA_ORG = """apiVersion: joinedcontext.com/v1alpha1
kind: Organization
metadata:
  name: zilina
  namespace: org
spec:
  domain: zilina.sk
"""

ZILINA = """apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: zilina
  namespace: org
spec:
  organizationRef: zilina
"""


@requires_helmfile
def test_layout_2_moves_org_yaml_to_two_organizations(script, tmp_path):
    """ADR-N-058 (T-5242): the seed's `org.yaml` becomes `organizations/hel.yaml` beside
    `organizations/zilina.yaml`. The run after the move deletes the `org.yaml` an earlier run
    wrote, so one Organization is never declared twice, and project zilina is registered under
    its own Organization."""
    forge = Forge(tmp_path, {".jc/layout": "2\n"})
    layout2(forge)
    first = forge.run(script, LAYOUT="2")
    assert first.returncode == 0, first.stderr
    assert "org.yaml" in forge.contents

    for old in forge.seed.iterdir():
        old.unlink()
    forge.put("organizations/hel.yaml", ORG_MANIFEST.replace("spec:\n", "spec:\n  installation: true\n"))
    forge.put("organizations/zilina.yaml", ZILINA_ORG)
    forge.put("projects/helsinki/project.yaml", HELSINKI)
    forge.put("projects/zilina/project.yaml", ZILINA)
    moved = forge.run(script, LAYOUT="2")
    assert moved.returncode == 0, moved.stderr

    org = forge.contents
    assert "org.yaml" not in org, "the old path is gone, so `hel` is declared once"
    assert org["organizations/zilina.yaml"] == ZILINA_ORG
    assert "installation: true" in org["organizations/hel.yaml"]
    assert "organizationRef: zilina\n" in org["projects/zilina.yaml"]
    assert "organizationRef: hel\n" in org["projects/helsinki.yaml"]
    repos = json.loads(forge.state.read_text())["repos"]
    assert repos["zilina"]["files"]["project.yaml"] == ZILINA

    again = forge.run(script, LAYOUT="2")
    assert again.returncode == 0, again.stderr
    assert "created" not in again.stdout, again.stdout
