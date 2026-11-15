"""The second factor for the smoke suites' and the rehearsal kit's own people (T-6894): with kaisatsu's
`[kaisatsu] second_factor = "required"` the password alone answers a second-factor page, not a
redirect with code=. `SECOND_FACTOR.answer` (an HTTP client's form post) and `SECOND_FACTOR.browser`
(a Playwright page) answer it the way a person with an authenticator app does:

- the set-up page (form kc-totp-settings-form): the seed shown under "Unable to scan?"
  (#kc-totp-secret-key) and the algorithm, digits and period listed beside it; the seed is kept;
- the one-time-code page (form kc-otp-login-form): a code from the seed kept for that person.

Codes are RFC 6238. A code is single use per period, so a second sign-in of the same person waits
for the next step. Seeds stay in this process's memory: never logged, written or put in a row.
Python 3 stdlib only."""

from __future__ import annotations

import base64
import hmac
import html
import re
import struct
import threading
import time
import urllib.parse

FORMS = ("kc-totp-settings-form", "kc-otp-login-form")
ALGORITHMS = {"SHA1": "sha1", "SHA256": "sha256", "SHA512": "sha512"}


def totp(seed: str, at: float, algorithm: str = "SHA1", digits: int = 6, period: int = 30) -> str:
    """The RFC 6238 code of a base32 `seed` at unix time `at`."""
    key = base64.b32decode(seed.replace(" ", "").upper() + "=" * (-len(seed.replace(" ", "")) % 8))
    mac = hmac.new(key, struct.pack(">Q", int(at // period)), ALGORITHMS[algorithm]).digest()
    off = mac[-1] & 0x0F
    return str((struct.unpack(">I", mac[off:off + 4])[0] & 0x7FFFFFFF) % 10 ** digits).zfill(digits)


def _attr(tag: str, name: str) -> str | None:
    m = re.search(rf'\b{name}="([^"]*)"', tag)
    return html.unescape(m.group(1)) if m else None


def parse(page: str) -> dict | None:
    """The second-factor form on `page`: {"form", "action", "hidden", "seed"?, "algorithm", "digits",
    "period"}, or None when the page holds neither form."""
    form = next((m for m in re.finditer(r"<form\b[^>]*>", page) if _attr(m.group(0), "id") in FORMS), None)
    if not form:
        return None
    end = page.find("</form>", form.end())
    inner = page[form.end():end if end >= 0 else len(page)]
    hidden = {}
    for tag in re.findall(r"<input\b[^>]*>", inner):
        if (_attr(tag, "type") or "").lower() == "hidden" and _attr(tag, "name"):
            hidden[_attr(tag, "name")] = _attr(tag, "value") or ""
    out = {"form": _attr(form.group(0), "id"), "action": _attr(form.group(0), "action") or "", "hidden": hidden,
           "algorithm": "SHA1", "digits": 6, "period": 30}
    if out["form"] == FORMS[0]:
        seed = re.search(r'id="kc-totp-secret-key"[^>]*>([^<]*)<', page)
        if not seed:
            raise RuntimeError("the authenticator set-up page shows no key (#kc-totp-secret-key)")
        out["seed"] = re.sub(r"\s+", "", html.unescape(seed.group(1)))
        # The listed policy: "Type: ...", "Algorithm: SHA1", "Digits: 6", "Interval: 30" (any language).
        manual = page[seed.end():]
        lines = [html.unescape(x).strip() for x in re.findall(r"<li\b[^>]*>([^<]*)</li>", manual[:manual.find("</ul>")])]
        alg = next((m.group(1) for x in lines if (m := re.search(r"\b(SHA1|SHA256|SHA512)\b", x.upper()))), None)
        nums = [int(m.group(1)) for x in lines if (m := re.search(r":\s*(\d+)\s*$", x))]
        out["algorithm"] = alg or "SHA1"
        if len(nums) >= 2:
            out["digits"], out["period"] = nums[0], nums[1]
    return out


class SecondFactor:
    """Seeds of the people this process enrolled, and the last step each one answered."""

    def __init__(self, clock=time.time, sleep=time.sleep):
        self.clock, self.sleep = clock, sleep
        self._seeds: dict[str, tuple[str, str, int, int]] = {}
        self._last: dict[str, int] = {}
        self._lock = threading.Lock()

    def __repr__(self) -> str:
        return f"SecondFactor({len(self._seeds)} people)"

    def code(self, username: str, form: dict) -> str:
        """The code for `form` (parsed); a set-up form's seed is kept for `username`'s later sign-ins."""
        with self._lock:  # suites sign in from threads: each step is handed out once per person
            if "seed" in form:
                self._seeds[username] = (form["seed"], form["algorithm"], form["digits"], form["period"])
            if username not in self._seeds:
                raise RuntimeError(f"{username} has an authenticator this run did not set up: "
                                   "remove its otp credential (realm admin) and sign in again")
            seed, alg, digits, period = self._seeds[username]
            step = max(int(self.clock() // period), self._last.get(username, -1) + 1)
            self._last[username] = step
        wait = step * period - self.clock()
        if wait > 0:  # this step's code is spent: the next one
            self.sleep(wait + 0.5)
        return totp(seed, step * period, alg, digits, period)

    def answer(self, call, url: str, st: int, headers: dict, body: bytes, username: str):
        """After the password's POST to `url`: when its answer (st, headers, body) is a second-factor
        page, posts the code with `call("POST", url, form=...)` and returns that answer; else the
        same answer unchanged. A refused code raises RuntimeError (no second try: wrong codes count
        toward the lockout)."""
        form = parse(body.decode(errors="replace")) if st == 200 else None
        if not form:
            return st, headers, body
        fields = {**form["hidden"], "otp": self.code(username, form)}
        if form["form"] == FORMS[0]:
            fields["userLabel"] = "smoke"
        st, headers, body = call("POST", urllib.parse.urljoin(url, form["action"] or url), form=fields)
        if st == 200 and parse(body.decode(errors="replace")):
            raise RuntimeError(f"the second factor of {username} was refused")
        return st, headers, body

    def browser(self, page, username: str, timeout: float = 30.0) -> None:
        """After the password's Enter in a Playwright `page`: waits until the page leaves the realm's
        pages or shows a second-factor form, and answers that form."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if "/realms/" not in urllib.parse.urlparse(page.url).path:
                return
            try:
                text = page.content()
            except Exception:  # noqa: BLE001 - the page is navigating: read it again
                text = ""
            form = parse(text) if text else None
            if form:
                if form["form"] == FORMS[0]:
                    page.fill("input[name=userLabel]", "smoke")
                page.fill("input[name=otp]", self.code(username, form))
                page.locator("input[name=otp]").press("Enter")
                return
            time.sleep(0.25)


SECOND_FACTOR = SecondFactor()
