# Mark CKAN's session and remember-me cookies `Secure` when the catalogue is served over https.
#
# The image's ckan.ini says `SESSION_COOKIE_SECURE = false` and `REMEMBER_COOKIE_SECURE = false`,
# and nothing else set them, so the `ckan` cookie of a signed-in person went out without `Secure`
# (T-3017): a browser with no HSTS entry for the host would send it over plain http. CKAN itself
# sees plain http from the edge either way; the attribute is about the browser's side of the
# edge, which is https whenever the site URL is. A plain-http site URL keeps the image's default,
# or the browser would drop the cookie and nobody could sign in.
#
# Sourced by `start_ckan.sh`, not executed: no `exit` on the path where there is nothing to do.
case "${CKAN_SITE_URL:-}" in
  https://*)
    if ckan config-tool "$CKAN_INI" \
        "SESSION_COOKIE_SECURE=true" \
        "REMEMBER_COOKIE_SECURE=true"; then
      echo "[jc] session cookies marked Secure"
    else
      echo "[jc] could not mark the session cookies Secure in $CKAN_INI" >&2
      exit 1
    fi
    ;;
esac
