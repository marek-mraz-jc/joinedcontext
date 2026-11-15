"""pin-images.py's tiers (T-4721): a production tier pins only digests that passed the full tests."""

import importlib.util
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/pin-images.py"
DIGEST = "sha256:" + "a" * 64


@pytest.fixture
def pin(monkeypatch):
    spec = importlib.util.spec_from_file_location("pin_images", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    written = []
    monkeypatch.setattr(module, "built", lambda repository: "c" * 40)
    monkeypatch.setattr(module, "green", lambda repository, workflow, commit: True)
    monkeypatch.setattr(module, "published", lambda image, commit: DIGEST)
    monkeypatch.setattr(module, "signed", lambda repository, image, digest: True)
    monkeypatch.setattr(module, "pin_files", lambda pins, commit=None: written.append(pins) or [Path("x")])
    module.written = written
    return module


def test_dev_pins_a_signed_digest_without_the_full_tests(pin, monkeypatch):
    monkeypatch.setattr(pin, "attested", lambda *a: pytest.fail("dev must not ask for the full tests"))
    pin.main("platform")
    assert pin.written and set(pin.written[0].values()) == {DIGEST}


def test_production_refuses_a_digest_without_the_full_tests_attestation(pin, monkeypatch):
    monkeypatch.setattr(pin, "attested", lambda repository, image, digest: False)
    with pytest.raises(SystemExit, match="has not passed the full tests"):
        pin.main("platform", "production")
    assert pin.written == []


def test_production_pins_a_promoted_digest(pin, monkeypatch):
    asked = []
    monkeypatch.setattr(pin, "attested", lambda repository, image, digest: asked.append(image) or True)
    pin.main("platform", "production")
    assert asked == pin.SOURCES["platform"][1]
    assert pin.written


def test_the_attestation_is_checked_against_promote_yml_on_main(pin, monkeypatch):
    calls = []
    monkeypatch.setattr(pin, "run", lambda *args, check=True: calls.append(args) or type("R", (), {"returncode": 1})())
    assert not pin.attested("joinedcontext-platform", "joinedcontext-platform", DIGEST)
    [args] = calls
    assert args[:4] == ("cosign", "verify-attestation", "--type", pin.FULL_TESTS)
    assert "https://github.com/marek-mraz-jc/joinedcontext-platform/.github/workflows/promote.yml@refs/heads/main" in args


def test_a_pin_moves_the_commit_its_block_records(pin):
    """check-docs-pin.sh reads `commit:` beside the gateway's digest (T-4483); a pin that moved only
    the digest left a pair the registry does not hold (2026-10-19, platform 4976de6)."""
    old_digest, new_digest = "sha256:" + "1" * 64, "sha256:" + "2" * 64
    text = (
        "  image:\n"
        "    repository: 'ghcr.io/marek-mraz-jc/joinedcontext-platform'\n"
        "    tag: 'main'\n"
        f"    digest: '{old_digest}'\n"
        "    # the platform commit\n"
        f"    commit: '{'a' * 40}'\n"
        "  checkouts:\n"
        "    repository: 'ghcr.io/marek-mraz-jc/joinedcontext-platform/checkouts'\n"
        f"    digest: '{old_digest}'\n"
    )
    out, n = pin.pin(text, "joinedcontext-platform", new_digest, "b" * 40)
    assert n == 1
    assert f"commit: '{'b' * 40}'" in out and f"digest: '{new_digest}'" in out
    # the next block keeps its own digest, and a pin without a commit leaves the line alone
    assert out.endswith(f"digest: '{old_digest}'\n")
    assert f"commit: '{'a' * 40}'" in pin.pin(text, "joinedcontext-platform", new_digest)[0]


@pytest.mark.parametrize("red", ["ci.yml", "image.yml"])
def test_a_pin_needs_its_ci_and_image_run_green(pin, monkeypatch, red):
    # T-6154: pin-images starts on image.yml's dispatch, before or after its run settled; a commit
    # whose ci or image run is red or still running is never pinned, and image-scan.yml is not asked.
    asked = []
    monkeypatch.setattr(pin, "green", lambda repository, workflow, commit: asked.append(workflow) or workflow != red)
    pin.main("platform")
    assert not pin.written
    assert "image-scan.yml" not in asked
