"""ADR-N-067 2.2, 2.7 (T-6408): every person's token names their Organizations (`jc_org`) and
their groups in them (`jc_groups`), and `groups` holds full paths.

A subgroup in a `full.path: false` groups claim arrives as its bare name, and the gateway PDP,
its project resolution and the Portal's RoleBinding subjects match `groups` by name: a member
of /helsinki/members would have matched any Policy, Group or project named `members`. With
full paths every reader trims the one leading `/`, so a root group reads as before and a
subgroup as `helsinki/members`, which no DNS-1123 name can equal.

Measured on Keycloak 26.8.1 with these mappers (T-6408): a member of /helsinki/members and
/helsinki/traffic-analysts gets `jc_org: ["helsinki"]` (one entry) and both paths in
`jc_groups`; a person in /helsinki alone gets `groups: ["/helsinki"]` and neither claim; a
`jc_org` user attribute set by an admin or through the account console is dropped, because
the realm's user profile holds no unmanaged attribute.
"""

import base64
import json
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
PEOPLE = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
CLAIMS = ("jc_org", "jc_groups")


def realm_of(rendered, env):
    for doc in rendered(env):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return json.loads(base64.b64decode(next(iter(doc["data"].values()))))
    pytest.fail(f"no keycloak-config-cli realm Secret in the {env} render")


def org_mappers(mappers):
    """The two claim mappers of a mapper list, by claim, after checking each reads groups only."""
    found = {}
    for mapper in mappers:
        claim = mapper.get("config", {}).get("claim.name")
        if claim not in CLAIMS:
            continue
        config = mapper["config"]
        assert mapper["protocolMapper"] == "oidc-usermodel-attribute-mapper", mapper
        # The attribute lives on the group; aggregation is what reads it from there.
        assert config["user.attribute"] == claim and config["aggregate.attrs"] == "true", mapper
        assert config["multivalued"] == "true" and config["access.token.claim"] == "true", mapper
        found[claim] = mapper
    return found


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_every_client_carries_both_organization_claims(rendered, env):
    """Both claims on every client that reaches the gateway or the Portal; the realm declares
    them per client, beside `groups`, and never as a realm default scope, which Keycloak would
    also link to every client the Portal creates without its own list (T-6301)."""
    realm = realm_of(rendered, env)
    for client in realm["clients"]:
        found = org_mappers(client.get("protocolMappers", []))
        assert set(found) == set(CLAIMS), f"{env}: {client['clientId']} carries {sorted(found)}"
    cimd = {s["name"]: s for s in realm["clientScopes"]}["connector"]
    assert set(org_mappers(cimd["protocolMappers"])) == set(CLAIMS), "the CIMD connector scope"


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_the_user_profile_holds_no_unmanaged_attribute(rendered, env):
    """`aggregate.attrs` reads a user attribute of the claim's name before the groups'. The
    realm's user profile refuses to store one only while unmanaged attributes stay disabled
    (unset is disabled), so turning them on would let an admin, or the person in the account
    console, write their own `jc_org`."""
    realm = realm_of(rendered, env)
    for component in realm.get("components", {}).get("org.keycloak.userprofile.UserProfileProvider", []):
        profile = json.loads(component["config"].get("kc.user.profile.config", ["{}"])[0])
        assert profile.get("unmanagedAttributePolicy") in (None, "DISABLED"), profile
        assert not {a["name"] for a in profile.get("attributes", [])} & set(CLAIMS), profile
    assert "unmanagedAttributePolicy" not in json.dumps(realm), f"{env} sets unmanagedAttributePolicy"


@pytest.mark.parametrize("env", ["local", "production"])
def test_each_organization_group_carries_its_claims(rendered, env):
    """/helsinki/members and /helsinki/traffic-analysts exist on every realm (prod empty), and
    each carries the Organization's name and its own path, the values the claims read."""
    groups = {g["name"]: g for g in realm_of(rendered, env)["groups"]}
    subs = {s["name"]: s for s in groups["helsinki"]["subGroups"]}
    assert set(subs) == {"members", "traffic-analysts"}, sorted(subs)
    for name, sub in subs.items():
        assert sub["path"] == f"/helsinki/{name}"
        assert sub["attributes"] == {"jc_org": ["helsinki"], "jc_groups": [f"/helsinki/{name}"]}
    # Only an Organization's subgroups carry the attributes, and no group is named by a path.
    for name, group in groups.items():
        assert "/" not in name and "attributes" not in group, name


def test_the_organization_members_are_demo_people_without_a_binding(rendered):
    """demo.orgmember and demo.analyst hold the subgroups and nothing else: no project group,
    no realm role, no RoleBinding in the seed (ADR-N-067 2.2). demo.member stays the T-4911
    sharing person."""
    users = {u["username"].split("@")[0]: u for u in realm_of(rendered, "local")["users"]}
    assert users["demo.orgmember"]["groups"] == ["/platform-readers", "/helsinki/members"]
    assert users["demo.analyst"]["groups"] == [
        "/platform-readers", "/helsinki/members", "/helsinki/traffic-analysts"]
    assert PEOPLE["demo.member"]["groups"] == ["sharing"]
    for name in ("demo.orgmember", "demo.analyst"):
        assert PEOPLE[name]["realmRoles"] == []
        person = PEOPLE[name]["id"]
        named = [p for p in (ROOT / "components/context-gateway/seed").rglob("*.yaml")
                 if person in p.read_text() or name in p.read_text()]
        assert not named, f"{name} is named in {named}"

