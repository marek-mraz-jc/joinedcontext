"""Cut a small export bundle (same layout) from a full one, to validate a suite on a small jc:
   python3 -I slice.py --export <full> --repos joinedcontext/zilina,joinedcontext/configuration --out <dir>
Keeps the named repositories, the Antares rows of the tenants their spaces map to, every realm and
the manifest (counts rewritten for the slice). Read only on <full>."""
import argparse
import json
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "endpoints"))
import export as ex  # noqa: E402


def main(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("--export", required=True)
    p.add_argument("--repos", required=True, help="owner/name,...")
    p.add_argument("--out", required=True)
    a = p.parse_args(argv)
    src, out = Path(a.export), Path(a.out)
    keep = set(a.repos.split(","))
    (out / "gitea" / "files").mkdir(parents=True, exist_ok=True)
    repos = [r for r in json.loads((src / "gitea" / "repos.json").read_text()) if f"{r['owner']}/{r['name']}" in keep]
    (out / "gitea" / "repos.json").write_text(json.dumps(repos, indent=1))
    for r in repos:
        dst = out / "gitea" / "files" / r["owner"] / r["name"]
        if not dst.exists():
            shutil.copytree(src / "gitea" / "files" / r["owner"] / r["name"], dst, symlinks=True)
    for d in ("keycloak",):
        if (src / d).exists() and not (out / d).exists():
            shutil.copytree(src / d, out / d)
    (out / "grafana" / "dashboards").mkdir(parents=True, exist_ok=True)
    tenants = {e.tenant for e in ex.load_endpoints(out / "gitea" / "files")}
    for f in out.glob("gitea/files/*/*/spaces/*/space.yaml"):
        for d in ex._load_yaml(f):
            if d.get("kind") == "ContextSpace":
                tenants.add(ex.urn_segment(f.parts[-4], d["metadata"]["name"], (d.get("spec") or {}).get("urnSegment")))
    (out / "antares").mkdir(exist_ok=True)
    counts = {}
    for name in ("entities.jsonl", "temporal.jsonl"):
        n = 0
        with (src / "antares" / name).open("rb") as fi, (out / "antares" / name).open("wb") as fo:
            for line in fi:
                t = json.loads(line[:200].split(b'",', 1)[0] + b'"}')["tenant"]
                if t in tenants:
                    fo.write(line)
                    n += 1
        counts[name] = n
    m = json.loads((src / "manifest.json").read_text())
    m["slice"] = {"of": str(src), "repos": sorted(keep), "tenants": sorted(tenants), "rows": counts}
    (out / "manifest.json").write_text(json.dumps(m, indent=1))
    print(json.dumps(m["slice"], indent=1))


if __name__ == "__main__":
    main()
