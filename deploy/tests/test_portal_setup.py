"""T-2748 (PF-90, UI-82, API/01 §25): the Portal's setup page is told what the deployment provides.

The Portal cannot see Keycloak's login theme or SMTP server or the database's backups, so the
deployment states them in `JC_SETUP_*` from the same values that configure those parts. A
statement the values do not back would tick a setup item that is not done.
"""

import pytest


def setup_env(docs):
    (portal,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"]
    (container,) = [c for c in portal["spec"]["template"]["spec"]["containers"] if c["name"] == "portal-portal"]
    return {e["name"]: e.get("value") for e in container.get("env", []) if e["name"].startswith("JC_SETUP_")}


def test_dev_says_its_login_theme_and_that_it_has_no_mail_and_backups(rendered):
    # Dev backs its Postgres up to the artifact store since T-5289.
    assert setup_env(rendered("dev")) == {
        "JC_SETUP_LOGIN_THEME": "joinedcontext",
        "JC_SETUP_SMTP": "false",
        "JC_SETUP_BACKUPS": "true",
    }


@pytest.mark.parametrize("env", ["production", "dev"])
def test_an_environment_with_backups_says_so(rendered, env):
    assert setup_env(rendered(env))["JC_SETUP_BACKUPS"] == "true"
