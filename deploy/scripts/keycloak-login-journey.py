#!/usr/bin/env python3
"""Browser journeys of the realm's login hardening (T-5120, components/keycloak/README.md).

  steward  a holder of `mfa-required` signs in with password and TOTP code
  plain    a person without it signs in with the password alone
  enrol    a throwaway holder of `mfa-required` without TOTP is sent to enrolment
  lockout  a throwaway person is locked after 10 wrong passwords, the right one is refused
  grant    a throwaway holder of `mfa-required` without TOTP gets no token by password grant
  stepup   a throwaway person without `mfa-required` (a steward by RoleBinding alone) signs in
           with the password: the token proves no second factor; the step-up login
           (acr_values=mfa, as the Portal's decision doors ask, T-5567) enrols TOTP and the
           token then carries acr `mfa`

Secrets come from the environment, never argv: JC_LOGIN_PASSWORD and JC_LOGIN_TOTP (the seed
in Secret keycloak-user-<name>, key `totp`) for steward/plain; KC_ADMIN_USER and
KC_ADMIN_PASSWORD (Secret keycloak-admin-user) for the throwaway checks, which delete their
user again whatever the outcome; JC_GRANT_CLIENT_ID and JC_GRANT_CLIENT_SECRET (a client with
direct access grants, e.g. apisix-gateway) for grant.

  scripts/keycloak-login-journey.py --base https://idp.example --realm dev steward --user demo.steward@hel.fi
"""

import argparse
import base64
import hashlib
import hmac
import json
import os
import secrets
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

FAILURE_FACTOR = 10  # components/keycloak/values/config/base-values.yaml.gotmpl failureFactor


def totp(seed: str | bytes, at: float, step: int = 30, digits: int = 6) -> str:
    """RFC 6238 with HMAC-SHA256 over the seed's UTF-8 bytes, as Keycloak keeps an OTP secret."""
    key = seed.encode() if isinstance(seed, str) else seed
    mac = hmac.new(key, struct.pack(">Q", int(at) // step), hashlib.sha256).digest()
    offset = mac[-1] & 0x0F
    code = struct.unpack(">I", mac[offset : offset + 4])[0] & 0x7FFFFFFF
    return str(code % 10**digits).zfill(digits)


def env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        sys.exit(f"keycloak-login-journey: {name} is not set")
    return value


class Admin:
    """The realm's admin REST API, for the throwaway users only."""

    def __init__(self, base: str, realm: str):
        self.base, self.realm = base, realm
        form = urllib.parse.urlencode(
            {"grant_type": "password", "client_id": "admin-cli",
             "username": env("KC_ADMIN_USER"), "password": env("KC_ADMIN_PASSWORD")}
        ).encode()
        with urllib.request.urlopen(f"{base}/realms/master/protocol/openid-connect/token", form, timeout=30) as r:
            self.token = json.load(r)["access_token"]

    def call(self, method: str, path: str, body=None):
        req = urllib.request.Request(
            f"{self.base}/admin/realms/{self.realm}{path}", method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read()
            return json.loads(raw) if raw else None

    def throwaway(self, role: str | None) -> tuple[str, str, str]:
        email = f"journey-{secrets.token_hex(4)}@example.invalid"
        password = "Jr1-" + secrets.token_urlsafe(18)
        self.call("POST", "/users", {
            "username": email, "email": email, "enabled": True, "emailVerified": True,
            "firstName": "Journey", "lastName": "Throwaway", "requiredActions": [],
            "credentials": [{"type": "password", "value": password, "temporary": False}],
        })
        uid = self.call("GET", f"/users?username={urllib.parse.quote(email)}&exact=true")[0]["id"]
        if role:
            self.call("POST", f"/users/{uid}/role-mappings/realm", [self.call("GET", f"/roles/{role}")])
        return uid, email, password


def open_login(page, base: str, realm: str) -> None:
    page.goto(f"{base}/realms/{realm}/account", wait_until="load")
    page.wait_for_selector("#username", timeout=30_000)


def submit_password(page, user: str, password: str) -> None:
    page.fill("#username", user)
    page.fill("#password", password)
    with page.expect_navigation(wait_until="load"):
        page.click("#kc-login")


def landed(page, base: str, realm: str) -> bool:
    return page.url.startswith(f"{base}/realms/{realm}/account") and not page.locator("#kc-form-login, #kc-otp-login-form").count()


def password_grant(base: str, realm: str, username: str, password: str) -> dict:
    form = {"grant_type": "password", "client_id": env("JC_GRANT_CLIENT_ID"),
            "client_secret": env("JC_GRANT_CLIENT_SECRET"), "username": username, "password": password}
    req = urllib.request.Request(f"{base}/realms/{realm}/protocol/openid-connect/token",
                                 data=urllib.parse.urlencode(form).encode())
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as err:
        return json.load(err)


ACCOUNT = "account-console"  # Keycloak's own public PKCE client of every realm


def code_login(page, base: str, realm: str, user: str, password: str, step_up: bool) -> tuple[dict, bool]:
    """A code flow of the account console, as the Portal starts one, and the ID token's claims.
    `step_up` adds what the Portal's `?step_up=true` adds. Enrols TOTP when Keycloak asks; the
    second value says whether it did."""
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    redirect = f"{base}/realms/{realm}/account/"
    query = {"client_id": ACCOUNT, "redirect_uri": redirect, "response_type": "code", "scope": "openid",
             "code_challenge": challenge, "code_challenge_method": "S256", "state": secrets.token_hex(8)}
    if step_up:
        query |= {"acr_values": "mfa", "prompt": "login"}
    page.goto(f"{base}/realms/{realm}/protocol/openid-connect/auth?{urllib.parse.urlencode(query)}", wait_until="load")
    page.wait_for_selector("#username", timeout=30_000)
    codes: list[str] = []
    page.on("request", lambda req: codes.extend(urllib.parse.parse_qs(urllib.parse.urlparse(req.url).query).get("code", []))
            if req.url.startswith(redirect) else None)
    submit_password(page, user, password)
    enrolled = False
    if page.locator("#kc-totp-settings-form").count():
        enrolled = True
        page.click("#mode-manual")
        key = page.locator("#kc-totp-secret-key").inner_text().replace(" ", "")
        secret = base64.b32decode(key + "=" * (-len(key) % 8))
        page.fill("#totp", totp(secret, time.time()))
        page.fill("#userLabel", "journey")
        with page.expect_navigation(wait_until="load"):
            page.click("#saveTOTPBtn")
    if not codes:
        return {"error": f"no code came back: {page.url}"}, enrolled
    form = urllib.parse.urlencode({"grant_type": "authorization_code", "client_id": ACCOUNT, "code": codes[0],
                                   "redirect_uri": redirect, "code_verifier": verifier}).encode()
    with urllib.request.urlopen(f"{base}/realms/{realm}/protocol/openid-connect/token", form, timeout=30) as r:
        id_token = json.load(r)["id_token"]
    payload = id_token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4))), enrolled


def proves(claims: dict) -> bool:
    """The Portal's rule (src/auth/session.rs proves_second_factor)."""
    amr = claims.get("amr") or []
    return claims.get("acr") == "mfa" or "otp" in ([amr] if isinstance(amr, str) else amr)


def run(args) -> list[str]:
    failures: list[str] = []
    if args.check == "grant":
        admin = Admin(args.admin_base or args.base, args.realm)
        uid, email, password = admin.throwaway("mfa-required")
        try:
            answer = password_grant(args.base, args.realm, email, password)
            if "access_token" in answer:
                failures.append("a holder of mfa-required without TOTP got a token by password alone")
        finally:
            admin.call("DELETE", f"/users/{uid}")
        return failures

    from playwright.sync_api import sync_playwright

    if args.check == "stepup":
        admin = Admin(args.admin_base or args.base, args.realm)
        uid, email, password = admin.throwaway(None)
        try:
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                plain, asked = code_login(browser.new_context(locale="en").new_page(),
                                          args.base, args.realm, email, password, step_up=False)
                if "error" in plain or asked or proves(plain):
                    failures.append(f"a password login proved a second factor or asked one: {plain}")
                page = browser.new_context(locale="en").new_page()
                stepped, enrolled = code_login(page, args.base, args.realm, email, password, step_up=True)
                if args.screenshot:
                    page.screenshot(path=args.screenshot, full_page=True)
                if not enrolled:
                    failures.append("the step-up login did not enrol a person without TOTP")
                # The account console has no amr mapper (the platform's clients do): acr carries it.
                if stepped.get("acr") != "mfa" or not proves(stepped):
                    failures.append(f"the step-up token says acr {stepped.get('acr')!r}, amr {stepped.get('amr')!r}: {stepped.get('error', '')}")
                browser.close()
        finally:
            admin.call("DELETE", f"/users/{uid}")
        return failures

    with sync_playwright() as pw:
        browser = pw.chromium.launch()
        page = browser.new_context(locale="en").new_page()
        if args.check in ("steward", "plain"):
            open_login(page, args.base, args.realm)
            submit_password(page, args.user, env("JC_LOGIN_PASSWORD"))
            asked = page.locator("#otp").count() > 0
            if args.check == "plain":
                if asked:
                    failures.append(f"{args.user} was asked for a TOTP code it never configured")
            elif not asked:
                failures.append(f"{args.user} signed in without a second factor")
            else:
                page.fill("#otp", totp(env("JC_LOGIN_TOTP"), time.time()))
                with page.expect_navigation(wait_until="load"):
                    page.click("#kc-login")
            if args.screenshot:
                page.screenshot(path=args.screenshot, full_page=True)
            if not failures and not landed(page, args.base, args.realm):
                failures.append(f"{args.user} did not reach the account console: {page.url}")
        else:
            admin = Admin(args.admin_base or args.base, args.realm)
            uid, email, password = admin.throwaway("mfa-required" if args.check == "enrol" else None)
            try:
                open_login(page, args.base, args.realm)
                if args.check == "enrol":
                    submit_password(page, email, password)
                    if args.screenshot:
                        page.screenshot(path=args.screenshot, full_page=True)
                    # Keycloak's CONFIGURE_TOTP page: the QR code and the code field to confirm it.
                    if not page.locator("#kc-totp-settings-form").count():
                        failures.append(f"a holder of mfa-required without TOTP was not sent to enrolment: {page.url}")
                else:
                    for _ in range(FAILURE_FACTOR):
                        # Slower than quickLoginCheckMilliSeconds, so the counter and not the
                        # quick-login rule is what locks.
                        time.sleep(1.2)
                        submit_password(page, email, password + "x")
                    status = admin.call("GET", f"/attack-detection/brute-force/users/{uid}")
                    if not status.get("disabled"):
                        failures.append(f"{FAILURE_FACTOR} wrong passwords did not lock the account: {status}")
                    submit_password(page, email, password)
                    if args.screenshot:
                        page.screenshot(path=args.screenshot, full_page=True)
                    if landed(page, args.base, args.realm):
                        failures.append("the locked account signed in with its right password")
            finally:
                admin.call("DELETE", f"/users/{uid}")
        browser.close()
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", required=True, help="Keycloak base URL, e.g. https://idp.dev.example")
    parser.add_argument("--admin-base", help="admin API base when the edge hides /admin (a port-forward)")
    parser.add_argument("--realm", required=True)
    parser.add_argument("--user", help="the person for steward/plain")
    parser.add_argument("--screenshot", help="PNG of the last page")
    parser.add_argument("check", choices=["steward", "plain", "enrol", "lockout", "grant", "stepup"])
    args = parser.parse_args()
    args.base = args.base.rstrip("/")
    if args.check in ("steward", "plain") and not args.user:
        parser.error("--user is required for steward and plain")
    failures = run(args)
    for failure in failures:
        print(f"KO {args.check}: {failure}")
    if not failures:
        print(f"ok {args.check}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
