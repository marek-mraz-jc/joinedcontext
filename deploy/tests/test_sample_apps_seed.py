"""The sample applications, each in its own repository on the forge (T-2599, AP-75, AP-77, AP-80).

The bootstrap Job creates `helsinki_<name>` with the Portal's token, pushes the application's
tree to `main` in one commit when it differs from what is there, and commits the App manifest
into the project pinned to that commit. These run the Job's real script against the fake forge,
and read the dev render for what the Job is handed.
"""

import json
import re
import shutil
import subprocess
import zlib
from pathlib import Path

import pytest
from conftest import env_global

import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

ROOT = Path(__file__).resolve().parent.parent
VENDORED = ROOT / "components/gitea/apps"

MANIFEST = """apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: helsinki-bikes
  namespace: helsinki
spec:
  kind: ui
  source:
    git:
      url: https://example.test/git/joinedcontext/helsinki_helsinki-bikes.git
      ref: main
"""

TREE = {
    "README.md": "# Helsinki city bikes\n",
    "src/App.tsx": "export default function App() { return null; }\n",
    # No newline at the end: the blob id has to be taken over the bytes as they are.
    ".gitea/workflows/build.yml": "on: push",
}


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


@pytest.fixture(scope="module")
def script(local):
    return one(local, "Job", "gitea-bootstrap")["spec"]["template"]["spec"]["containers"][0]["args"][0]


def seeded(tmp_path, tree=TREE):
    from test_forge_seed_converges import Forge

    forge = Forge(tmp_path, {})
    app = tmp_path / "apps" / "helsinki-bikes"
    app.mkdir(parents=True)
    for path, text in tree.items():
        (app / path.replace("/", "__")).write_text(text)
    (app / "app.yaml").write_text(MANIFEST)
    return forge, app


def run(forge, script):
    return forge.run(
        script,
        APPS_DIR=str(forge.root / "apps"),
        APPS_PROJECT="helsinki",
        APPS_PUBLIC_BASE="https://joinedcontext.test/git",
    )


def repos(forge):
    import json

    return json.loads(forge.state.read_text())["repos"]


def write_status(forge, path, commit):
    """The Portal appends the lane's status to the manifest's own text, comments kept (AP-13a)."""
    state = json.loads(forge.state.read_text())
    text = state["contents"][path]
    status = {"status": {"build": {"commit": commit, "digest": "sha256:" + "a" * 64}}}
    state["contents"][path] = text.rstrip("\n") + "\n" + yaml.safe_dump(status, sort_keys=False)
    forge.state.write_text(json.dumps(state))


@requires_helmfile
def test_each_app_gets_a_private_repository_holding_its_tree_and_a_manifest_pinned_to_it(script, tmp_path):
    """AP-75, AP-80: the repository is created private under the Portal's naming, holds the tree
    and not the manifest, and the manifest committed into the project names that repository at
    its public clone address and its head commit, never `main`."""
    forge, _ = seeded(tmp_path)
    result = run(forge, script)
    assert result.returncode == 0, result.stderr
    repo = repos(forge)["helsinki_helsinki-bikes"]
    assert repo["private"] is True
    assert repo["files"] == TREE, "the auto-init README is replaced and app.yaml stays out"
    assert len(repo["commits"]) == 1, "the tree goes in as one commit"

    manifest = yaml.safe_load(forge.contents["projects/helsinki/apps/helsinki-bikes/app.yaml"])
    git = manifest["spec"]["source"]["git"]
    assert git["url"] == "https://joinedcontext.test/git/joinedcontext/helsinki_helsinki-bikes.git"
    assert git["ref"] == repo["head"] and len(git["ref"]) == 40
    assert "Authorization: token" not in result.stdout + result.stderr


@requires_helmfile
def test_a_second_run_changes_nothing_and_says_so(script, tmp_path):
    """AP-75: the blob ids match, so no commit and no manifest write; the log says both."""
    forge, _ = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    calls = forge.calls.read_text()

    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    assert repos(forge)["helsinki_helsinki-bikes"]["head"] == head
    assert len(repos(forge)["helsinki_helsinki-bikes"]["commits"]) == 1
    assert "app helsinki-bikes: joinedcontext/helsinki_helsinki-bikes already holds this tree" in second.stdout
    assert f"app helsinki-bikes: projects/helsinki/apps/helsinki-bikes/app.yaml already names {head}" in second.stdout
    written = forge.calls.read_text()[len(calls):]
    assert "POST http://forge.test/api/v1/orgs/joinedcontext/repos" not in written, "never recreated"
    assert "PUT http://forge.test/api/v1/repos/joinedcontext/configuration/contents/projects/helsinki/apps" not in written


@requires_helmfile
def test_a_rerun_keeps_the_build_the_lane_wrote_for_the_same_head(script, tmp_path):
    """T-2633, AP-13a: the Portal rewrote the manifest with status.build; a rerun at the same head
    leaves it, and a new head still moves the manifest. T-2948, AP-80: the new head keeps the
    previous head's build, which serves until the lane proposes the new one; a failed build of
    the new head must not take the App off the air."""
    forge, app = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    path = "projects/helsinki/apps/helsinki-bikes/app.yaml"
    head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    write_status(forge, path, head)

    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    assert f"{path} already names {head} and its build" in second.stdout
    assert yaml.safe_load(forge.contents[path])["status"]["build"]["commit"] == head

    (app / "README.md").write_text("# changed\n")
    third = run(forge, script)
    assert third.returncode == 0, third.stderr
    moved = yaml.safe_load(forge.contents[path])
    new_head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    assert moved["spec"]["source"]["git"]["ref"] == new_head != head
    assert moved["status"] == {"build": {"commit": head, "digest": "sha256:" + "a" * 64}}
    assert f"{path} now names {new_head}, the build of {head} serving until its own" in third.stdout

    # The next run at that head finds the carried build and leaves the file as it is.
    fourth = run(forge, script)
    assert fourth.returncode == 0, fourth.stderr
    assert f"{path} already names {new_head} and its build" in fourth.stdout


@requires_helmfile
def test_a_seed_edit_at_the_same_head_reaches_a_built_manifest_and_keeps_its_build(script, tmp_path):
    """T-4601/T-4597: the seed dropped a removed representation from app.yaml while the App's
    tree stayed at the same head; the built manifest kept the old spec, stopped parsing and took
    five Apps off the edge. The spec moves, the lane's build stays."""
    forge, app = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    path = "projects/helsinki/apps/helsinki-bikes/app.yaml"
    head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    write_status(forge, path, head)

    (app / "app.yaml").write_text(MANIFEST + "# edited\n")
    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    held = forge.contents[path]
    assert "# edited" in held
    moved = yaml.safe_load(held)
    assert moved["spec"]["source"]["git"]["ref"] == head
    assert moved["status"] == {"build": {"commit": head, "digest": "sha256:" + "a" * 64}}

    third = run(forge, script)
    assert third.returncode == 0, third.stderr
    assert f"{path} already names {head} and its build" in third.stdout


@requires_helmfile
def test_a_new_head_of_an_app_that_never_built_carries_no_status(script, tmp_path):
    """T-2948, AP-13a: only a build the lane wrote is carried; the seed never invents one."""
    forge, app = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    path = "projects/helsinki/apps/helsinki-bikes/app.yaml"
    (app / "README.md").write_text("# changed\n")
    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    moved = yaml.safe_load(forge.contents[path])
    assert moved["spec"]["source"]["git"]["ref"] == repos(forge)["helsinki_helsinki-bikes"]["head"]
    assert "status" not in moved


def set_run(forge, repo, **run):
    state = json.loads(forge.state.read_text())
    state.setdefault("runs", {})[repo] = run
    forge.state.write_text(json.dumps(state))


def dispatched(forge):
    return json.loads(forge.state.read_text()).get("dispatched", [])


@requires_helmfile
def test_a_failed_build_of_the_head_is_dispatched_again_at_the_next_run(script, tmp_path):
    """T-5774: prod's builds of 2026-10-19 failed on a runner check fixed hours later, and the
    lane builds on a push only, so ten public Apps answered 404 for a week. The next run of the
    Job dispatches build.yml of an App whose newest run is its head's and failed."""
    forge, _ = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    set_run(forge, "helsinki_helsinki-bikes", head_sha=head, conclusion="failure")

    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    assert dispatched(forge) == ["joinedcontext/helsinki_helsinki-bikes@main"]
    assert f"app helsinki-bikes: the build of {head} had failed; build.yml dispatched again" in second.stdout


@requires_helmfile
@pytest.mark.parametrize("conclusion,status", [("success", "completed"), ("cancelled", "completed"), ("", "running")])
def test_a_passed_cancelled_or_running_build_is_not_dispatched(script, tmp_path, conclusion, status):
    """T-5774: only a failure is retried; a person's cancel and a run in flight stay as they are."""
    forge, _ = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    head = repos(forge)["helsinki_helsinki-bikes"]["head"]
    set_run(forge, "helsinki_helsinki-bikes", head_sha=head, conclusion=conclusion, status=status)
    assert run(forge, script).returncode == 0
    assert dispatched(forge) == []


@requires_helmfile
def test_a_failed_build_of_an_older_commit_is_not_dispatched(script, tmp_path):
    """T-5774: the seed's new head already started its own build; the old failure is history."""
    forge, _ = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    set_run(forge, "helsinki_helsinki-bikes", head_sha="b" * 40, conclusion="failure")
    assert run(forge, script).returncode == 0
    assert dispatched(forge) == []


@requires_helmfile
def test_a_changed_and_a_dropped_file_are_one_commit_and_the_manifest_follows(script, tmp_path):
    """AP-80: an edit of the platform's copy reaches the repository as one commit that updates
    and deletes against the blobs there, and the manifest moves to the new head."""
    forge, app = seeded(tmp_path)
    assert run(forge, script).returncode == 0
    (app / "src__App.tsx").write_text("export default function App() { return 'bikes'; }\n")
    (app / "README.md").unlink()

    result = run(forge, script)
    assert result.returncode == 0, result.stderr
    repo = repos(forge)["helsinki_helsinki-bikes"]
    assert "README.md" not in repo["files"]
    assert repo["files"]["src/App.tsx"].endswith("'bikes'; }\n")
    assert len(repo["commits"]) == 2
    assert "committed 2 file(s)" in result.stdout
    manifest = yaml.safe_load(forge.contents["projects/helsinki/apps/helsinki-bikes/app.yaml"])
    assert manifest["spec"]["source"]["git"]["ref"] == repo["head"]


@requires_helmfile
def test_the_blob_id_is_the_one_git_computes(script, tmp_path):
    """AP-75: the Job's `blob_id` is `git hash-object`, or every run would push the whole tree."""
    body = script.split("blob_id() {", 1)[1].split("}\n", 1)[0] + "}"
    sample = tmp_path / "no-newline"
    sample.write_text("on: push")
    ours = subprocess.run(["sh", "-c", f"blob_id() {{{body}\nblob_id {sample}"], capture_output=True, text=True)
    theirs = subprocess.run(["git", "hash-object", str(sample)], capture_output=True, text=True)
    assert ours.stdout.strip() == theirs.stdout.strip() != ""


# dev's one rewrite of the vendored Apps (T-6542): the presenter beside the steward on each member line.
def presenter_id() -> str:
    return env_global("dev", "presenterUserId")


def presented(text: str) -> str:
    member = r"(?m)^([ \t]*)- user: (de300000-0000-4000-8000-00000000000[1567]( # demo\.[a-z]+)?|demo\.(steward|zilina|barcelona|bratislava)@[A-Za-z0-9.-]+[^\n]*)\n"
    return re.sub(member, lambda m: f"{m.group(1)}- user: {presenter_id()} # presenter\n{m.group(0)}", text)


@requires_helmfile
def test_dev_hands_the_job_every_vendored_file_byte_for_byte(dev):
    """AP-75: each app is one ConfigMap, every file of the vendored tree under its `__` key with
    its exact bytes (a rewritten trailing newline would make every run commit again), all of it
    mounted, and the Job told where a person clones from."""
    index = yaml.safe_load((VENDORED / "index.yaml").read_text())
    job = one(dev, "Job", "gitea-bootstrap")
    volume = next(v for v in job["spec"]["template"]["spec"]["volumes"] if v["name"] == "apps")
    mounted = {
        item["path"] for source in volume["projected"]["sources"] for item in source["configMap"]["items"]
    }
    assert sorted(index["apps"]) == [
        "air-quality", "air-weather-explorer", "alerts-desk", "alerts-heatmap", "bike-rebalancing",
        "bike-weather-demand", "data-quality-inspector", "district-compare", "event-day-planner",
        "helsinki-alerts", "helsinki-bikes", "helsinki-events", "hsl-transport", "kpi-forecast", "news-topics",
        "transit-reach",
    ]
    for app, files in index["apps"].items():
        on_disk = sorted(str(p.relative_to(VENDORED / app)) for p in (VENDORED / app).rglob("*") if p.is_file())
        assert sorted(files) == on_disk, f"{app}: index.yaml and the vendored tree disagree"
        # A release of its own (T-3175): the bootstrap's would not fit Helm's Secret with them; one
        # of three, the one the app's name hashes to, since one no longer holds them all (T-3333).
        config_map = one(dev, "ConfigMap", f"gitea-sample-apps-{app}")
        # Each of the three splits by the hash's next digit, the odd ones into -4 (T-3393), -5
        # (T-3350) and -6 (T-4195); -4 again by the third digit into -7 (T-4195).
        hashed = zlib.adler32(app.encode())
        shard = hashed % 3 + 1
        if hashed // 3 % 2 == 1:
            shard = {1: 4, 2: 6, 3: 5}[shard]
        if shard == 4 and hashed // 6 % 2 == 1:
            shard = 7
        release = "gitea-sample-apps" if shard == 1 else f"gitea-sample-apps-{shard}"
        assert config_map["metadata"]["labels"]["app.kubernetes.io/instance"] == release
        data = config_map["data"]
        for path in files:
            key = path.replace("/", "__")
            assert "__" not in path, f"{app}/{path} would not survive the key encoding"
            assert data[key] == presented((VENDORED / app / path).read_text()), f"{app}/{path} changed on the way"
            assert f"{app}/{key}" in mounted
        assert len(yaml.safe_dump(data).encode()) < 1024 * 1024, f"{app} does not fit one ConfigMap"
        manifest = (VENDORED / app / "app.yaml").read_text()
        assert manifest.count("\n      url: ") == 1 and manifest.count("\n      ref: ") == 1, (
            f"{app}/app.yaml: the Job rewrites exactly one source.git url and ref"
        )
        assert yaml.safe_load(manifest)["metadata"]["namespace"] == "helsinki"
    env = {e["name"]: e.get("value") for e in job["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["APPS_PROJECT"] == "helsinki"
    assert env["APPS_PUBLIC_BASE"].endswith("/git") and env["APPS_PUBLIC_BASE"].startswith("https://")


@requires_helmfile
def test_layout_2_commits_the_manifest_into_the_project_repository(script, tmp_path):
    """CC-85: in layout 2 the project's apps live in its own repository, so the manifest goes
    there at `apps/{name}/app.yaml` and the organization repository gets none."""
    from test_forge_seed_converges import HELSINKI

    forge, _ = seeded(tmp_path)
    forge.state.write_text(forge.state.read_text().replace('"contents": {}', '"contents": {".jc/layout": "2\\n"}'))
    forge.put("projects/helsinki/project.yaml", HELSINKI)
    result = forge.run(
        script,
        LAYOUT="2",
        APPS_DIR=str(forge.root / "apps"),
        APPS_PROJECT="helsinki",
        APPS_PUBLIC_BASE="https://joinedcontext.test/git",
    )
    assert result.returncode == 0, result.stderr
    manifest = yaml.safe_load(repos(forge)["helsinki"]["files"]["apps/helsinki-bikes/app.yaml"])
    assert manifest["spec"]["source"]["git"]["ref"] == repos(forge)["helsinki_helsinki-bikes"]["head"]
    assert not [path for path in forge.contents if "/apps/" in path], forge.contents.keys()


GRANT_ENDPOINT = "projects__helsinki__spaces__helsinki__endpoints__app-helsinki-bikes.yaml"
GRANT_POLICY = "projects__helsinki__policies__app-helsinki-bikes-1.yaml"
GRANTS = {
    f"grants__{GRANT_ENDPOINT}": "kind: Endpoint\nmetadata:\n  name: app-helsinki-bikes\n",
    f"grants__{GRANT_POLICY}": "kind: Policy\nmetadata:\n  name: app-helsinki-bikes-1\n",
}


def with_grants(app):
    for key, text in GRANTS.items():
        (app / key).write_text(text)


@requires_helmfile
def test_the_grants_the_portal_compiled_go_beside_the_manifest_and_never_into_the_app(script, tmp_path):
    """T-2667, CC-61, AP-96: the gateway reads an App's Endpoint and Policies from the
    configuration repository alone, so the seed commits them there, at the paths the Portal's door
    uses, and the application's own repository never sees them."""
    forge, app = seeded(tmp_path)
    with_grants(app)
    result = run(forge, script)
    assert result.returncode == 0, result.stderr
    assert repos(forge)["helsinki_helsinki-bikes"]["files"] == TREE
    for key, text in GRANTS.items():
        path = key.removeprefix("grants__").replace("__", "/")
        assert forge.contents[path] == text, path

    second = run(forge, script)
    assert second.returncode == 0, second.stderr
    assert "projects/helsinki/policies/app-helsinki-bikes-1.yaml is current" in second.stdout


@requires_helmfile
def test_layout_2_commits_the_grants_into_the_project_repository(script, tmp_path):
    """CC-85: in layout 2 a grant's path loses the `projects/{project}/` prefix."""
    from test_forge_seed_converges import HELSINKI

    forge, app = seeded(tmp_path)
    with_grants(app)
    forge.state.write_text(forge.state.read_text().replace('"contents": {}', '"contents": {".jc/layout": "2\\n"}'))
    forge.put("projects/helsinki/project.yaml", HELSINKI)
    result = forge.run(
        script,
        LAYOUT="2",
        APPS_DIR=str(forge.root / "apps"),
        APPS_PROJECT="helsinki",
        APPS_PUBLIC_BASE="https://joinedcontext.test/git",
    )
    assert result.returncode == 0, result.stderr
    files = repos(forge)["helsinki"]["files"]
    assert "spaces/helsinki/endpoints/app-helsinki-bikes.yaml" in files
    assert "policies/app-helsinki-bikes-1.yaml" in files


DEFAULT_GROUP = "users__groups__helsinki-bikes-viewer.yaml"


@requires_helmfile
def test_layout_2_keeps_the_default_groups_in_the_organization_repository(script, tmp_path):
    """AP-118, CC-85: an App's default groups are organization files, so the seed commits them
    to the organization repository in layout 2 too, never into the project's."""
    from test_forge_seed_converges import HELSINKI

    forge, app = seeded(tmp_path)
    with_grants(app)
    group = "kind: Group\nmetadata:\n  name: helsinki-bikes-viewer\n"
    (app / f"grants__{DEFAULT_GROUP}").write_text(group)
    forge.state.write_text(forge.state.read_text().replace('"contents": {}', '"contents": {".jc/layout": "2\\n"}'))
    forge.put("projects/helsinki/project.yaml", HELSINKI)
    result = forge.run(
        script,
        LAYOUT="2",
        APPS_DIR=str(forge.root / "apps"),
        APPS_PROJECT="helsinki",
        APPS_PUBLIC_BASE="https://joinedcontext.test/git",
    )
    assert result.returncode == 0, result.stderr
    assert forge.contents["users/groups/helsinki-bikes-viewer.yaml"] == group
    files = repos(forge)["helsinki"]["files"]
    assert not [path for path in files if path.startswith("users/")], sorted(files)
    assert "policies/app-helsinki-bikes-1.yaml" in files


def test_every_person_a_sample_app_admits_is_admitted_by_its_endpoint():
    """T-2672, EP-14: the gateway admits a person to a `project-list` Endpoint only when a group
    in their token names one of its projects, before it reads any Policy. An App whose `access`
    names a demo person, or whose visibility is `project`, is read through such an Endpoint; a
    demo person in no group of the project gets `403 Access Denied by Policy` on every row, as
    helsinki-alerts did on dev for its own viewer and steward."""
    demo = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
    # A member is named by Keycloak id (PF-126) or, in older trees, by e-mail.
    by_id = {person["id"]: name for name, person in demo.items()}

    def name(user: str) -> str:
        return by_id.get(user, user.split("@")[0])

    checked = 0
    for endpoint_file in sorted(VENDORED.glob("*/grants/projects/*/spaces/*/endpoints/*.yaml")):
        endpoint = yaml.safe_load(endpoint_file.read_text())
        if endpoint["spec"]["audience"] != "project-list":
            continue
        admitted = {endpoint["metadata"]["namespace"], *endpoint["spec"].get("allowedProjects", [])}
        app = yaml.safe_load((endpoint_file.parents[6] / "app.yaml").read_text())
        people = {
            name(subject["user"])
            for entry in app["spec"].get("access", [])
            for subject in entry["subjects"]
            if "user" in subject
        }
        # A role given to a default group reaches its members (AP-118).
        for group_file in (endpoint_file.parents[6] / "grants/users/groups").glob("*.yaml"):
            group = yaml.safe_load(group_file.read_text())
            people |= {name(member["user"]) for member in group["spec"].get("members", [])}
        # The live journeys open every sample as these people (ui/e2e/live/apps-*.spec.ts).
        people |= {"demo.viewer", "demo.steward"}
        for person in sorted(people):
            groups = set(demo[person].get("groups", []))
            assert groups & admitted, (
                f"{endpoint_file.relative_to(VENDORED)} admits {sorted(admitted)}; {person} is in {sorted(groups)}"
            )
            checked += 1
    assert checked, "no sample app reads through a project-list endpoint any more: drop this test"


@requires_helmfile
def test_no_gitea_release_outgrows_the_secret_helm_keeps_it_in(dev):
    """Helm stores a release (its values and its manifest, gzipped and base64-encoded) in one
    Secret of at most 1 MiB; gitea-bootstrap passed it when alerts-desk joined the seed and every
    apply stopped there (T-3175). The values carry what the manifest renders, so twice the
    manifest is the estimate, and a quarter of the Secret stays free for the next app."""

    for release in ("gitea-bootstrap", *SEED_RELEASES, "gitea-sample-apps", "gitea-sample-apps-2",
                    "gitea-sample-apps-3", "gitea-sample-apps-4", "gitea-sample-apps-5", "gitea-sample-apps-6",
                    "gitea-sample-apps-7"):
        docs = [
            d for d in dev
            if (d.get("metadata", {}).get("labels") or {}).get("app.kubernetes.io/instance") == release
        ]
        assert docs, f"{release} renders nothing labelled as its own"
        stored = stored_bytes(docs)
        assert stored < 0.75 * 1024 * 1024, f"{release} would be stored in {stored} bytes"


SEED_RELEASES = ("gitea-seed", *(f"gitea-seed-{n}" for n in range(2, 17)))


def stored_bytes(docs):
    """What Helm's release Secret holds for these objects: values and manifest, gzipped, base64."""
    import base64
    import gzip

    return len(base64.b64encode(gzip.compress(yaml.safe_dump_all(docs).encode() * 2)))


def seed_configmaps(docs):
    return [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith("gitea-seed-")]


@requires_helmfile
def test_every_seed_release_keeps_a_sixth_of_its_ceiling_free(dev):
    """T-4431: the seed is packed by size over the seed releases, so today's seed leaves each of
    them at least 15 % under the 0.75 MiB ceiling; the next open-data change fits without a hand-
    added release."""
    for release in SEED_RELEASES:
        docs = [d for d in seed_configmaps(dev) if d["metadata"]["labels"]["app.kubernetes.io/instance"] == release]
        stored = stored_bytes(docs)
        assert stored < 0.85 * 0.75 * 1024 * 1024, f"{release} holds {stored} bytes"


def synthetic_project(tree):
    """A 900 kB project of seed-like manifests beside Slovakia's, in a copied tree."""
    import random

    folder = tree / "components/context-gateway/seed/slovensko"
    index = yaml.safe_load((folder / "index.yaml").read_text())
    words = random.Random(4431).choices(
        ["urn", "space", "pipeline", "dataset", "obec", "kraj", "rok", "hodnota", "zdroj", "typ"], k=4000)
    total, n = 0, 0
    while total < 900_000:
        name = f"zz-synth-{n:03}.yaml"
        rng = random.Random(n)
        body = "".join(
            f"- id: urn:ngsi-ld:Thing:{rng.randrange(10**9)}\n  name: {' '.join(rng.choices(words, k=12))}\n"
            for _ in range(60))
        (folder / name).write_text(body)
        index[name] = f"projects/zz-synthetic/spaces/zz/{name}"
        total, n = total + len(body), n + 1
    (folder / "index.yaml").write_text(yaml.safe_dump(index))
    return {p.replace("/", "__") for p in index.values() if p.startswith("projects/zz-synthetic/")}


@requires_helmfile
def test_a_900_kb_project_is_split_over_releases_and_every_file_renders_once(rendered_variant):
    """T-4431: a project past what one release has room for spans two releases, each of its files
    in exactly one ConfigMap, and the Job mounts exactly the ConfigMaps rendered."""
    keys = {}

    def edit(tree):
        keys["synthetic"] = synthetic_project(tree)

    docs = rendered_variant("dev", edit)
    cms = seed_configmaps(docs)
    holding = [cm for cm in cms if any(k in keys["synthetic"] for k in cm["data"])]
    releases = {cm["metadata"]["labels"]["app.kubernetes.io/instance"] for cm in holding}
    assert len(releases) >= 2, releases
    rendered = [k for cm in holding for k in cm["data"]]
    assert sorted(rendered) == sorted(keys["synthetic"]), "every synthetic file once"
    job = one(docs, "Job", "gitea-bootstrap")["spec"]["template"]["spec"]
    seed = next(v for v in job["volumes"] if v["name"] == "seed")
    mounted = {s["configMap"]["name"] for s in seed["projected"]["sources"]}
    assert {cm["metadata"]["name"] for cm in cms} == {n for n in mounted if n.startswith("gitea-seed-")}


@requires_helmfile
def test_an_unchanged_seed_renders_the_same_bytes(dev, rendered_variant):
    """T-4431: the packing is deterministic, so an apply of an unchanged seed changes no ConfigMap."""
    again = rendered_variant("dev", lambda tree: None)
    assert yaml.safe_dump_all(seed_configmaps(again)) == yaml.safe_dump_all(seed_configmaps(dev))


@requires_helmfile
def test_the_job_mounts_every_project_s_seed_and_the_releases_split_it(dev):
    """The projects' seed is the gitea-seed releases (T-3998, T-4431): the Job's tree is the
    organization's ConfigMap and exactly the gitea-seed-s<n>-<part> ConfigMaps rendered, each in
    the release its name says, every file of every seed index in one of them."""
    rendered = {
        d["metadata"]["name"]: d for d in dev
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith(("gitea-bootstrap-seed", "gitea-seed-"))
    }
    job = one(dev, "Job", "gitea-bootstrap")["spec"]["template"]["spec"]
    seed = next(v for v in job["volumes"] if v["name"] == "seed")
    assert {s["configMap"]["name"] for s in seed["projected"]["sources"]} == set(rendered)
    assert "gitea-bootstrap-seed" in rendered
    indexed = set()
    for seed_dir in ("helsinki", "banskabystrica", "bbsk", "praha", "zilina", "slovensko"):
        index = yaml.safe_load((ROOT / f"components/context-gateway/seed/{seed_dir}/index.yaml").read_text())
        indexed |= {p.replace("/", "__") for p in index.values()}
    for name, cm in rendered.items():
        release = cm["metadata"]["labels"]["app.kubernetes.io/instance"]
        if name == "gitea-bootstrap-seed":
            assert release == "gitea-bootstrap" and not any(k.startswith("projects__") for k in cm["data"])
            continue
        # A project file an environment adds to the bootstrap seed (dev's preview-steward-write
        # Policies, T-6166) is the chart's own per-project group; never a file of a seed index.
        grouped = re.fullmatch(r"gitea-bootstrap-seed-([a-z0-9-]+)", name)
        if grouped:
            assert release == "gitea-bootstrap"
            assert all(k.startswith(f"projects__{grouped[1]}__") for k in cm["data"]), name
            assert not set(cm["data"]) & indexed, f"{name} holds a seed index file"
            continue
        shard, part = re.fullmatch(r"gitea-seed-s(\d+)-(.+)", name).groups()
        project = re.sub(r"--\d+$", "", part)
        assert release == ("gitea-seed" if shard == "1" else f"gitea-seed-{shard}")
        assert all(k.startswith(f"projects__{project}__") for k in cm["data"])
    keys = [k for cm in rendered.values() for k in cm["data"]]
    assert len(keys) == len(set(keys)), "a file in two ConfigMaps"
    for seed_dir in ("helsinki", "banskabystrica", "bbsk", "praha", "zilina", "slovensko"):
        index = yaml.safe_load((ROOT / f"components/context-gateway/seed/{seed_dir}/index.yaml").read_text())
        missing = {p.replace("/", "__") for p in index.values()} - set(keys)
        assert not missing, f"{seed_dir}: {sorted(missing)[:3]}"


@requires_helmfile
def test_no_configmap_of_dev_nears_the_one_mib_an_object_may_hold(dev):
    """The API server refuses an object over 1 MiB; the forge seed did when Zilina joined it and
    every apply stopped at gitea-bootstrap (T-3177). A quarter stays free for the next project."""
    for d in dev:
        if d.get("kind") == "ConfigMap":
            size = len(yaml.safe_dump(d).encode())
            assert size < 0.75 * 1024 * 1024, f"ConfigMap {d['metadata']['name']} is {size} bytes"


def test_every_demo_person_a_seeded_app_admits_is_admitted_by_its_endpoint():
    """T-3598, T-3595, EP-14: the city Apps reach dev through the gateway seed, not the vendored
    samples, so the check above never read them. praha-odpad and bbsk-mosty gave their default
    group to demo.steward and demo.viewer, whose project groups named neither `praha` nor `bbsk`,
    and both Apps answered `403 Access Denied by Policy` on dev."""
    demo = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
    docs = [
        doc
        for path in sorted((ROOT / "components/context-gateway/seed").glob("*/*.yaml"))
        for doc in yaml.safe_load_all(path.read_text())
        if isinstance(doc, dict) and "kind" in doc
    ]
    members = {}
    for group in (doc for doc in docs if doc["kind"] == "Group"):
        app = group["metadata"].get("annotations", {}).get("joinedcontext.com/app")
        if app:
            names = {m["user"].split("@")[0] for m in group["spec"].get("members", []) if "user" in m}
            members.setdefault(app, set()).update(names)
    checked = 0
    for endpoint in (doc for doc in docs if doc["kind"] == "Endpoint"):
        meta, spec = endpoint["metadata"], endpoint["spec"]
        if not meta["name"].startswith("app-") or spec["audience"] != "project-list":
            continue
        admitted = {meta["namespace"], *spec.get("allowedProjects", [])}
        people = {"demo.viewer", "demo.steward"} | members.get(f"{meta['namespace']}/{meta['name'][4:]}", set())
        for person in sorted(people & demo.keys()):
            groups = set(demo[person].get("groups", []))
            assert groups & admitted, f"{meta['namespace']}/{meta['name']} admits {sorted(admitted)}; {person} is in {sorted(groups)}"
            checked += 1
    assert checked, "no seeded App endpoint was checked"


def generated_policy(name, assignee, operations):
    return (
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n"
        f"  name: {name}\n  namespace: helsinki\n  annotations:\n"
        "    joinedcontext.com/generated-by: portal/app-reconciler\n"
        f"spec:\n  assignee:\n    id: {assignee}\n    kind: role\n  operations: {operations}\n"
    )


@requires_helmfile
def test_a_policy_the_app_no_longer_compiles_to_is_removed_and_nothing_else(script, tmp_path):
    """T-5091: alerts-desk went read-only, and its old write Policy app-alerts-desk-2 stayed in the
    repository, so every project member, the viewer included, could still create and update Alert.
    The seed removes such a file; a hand-written one and another app's stay."""
    forge, app = seeded(tmp_path)
    with_grants(app)
    held = {
        # The app's own, compiled before and not any more: removed.
        "projects/helsinki/policies/app-helsinki-bikes-2.yaml": generated_policy(
            "app-helsinki-bikes-2", "endpoint:helsinki/app-helsinki-bikes", "[createEntity, updateAttrs]"),
        "projects/helsinki/policies/app-helsinki-bikes-3-steward.yaml": generated_policy(
            "app-helsinki-bikes-3-steward", "endpoint:helsinki/app-helsinki-bikes/steward", "[deleteEntity]"),
        # Somebody's own Policy under the app's name prefix: kept.
        "projects/helsinki/policies/app-helsinki-bikes-notes.yaml": (
            "kind: Policy\nmetadata:\n  name: app-helsinki-bikes-notes\n"
            "spec:\n  assignee:\n    id: endpoint:helsinki/app-helsinki-bikes\n    kind: role\n"),
        # An app whose name starts the same: its grant is not this app's to remove.
        "projects/helsinki/policies/app-helsinki-bikes-map-1.yaml": generated_policy(
            "app-helsinki-bikes-map-1", "endpoint:helsinki/app-helsinki-bikes-map", "[queryEntity]"),
    }
    state = json.loads(forge.state.read_text())
    state["contents"].update(held)
    forge.state.write_text(json.dumps(state))

    result = run(forge, script)
    assert result.returncode == 0, result.stderr
    left = set(forge.contents)
    assert "projects/helsinki/policies/app-helsinki-bikes-1.yaml" in left
    assert "projects/helsinki/policies/app-helsinki-bikes-2.yaml" not in left
    assert "projects/helsinki/policies/app-helsinki-bikes-3-steward.yaml" not in left
    assert "projects/helsinki/policies/app-helsinki-bikes-notes.yaml" in left
    assert "projects/helsinki/policies/app-helsinki-bikes-map-1.yaml" in left
    assert "app-helsinki-bikes-2.yaml removed" in result.stdout

    again = run(forge, script)
    assert again.returncode == 0, again.stderr
    assert "the application no longer compiles to it" not in again.stdout


READS = re.compile(r"^(query|retrieve)|^queryBatch$")


def test_no_app_endpoint_policy_grants_a_write_to_everyone_it_admits_or_a_viewer():
    """T-5091: a write granted to `endpoint:{project}/{app}` itself reaches every person the
    endpoint admits, the project's viewers included (app-alerts-desk-2 on Alert, app-bbsk-zaznamy-2
    on the steward's note). A write goes to a named role of the app, never `viewer`, held by a
    group of its own, never the project's group."""
    paths = [*sorted((ROOT / "components/context-gateway/seed").glob("*/*.yaml")),
             *sorted(VENDORED.glob("*/grants/**/*.yaml"))]
    docs = [doc for path in paths for doc in yaml.safe_load_all(path.read_text())
            if isinstance(doc, dict) and "kind" in doc]
    endpoints = {f"endpoint:{d['metadata']['namespace']}/{d['metadata']['name']}": d["spec"]
                 for d in docs if d["kind"] == "Endpoint" and d["metadata"]["name"].startswith("app-")}
    checked = 0
    for policy in (d for d in docs if d["kind"] == "Policy"):
        assignee = policy["spec"]["assignee"]["id"]
        writes = [op for op in policy["spec"].get("operations", []) if not READS.search(op)
                  and not op.endswith("Subscription")]
        base = assignee.rsplit("/", 1)[0]
        if not writes or not (assignee in endpoints or base in endpoints):
            continue
        name = policy["metadata"]["name"]
        assert assignee not in endpoints, f"{name} grants {writes} to every caller of {assignee}"
        role = assignee.rsplit("/", 1)[1]
        spec = endpoints[base]
        assert role != "viewer", f"{name} grants {writes} to the viewer role"
        subjects = {role["name"]: role["subjects"] for role in spec.get("roles", [])}.get(role)
        assert subjects, f"{name}: {base} names no subjects for the role {role}"
        projects = {base.split(":")[1].split("/")[0], *spec.get("allowedProjects", [])}
        for subject in subjects:
            group = subject.get("group", "")
            assert group not in projects and not group.endswith("-viewer"), f"{name}: {role} is held by {group}"
        checked += 1
    assert checked >= 5, checked


def test_every_app_offering_a_map_download_is_granted_the_geometry():
    """An App naming ogc-features or geojson among its downloads serves features without geometry
    unless its grant names `location`: the city data Apps promised GeoJSON and sent none (F-BNT1-018)."""
    paths = sorted((ROOT / "components/context-gateway/seed").glob("*/*.yaml"))
    apps = [doc for path in paths for doc in yaml.safe_load_all(path.read_text())
            if isinstance(doc, dict) and doc.get("kind") == "App"]
    checked = 0
    for app in apps:
        for need in app["spec"].get("dataNeeds") or []:
            if {"ogc-features", "geojson"} & set(need.get("representations") or []) and need.get("attrs"):
                assert "location" in need["attrs"], app["metadata"]["name"]
                checked += 1
    assert checked >= 2


UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def test_every_sample_app_group_on_production_parses_and_names_people_by_id(rendered):
    """T-5774, PF-126: smartcity's rewrites drop the demo people, and a Group left with a bare
    `members:` was null, which the Portal refused ("invalid type: null, expected a sequence"),
    and the steward line became an e-mail, which it refuses too. Every default Group the
    production render seeds holds a list of members, each named by a Keycloak user id."""
    docs = rendered("smartcity")
    groups = {
        key: yaml.safe_load(text)
        for d in docs
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith("gitea-sample-apps-")
        for key, text in (d.get("data") or {}).items()
        if key.startswith("grants__users__groups__")
    }
    assert groups, "the production render seeds the sample apps' default groups"
    for key, group in groups.items():
        members = group["spec"].get("members", [])
        assert isinstance(members, list), f"{key}: members is {members!r}"
        for member in members:
            assert UUID.match(member.get("user", "")), f"{key}: {member} is not a Keycloak user id"
