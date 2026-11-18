"""Unit tests of the crossrealm suite (the live proof is a run against jc-dev, T-7230).

    python3 -m pytest bins/jc/smoke/crossrealm/tests -q
"""

import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent

_spec = importlib.util.spec_from_file_location("smoke_crossrealm_suite", HERE.parent / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

TOML = """
base_host = "jc.example"
mode = "slug"
[identity]
realm = "dev"
[[organizations]]
name = "ba"
hosts = { portal = "Portal.BA.example", api = "api.ba.example", idm = "idm.ba.example" }
[[organizations]]
name = "hel"
hosts = { portal = "portal.hel.example", api = "api.hel.example", idm = "idm.hel.example" }
[[organizations]]
name = "shared"
hosts = { portal = "portal.shared.example" }
"""


def doc():
    return suite.tomllib.loads(TOML)


def test_only_organizations_with_a_sign_in_host_of_their_own_count():
    orgs = suite.organizations(doc())
    assert list(orgs) == ["ba", "hel"]
    assert orgs["ba"]["portal"] == "portal.ba.example"
    assert suite.installation_login(doc()) == "https://jc.example/login"


def test_one_realm_serves_every_row_as_one_skip(tmp_path):
    (tmp_path / "jc.toml").write_text('base_host = "jc.example"\n')
    assert suite.main(["--jc-config", str(tmp_path / "jc.toml"), "--out", str(tmp_path / "out")]) == 0
    rows = json.loads((tmp_path / "out" / "results.json").read_text())["checks"]
    assert [(r["id"], r["result"]) for r in rows] == [("one-realm", "skip")]
    assert rows[0]["reason"] == suite.ONE_REALM


def test_an_organization_without_a_certificate_is_not_routed():
    def shake(host):
        if host.startswith("api."):
            raise suite.ssl.SSLCertVerificationError("self-signed")

    assert suite.routed({"portal": "portal.ba.example"}, shake) is None
    assert suite.routed({"portal": "portal.ba.example", "api": "api.ba.example"}, shake) == \
        "api.ba.example: SSLCertVerificationError"


def test_the_guard_keeps_to_the_runs_hosts_and_writes():
    g = suite.Guard({"idm.ba.example", "api.hel.example"}, {"ba"}, None)
    g.check("GET", "https://api.hel.example/api/endpoints")
    g.check("POST", "https://idm.ba.example/realms/hel/protocol/openid-connect/token")
    g.check("POST", "https://idm.ba.example/realms/ba/login-actions/authenticate")
    g.check("POST", "https://idm.ba.example/admin/realms/ba/clients")
    g.check("DELETE", "https://idm.ba.example/admin/realms/ba/users/1f0e-2a")
    refused = [
        ("GET", "http://api.hel.example/api/endpoints"),          # plain http
        ("GET", "https://evil.example/"),                          # another host
        ("POST", "https://idm.ba.example/admin/realms/hel/users"),  # a realm the run holds no admin of
        ("DELETE", "https://idm.ba.example/admin/realms/ba/groups/1"),
        ("PUT", "https://api.hel.example/api/v1/projects/x"),
    ]
    for method, url in refused:
        try:
            g.check(method, url)
        except suite.Refused:
            continue
        raise AssertionError(f"{method} {url} passed the guard")


def test_a_refused_request_is_a_failed_call_logged_without_its_query(tmp_path):
    log = tmp_path / "requests.log"
    h = suite.Http(suite.Guard({"api.ba.example"}, set(), log))
    st, _, body = h.call("GET", "https://evil.example/x?token=secret")
    assert st == 0 and b"not a host of this run" in body
    assert log.read_text() == "GET evil.example/x REFUSED\n"


class Canned:
    """Answers each GET with the canned (status, headers, body) of its URL."""

    def __init__(self, answers):
        self.answers = answers
        self.set_cookies = []

    def call(self, method, url, **_):
        return self.answers.get(url, (404, {}, b""))


def test_metadata_naming_a_second_realm_or_the_installations_fails(monkeypatch):
    res = suite.Results("crossrealm", "jc.example")
    s = suite.Suite(doc(), res, suite.Guard(set(), set(), None), None, [])
    url = "https://api.ba.example/.well-known/oauth-protected-resource/api/mcp"
    own = "https://idm.ba.example/realms/ba"
    for name, servers, ok in [
        ("own", ["https://jc.example", own], True),               # the gateway's own server + ba's realm
        ("installation", ["https://jc.example/login/realms/dev"], False),
        ("both", [own, "https://jc.example/login/realms/dev"], False),
        ("wrong", ["https://idm.hel.example/realms/hel"], False),
    ]:
        body = json.dumps({"authorization_servers": servers}).encode()
        monkeypatch.setattr(s, "anon", lambda b=body: Canned({url: (200, {}, b)}))
        s.metadata("ba", "api", "/.well-known/oauth-protected-resource/api/mcp", name)
        assert res.rows[-1]["result"] == ("pass" if ok else "fail"), name


def test_the_sign_in_must_go_to_the_organizations_own_realm(monkeypatch):
    res = suite.Results("crossrealm", "jc.example")
    s = suite.Suite(doc(), res, suite.Guard(set(), set(), None), None, [])
    s.orgs = {"ba": s.orgs["ba"]}
    for realm, ok in [("https://idm.ba.example/realms/ba", True), ("https://jc.example/login/realms/dev", False)]:
        answers = {"https://portal.ba.example/api/v1/auth/login":
                   (303, {"location": f"{realm}/protocol/openid-connect/auth?client_id=portal-api"}, b"")}
        monkeypatch.setattr(s, "anon", lambda a=answers: Canned(a))
        n = len(res.rows)
        s.one_realm()
        row = next(r for r in res.rows[n:] if r["id"] == "one-realm.signin.ba")
        assert row["result"] == ("pass" if ok else "fail"), realm
