"""Moves a synthetic export bundle to now: every UTC timestamp in antares/*.jsonl and the
manifest's exportedAt shift by (now - exportedAt), so the 24-hour rule and the suites' windows
hold for a bundle committed days ago. The manifest is written last (the import takes only a
settled bundle: no data file newer than manifest.json).
   python3 -I redate.py <export dir>"""
import datetime as dt
import json
import re
import sys
from pathlib import Path

STAMP = re.compile(rb'"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(\.\d+)?Z"')
FMT = "%Y-%m-%dT%H:%M:%S"


def shifted(raw: bytes, delta: dt.timedelta) -> bytes:
    def move(m):
        t = dt.datetime.strptime(m.group(1).decode(), FMT) + delta
        return b'"' + t.strftime(FMT).encode() + (m.group(2) or b"") + b'Z"'
    return STAMP.sub(move, raw)


def main(argv):
    root = Path(argv[0])
    manifest = json.loads((root / "manifest.json").read_text())
    then = dt.datetime.strptime(manifest["exportedAt"], FMT + "Z")
    now = dt.datetime.now(dt.UTC).replace(tzinfo=None, microsecond=0)
    delta = now - then
    for f in sorted((root / "antares").glob("*.jsonl")):
        f.write_bytes(shifted(f.read_bytes(), delta))
    manifest["exportedAt"] = now.strftime(FMT) + "Z"
    manifest["redated"] = {"from": then.strftime(FMT) + "Z", "bySeconds": int(delta.total_seconds())}
    (root / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(f"redate: {root} moved by {delta}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
