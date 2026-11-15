"""pin-images.py antares (T-6330): Antares dev is pinned by machine only over a range that brings no
migration and no new runtime key; every other range, or one it cannot read whole, stays a hand pin."""

import importlib.util
import json
import shutil
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/pin-images.py"
OLD, NEW = "a" * 40, "b" * 40
DIGEST = "sha256:" + "d" * 64
KEYS = {"ANTARES_POLICY", "ANTARES_API_SURFACES"}


@pytest.fixture
def pin(monkeypatch, tmp_path):
    spec = importlib.util.spec_from_file_location("pin_images", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    (tmp_path / "components/context-broker").mkdir(parents=True)
    shutil.copy(ROOT / "components/context-broker/images.yaml", tmp_path / "components/context-broker/images.yaml")
    text = (tmp_path / "components/context-broker/images.yaml").read_text()
    pinned, tag = module.antares_pinned(text)
    (tmp_path / "components/context-broker/images.yaml").write_text(text.replace(pinned, OLD))
    monkeypatch.setattr(module, "ROOT", tmp_path)
    monkeypatch.setattr(module, "antares_shipped", lambda: (NEW, 41000000001, 200))
    monkeypatch.setattr(module, "antares_keys", lambda commit: set(KEYS))
    monkeypatch.setattr(module, "published", lambda image, tag: DIGEST)
    monkeypatch.setattr(module, "antares_signed", lambda digest, commit: commit == NEW)
    module.compare = compare(file("crates/antares-api/src/attrs.rs"))
    monkeypatch.setattr(module, "run", lambda *a, check=True: type(
        "R", (), {"returncode": 0, "stdout": json.dumps(module.compare)})())
    module.old_tag = tag
    module.path = tmp_path / "components/context-broker/images.yaml"
    return module


def file(name, status="modified", previous=None):
    return {"filename": name, "status": status, **({"previous_filename": previous} if previous else {})}


def compare(*files, status="ahead"):
    return {"status": status, "ahead_by": 2, "files": list(files),
            "commits": [{"commit": {"message": "views: bound reads (T-6312)"}},
                        {"commit": {"message": "merge train T-6276 and T-6312"}}]}


def test_a_range_with_neither_a_migration_nor_a_new_key_is_safe(pin):
    assert pin.judge(compare(file("crates/antares-mcp/src/tools.rs")), KEYS, KEYS) is None


@pytest.mark.parametrize("change", [
    file("crates/antares-sql/migrations/0042_widen.sql", "added"),
    file("crates/antares-sql/migrations/0042_widen.sql", "modified"),
    file("crates/antares-sql/src/x.sql", "renamed", previous="crates/antares-sql/migrations/0041_x.sql"),
])
def test_a_range_with_a_migration_is_a_hand_pin(pin, change):
    assert pin.judge(compare(file("crates/a.rs"), change), KEYS, KEYS).startswith("a migration: 004")


@pytest.mark.parametrize("after, sign", [(KEYS | {"ANTARES_NEW_KNOB"}, "+"), (KEYS - {"ANTARES_POLICY"}, "-")])
def test_a_range_that_adds_or_drops_a_key_is_a_hand_pin(pin, after, sign):
    reason = pin.judge(compare(file("docs/src/configuration.md")), KEYS, after)
    assert reason.startswith("runtime keys: " + sign)


@pytest.mark.parametrize("answer, before, after", [
    (None, KEYS, KEYS),                                         # the compare call failed
    (compare(status="diverged"), KEYS, KEYS),                   # dev was rewritten or is behind
    (compare(*[file(f"crates/f{i}.rs") for i in range(300)]), KEYS, KEYS),  # cut at the API's cap
    ({"status": "ahead"}, KEYS, KEYS),                          # no file list
    (compare(file("crates/a.rs")), None, KEYS),                 # the key doc did not read
    (compare(file("crates/a.rs")), KEYS, None),
])
def test_a_range_the_judge_cannot_read_is_a_hand_pin(pin, answer, before, after):
    assert pin.judge(answer, before, after)


def test_a_safe_range_pins_digest_tag_and_commit_with_the_facts(pin, capsys):
    pin.antares()
    text = pin.path.read_text()
    assert pin.antares_pinned(text) == (NEW, "dev-200")
    assert pin.written(text, "antares-broker-jc") == DIGEST
    assert "Antares dev bbbbbbb (dev-200, over aaaaaaa: T-6276, T-6312). No migration" in text
    assert f"Rollback: {pin.old_tag}. Signed by ship.yml on dev at bbbbbbb (run 41000000001)" in text
    message = capsys.readouterr().out.strip()
    assert message == ("pin Antares bbbbbbb (dev-200: T-6276, T-6312; no migration, no new key; "
                       "signed by ship.yml run 41000000001, cosign verified)")


def test_an_unsafe_range_writes_nothing_and_judge_mode_exits_3(pin, capsys):
    before = pin.path.read_text()
    pin.compare = compare(file("crates/antares-sql/migrations/0042_widen.sql", "added"))
    pin.antares()
    assert pin.path.read_text() == before
    assert "hand pin: a migration: 0042_widen.sql" in capsys.readouterr().err
    with pytest.raises(SystemExit) as stop:
        pin.antares(judge_only=True)
    assert stop.value.code == 3


def test_a_digest_ship_yml_did_not_sign_at_that_commit_is_refused(pin, monkeypatch):
    before = pin.path.read_text()
    monkeypatch.setattr(pin, "antares_signed", lambda digest, commit: False)
    with pytest.raises(SystemExit, match="no signature of ship.yml on dev"):
        pin.antares()
    assert pin.path.read_text() == before


def test_the_signature_is_checked_against_ship_yml_on_dev_at_the_commit(monkeypatch):
    calls = []
    spec = importlib.util.spec_from_file_location("fresh", SCRIPT)
    fresh = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fresh)
    monkeypatch.setattr(fresh, "run", lambda *a, check=True: calls.append(a) or type("R", (), {"returncode": 1})())
    assert not fresh.antares_signed(DIGEST, NEW)
    [args] = calls
    assert args[args.index("--certificate-identity") + 1] == (
        "https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker/.github/workflows/ship.yml@refs/heads/dev")
    assert args[args.index("--certificate-github-workflow-sha") + 1] == NEW


def test_the_deployed_broker_block_records_its_commit():
    spec = importlib.util.spec_from_file_location("deployed", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    commit, tag = module.antares_pinned((ROOT / "components/context-broker/images.yaml").read_text())
    assert commit and tag and tag.startswith("dev-")


@pytest.mark.parametrize(
    "runs, answer",
    [
        ([("completed", "skipped"), ("completed", "success")], True),
        ([("completed", "skipped"), ("completed", "failure"), ("completed", "success")], False),
        ([("in_progress", ""), ("completed", "success")], False),
        ([("completed", "skipped")], False),
        ([], False),
    ],
)
def test_a_skipped_run_hides_no_image_run(pin, monkeypatch, runs, answer):
    stdout = json.dumps([{"status": status, "conclusion": conclusion} for status, conclusion in runs])
    monkeypatch.setattr(pin, "run", lambda *args, **kwargs: type("Done", (), {"stdout": stdout}))
    assert pin.green("joinedcontext-portal", "image.yml", "a" * 40) is answer
