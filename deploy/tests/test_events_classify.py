"""scripts/events-classify.py: the offline decision model for event categories (T-5490)."""

import csv
import importlib.util
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/events-classify.py"
sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_bbsk_events_pipelines import CATEGORISED

spec = importlib.util.spec_from_file_location("events_classify", SCRIPT)
classify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(classify)


def test_selftest_passes_without_a_network_or_a_key():
    out = subprocess.run([sys.executable, str(SCRIPT), "--selftest"], capture_output=True, text=True, timeout=60, check=False)
    assert out.returncode == 0 and "selftest ok" in out.stdout, out.stderr


def test_the_python_rule_is_the_pipelines_rule_on_every_recorded_case():
    # The same cases the Bento tail answers in test_bbsk_events_pipelines (Docker): the port must agree.
    for word, name, code in CATEGORISED:
        assert classify.rule(word or "", name) == code, (word, name)


def test_the_sample_is_100_hand_labelled_events_of_the_closed_list():
    with (ROOT / "seeds/bbsk-events/category-sample.csv").open(encoding="utf-8", newline="") as f:
        rows = list(csv.DictReader(f))
    assert len(rows) == 100 and {r["label"] for r in rows} <= set(classify.CODES)
    assert len({r["name"].lower() for r in rows}) == 100


def test_every_cached_answer_is_a_code_of_the_list_for_a_known_source():
    with (ROOT / "seeds/bbsk-events/categories.csv").open(encoding="utf-8", newline="") as f:
        rows = list(csv.DictReader(f))
    assert list(rows[0].keys()) == list(classify.CACHE_FIELDS) if rows else True
    for r in rows:
        assert r["category"] in classify.CODES and len(r["key"]) == 16 and r["source"].startswith("https://"), r
