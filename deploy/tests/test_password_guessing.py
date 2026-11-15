"""Attack vector: password guessing and the demo accounts in production (T-1680, PF-46).

The attack, in three moves:

1. a thousand passwords against one account,
2. one password against a thousand account names,
3. the demo people and the bootstrap administrator, tried with the credentials whoever
   reads `DEMO.md` already knows.

Every defence against the three is realm configuration or edge configuration, so every
assertion here reads the rendered deployment and needs no running system: the suite is part
of the fast lane and is replayed on every commit rather than on a cluster somebody remembers
to point it at.

A second factor is required of every holder of `mfa-required` (approvers and platform
administrators, T-5120: tests/test_keycloak_login_hardening.py) and prompted for anyone else who
enrolled one, which is what
`test_an_administrator_who_enrolled_a_second_factor_is_always_asked_for_it` asserts.
"""

import base64
import json

import pytest

# The three environments Keycloak is rendered for. `production` is the one the attack is aimed
# at; `dev` and `local` are here because a defence that is only on in production is a defence
# nobody ever sees fail.
ENVIRONMENTS = ["local", "dev", "production"]

#: How many wrong passwords a single account tolerates before Keycloak starts making the
#: attacker wait. Ten is the realm's `failureFactor`; a higher number is the defence weakening.
MAX_GUESSES_PER_ACCOUNT = 10


def realms(rendered, env):
    """Every realm of an environment's render, by realm name."""
    for doc in rendered(env):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return {
                realm["realm"]: realm
                for realm in (json.loads(base64.b64decode(v)) for v in doc["data"].values())
            }
    pytest.fail(f"no keycloak-config-cli realm Secret in the {env} render")


def platform_realm(rendered, env):
    """The realm the platform's people and workloads live in. It is named after the
    environment in `local` and `dev` and `jc` in production, so it is found by not being
    `master` rather than by a name."""
    found = realms(rendered, env)
    rest = [name for name in found if name != "master"]
    assert len(rest) == 1, f"{env} renders {rest} beside master"
    return found[rest[0]]


def people(realm):
    """The realm users that are people; a service account is a user Keycloak owns."""
    return {u["username"]: u for u in realm.get("users", []) if "serviceAccountClientId" not in u}


# --- move 1: a thousand passwords against one account ----------------------------------------


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_a_thousand_guesses_against_one_account_are_stopped_after_ten(rendered, env):
    """TR-03187: every realm detects brute force, and the attacker waits longer each time.

    `failureFactor` is where the thousand guesses die: attempt eleven is answered with a
    temporary lockout, and `waitIncrementSeconds` makes each further burst cost more. The
    lockout is temporary on purpose — `permanentLockout` would hand an attacker a denial of
    service against any account whose name they know.
    """
    for name, realm in realms(rendered, env).items():
        where = f"{env}/{name}"
        assert realm["bruteForceProtected"] is True, where
        assert 0 < realm["failureFactor"] <= MAX_GUESSES_PER_ACCOUNT, where
        assert realm["permanentLockout"] is False, f"{where}: a permanent lockout is a DoS"
        assert realm["waitIncrementSeconds"] > 0, f"{where}: every burst must cost more"
        assert realm["maxFailureWaitSeconds"] >= 900, where
        # A burst fired faster than a person can type is counted as one quick-login failure
        # however many requests it was, which is what makes a scripted thousand cheap to catch.
        assert realm["quickLoginCheckMilliSeconds"] > 0, where
        assert realm["minimumQuickLoginWaitSeconds"] > 0, where


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_a_guessed_password_is_long_enough_for_the_guessing_to_be_hopeless(rendered, env):
    """TR-03187 AUT-7: the platform realm requires twelve characters of three classes, and the
    master realm — where the administrators are — twenty of four. The platform realm also refuses
    the username, the e-mail and the last five passwords (T-5120)."""
    platform_policy = ("length(12) and upperCase(1) and lowerCase(1) and digits(1) and notUsername(undefined)"
                       " and notEmail(undefined) and passwordHistory(5)")
    master_policy = "length(20) and upperCase(1) and lowerCase(1) and digits(1) and specialChars(1)"
    found = realms(rendered, env)
    for name, realm in found.items():
        expected = master_policy if name == "master" else platform_policy
        assert realm["passwordPolicy"] == expected, f"{env}/{name}"
    assert "master" in found, f"{env} renders no master realm to harden"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_nobody_may_sign_themselves_up_and_nobody_is_remembered(rendered, env):
    """Two doors the attack would otherwise walk through: self-registration hands the attacker
    an account in the realm, and `rememberMe` turns one stolen browser into a standing session.
    """
    realm = platform_realm(rendered, env)
    assert realm["registrationAllowed"] is False, env
    assert realm["rememberMe"] is False, env
    assert realm["sslRequired"] in ("external", "all"), f"{env}: a password over http"
    # The master realm is configured by patch, so it carries only the fields this repository
    # overrides; `rememberMe` is the one of the three that matters most there and is one of them.
    assert realms(rendered, env)["master"]["rememberMe"] is False, f"{env}/master"


# --- move 2: one password against a thousand account names -----------------------------------


def test_one_password_against_a_thousand_names_is_bounded_at_the_edge(rendered):
    """Keycloak counts failures per account, so spraying — one password, many names — never
    reaches a `failureFactor`. What bounds it is the edge: the login pages and the token
    endpoint are rate limited per source address, because there is no identity yet to key on.
    """
    configs = {}
    for doc in rendered("production"):
        if doc.get("kind") == "ConfigMap" and "apisix.yaml" in doc.get("data", {}):
            parsed = next(
                d for d in __import__("yaml").safe_load_all(doc["data"]["apisix.yaml"]) if isinstance(d, dict)
            )
            for config in parsed.get("plugin_configs", []):
                configs[config["id"]] = config["plugins"]
    assert configs, "no APISIX plugin configs in the production render"

    limit = configs["keycloak"]["limit-count"]
    assert limit["key_type"] == "var" and limit["key"] == "remote_addr", (
        "a spray has no bearer token and no session: the only thing left to key on is the source"
    )
    assert limit["rejected_code"] == 429
    assert limit["time_window"] == 60
    assert limit["count"] <= 1200, (
        "the per-minute ceiling on one source address; raising it widens the spray"
    )


# --- move 3: the demo people and the bootstrap administrator ---------------------------------


def test_production_renders_no_person_and_no_credential_of_its_own(rendered):
    """The demo accounts of `DEMO.md` are the first thing an attacker tries. A production
    render contains no person at all, so there is nothing to try."""
    assert people(platform_realm(rendered, "production")) == {}
    assert not [
        d for d in rendered("production")
        if d.get("kind") == "Secret" and d["metadata"]["name"].startswith("keycloak-user-")
    ], "a production render carries a seeded person's password"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_no_realm_carries_a_password_anybody_could_read(rendered, env):
    """A credential in the rendered realm is a credential in Git, in the Helm release and in
    every `helmfile template` anyone runs. The demo passwords are per-cluster Secrets
    substituted by keycloak-config-cli at import time, so the render only ever holds their
    placeholder."""
    for name, realm in realms(rendered, env).items():
        for username, user in people(realm).items():
            for credential in user.get("credentials", []):
                # A TOTP seed (T-5120) is a credential as well: its secretData is a placeholder too.
                secret = credential.get("value") or json.loads(credential["secretData"])["value"]
                assert secret.startswith(("$(USER_PASSWORD_", "$(USER_TOTP_")), (
                    f"{env}/{name}/{username} carries a literal credential"
                )


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_administrator_password_is_generated_per_cluster_and_never_written_down(rendered, env):
    """`admin` is the one account that exists before any realm is imported, and its password is
    the classic default. Here it is a generated 32-character Secret reached by `secretKeyRef`,
    so there is no known credential to try and none in the manifest to read."""
    env_documents = rendered(env)
    admin_secrets = [
        d for d in env_documents
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "keycloak-admin-user"
    ]
    assert admin_secrets, f"{env} renders no keycloak-admin-user Secret"

    containers = [
        container
        for d in env_documents
        if d.get("kind") in ("Deployment", "StatefulSet")
        and "keycloak" in d["metadata"]["name"]
        for container in d["spec"]["template"]["spec"]["containers"]
    ]
    bootstrap = [
        variable
        for container in containers
        for variable in container.get("env", [])
        if variable["name"] == "KC_BOOTSTRAP_ADMIN_PASSWORD"
    ]
    assert bootstrap, f"{env}: no bootstrap administrator password is configured at all"
    for variable in bootstrap:
        assert "value" not in variable, f"{env}: the administrator password is a literal"
        assert variable["valueFrom"]["secretKeyRef"]["name"] == "keycloak-admin-user", env


# --- what the administrator's second factor actually is --------------------------------------


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_an_administrator_who_enrolled_a_second_factor_is_always_asked_for_it(rendered, env):
    """TR-03187 AUT-2: the platform realm's browser flow ends in a conditional second factor.

    Whoever enrolled TOTP or a security key is asked for it on every login; a holder of
    `mfa-required` must give TOTP whether enrolled or not (T-5120, its own subflow).
    """
    realm = platform_realm(rendered, env)
    assert realm["browserFlow"] == "custom browser"

    flows = {flow["alias"]: flow for flow in realm["authenticationFlows"]}
    forms = flows["custom browser forms"]["authenticationExecutions"]
    second_factor = next(e for e in forms if e.get("flowAlias") == "custom browser 2fa")
    assert second_factor["requirement"] == "CONDITIONAL"

    steps = flows["custom browser 2fa"]["authenticationExecutions"]
    assert [s for s in steps if s["authenticator"] == "conditional-user-configured"], (
        "without the condition the flow prompts everybody or nobody"
    )
    offered = {s["authenticator"] for s in steps if s["requirement"] == "ALTERNATIVE"}
    assert {"auth-otp-form", "webauthn-authenticator"} <= offered

    # A one-time code is only as good as its algorithm and its window (TR-03187 AR-11).
    assert realm["otpPolicyAlgorithm"] == "HmacSHA256"
    assert realm["otpPolicyDigits"] >= 6
    assert realm["otpPolicyLookAheadWindow"] <= 1, "a wide window is a wider guess"
