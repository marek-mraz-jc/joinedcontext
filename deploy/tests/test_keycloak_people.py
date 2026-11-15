"""PF-126, ADR-N-061: the seeds name people by fixed Keycloak ids, and the people Job gives them.

keycloak-config-cli creates a user through the admin API, which drops the `id` it is given
(measured on Keycloak 26.8.0 with keycloak-config-cli 6.5.1-26 on 2026-10-25: a user imported
with id de300000-...-01 came out as 5079bcc6-...). The people Job (components/keycloak/charts/people)
creates each seeded person through the realm's partial import, which keeps the id, before the
realm import sets their password, roles and groups. Here its script runs in the pinned Keycloak;
the render and seed checks need no container.
"""

import base64
import json
import re
import shutil
import subprocess
import time
import uuid
from pathlib import Path

import pytest
from conftest import env_global
import yaml
from test_keycloak_admin_permissions import call, image, token

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "components/keycloak/charts/people/files/ensure-user-ids.sh"
SEED = ROOT / "components/context-gateway/seed"
DEMO = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
COMPONENT = yaml.safe_load((ROOT / "components/keycloak/component.yaml").read_text())
REALM = "pf126"
# The owner's real Keycloak id on smartcity (global.initialUserId): the people Job refuses an id
# another person already holds, so production never takes the defaults' placeholder.
def smartcity_steward() -> str:
    return env_global("smartcity", "participationStewardId")
def smartcity_owner() -> str:
    return env_global("smartcity", "initialUserId")
# The owner's e-mail (global.initialUserEmail): a private value, read where it is set (T-6982).
def smartcity_owner_email() -> str:
    return env_global("smartcity", "initialUserEmail")

# One worker runs the module, so one Keycloak serves its tests (T-3897, T-5370).
pytestmark = [pytest.mark.xdist_group("docker-keycloak-people")]
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
# Where a manifest names a person: a Group member or RoleBinding/App subject, a user assignee of
# a Policy and a ServiceAccount's accountable owner.
PERSON = re.compile(
    r"^\s*-?\s*user: (?P<a>[^\s#}]+)|kind: user, id: (?P<b>[^\s#}]+)|owner: \{ user: (?P<c>[^\s#}]+)"
    r"|^\s+kind: user\n\s+id: (?P<d>[^\s#}]+)",
    re.MULTILINE,
)


def seed_files():
    return sorted(p for p in SEED.rglob("*") if p.suffix in (".yaml", ".yml"))


def test_no_seed_names_a_person_by_e_mail():
    found = [
        f"{p.relative_to(ROOT)}:{n}: {line.strip()}"
        for p in seed_files()
        for n, line in enumerate(p.read_text().splitlines(), 1)
        if re.search(r"(user|id): [^\s#]*@", line)
    ]
    assert found == [], "name the person by their Keycloak id (PF-126):\n" + "\n".join(found)


def test_every_person_a_seed_names_is_a_demo_person_by_id():
    ids = {user["id"] for user in DEMO.values()}
    names = []
    for p in seed_files():
        for match in PERSON.finditer(p.read_text()):
            value = next(v for v in match.groups() if v)
            if not value.startswith("service-account-"):
                names.append((p.relative_to(ROOT).as_posix(), value))
    assert len(names) > 30, "the seed tree moved: update PERSON"
    assert [n for n in names if n[1] not in ids] == []


def test_each_demo_person_has_their_own_fixed_id():
    ids = [user.get("id", "") for user in DEMO.values()]
    assert all(UUID.match(i) and i.startswith("de300000-") for i in ids), ids
    assert len(set(ids)) == len(ids)


def test_the_people_run_before_the_realm_import():
    parts = {part["name"]: part for part in COMPONENT["parts"]}
    assert parts["people"]["needs"] == ["keycloak.app"]
    assert parts["config"]["needs"] == ["keycloak.people"]


def people_job(docs):
    users = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "keycloak-user-ids")
    job = next(d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "keycloak-user-ids")
    return [line.split() for line in users["data"]["users"].splitlines() if line.strip()], job


def test_dev_gives_every_demo_person_their_id(rendered):
    lines, job = people_job(rendered("dev"))
    # The presenter (T-6542) beside them, never deleted.
    expected = [[u["id"], f"{name}@hel.fi", "true"] for name, u in DEMO.items()] + [[presenter(), "presenter@hel.fi", "false"]]
    assert sorted(lines) == sorted(expected)
    assert job["metadata"]["annotations"]["helm.sh/hook"] == "post-install,post-upgrade"
    pod = job["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False
    assert pod["containers"][0]["image"].split("@")[1].startswith("sha256:")


def test_a_production_instance_names_its_owner_by_id_and_never_deletes_them(rendered):
    docs = rendered("smartcity")
    lines, _ = people_job(docs)
    assert UUID.match(smartcity_owner()) and not smartcity_owner().startswith(("0a000000-", "de300000-"))
    # The owner, never deleted, and the Decidim administrator (T-6448), both under their own ids.
    assert lines == [
        [smartcity_steward(), "participacia.steward@hel.fi", "true"],
        [presenter(), "presenter@hel.fi", "false"],
        [smartcity_owner(), smartcity_owner_email(), "false"],
    ]
    assert not smartcity_steward().startswith(("0a000000-", "de300000-"))
    seeds = "\n".join(
        str(v) for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith("gitea-seed")
        for v in (d.get("data") or {}).values()
    )
    assert "de300000-" not in seeds, "a demo person's id reached a production seed"
    assert smartcity_owner() in seeds, "the steward's grants lost their holder"


# --- the script in the pinned Keycloak ------------------------------------------------------


@pytest.fixture(scope="module")
def keycloak():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime for Keycloak")
    name = f"pf126-kc-{uuid.uuid4().hex[:12]}"
    subprocess.run(
        ["docker", "run", "-d", "--name", name, "-P", "-e", "KC_BOOTSTRAP_ADMIN_USERNAME=admin",
         "-e", "KC_BOOTSTRAP_ADMIN_PASSWORD=admin", image(), "start-dev"],
        check=True, capture_output=True,
    )
    try:
        port = subprocess.run(["docker", "port", name, "8080/tcp"], check=True, capture_output=True,
                              text=True).stdout.split("\n")[0].rsplit(":", 1)[1]
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 180
        while time.time() < deadline:
            try:
                if call(base, "GET", "/realms/master")[0] == 200:
                    break
            except OSError:
                pass
            time.sleep(2)
        else:
            pytest.fail("Keycloak did not start within 3 minutes")
        # Every test but the fresh-installation one starts from an existing realm, so each runs alone.
        status, body, _ = call(base, "POST", "/admin/realms", token(base, "master", "admin-cli", user="admin"),
                               {"realm": REALM, "enabled": True})
        assert status == 201, body
        subprocess.run(["docker", "cp", str(SCRIPT), f"{name}:/tmp/people.sh"], check=True)
        yield name, base
    finally:
        subprocess.run(["docker", "rm", "-f", name], check=False, capture_output=True)


def run(keycloak, users):
    name, _ = keycloak
    lines = "".join(f"{i} {u} {r}\n" for i, u, r in users)
    return subprocess.run(
        ["docker", "exec", "-i", "-e", "HOME=/tmp", "-e", "KEYCLOAK_URL=http://127.0.0.1:8080",
         "-e", "KEYCLOAK_USER=admin", "-e", "KEYCLOAK_PASSWORD=admin", "-e", f"REALM={REALM}",
         "-e", "USERS_FILE=/dev/stdin", name, "/bin/bash", "/tmp/people.sh"],
        check=False, input=lines, capture_output=True, text=True, timeout=300,
    )


def held(base, username):
    admin = token(base, "master", "admin-cli", user="admin")
    status, body, _ = call(base, "GET", f"/admin/realms/{REALM}/users?exact=true&username={username}", admin)
    assert status == 200, body
    return [u["id"] for u in body]


# The person the owner presents as on dev, stage and smartcity (T-6542, tests/test_presenter.py).
def presenter() -> str:
    return env_global("smartcity", "presenterUserId")
STEWARD = ("de300000-0000-4000-8000-000000000001", "demo.steward@hel.fi", "true")
OWNER = ("0a000000-0000-4000-8000-000000000001", "owner@example.org", "false")


def test_a_fresh_installation_gets_the_realm_and_the_ids(keycloak):
    _, base = keycloak
    admin = token(base, "master", "admin-cli", user="admin")
    call(base, "DELETE", f"/admin/realms/{REALM}", admin)
    result = run(keycloak, [STEWARD, OWNER])
    assert result.returncode == 0, result.stderr
    assert held(base, STEWARD[1]) == [STEWARD[0]]
    assert held(base, OWNER[1]) == [OWNER[0]]
    again = run(keycloak, [STEWARD, OWNER])
    assert again.returncode == 0, again.stderr
    assert again.stdout.count("is in place") == 2


def test_a_demo_person_under_another_id_is_made_again_and_signs_in_under_theirs(keycloak):
    _, base = keycloak
    admin = token(base, "master", "admin-cli", user="admin")
    for user_id in held(base, STEWARD[1]):
        call(base, "DELETE", f"/admin/realms/{REALM}/users/{user_id}", admin)
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/users", admin,
                           {"username": STEWARD[1], "email": STEWARD[1], "enabled": True})
    assert status == 201, body
    assert held(base, STEWARD[1]) != [STEWARD[0]]
    result = run(keycloak, [STEWARD])
    assert result.returncode == 0, result.stderr
    assert held(base, STEWARD[1]) == [STEWARD[0]]
    # What the realm import does next: a name and a password. The token's subject is the fixed id.
    status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/users/{STEWARD[0]}", admin,
                           {"firstName": "Demo", "lastName": "Steward", "email": STEWARD[1]})
    assert status == 204, body
    status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/users/{STEWARD[0]}/reset-password", admin,
                           {"type": "password", "value": STEWARD[1], "temporary": False})
    assert status == 204, body
    # admin-cli's access token is a lightweight one without `sub`; the ID token carries it.
    status, body, _ = call(base, "POST", f"/realms/{REALM}/protocol/openid-connect/token", form={
        "client_id": "admin-cli", "grant_type": "password", "scope": "openid",
        "username": STEWARD[1], "password": STEWARD[1]})
    assert status == 200, body
    claims = body["id_token"].split(".")[1]
    assert json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4)))["sub"] == STEWARD[0]


def test_an_owner_under_another_id_is_never_deleted(keycloak):
    _, base = keycloak
    admin = token(base, "master", "admin-cli", user="admin")
    for user_id in held(base, OWNER[1]):
        call(base, "DELETE", f"/admin/realms/{REALM}/users/{user_id}", admin)
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/users", admin,
                           {"username": OWNER[1], "email": OWNER[1], "enabled": True})
    assert status == 201, body
    before = held(base, OWNER[1])
    result = run(keycloak, [OWNER])
    assert result.returncode != 0
    assert f"holds {before[0]}" in result.stderr and "global.initialUserId" in result.stderr
    assert held(base, OWNER[1]) == before


def test_an_id_that_is_not_a_keycloak_id_is_refused(keycloak):
    result = run(keycloak, [("demo.steward", STEWARD[1], "true")])
    assert result.returncode != 0 and "is not a lower-case Keycloak user id" in result.stderr


def test_a_refused_line_changes_nobody_else_in_the_run(keycloak):
    # Every line is checked before the first write (T-5846): a new person beside a refused owner
    # is not created either, so a fixed deployment applies the whole list in one go.
    _, base = keycloak
    admin = token(base, "master", "admin-cli", user="admin")
    newcomer = ("de300000-0000-4000-8000-0000000000aa", "demo.newcomer@hel.fi", "true")
    for user_id in held(base, OWNER[1]):
        call(base, "DELETE", f"/admin/realms/{REALM}/users/{user_id}", admin)
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/users", admin,
                           {"username": OWNER[1], "email": OWNER[1], "enabled": True})
    assert status == 201, body
    result = run(keycloak, [newcomer, OWNER])
    assert result.returncode != 0 and "global.initialUserId" in result.stderr
    assert held(base, newcomer[1]) == []


def test_every_demo_person_lands_in_one_run(keycloak):
    _, base = keycloak
    people = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
    users = [(p["id"], name, "true") for name, p in people.items()]
    admin = token(base, "master", "admin-cli", user="admin")
    for user_id, _, _ in users:  # the earlier tests gave the steward's id to another username
        call(base, "DELETE", f"/admin/realms/{REALM}/users/{user_id}", admin)
    result = run(keycloak, users)
    assert result.returncode == 0, result.stderr
    for user_id, name, _ in users:
        assert held(base, name) == [user_id], name
    assert result.stdout.count(" created") == len(users) - result.stdout.count("is in place")
