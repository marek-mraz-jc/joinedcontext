# Pin CKAN's SECRET_KEY to the one the cluster generated, after the entrypoint minted its own.
#
# `start_ckan.sh` fills an empty SECRET_KEY with `secrets.token_urlsafe()` and derives the CSRF
# and API-token secrets from it. The test it guards that branch with reads the RELATIVE
# `ckan.ini` — the pristine one in the image, whose SECRET_KEY is always empty — so the branch
# fires on every start no matter what $CKAN_INI already holds. $CKAN_INI is an emptyDir, new
# with every pod, so a restart invalidates every session cookie and every API token CKAN has
# issued, and logs nothing about it.
#
# The env var cannot do this. CKAN 2.11 has no `CKAN___<OPTION>` override: `CKANConfigLoader`
# puts every `CKAN_*` variable into the ConfigParser defaults under its own literal name, and a
# value in `[app:main]` shadows a default, so an ini line the entrypoint just wrote always wins.
# `/docker-entrypoint.d/*.sh` is sourced after that branch and before uWSGI, which is the one
# place a stable value can be put in without patching the image.
#
# One value, four options: CKAN derives WTF_CSRF_SECRET_KEY and both JWT secrets from SECRET_KEY
# when they are unset, and `beaker.session.secret` is SECRET_KEY's own legacy name. Writing them
# explicitly says the same thing, and says it over the entrypoint's values.
if [ -z "${JC_CKAN_SECRET_KEY:-}" ]; then
  echo "[jc] JC_CKAN_SECRET_KEY is empty: refusing to serve with a key that dies with the pod" >&2
  exit 1
fi

if ckan config-tool "$CKAN_INI" \
    "SECRET_KEY=$JC_CKAN_SECRET_KEY" \
    "WTF_CSRF_SECRET_KEY=$JC_CKAN_SECRET_KEY" \
    "api_token.jwt.encode.secret=string:$JC_CKAN_SECRET_KEY" \
    "api_token.jwt.decode.secret=string:$JC_CKAN_SECRET_KEY"; then
  echo "[jc] secret key pinned; sessions and API tokens survive this pod"
else
  echo "[jc] could not pin the secret key in $CKAN_INI" >&2
  exit 1
fi
