import datetime as dt
import json

import redate


def test_every_stamp_moves_by_the_same_amount_and_the_manifest_comes_last(tmp_path):
    (tmp_path / "antares").mkdir()
    old = "2026-11-01T10:00:00Z"
    (tmp_path / "manifest.json").write_text(json.dumps({"exportedAt": old}))
    line = {"tenant": "t", "instance": {"observedAt": "2026-11-01T09:00:00.250Z", "value": "2026-11-01T09:00:00Z"}}
    (tmp_path / "antares" / "temporal.jsonl").write_text(json.dumps(line) + "\n")
    redate.main([str(tmp_path)])
    m = json.loads((tmp_path / "manifest.json").read_text())
    now = dt.datetime.strptime(m["exportedAt"], "%Y-%m-%dT%H:%M:%SZ")
    assert abs((dt.datetime.now(dt.timezone.utc).replace(tzinfo=None) - now).total_seconds()) < 5
    got = json.loads((tmp_path / "antares" / "temporal.jsonl").read_text())
    obs = dt.datetime.strptime(got["instance"]["observedAt"], "%Y-%m-%dT%H:%M:%S.250Z")
    assert now - obs == dt.timedelta(hours=1)
    assert (tmp_path / "manifest.json").stat().st_mtime >= (tmp_path / "antares" / "temporal.jsonl").stat().st_mtime
