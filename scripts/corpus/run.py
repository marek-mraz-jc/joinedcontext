#!/usr/bin/env python3
"""Run every config of corpus/manifest.tsv through Bento 1.22.0 (.bento/bento) and ekiben, write corpus/RESULTS.md.

Per config:
  lint      `bento lint --skip-env-var-check` vs `ekiben lint --chilled`: agreement = same verdict (exit 0 or not).
  mappings  every Bloblang text (mapping, bloblang, mutation, request_map, result_map; check as `root = <check>`)
            through `blobl` in both, over the config's unit test inputs plus documents built from the `this.*` paths
            the text reads. Agreement = identical stdout and the same number of stderr lines (errors on the same
            inputs); the error text itself is counted apart. Texts calling a clock, random or host function are skipped.
  pipeline  configs whose input is generate/stdin and output stdout/file/drop, every component in ekiben and no
            nondeterministic function: run one-shot in both (generate capped at 5 messages, output to a file),
            outputs compared as sorted lines (Bento's parallel pipeline threads do not keep order).
Components Bento has and ekiben lacks are counted per name.

Usage: python3 scripts/corpus/run.py [--ekiben PATH] [--jobs N] [--only ID] [--diffs FILE]
"""
import argparse
import collections
import concurrent.futures as cf
import copy
import json
import os
import re
import subprocess
import tempfile

import yaml

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FILES = os.path.join(ROOT, ".corpus", "files")
BENTO = os.path.join(ROOT, ".bento", "bento")
NONDET = re.compile(r"\b(now|timestamp_unix\w*|timestamp|uuid_v\d|nanoid|ksuid|random_int|hostname|env|file|"
                    r"file_rel|counter|tracing_\w+|fake|snowflake_id|ulid)\s*\(|\$\{")
BLOBL_KEYS = ("mapping", "bloblang", "mutation", "request_map", "result_map", "check")
RUN_INPUTS = {"generate", "stdin"}
RUN_OUTPUTS = {"stdout", "file", "drop"}


def sh(cmd, stdin=None, timeout=20):
    try:
        p = subprocess.run(cmd, input=stdin, capture_output=True, timeout=timeout)
        return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return "timeout", "", ""


def components(node, names, kind_hint="processor", out=None):
    """(kind, name) of every component node: a map whose keys minus label/processors are one Bento name."""
    out = [] if out is None else out
    if isinstance(node, list):
        for x in node:
            components(x, names, kind_hint, out)
        return out
    if not isinstance(node, dict):
        return out
    keys = set(node) - {"label", "processors"}
    if len(keys) == 1:
        (k,) = keys
        kind = {"input": "inputs", "output": "outputs", "processor": "processors", "cache": "caches",
                "rate_limit": "rate-limits", "buffer": "buffers"}[kind_hint]
        if k in names.get(kind, ()):
            out.append((kind, k))
    for k, v in node.items():
        hint = kind_hint
        if k in ("input", "inputs", "input_resources"):
            hint = "input"
        elif k in ("output", "outputs", "output_resources", "fallback"):
            hint = "output"
        elif k in ("processors", "processor_resources", "request_map_processors"):
            hint = "processor"
        elif k == "cache_resources":
            hint = "cache"
        elif k == "rate_limit_resources":
            hint = "rate_limit"
        elif k == "buffer":
            hint = "buffer"
        if k == "tests":
            continue
        components(v, names, hint, out)
    return out


def blobl_texts(node, out=None):
    out = [] if out is None else out
    if isinstance(node, list):
        for x in node:
            blobl_texts(x, out)
    elif isinstance(node, dict):
        for k, v in node.items():
            if k == "tests":
                continue
            if k in BLOBL_KEYS and isinstance(v, str) and v.strip():
                out.append(f"root = {v}" if k == "check" else v)
            else:
                blobl_texts(v, out)
    return out


def test_inputs(doc):
    lines = []
    for t in doc.get("tests") or [] if isinstance(doc.get("tests"), list) else []:
        for m in (t or {}).get("input_batch") or []:
            if not isinstance(m, dict):
                continue
            if "json_content" in m:
                try:
                    lines.append(json.dumps(m["json_content"], separators=(",", ":")))
                except (ValueError, TypeError):  # a self-referencing YAML alias
                    continue
            elif isinstance(m.get("content"), str) and "\n" not in m["content"]:
                lines.append(m["content"])
    return lines


def synth_inputs(text):
    """Documents holding every `this.a.b` path the text reads, filled with a string, a number, an array and null."""
    paths = {p for p in re.findall(r"\bthis((?:\.[A-Za-z_][A-Za-z0-9_]*)+)", text)}
    docs = []
    for fill in ("abc", 3, 2.5, [1, "x", {"k": "v"}], {"k": "v", "n": 1}, None, True, "2024-01-02T03:04:05Z"):
        d = {}
        for p in sorted(paths):
            cur, parts = d, p.strip(".").split(".")
            for part in parts[:-1]:
                nxt = cur.get(part)
                if not isinstance(nxt, dict):
                    nxt = cur[part] = {}
                cur = nxt
            cur.setdefault(parts[-1], copy.deepcopy(fill))
        docs.append(json.dumps(d, separators=(",", ":")))
    docs += ['{"id":"a1","name":"Name","value":42,"tags":["x","y"],"nested":{"a":1.5,"b":null}}', '"plain"', "7"]
    return list(dict.fromkeys(docs))


def compare_mapping(text, inputs, ekiben):
    stdin = ("\n".join(inputs) + "\n").encode()
    b = sh([BENTO, "blobl", text], stdin)
    e = sh([ekiben, "blobl", text], stdin)
    berr = [l for l in b[2].splitlines() if l.strip()]
    eerr = [l for l in e[2].splitlines() if l.strip()]
    same = b[1] == e[1] and len(berr) == len(eerr) and (b[0] == 0) == (e[0] == 0)
    return {"same": same, "same_errors": same and berr == eerr, "bento": b, "ekiben": e}


def run_pipeline(doc, path_text, ekiben, inputs):
    results = {}
    for name in ("bento", "ekiben"):
        d = copy.deepcopy(doc)
        d.pop("tests", None)
        d["logger"] = {"level": "none"}
        d["http"] = {"enabled": False}
        d.pop("metrics", None)
        d.pop("tracer", None)
        inp = d["input"]
        if "generate" in inp:
            g = inp["generate"] if isinstance(inp["generate"], dict) else {}
            g["count"] = min(int(g.get("count") or 5), 5)
            g["interval"] = ""
            inp["generate"] = g
        with tempfile.TemporaryDirectory() as tmp:
            outfile = os.path.join(tmp, "out.txt")
            out = d["output"]
            if "stdout" in out:
                out["file"] = {"path": outfile, "codec": (out.get("stdout") or {}).get("codec", "lines")}
                del out["stdout"]
            elif "file" in out and isinstance(out["file"], dict):
                out["file"]["path"] = outfile
            cfg = os.path.join(tmp, "c.yaml")
            with open(cfg, "w") as f:
                yaml.safe_dump(d, f, sort_keys=False)
            stdin = ("\n".join(inputs) + "\n").encode()
            cmd = [BENTO, "-c", cfg] if name == "bento" else [ekiben, "run", cfg]
            rc, _, err = sh(cmd, stdin, timeout=30)
            text = open(outfile, errors="replace").read() if os.path.exists(outfile) else ""
            results[name] = (rc, sorted(text.splitlines()), err[-2000:])
    b, e = results["bento"], results["ekiben"]
    return {"same": b[1] == e[1] and (b[0] == 0) == (e[0] == 0), "bento": b, "ekiben": e}


def missing_blobl(text, names):
    """Bloblang methods and functions Bento has that ekiben's parse error names as unrecognised."""
    out = set()
    for kind, name in re.findall(r"unrecognised (method|function) '(\w+)'", text):
        key = f"bloblang-{kind}s"
        if name in names["bento"].get(key, ()) and name not in names["ekiben"].get(key, ()):
            out.add(f"{key}/{name}")
    return out


def check(item, names, ekiben):
    cid, row = item
    path = os.path.join(FILES, cid + ".yaml")
    text = open(path, errors="replace").read()
    r = {"id": cid, "repo": row["repo"], "path": row["path"]}
    try:
        doc = next(yaml.safe_load_all(text), None)
    except Exception:
        doc = None
    if not isinstance(doc, dict):
        r["error"] = "unparsable"
        return r
    comps = components(doc, names["bento"])
    r["missing"] = sorted({f"{k}/{n}" for k, n in comps if n not in names["ekiben"].get(k, ())})
    bl = sh([BENTO, "lint", "--skip-env-var-check", path])
    el = sh([ekiben, "lint", "--chilled", path])
    if bl[0] == 0 and el[0] != 0:
        r["missing"] = sorted(set(r["missing"]) | missing_blobl(el[1] + el[2], names))
    if bl[0] == 0:  # `bento list` leaves out deprecated components; ekiben's lint names any it lacks
        for kind, name in re.findall(r"(input|output|processor|cache|rate_limit|buffer): type '(\w+)' was not "
                                     r"recognised", el[1] + el[2]):
            r["missing"] = sorted(set(r["missing"]) | {f"{kind.replace('_', '-')}s/{name}"})
    r["lint"] = {"bento": bl[0] == 0, "ekiben": el[0] == 0, "bento_out": (bl[1] + bl[2])[-1500:],
                 "ekiben_out": (el[1] + el[2])[-1500:]}
    tins = test_inputs(doc)
    r["mappings"] = []
    for t in blobl_texts(doc):
        if NONDET.search(t):
            r["mappings"].append({"skipped": "nondeterministic"})
            continue
        res = compare_mapping(t, tins + synth_inputs(t), ekiben)
        res["text"] = t
        gone = missing_blobl(res["ekiben"][2], names)
        if gone and not res["same"]:
            r["missing"] = sorted(set(r["missing"]) | gone)
            r["mappings"].append({"skipped": "missing " + ", ".join(sorted(gone))})
            continue
        r["mappings"].append(res)
    inp, out = doc.get("input"), doc.get("output")
    runnable = (isinstance(inp, dict) and isinstance(out, dict) and RUN_INPUTS & set(inp) and RUN_OUTPUTS & set(out)
                and not r["missing"] and not NONDET.search(text) and r["lint"]["bento"])
    if runnable:
        r["pipeline"] = run_pipeline(doc, text, ekiben, tins or synth_inputs(text)[:5])
    return r


def pct(a, b):
    return f"{100.0 * a / b:.1f} %" if b else "n/a"


def report(results, total, out_md):
    lint_all = [r for r in results if "lint" in r]
    lint_ok = sum(r["lint"]["bento"] == r["lint"]["ekiben"] for r in lint_all)
    lint_have = [r for r in lint_all if not r["missing"]]
    lint_have_ok = sum(r["lint"]["bento"] == r["lint"]["ekiben"] for r in lint_have)
    maps = [m for r in results for m in r.get("mappings", []) if "same" in m]
    skipped = sum(1 for r in results for m in r.get("mappings", []) if m.get("skipped") == "nondeterministic")
    gone = sum(1 for r in results for m in r.get("mappings", []) if m.get("skipped", "").startswith("missing"))
    pipes = [r["pipeline"] for r in results if "pipeline" in r]
    missing = collections.Counter(m for r in results for m in r.get("missing", []))
    lines = [
        "# Public pipeline corpus parity (Bento 1.22.0 vs ekiben)",
        "",
        "Generated by `python3 scripts/corpus/run.py` over `corpus/manifest.tsv` (sources and search queries:",
        "`corpus/queries.tsv`). Fetch the files with `python3 scripts/corpus/collect.py` first.",
        "",
        "| Measure | Agree | Of | Agreement |",
        "|---|---:|---:|---:|",
        f"| Configs collected (distinct) | | {total} | |",
        f"| Lint verdict, all configs | {lint_ok} | {len(lint_all)} | {pct(lint_ok, len(lint_all))} |",
        f"| Lint verdict, configs using only components ekiben has | {lint_have_ok} | {len(lint_have)} | "
        f"{pct(lint_have_ok, len(lint_have))} |",
        f"| Mapping output (stdout and erroring inputs) | {sum(m['same'] for m in maps)} | {len(maps)} | "
        f"{pct(sum(m['same'] for m in maps), len(maps))} |",
        f"| Mapping error text too | {sum(m['same_errors'] for m in maps)} | {len(maps)} | "
        f"{pct(sum(m['same_errors'] for m in maps), len(maps))} |",
        f"| Runnable pipelines identical | {sum(p['same'] for p in pipes)} | {len(pipes)} | "
        f"{pct(sum(p['same'] for p in pipes), len(pipes))} |",
        "",
        f"Mapping texts skipped as nondeterministic (clock, random, host, env): {skipped}; calling a Bloblang method "
        f"or function ekiben lacks (counted below): {gone}.",
        "",
        "## Known differences left open",
        "",
        "- `compress(\"zlib\"|\"flate\")` output bytes: Go's compress/flate writes stored blocks for small inputs;",
        "  both sides decompress each other's output, only the bytes differ.",
        "- `format_yaml`: Bento indents by 4 (yaml.v3) and keeps a parsed JSON number as `json.Number`, so it prints",
        "  `\"3\"`; ekiben indents by 2 and prints `3`. `1.50` and `1e6` also pass through Bento unchanged.",
        "- `key_values()` order follows Go map iteration (random in Bento), so mappings indexing it differ.",
        "- Error texts of Go's JSON decoder, and parse-time constant folding (`5 % 0.5` is a parse error in Bento).",
        "- Lint: Bento also lints the config schema (unknown, required and deprecated fields); ekiben `lint --chilled`",
        "  checks components and Bloblang only. Most lint disagreements on configs using only ekiben components are",
        "  schema findings.",
        "- `json_schema` is refused (unknown method/processor) until ekiben validates JSON Schema.",
        "",
        "## Components and Bloblang methods Bento has and ekiben lacks (configs using each)",
        "",
        "| Component | Configs |",
        "|---|---:|",
    ]
    lines += [f"| {k} | {v} |" for k, v in sorted(missing.items(), key=lambda kv: (-kv[1], kv[0]))]
    with open(out_md, "w") as f:
        f.write("\n".join(lines) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ekiben", default=os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")),
                                                   "release", "ekiben"))
    ap.add_argument("--jobs", type=int, default=6)
    ap.add_argument("--only")
    ap.add_argument("--diffs", default=os.path.join(ROOT, ".corpus", "diffs.json"))
    ap.add_argument("--out", default=os.path.join(ROOT, "corpus", "RESULTS.md"))
    a = ap.parse_args()
    names = {"bento": json.loads(sh([BENTO, "list", "--format", "json"])[1]),
             "ekiben": json.loads(sh([a.ekiben, "list", "--format", "json"])[1])}
    with open(os.path.join(ROOT, "corpus", "manifest.tsv")) as f:
        header = f.readline().rstrip("\n").split("\t")
        rows = {r[0]: dict(zip(header, r)) for r in (l.rstrip("\n").split("\t") for l in f)}
    items = [(k, v) for k, v in rows.items() if not a.only or k == a.only]
    with cf.ThreadPoolExecutor(a.jobs) as ex:
        results = list(ex.map(lambda it: check(it, names, a.ekiben), items))
    diffs = []
    for r in results:
        d = {"id": r["id"], "repo": r["repo"], "path": r["path"]}
        if "lint" in r and r["lint"]["bento"] != r["lint"]["ekiben"]:
            d["lint"] = r["lint"]
        bad = [m for m in r.get("mappings", []) if "same" in m and not m["same"]]
        if bad:
            d["mappings"] = bad
        if "pipeline" in r and not r["pipeline"]["same"]:
            d["pipeline"] = r["pipeline"]
        d["missing"] = r.get("missing", [])
        if len(d) > 4:
            diffs.append(d)
    with open(a.diffs, "w") as f:
        json.dump(diffs, f, indent=1, default=str)
    if not a.only:
        report(results, len(rows), a.out)
    print(f"{len(diffs)} configs with differences -> {a.diffs}")


if __name__ == "__main__":
    main()
