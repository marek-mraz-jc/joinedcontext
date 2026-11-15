"""T-6542: one presenter, the same login and rights on dev, stage and the production instance.

The owner presents as `presenter@hel.fi` on all three. Each realm seeds the person under one id
with the same realm roles and groups, the forge seeds bind them beside the demo steward and each
other Organization's administrator, with no realm role and no second factor (owner 04:52Z), and
the password is the owner's dev credential, copied hashed into the sops file (owner 04:48Z).
"""

import base64
import json
import os
import re
import subprocess
import uuid
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
ENVIRONMENTS = ROOT / "ci/example-deployments/environments"
KEYCLOAK = ROOT / "components/keycloak"
INSTANCES = ("dev", "stage", "smartcity")
PRESENTER = yaml.safe_load((KEYCLOAK / "presenter.yaml").read_text())
PEOPLE = yaml.safe_load((KEYCLOAK / "demo-users.yaml").read_text())
ADMINS = ("demo.steward", "demo.zilina", "demo.barcelona", "demo.bratislava")


def presenter_id(env: str) -> str | None:
    path = ENVIRONMENTS / env / "global.yaml.gotmpl"
    found = re.findall(r"(?m)^  presenterUserId: (\S+)", path.read_text()) if path.is_file() else []
    return found[0] if found else None


def test_the_three_instances_name_one_presenter_and_no_other_does():
    ids = {env: presenter_id(env) for env in INSTANCES}
    assert len(set(ids.values())) == 1, ids
    [one] = set(ids.values())
    assert uuid.UUID(one).version == 4 and not one.startswith("de300000-"), "a demo id is refused on production"
    others = [p.name for p in ENVIRONMENTS.iterdir() if p.is_dir() and p.name not in INSTANCES and presenter_id(p.name)]
    assert not others, others


def test_the_presenter_reads_what_the_administrators_read_with_no_realm_role():
    """Security: the groups of the steward and the other Organizations' administrators, and no realm
    role: `portal-approver` carries `mfa-required` on stage and prod, and the owner wants no TOTP
    (04:52Z). What the person may do is the seeds' bindings beside those administrators."""
    assert PRESENTER["realmRoles"] == []
    assert set(PRESENTER["groups"]) == set().union(*(PEOPLE[name]["groups"] for name in ADMINS))
    assert PRESENTER["credential"] == "copied"
    assert "id" not in PRESENTER, "the id is per instance values, the same on all three"


def test_the_copied_credential_is_only_in_the_sops_file():
    """The owner's dev password as Keycloak stores it (04:48Z), and only once `capture` ran."""
    path = KEYCLOAK / "presenter.enc.yaml"
    if not path.exists():
        pytest.skip("scripts/presenter-secret.sh capture has not run on dev yet")
    encrypted = yaml.safe_load(path.read_text())
    values = {key: value for key, value in encrypted.items() if key != "sops"}
    assert set(values) == {"from", "secretData", "credentialData"}
    assert all(str(value).startswith("ENC[AES256_GCM,") for value in values.values()), "a plain value in Git"


@pytest.mark.parametrize("args,refusal", [
    ([], "usage"),
    (["load"], "--realm is required"),
    (["capture", "--realm", "dev"], "--from is required"),
    (["capture", "--realm", "dev", "--from", "a b"], "takes a Keycloak username"),
    (["load", "--realm", "Dev;x"], "lowercase letters"),
    (["load", "--realm", "stage"], "--instance is required"),
])
def test_the_script_refuses_before_it_reaches_a_cluster(args, refusal):
    result = subprocess.run(
        [str(ROOT / "scripts/presenter-secret.sh"), *args], cwd=ROOT, capture_output=True, text=True, check=False,
        env={"PATH": os.environ["PATH"]},
    )
    assert result.returncode == 2 and refusal in result.stderr, result.stderr


def config_users(docs: list[dict]) -> dict[str, dict]:
    """The realm import's users, from the keycloak-config-cli realm Secret."""
    for doc in docs:
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realms = [json.loads(base64.b64decode(value)) for value in doc["data"].values()]
            return {user["username"]: user for realm in realms for user in realm.get("users") or []}
    pytest.fail("no keycloak-config-cli realm Secret in the render")


def seed_texts(docs: list[dict]) -> list[str]:
    return [
        text
        for doc in docs
        if doc.get("kind") == "ConfigMap" and doc["metadata"]["name"].startswith(("gitea-", "sample-apps"))
        for text in (doc.get("data") or {}).values()
        if isinstance(text, str)
    ]


@pytest.mark.parametrize("env", INSTANCES)
def test_each_realm_seeds_the_same_person_with_no_second_factor_and_no_password_of_its_own(rendered, env):
    docs = rendered(env)
    user = config_users(docs).get("presenter@hel.fi")
    assert user, f"{env}: the realm import seeds no presenter"
    [default] = user["realmRoles"]
    assert default.startswith("default-roles-")
    assert set(user["groups"]) == {"/platform-readers"} | {f"/{g}" for g in PRESENTER["groups"]}
    # keycloak-config-cli leaves the copied credential alone only when it is given none.
    assert "credentials" not in user and user["requiredActions"] == []
    assert not [d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"] == "keycloak-user-presenter"]
    assert "USER_PASSWORD_PRESENTER" not in json.dumps(docs)


@pytest.mark.parametrize("env", INSTANCES)
def test_the_seeds_bind_the_presenter_beside_every_administrator_once(rendered, env):
    one = presenter_id(env)
    texts = seed_texts(rendered(env))
    bound = [t for t in texts if one in t]
    assert bound, f"{env}: no seed names the presenter"
    for org in ("hel", "zilina", "barcelona", "bratislava"):
        assert any(
            doc.get("kind") == "RoleBinding" and doc["spec"]["role"] == "org-admin"
            and doc["spec"]["scope"] == {"organization": org}
            and {"user": one} in doc["spec"]["subjects"]
            for text in bound for doc in yaml.safe_load_all(text) if isinstance(doc, dict)
        ), f"{env}: the presenter administers no {org}"
    for text in bound:
        for doc in yaml.safe_load_all(text):
            if not isinstance(doc, dict):
                continue
            members = (doc.get("spec") or {}).get("subjects") or (doc.get("spec") or {}).get("members") or []
            assert sum(m == {"user": one} for m in members) <= 1, doc["metadata"]
