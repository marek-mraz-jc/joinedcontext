"""Keeps a CKAN API token of the site administrator in one Secret (T-0493), and, when the
catalogue harvests (HARVEST_ORGS), a token of the non-administrator harvest user in another
(F-DEP1-002): that user edits the harvested organizations and nothing else, so a harvest pod
parsing outside data never holds the site administrator's token.

Runs as `python3 -c` in the catalogue's image. Prints what it did and never a token, a password
or a response body that could carry either.
"""
import base64
import http.client
import json
import os
import re
import secrets
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

CATALOGUE = os.environ["CATALOGUE"]
USER = os.environ["ADMIN_USER"]
PASSWORD = os.environ["ADMIN_PASSWORD"]
SECRET = os.environ["TOKEN_SECRET"]
NAME = os.environ["TOKEN_NAME"]
NAMESPACE = os.environ["NAMESPACE"]
KUBE = os.environ.get("KUBE_API", "https://kubernetes.default.svc")
SA = os.environ.get("SA_DIR", "/var/run/secrets/kubernetes.io/serviceaccount")
# The organizations the harvests write, created here by the administrator: [{name, title, description}].
HARVEST_ORGS = json.loads(os.environ.get("HARVEST_ORGS") or "[]")
HARVEST_USER = os.environ.get("HARVEST_USER", "harvest")
HARVEST_SECRET = os.environ.get("HARVEST_SECRET", "")
TOKEN_SHAPE = re.compile(r"[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")

cookies = {}


def ckan(method, path, body=None, headers=None):
    """One call on the catalogue's Service, carrying the session like a browser would. The
    session cookie is `Secure` (the site URL is https), which a cookie jar will not send over
    the plain in-cluster hop, so it is kept by hand."""
    host, _, port = CATALOGUE.partition(":")
    headers = dict(headers or {})
    if cookies:
        headers["Cookie"] = "; ".join(f"{k}={v}" for k, v in cookies.items())
    conn = http.client.HTTPConnection(host, int(port or 5000), timeout=30)
    try:
        conn.request(method, path, body=body, headers=headers)
        response = conn.getresponse()
        data = response.read().decode("utf-8", "replace")
        for key, value in response.getheaders():
            if key.lower() == "set-cookie":
                name, _, rest = value.partition("=")
                cookies[name.strip()] = rest.split(";", 1)[0]
        return response.status, data, response.getheader("Location") or ""
    finally:
        conn.close()


def action(name, payload, token=None, csrf=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = token
    if csrf:
        headers["X-CSRFToken"] = csrf
    status, data, _ = ckan("POST", f"/api/3/action/{name}", json.dumps(payload), headers)
    result = json.loads(data).get("result") if status == 200 else None
    return status, result


def kube(method, path, body=None):
    request = urllib.request.Request(
        KUBE + path,
        data=json.dumps(body).encode() if body is not None else None,
        method=method,
        headers={"Content-Type": "application/json"},
    )
    token_file = os.path.join(SA, "token")
    if os.path.exists(token_file):
        with open(token_file) as handle:
            request.add_header("Authorization", f"Bearer {handle.read().strip()}")
    ca = os.path.join(SA, "ca.crt")
    context = ssl.create_default_context(cafile=ca) if KUBE.startswith("https") else None
    try:
        with urllib.request.urlopen(request, timeout=30, context=context) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as err:
        return err.code, b""


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def authenticates(token, user=USER):
    """A token CKAN no longer knows is read as anonymous, and listing `user`'s tokens is
    refused to anonymous: 200 here means the token is `user`'s (or an administrator's)."""
    saved = dict(cookies)
    cookies.clear()
    try:
        return action("api_token_list", {"user_id": user}, token=token)[0] == 200
    finally:
        cookies.update(saved)


def stored_token(name):
    """The token Secret `name` holds, or "" when there is none."""
    status, raw = kube("GET", f"/api/v1/namespaces/{NAMESPACE}/secrets/{name}")
    if status == 404:
        return ""
    if status != 200:
        fail(f"reading Secret {name} answered {status}")
    encoded = (json.loads(raw).get("data") or {}).get("token", "")
    return base64.b64decode(encoded).decode() if encoded else ""


def store_token(name, token):
    """Secret `name` holds `token`: replaced if it is there, created if it is not."""
    body = {
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {"name": name, "namespace": NAMESPACE},
        "type": "Opaque",
        "data": {"token": base64.b64encode(token.encode()).decode()},
    }
    status, _ = kube("PUT", f"/api/v1/namespaces/{NAMESPACE}/secrets/{name}", body)
    if status == 404:
        status, _ = kube("POST", f"/api/v1/namespaces/{NAMESPACE}/secrets", body)
    if status not in (200, 201):
        fail(f"writing Secret {name} answered {status}")


def harvest_token(admin):
    """The harvest user, an editor of each harvested organization and nothing more, and its
    token in HARVEST_SECRET. Every call is the administrator's token's, so no session."""
    for org in HARVEST_ORGS:
        status, _ = action("organization_show", {"id": org["name"]}, token=admin)
        if status == 404:
            status, _ = action("organization_create", {"name": org["name"], "title": org["title"],
                                                       "description": org.get("description", "")},
                               token=admin)
            if status != 200:
                fail(f"creating organization {org['name']} answered {status}")
            print(f"created organization {org['name']}")
        elif status != 200:
            fail(f"reading organization {org['name']} answered {status}")
    status, user = action("user_show", {"id": HARVEST_USER}, token=admin)
    if status == 404:
        # Nobody signs in as it: the password is random and never kept.
        status, user = action("user_create", {"name": HARVEST_USER, "email": f"{HARVEST_USER}@harvest.invalid",
                                              "password": secrets.token_urlsafe(32)}, token=admin)
        if status != 200:
            fail(f"creating the harvest user answered {status}")
        print(f"created the harvest user {HARVEST_USER}")
    elif status != 200:
        fail(f"reading the harvest user answered {status}")
    if (user or {}).get("sysadmin"):
        fail(f"the harvest user {HARVEST_USER} is a site administrator; its token would be one too")
    for org in HARVEST_ORGS:
        status, _ = action("organization_member_create",
                           {"id": org["name"], "username": HARVEST_USER, "role": "editor"}, token=admin)
        if status != 200:
            fail(f"making {HARVEST_USER} an editor of {org['name']} answered {status}")
    existing = stored_token(HARVEST_SECRET)
    if existing and authenticates(existing, HARVEST_USER):
        print(f"the token in {HARVEST_SECRET} still authenticates")
        return
    status, tokens = action("api_token_list", {"user_id": HARVEST_USER}, token=admin)
    if status != 200:
        fail(f"listing the harvest user's tokens answered {status}")
    for token in tokens or []:
        if token.get("name") == HARVEST_USER:
            status, _ = action("api_token_revoke", {"jti": token["id"]}, token=admin)
            if status != 200:
                fail(f"revoking the previous {HARVEST_USER} token answered {status}")
            print(f"revoked the previous {HARVEST_USER} token")
    status, result = action("api_token_create", {"user": HARVEST_USER, "name": HARVEST_USER}, token=admin)
    token = (result or {}).get("token", "") if isinstance(result, dict) else ""
    if status != 200 or not TOKEN_SHAPE.fullmatch(token):
        fail(f"minting the {HARVEST_USER} token answered {status} without a token")
    store_token(HARVEST_SECRET, token)
    print(f"minted {HARVEST_USER} into Secret {HARVEST_SECRET}")


def main():
    # 1. The catalogue answers. The Deployment being ready is not CKAN having finished its
    #    own start, and the release applies the Job right behind it.
    for attempt in range(1, 61):
        try:
            if ckan("GET", "/api/3/action/status_show")[0] == 200:
                break
        except OSError:
            pass
        print(f"waiting for the catalogue ({attempt}/60)", flush=True)
        time.sleep(5)
    else:
        fail("the catalogue did not answer status_show after five minutes")

    # 2. The record. A token that still authenticates is kept: re-minting on every apply would
    #    churn a credential a publisher may be holding.
    admin = stored_token(SECRET)
    if admin and authenticates(admin):
        print(f"the token in {SECRET} still authenticates")
    else:
        admin = mint_admin()
    if HARVEST_ORGS and HARVEST_SECRET:
        harvest_token(admin)


def mint_admin():
    """A new administrator token in SECRET, minted on a signed-in session."""
    # 3. The administrator signs in on the form, and the page after it carries the CSRF token
    #    an API call on a session has to send back.
    status, page, _ = ckan("GET", "/user/login")
    field = re.search(r'name="_csrf_token"[^>]*value="([^"]+)"', page)
    if status != 200 or not field:
        fail(f"the login page answered {status} without a CSRF field")
    form = urllib.parse.urlencode({"login": USER, "password": PASSWORD, "_csrf_token": field.group(1)})
    status, _, location = ckan("POST", "/user/login", form, {"Content-Type": "application/x-www-form-urlencoded"})
    if status != 302 or urllib.parse.urlsplit(location).path.startswith("/user/login"):
        fail(f"the administrator could not sign in ({status})")
    status, page, _ = ckan("GET", "/dataset/")
    meta = re.search(r'<meta name="_csrf_token" content="([^"]+)"', page)
    if status != 200 or not meta:
        fail(f"the signed-in page answered {status} without a CSRF token")
    csrf = meta.group(1)

    # 4. One token of this name: the old one is revoked before the new one is minted, so a
    #    token that left with a lost Secret stops working.
    status, tokens = action("api_token_list", {"user_id": USER}, csrf=csrf)
    if status != 200:
        fail(f"listing the administrator's tokens answered {status}")
    for token in tokens or []:
        if token.get("name") == NAME:
            status, _ = action("api_token_revoke", {"jti": token["id"]}, csrf=csrf)
            if status != 200:
                fail(f"revoking the previous {NAME} token answered {status}")
            print(f"revoked the previous {NAME} token")
    status, result = action("api_token_create", {"user": USER, "name": NAME}, csrf=csrf)
    token = (result or {}).get("token", "") if isinstance(result, dict) else ""
    if status != 200 or not TOKEN_SHAPE.fullmatch(token):
        fail(f"minting {NAME} answered {status} without a token")
    if not authenticates(token):
        fail(f"the {NAME} token CKAN just minted does not authenticate")

    # 5. The Secret: replaced if it is there, created if it is not.
    store_token(SECRET, token)
    print(f"minted {NAME} into Secret {SECRET}")
    return token

main()
