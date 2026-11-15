"""Which deployment lets a decision through without a one-time code (T-6083, AUT-2, PF-128).

The owner turned the Portal's second-factor gate off on dev only: dev's demo approvers sign in
with a password alone (T-5776). smartcity reuses dev's values profile and serves public users, so
the key must stay out of it and out of the generic production profile, where the Portal's
default (on) holds.
"""

import pytest
from test_portal_oidc_trust import portal_env, portal_pod_spec

KEY = "JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR"


def test_dev_decides_without_a_second_factor(rendered):
    assert portal_env(portal_pod_spec(rendered("dev"))).get(KEY) == "false"


@pytest.mark.parametrize("env", ["smartcity", "production"])
def test_prod_and_production_keep_the_second_factor(rendered, env):
    assert KEY not in portal_env(portal_pod_spec(rendered(env)))
