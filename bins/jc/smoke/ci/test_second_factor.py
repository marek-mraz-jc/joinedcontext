"""The second-factor helper on kaisatsu's own pages (captured from `kaisatsu serve`, a person
holding mfa-required; the seed swapped for RFC 6238's test seed, the QR removed)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))  # bins/jc/smoke
import second_factor as sf

SETUP = (HERE / "fixtures" / "kaisatsu-totp-setup.html").read_text()
OTP = (HERE / "fixtures" / "kaisatsu-otp-login.html").read_text()
LOGIN_URL = "http://127.0.0.1:8080/id/realms/smoke/login-actions/authenticate?session_code=x"
RFC_SEED = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"  # "12345678901234567890"


class Clock:
    def __init__(self, now: float):
        self.now, self.slept = now, []

    def __call__(self) -> float:
        return self.now

    def sleep(self, s: float) -> None:
        self.slept.append(s)
        self.now += s


class Server:
    """Answers each POST from a list; records what was posted."""

    def __init__(self, *answers):
        self.answers, self.posts = list(answers), []

    def call(self, method, url, form=None):
        self.posts.append((method, url, form))
        return self.answers.pop(0)


def test_rfc_6238_vectors():
    assert sf.totp(RFC_SEED, 59, "SHA1", 8) == "94287082"
    assert sf.totp(RFC_SEED, 1111111109, "SHA1", 8) == "07081804"
    assert sf.totp("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA", 59, "SHA256", 8) == "46119246"


def test_the_setup_page_gives_seed_policy_and_form():
    form = sf.parse(SETUP)
    assert form["form"] == "kc-totp-settings-form"
    assert form["seed"] == RFC_SEED
    assert (form["algorithm"], form["digits"], form["period"]) == ("SHA1", 6, 30)
    assert form["action"] == "/realms/smoke/login-actions/authenticate?session_code=fixture&client_id=fixture"
    assert form["hidden"] == {"csrf_token": "fixture", "session_code": "fixture", "step": "config-totp"}


def test_the_setup_page_says_which_algorithm():
    form = sf.parse(SETUP.replace("Algorithm: SHA1", "Algorithm: SHA256").replace("Digits: 6", "Digits: 8"))
    assert (form["algorithm"], form["digits"], form["period"]) == ("SHA256", 8, 30)


def test_a_page_without_a_second_factor_form_is_left_alone():
    assert sf.parse("<form id=\"kc-form-login\" action=\"/x\"><input type=\"hidden\" name=\"a\" value=\"b\"></form>") is None
    s = Server()
    assert sf.SecondFactor().answer(s.call, LOGIN_URL, 303, {"location": "/cb?code=c"}, b"", "p") == \
        (303, {"location": "/cb?code=c"}, b"")
    assert s.posts == []


def test_enrolment_then_a_stored_otp_sign_in_in_the_next_step():
    clock = Clock(1111111109)
    f = sf.SecondFactor(clock, clock.sleep)
    s = Server((303, {"location": "/cb?code=a"}, b""), (303, {"location": "/cb?code=b"}, b""))
    st, h, _ = f.answer(s.call, LOGIN_URL, 200, {}, SETUP.encode(), "p")
    assert (st, h["location"]) == (303, "/cb?code=a")
    _, url, form = s.posts[0]
    assert url == "http://127.0.0.1:8080/realms/smoke/login-actions/authenticate?session_code=fixture&client_id=fixture"
    assert form == {"csrf_token": "fixture", "session_code": "fixture", "step": "config-totp",
                    "otp": sf.totp(RFC_SEED, 1111111109), "userLabel": "smoke"}
    # The same person again within the same 30 s step: the spent code is not sent twice.
    st, _, _ = f.answer(s.call, LOGIN_URL, 200, {}, OTP.encode(), "p")
    assert st == 303 and clock.slept and clock.now >= 1111111110
    _, _, form = s.posts[1]
    assert form == {"csrf_token": "fixture", "session_code": "fixture", "step": "login-otp",
                    "otp": sf.totp(RFC_SEED, 1111111110)}
    assert form["otp"] != s.posts[0][2]["otp"]


def test_an_otp_page_for_a_person_this_run_did_not_enrol_says_what_to_do():
    with pytest.raises(RuntimeError, match="remove its otp credential"):
        sf.SecondFactor().answer(Server().call, LOGIN_URL, 200, {}, OTP.encode(), "stranger")


def test_a_refused_code_raises_and_is_not_retried():
    s = Server((200, {}, OTP.encode()))
    with pytest.raises(RuntimeError, match="was refused"):
        sf.SecondFactor().answer(s.call, LOGIN_URL, 200, {}, SETUP.encode(), "p")
    assert len(s.posts) == 1


def test_no_seed_in_repr():
    f = sf.SecondFactor()
    f.code("p", sf.parse(SETUP))
    assert RFC_SEED not in repr(f) and "GEZD" not in repr(f)
