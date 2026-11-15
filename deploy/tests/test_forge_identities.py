"""The platform's own forge identities and the rule on `main` (T-2352, T-2353, PF-105, PF-106).

The Portal's and the gateway's tokens used to be minted on the site administrator, so a leaked
`gitea-token-portal` administered the whole forge, and nothing protected the branch the platform
reads its configuration from. These tests run the bootstrap Job's own rendered script against
`tests/fake_forge.py`, as `test_forge_seed_converges.py` does, and read what it did to the forge.
"""

import base64
import json
import re

import pytest

from test_forge_seed_converges import ORG, Forge, requires_helmfile


@pytest.fixture(scope="module")
def script(rendered):
    job = next(
        d
        for d in rendered("local")
        if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap"
    )
    return job["spec"]["template"]["spec"]["containers"][0]["args"][0]


def state(forge: Forge) -> dict:
    return json.loads(forge.state.read_text())


def token_of(forge: Forge, secret: str) -> str:
    return base64.b64decode(forge.secrets[secret]["data"]["token"]).decode()


# An environment with an organization of its own for the applications (PF-106, T-2856).
APART = {
    "APPS_ORG": "joinedcontext-apps",
    "APPS_USER": "jc-apps",
    "APPS_SECRET": "gitea-token-apps",
    "APPS_SCOPES": '["write:repository","write:package"]',
    "APPS_NAMESPACES": "dev",
    "APPS_RESTART": "dev/portal",
    "REGISTRY_SECRET": "app-registry",
    "REGISTRY_SCOPES": '["read:package"]',
    "REGISTRY_HOST": "forge.test",
    "REGISTRY_NAMESPACES": "dev-apps",
    "LANE_SECRET": "gitea-token-lane-secret",
    "LANE_SCOPES": '["write:organization"]',
    "LANE_NAMESPACES": "dev",
    "RUNNER_SECRET": "gitea-runner-registration",
    "RUNNER_NAMESPACES": "dev",
}


@requires_helmfile
def test_each_token_is_minted_on_a_machine_user_that_administers_nothing(script, tmp_path):
    """PF-106: two users, neither an administrator nor able to open an organization, each a
    collaborator on the configuration repository with the one verb it needs, and each token
    minted on its own user. The Secret records on whom, so a token of another user is replaced."""
    forge = Forge(tmp_path, {})
    result = forge.run(script)
    assert result.returncode == 0, result.stderr
    held = state(forge)

    assert held["minted"][:2] == ["jc-portal/jc-portal", "jc-gateway/jc-gateway"], held["minted"]
    for user in ("jc-portal", "jc-gateway"):
        assert held["users"][user]["password_set"], "the forge requires one; it is random and dropped"
        patched = held["users"][user]["patched"]
        assert patched["admin"] is False and patched["allow_create_organization"] is False, patched
    assert held["collaborators"]["configuration/jc-portal"] == "write"
    assert held["collaborators"]["configuration/jc-gateway"] == "read"
    # The Portal publishes a checked App build as a package of the organization; the forge
    # refuses that write (401 reqPackageAccess) unless its team holds the package unit.
    assert held["team_units"]["jc-portal"] == {"repo.code": "read", "repo.packages": "write"}
    for secret, user in (("gitea-token-portal", "jc-portal"), ("gitea-token-gateway", "jc-gateway")):
        owner = base64.b64decode(forge.secrets[secret]["data"]["owner"]).decode()
        assert owner == user, secret
    for line in result.stdout.splitlines() + result.stderr.splitlines():
        assert "password" not in line.lower() or "not-a-real-password" not in line, line


@requires_helmfile
def test_the_administrators_tokens_are_retired_after_their_readers_restart(script, tmp_path):
    """PF-106, T-2353: the forge as it was, both tokens on the administrator and the Secrets
    without an owner. One run mints on the machine users, restarts what read the old tokens at
    start, then deletes the old tokens; the next run changes none of it."""
    forge = Forge(tmp_path, {})
    before = state(forge)
    before["tokens"] = {"forge-admin/jc-portal": True, "forge-admin/jc-gateway": True}
    before["deployments_exist"] = True
    forge.state.write_text(json.dumps(before))

    first = forge.run(script)
    assert first.returncode == 0, first.stderr
    held = state(forge)
    assert held["restarted"] == ["dev/deployments/portal", "dev/deployments/context-gateway"]
    assert "forge-admin/jc-portal" not in held["tokens"] and "forge-admin/jc-gateway" not in held["tokens"]
    order = [line for line in forge.log if "deployments/portal" in line or "forge-admin/tokens/jc-portal" in line]
    assert order[0].startswith("PATCH") and order[-1].startswith("DELETE"), (
        f"the old token went before its reader restarted: {order}"
    )

    second = forge.run(script)
    assert second.returncode == 0, second.stderr
    again = state(forge)
    assert again["restarted"] == held["restarted"], "a token that still authenticates restarted its reader"
    assert again["minted"] == held["minted"]


@requires_helmfile
def test_main_takes_only_the_portals_push_and_merge_never_over_an_outdated_base(script, tmp_path):
    """PF-105, CC-41: one rule on `main`, converged every run. The whitelist names the Portal's
    user alone, stale approvals drop, an outdated branch does not merge, nobody overrides it,
    and no forge approval is asked for because the Verdict is the Portal's."""
    forge = Forge(tmp_path, {})
    assert forge.run(script).returncode == 0
    rule = state(forge)["rules"]["configuration/main"]
    assert rule["push_whitelist_usernames"] == ["jc-portal"] and rule["enable_push_whitelist"] is True
    assert rule["merge_whitelist_usernames"] == ["jc-portal"] and rule["enable_merge_whitelist"] is True
    assert rule["required_approvals"] == 0
    assert rule["dismiss_stale_approvals"] is True
    assert rule["block_on_outdated_branch"] is True
    assert rule["block_admin_merge_override"] is True
    assert rule["enable_bypass_allowlist"] is False and rule["enable_force_push"] is False

    widened = state(forge)
    widened["rules"]["configuration/main"]["push_whitelist_usernames"] = ["jc-portal", "someone"]
    widened["rules"]["configuration/main"]["block_on_outdated_branch"] = False
    forge.state.write_text(json.dumps(widened))
    assert forge.run(script).returncode == 0
    rule = state(forge)["rules"]["configuration/main"]
    assert rule["push_whitelist_usernames"] == ["jc-portal"], "a hand-widened rule stayed wide"
    assert rule["block_on_outdated_branch"] is True


@requires_helmfile
def test_the_seed_commits_as_the_portal_and_never_as_the_administrator(script, tmp_path):
    """PF-105: the rule refuses the administrator's push, so the seed is the Portal's commit."""
    forge = Forge(tmp_path, {})
    forge.put("projects/bbsk/project.yaml", "apiVersion: v1\nkind: Project\nmetadata:\n  name: bbsk\n")
    assert forge.run(script).returncode == 0
    portal = "token:" + token_of(forge, "gitea-token-portal")
    # One ChangeFiles commit per repository (T-4709) or a single file: both are `/contents`.
    writes = [w for w in state(forge)["writes"] if "/contents" in w["url"]]
    assert writes, "the seed wrote nothing"
    assert {w["who"] for w in writes} == {portal}, writes


@requires_helmfile
def test_an_application_repository_already_in_the_organization_stays_the_portals(script, tmp_path):
    """AP-75 until the applications move to `{org}-apps`: what the Portal created with the
    administrator's token it still administers, and the configuration repository is not one."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["repos"] = {"helsinki_bikes": {"files": {}, "head": "c" * 40}}
    forge.state.write_text(json.dumps(held))
    assert forge.run(script).returncode == 0
    collaborators = state(forge)["collaborators"]
    assert collaborators["helsinki_bikes/jc-portal"] == "admin"
    assert collaborators["configuration/jc-portal"] == "write"


@requires_helmfile
def test_the_applications_move_to_their_own_organization_and_machine_user(script, tmp_path):
    """PF-106, T-2856: with an organization for the applications, the Job moves every
    `{project}_{name}` repository there with its history, makes jc-apps their administrator,
    mints the applications', the registry's and the lane's tokens on jc-apps against that
    organization, and takes jc-portal out of the team that may create repositories beside the
    configuration. A second run moves and mints nothing."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["repos"] = {"helsinki_bikes": {"files": {}, "head": "c" * 40}, "helsinki": {"files": {}, "head": "d" * 40}}
    # The team jc-portal created repositories through, before the applications moved.
    held["teams"] = {"joinedcontext/jc-portal": {"id": 3, "spec": {}, "members": ["jc-portal"]}}
    forge.state.write_text(json.dumps(held))
    apart = APART
    first = forge.run(script, **apart)
    assert first.returncode == 0, first.stderr
    held = state(forge)
    assert held["moved"] == {"helsinki_bikes": "joinedcontext-apps"}, "a project repository moved too"
    assert held["collaborators"]["helsinki_bikes/jc-apps"] == "admin"
    # T-2969: the forge mints a repository's runner registration token for an owner of the
    # organization only, and the Portal's build-pods pass asks for it with jc-apps' token.
    assert held["teams"]["joinedcontext-apps/Owners"]["members"] == ["jc-apps"], "jc-apps is not the only new owner"
    assert "jc-portal" in held["left"], "jc-portal still creates repositories beside the configuration"
    assert held["users"]["jc-apps"]["patched"]["admin"] is False
    minted = [m for m in held["minted"] if not m.startswith(("jc-portal/", "jc-gateway/"))]
    assert minted == ["jc-apps/jc-apps", "jc-apps/jc-registry-pull", "jc-apps/jc-lane-secret"], minted
    pull = json.loads(base64.b64decode(forge.secrets["app-registry"]["data"][".dockerconfigjson"]))
    user = base64.b64decode(pull["auths"]["forge.test"]["auth"]).decode().split(":", 1)[0]
    assert user == "jc-apps", "a node pulls as the applications' user, not the administrator"
    assert "PATCH http://kube.test/apis/apps/v1/namespaces/dev/deployments/portal" in forge.log
    assert any("/orgs/joinedcontext-apps/actions/runners/registration-token" in line for line in forge.log)

    second = forge.run(script, **apart)
    assert second.returncode == 0, second.stderr
    again = state(forge)
    assert again["minted"] == held["minted"] and again["moved"] == held["moved"]


@requires_helmfile
def test_a_configuration_repository_named_like_an_application_stays_where_it_is(script, tmp_path):
    """F-R2DEP2-016: the move takes `{project}_{name}` repositories; a configRepo whose name
    holds `_` is the configuration itself and never leaves its organization."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["repos"] = {"helsinki_bikes": {"files": {}, "head": "c" * 40}}
    forge.state.write_text(json.dumps(held))
    run = forge.run(script, REPO="city_config", **APART)
    assert run.returncode == 0, run.stderr
    assert state(forge)["moved"] == {"helsinki_bikes": "joinedcontext-apps"}


@requires_helmfile
def test_a_reader_of_the_configuration_reads_every_application_in_their_own_organization(script, tmp_path):
    """T-3030, PF-79, AP-78: after the move the apps organization had no readers team, so no
    reader opened an App's repository. The Job gives it the same read-only teams, including every
    repository, with the configuration team's members; one who leaves that team leaves this one."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["teams"] = {"joinedcontext/viewers": {"id": 3, "spec": {}, "members": ["demo.steward", "demo.viewer"]}}
    forge.state.write_text(json.dumps(held))
    first = forge.run(script, **APART)
    assert first.returncode == 0, first.stderr
    team = state(forge)["teams"]["joinedcontext-apps/viewers"]
    assert team["spec"]["permission"] == "read" and team["spec"]["includes_all_repositories"] is True, team
    assert team["spec"]["units"] == ["repo.code", "repo.issues", "repo.pulls"], team
    assert sorted(team["members"]) == ["demo.steward", "demo.viewer"], team

    held = state(forge)
    held["teams"]["joinedcontext/viewers"]["members"] = ["demo.steward"]
    forge.state.write_text(json.dumps(held))
    second = forge.run(script, **APART)
    assert second.returncode == 0, second.stderr
    assert state(forge)["teams"]["joinedcontext-apps/viewers"]["members"] == ["demo.steward"]


@requires_helmfile
def test_a_moved_repositorys_waiting_builds_are_dispatched_again_under_its_new_owner(script, tmp_path):
    """T-3026: a run queued before its repository moved keeps the old organization as owner, the
    new organization's runners never take it, and the forge will not cancel or rerun it. The Job
    dispatches build.yml once per branch that had a run waiting, under the new owner, right after
    the move; a repository that stays, and a second run of the Job, dispatch nothing."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["repos"] = {
        "helsinki_bikes": {"files": {}, "head": "c" * 40},
        "praha_desk": {"files": {}, "head": "e" * 40},
        "helsinki": {"files": {}, "head": "d" * 40},
    }
    held["queued"] = {
        "helsinki_bikes": ["main", "agent/app-bikes/run-1", "main"],
        "helsinki": ["main"],
    }
    forge.state.write_text(json.dumps(held))

    first = forge.run(script, **APART)
    assert first.returncode == 0, first.stderr
    assert sorted(state(forge)["dispatched"]) == [
        "joinedcontext-apps/helsinki_bikes@agent/app-bikes/run-1",
        "joinedcontext-apps/helsinki_bikes@main",
    ]

    second = forge.run(script, **APART)
    assert second.returncode == 0, second.stderr
    assert len(state(forge)["dispatched"]) == 2, "a run of the Job without a move dispatched a build"


@requires_helmfile
def test_the_runners_restart_once_when_their_token_moves_to_the_applications_organization(script, tmp_path):
    """T-2969: a runner reads its registration token once, at start, and registers with it after
    every job. When the token it holds is the configuration organization's and the Job now writes
    the applications organization's, the runners restart once; a second run restarts nothing."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    old = {"token": base64.b64encode(b"R" * 40).decode(), "owner": base64.b64encode(ORG.encode()).decode()}
    held["secrets"]["gitea-runner-registration"] = {"data": old}
    held["deployments_exist"] = True
    forge.state.write_text(json.dumps(held))
    apart = {
        **APART,
        "RUNNER_NAMESPACES": "dev-runner",
        "RUNNER_RESTART": "dev-runner/gitea-runner dev-runner/gitea-runner-rust",
    }
    first = forge.run(script, **apart)
    assert first.returncode == 0, first.stderr
    runners = [r for r in state(forge)["restarted"] if "gitea-runner" in r]
    assert runners == ["dev-runner/deployments/gitea-runner", "dev-runner/deployments/gitea-runner-rust"], runners
    owner = base64.b64decode(forge.secrets["gitea-runner-registration"]["data"]["owner"]).decode()
    assert owner == "joinedcontext-apps"

    second = forge.run(script, **apart)
    assert second.returncode == 0, second.stderr
    assert [r for r in state(forge)["restarted"] if "gitea-runner" in r] == runners, "a runner restarted again"


@requires_helmfile
def test_without_an_applications_organization_nothing_moves(script, tmp_path):
    """T-2856: the default keeps the applications beside the configuration, jc-portal creating
    and administering them, and no apps token is minted."""
    forge = Forge(tmp_path, {})
    held = state(forge)
    held["repos"] = {"helsinki_bikes": {"files": {}, "head": "c" * 40}}
    forge.state.write_text(json.dumps(held))
    assert forge.run(script, APPS_ORG="").returncode == 0
    held = state(forge)
    assert not held.get("moved") and not held.get("left")
    owners = {k: t["members"] for k, t in held.get("teams", {}).items() if k.endswith("/Owners")}
    assert not any(owners.values()), f"somebody became an owner: {owners}"
    assert not any(m.startswith("jc-apps/") for m in held["minted"])


def _portal_env(docs):
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    return {e["name"]: e for e in portal["spec"]["template"]["spec"]["containers"][0]["env"]}


@requires_helmfile
def test_one_value_moves_the_applications_and_the_portal_follows(rendered, rendered_variant):
    """PF-106, T-2856: `gitea.forge.appsOrganization` is the switch. Off by default, so no
    environment moves a repository by surprise; on, the Job gets the organization and the
    Portal the owner and the apps token, by secretRef only."""
    job = next(d for d in rendered("local") if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap")
    env = {e["name"]: e.get("value") for e in job["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["APPS_ORG"] == ""
    assert "JC_GITEA_APPS_OWNER" not in _portal_env(rendered("local"))
    reader = _portal_env(rendered("local"))["JC_GITEA_READER"]["value"]
    assert reader == env["GATEWAY_USER"], "the Portal names another reader than the gateway's user"

    def apart(tree):
        path = tree / "components/gitea/default-environment.yaml.gotmpl"
        text = path.read_text()
        assert '    appsOrganization: ""\n' in text
        path.write_text(text.replace('    appsOrganization: ""\n', "    appsOrganization: joinedcontext-apps\n"))

    docs = rendered_variant("local", apart)
    portal = _portal_env(docs)
    assert portal["JC_GITEA_APPS_OWNER"]["value"] == "joinedcontext-apps"
    token = portal["JC_GITEA_APPS_TOKEN"]
    assert "value" not in token and token["valueFrom"]["secretKeyRef"]["name"] == "gitea-token-apps"
    job = next(d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap")
    env = {e["name"]: e.get("value") for e in job["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["APPS_ORG"] == "joinedcontext-apps"
    assert env["APPS_RESTART"] == "local/portal"
    lane = [d for d in docs if d.get("kind") == "CronJob" and "lane" in d["metadata"]["name"]]
    for cronjob in lane:
        containers = cronjob["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"]
        org = next(e["value"] for e in containers[0]["env"] if e["name"] == "ORG")
        assert org == "joinedcontext-apps", "the lane's secret belongs to the organization its builds run in"


@requires_helmfile
def test_dev_keeps_its_applications_apart_and_restarts_only_its_runners(rendered):
    """T-2969 (owner decision 2026-09-26): dev keeps the applications in `joinedcontext-apps`,
    so the build-pods pass mints runner tokens as jc-apps, an owner of that organization only.
    The Job may restart the runner Deployments, by name only."""
    docs = rendered("dev")
    job = next(d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "gitea-bootstrap")
    env = {e["name"]: e.get("value") for e in job["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["APPS_ORG"] == "joinedcontext-apps"
    assert _portal_env(docs)["JC_GITEA_APPS_OWNER"]["value"] == "joinedcontext-apps"
    # T-3030: a sign-in keeps the reading teams of both organizations in step with the group.
    configure = next(
        d["stringData"]["configure_gitea.sh"] for d in docs
        if d.get("kind") == "Secret" and "configure_gitea.sh" in d.get("stringData", {})
    )
    maps = re.findall(r'--group-team-map "(.*?)" --group-team-map-removal', configure)
    assert maps, "no group-team map on the forge's login"
    for raw in maps:
        for group, orgs in json.loads(raw.replace('\\"', '"')).items():
            # A project's own two teams (PF-87, T-2647) reach that project's repository in the
            # configuration's organization alone; every other group reads both organizations.
            # The claim holds full paths (ADR-N-067 2.2, T-6408): the key is `/{group}`.
            assert group.startswith("/") and "/" not in group[1:], group
            if group.endswith(("-readers", "-writers")) and group != "/platform-readers":
                assert orgs == {"joinedcontext": [group[1:]]}, (group, orgs)
                continue
            assert orgs["joinedcontext"] and orgs.get("joinedcontext-apps") == orgs["joinedcontext"], (group, orgs)
    targets = env["RUNNER_RESTART"].split()
    assert [t.split("/")[1] for t in targets] == ["gitea-runner", "gitea-runner-rust"], targets
    namespace = targets[0].split("/")[0]
    deployments = {d["metadata"]["name"] for d in docs if d.get("kind") == "Deployment" and d["metadata"].get("namespace") == namespace}
    assert {"gitea-runner", "gitea-runner-rust"} <= deployments, deployments
    role = next(
        d for d in docs
        if d.get("kind") == "Role" and d["metadata"]["name"] == "gitea-bootstrap" and d["metadata"]["namespace"] == namespace
    )
    patch = [r for r in role["rules"] if "deployments" in r.get("resources", [])]
    assert len(patch) == 1 and patch[0]["verbs"] == ["patch"], patch
    assert {"gitea-runner", "gitea-runner-rust"} <= set(patch[0].get("resourceNames", [])), patch
