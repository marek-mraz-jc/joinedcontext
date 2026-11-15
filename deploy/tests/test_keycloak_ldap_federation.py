"""PF-127: a municipality's directory keeps its people and department groups, Keycloak federates it.

The pinned Keycloak and an OpenLDAP server run in containers on one network. The realm gets the
LDAP user federation docs Deployment/18-directory-federation.md describes (imported users, read
only, a group mapper under the parent group `/directory`), and the test checks what the page
promises: a directory person signs in and gets a Keycloak user id, the form a manifest names
them by (PF-126); their department is a Keycloak group a `RoleBinding` binds with
`source: provider`; leaving the department ends the membership, and deleting the person ends
the login, with no change to any repository.
"""

import base64
import json
import re
import shutil
import subprocess
import time
import uuid

import pytest
from test_keycloak_admin_permissions import call, image, token

# osixia/openldap 1.5.0, pinned like every image a test starts.
OPENLDAP = "docker.io/osixia/openldap:1.5.0@sha256:18742e9c449c9c1afe129d3f2f3ee15fb34cc43e5f940a20f3399728f41d7c28"
REALM = "pf127"
BASE_DN = "dc=example,dc=org"
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
# A test password for a throwaway container on a private network, never a credential.
LDAP_ADMIN = "admin-pf127"

pytestmark = [
    pytest.mark.skipif(shutil.which("docker") is None, reason="no container runtime"),
    pytest.mark.xdist_group("docker-keycloak-ldap"),
]

PEOPLE = f"""
dn: ou=people,{BASE_DN}
objectClass: organizationalUnit
ou: people

dn: ou=groups,{BASE_DN}
objectClass: organizationalUnit
ou: groups

dn: uid=jana,ou=people,{BASE_DN}
objectClass: inetOrgPerson
uid: jana
cn: Jana Testova
givenName: Jana
sn: Testova
mail: jana@example.org
userPassword: jana-pf127

dn: uid=peter,ou=people,{BASE_DN}
objectClass: inetOrgPerson
uid: peter
cn: Peter Testovy
givenName: Peter
sn: Testovy
mail: peter@example.org
userPassword: peter-pf127

dn: cn=odbor-dopravy,ou=groups,{BASE_DN}
objectClass: groupOfNames
cn: odbor-dopravy
member: uid=jana,ou=people,{BASE_DN}
member: uid=peter,ou=people,{BASE_DN}
"""


def ldap(name, tool, ldif=None, *args):
    return subprocess.run(
        ["docker", "exec", "-i", name, tool, "-x", "-H", "ldap://localhost",
         "-D", f"cn=admin,{BASE_DN}", "-w", LDAP_ADMIN, *args],
        input=ldif, capture_output=True, text=True, timeout=60, check=False,
    )


@pytest.fixture(scope="module")
def stack():
    tag = uuid.uuid4().hex[:12]
    net, ldap_name, kc_name = f"pf127-{tag}", f"pf127-ldap-{tag}", f"pf127-kc-{tag}"
    subprocess.run(["docker", "network", "create", net], check=True, capture_output=True)
    try:
        subprocess.run(
            ["docker", "run", "-d", "--name", ldap_name, "--network", net, "--network-alias", "ldap",
             "-e", "LDAP_ORGANISATION=Example", "-e", "LDAP_DOMAIN=example.org",
             "-e", f"LDAP_ADMIN_PASSWORD={LDAP_ADMIN}", OPENLDAP],
            check=True, capture_output=True,
        )
        subprocess.run(
            ["docker", "run", "-d", "--name", kc_name, "--network", net, "-P",
             "-e", "KC_BOOTSTRAP_ADMIN_USERNAME=admin", "-e", "KC_BOOTSTRAP_ADMIN_PASSWORD=admin",
             image(), "start-dev"],
            check=True, capture_output=True,
        )
        port = subprocess.run(["docker", "port", kc_name, "8080/tcp"], check=True, capture_output=True,
                              text=True).stdout.split("\n")[0].rsplit(":", 1)[1]
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 180
        while time.time() < deadline:
            ldap_up = ldap(ldap_name, "ldapsearch", None, "-b", BASE_DN, "-s", "base").returncode == 0
            try:
                kc_up = call(base, "GET", "/realms/master")[0] == 200
            except OSError:
                kc_up = False
            if ldap_up and kc_up:
                break
            time.sleep(2)
        else:
            pytest.fail("OpenLDAP and Keycloak did not start within 3 minutes")
        added = ldap(ldap_name, "ldapadd", PEOPLE)
        assert added.returncode == 0, added.stderr
        yield ldap_name, base
    finally:
        subprocess.run(["docker", "rm", "-f", ldap_name, kc_name], capture_output=True, check=False)
        subprocess.run(["docker", "network", "rm", net], capture_output=True, check=False)


@pytest.fixture(scope="module")
def realm(stack):
    """The realm as the page configures it: LDAP federation, imported, read only, one group mapper."""
    _, base = stack
    admin = token(base, "master", "admin-cli", user="admin")
    status, body, _ = call(base, "POST", "/admin/realms", admin, {"realm": REALM, "enabled": True})
    assert status == 201, body
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/groups", admin, {"name": "directory"})
    assert status == 201, body
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/components", admin, {
        "name": "directory", "providerId": "ldap",
        "providerType": "org.keycloak.storage.UserStorageProvider",
        "config": {
            "enabled": ["true"], "vendor": ["other"], "connectionUrl": ["ldap://ldap:389"],
            "authType": ["simple"], "bindDn": [f"cn=admin,{BASE_DN}"], "bindCredential": [LDAP_ADMIN],
            "usersDn": [f"ou=people,{BASE_DN}"], "searchScope": ["1"], "pagination": ["true"],
            "usernameLDAPAttribute": ["uid"], "rdnLDAPAttribute": ["uid"],
            "uuidLDAPAttribute": ["entryUUID"], "userObjectClasses": ["inetOrgPerson"],
            "editMode": ["READ_ONLY"], "importEnabled": ["true"], "syncRegistrations": ["false"],
            "fullSyncPeriod": ["-1"], "changedSyncPeriod": ["-1"], "trustEmail": ["true"],
        },
    })
    assert status == 201, body
    provider = headers["Location"].rsplit("/", 1)[1]
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/components", admin, {
        "name": "departments", "providerId": "group-ldap-mapper", "parentId": provider,
        "providerType": "org.keycloak.storage.ldap.mappers.LDAPStorageMapper",
        "config": {
            "groups.dn": [f"ou=groups,{BASE_DN}"], "group.name.ldap.attribute": ["cn"],
            "group.object.classes": ["groupOfNames"], "membership.ldap.attribute": ["member"],
            "membership.attribute.type": ["DN"], "membership.user.ldap.attribute": ["uid"],
            "mode": ["READ_ONLY"], "user.roles.retrieve.strategy": ["LOAD_GROUPS_BY_MEMBER_ATTRIBUTE"],
            "preserve.group.inheritance": ["false"], "ignore.missing.groups": ["false"],
            "drop.non.existing.groups.during.sync": ["true"], "groups.path": ["/directory"],
        },
    })
    assert status == 201, body
    mapper = headers["Location"].rsplit("/", 1)[1]
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/user-storage/{provider}/sync?action=triggerFullSync", admin)
    assert status == 200, body
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/user-storage/{provider}/mappers/{mapper}/sync?direction=fedToKeycloak", admin)
    assert status == 200, body
    return provider


def sub_of(base, username, password):
    status, body, _ = call(base, "POST", f"/realms/{REALM}/protocol/openid-connect/token", form={
        "client_id": "admin-cli", "grant_type": "password", "scope": "openid",
        "username": username, "password": password})
    if status != 200:
        return None
    claims = body["id_token"].split(".")[1]
    return json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4)))["sub"]


def department_members(base):
    admin = token(base, "master", "admin-cli", user="admin")
    status, body, _ = call(base, "GET", f"/admin/realms/{REALM}/group-by-path/directory/odbor-dopravy", admin)
    assert status == 200, body
    status, members, _ = call(base, "GET", f"/admin/realms/{REALM}/groups/{body['id']}/members", admin)
    assert status == 200, members
    return {m["username"]: m["id"] for m in members}


def test_a_directory_person_signs_in_under_a_keycloak_id_in_their_department(stack, realm):
    _, base = stack
    members = department_members(base)
    assert set(members) == {"jana", "peter"}
    assert all(UUID.match(i) for i in members.values()), members
    assert sub_of(base, "jana", "jana-pf127") == members["jana"]


def test_leaving_the_department_in_the_directory_ends_the_membership(stack, realm):
    ldap_name, base = stack
    changed = ldap(ldap_name, "ldapmodify", (
        f"dn: cn=odbor-dopravy,ou=groups,{BASE_DN}\nchangetype: modify\n"
        f"delete: member\nmember: uid=peter,ou=people,{BASE_DN}\n"))
    assert changed.returncode == 0, changed.stderr
    assert set(department_members(base)) == {"jana"}


def test_a_person_deleted_in_the_directory_no_longer_signs_in(stack, realm):
    ldap_name, base = stack
    assert sub_of(base, "peter", "peter-pf127")
    deleted = ldap(ldap_name, "ldapdelete", None, f"uid=peter,ou=people,{BASE_DN}")
    assert deleted.returncode == 0, deleted.stderr
    assert sub_of(base, "peter", "peter-pf127") is None
