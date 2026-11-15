# Point ckanext-oidc-pkce at this cluster's realm, after the entrypoint has written the ini.
#
# The extension reads its base URL, client id and client secret straight from the environment
# (`os.environ.get` in its `config.py`), but every path it appends to them — authorize, token,
# userinfo, logout — it reads through `tk.config`, and its defaults are Okta's paths. Against
# Keycloak a login would ask for `/oauth2/default/v1/authorize` and get a 404.
#
# Its callback also sends the person on to `ckan.route_after_login`, the pre-2.10 name, whose
# fallback `dashboard.index` resolves to `activity.dashboard`: without the activity plugin that
# is a 500 on /user/login/oidc-pkce/callback after a sign-in that worked (T-2888). The ini's
# `ckan.auth.route_after_login` is the 2.11 name, which the extension never reads.
#
# So they are written into $CKAN_INI here, under the names the extension looks up, rather than
# left to the `envvars` plugin's `CKAN___` spelling: one option, one name, in the file the rest
# of this chart's configuration already lives in.
#
# Sourced by `start_ckan.sh`, not executed — so no `exit` on the path where there is nothing to
# do, or the entrypoint would stop before uWSGI (see 00-secret-key.sh for the order).
if [ -n "${JC_OIDC_REALM_PATH:-}" ]; then
  if ckan config-tool "$CKAN_INI" \
      "ckanext.oidc_pkce.auth_path=${JC_OIDC_REALM_PATH}/protocol/openid-connect/auth" \
      "ckanext.oidc_pkce.token_path=${JC_OIDC_REALM_PATH}/protocol/openid-connect/token" \
      "ckanext.oidc_pkce.userinfo_path=${JC_OIDC_REALM_PATH}/protocol/openid-connect/userinfo" \
      "ckanext.oidc_pkce.logout_path=${JC_OIDC_REALM_PATH}/protocol/openid-connect/logout" \
      "ckanext.oidc_pkce.scope=${JC_OIDC_SCOPE:-openid email profile}" \
      "ckanext.oidc_pkce.munge_password=false" \
      "ckan.route_after_login=dashboard.datasets"; then
    echo "[jc] Keycloak login configured against ${JC_OIDC_REALM_PATH}"
  else
    echo "[jc] could not write the OIDC endpoints into $CKAN_INI" >&2
    exit 1
  fi
fi
