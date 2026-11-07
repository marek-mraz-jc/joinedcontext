#!/usr/bin/env python3
"""Collect a corpus of public Bento/Benthos pipeline configs into .corpus/ and write corpus/manifest.tsv.

Sources:
  * Bento v1.22.0 and Benthos v4.24.0 (both MIT): every YAML file and every ```yaml block of their docs that
    holds a stream config (top-level input/pipeline/output/buffer/resources keys).
  * GitHub code search (the queries in QUERIES): one file per hit, pinned to the commit the search returned.

Files go to .corpus/files/<sha256>.yaml (outside git). The manifest records source, repo, path, commit and
licence per kept file; the query texts and the date go to corpus/queries.tsv. A file is dropped when it is a
duplicate (same text after stripping comments and blank lines), not a stream config, or when gitleaks flags it.

Usage: python3 scripts/corpus/collect.py [--no-search]   (needs git, gh with a token, gitleaks)
"""
import datetime
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.parse

import yaml

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CORPUS = os.path.join(ROOT, ".corpus")
FILES = os.path.join(CORPUS, "files")
SRC = os.path.join(CORPUS, "src")
MANIFEST = os.path.join(ROOT, "corpus", "manifest.tsv")
QUERIES_OUT = os.path.join(ROOT, "corpus", "queries.tsv")

REPOS = [
    ("warpstreamlabs/bento", "v1.22.0", "MIT"),
    ("redpanda-data/connect", "v4.24.0", "MIT"),  # last Benthos release under MIT
]
QUERIES = [
    'bloblang "pipeline:" "processors:" language:YAML',
    'benthos "pipeline:" "processors:" language:YAML',
    'bento "pipeline:" "processors:" language:YAML',
    '"mapping:" "pipeline:" "processors:" "output:" language:YAML',
    '"bloblang:" "pipeline:" "input:" language:YAML',
    '"mapping: |" "input:" "output:" "processors:" language:YAML',
    '"branch:" "request_map:" "result_map:" language:YAML',
    '"generate:" "mapping:" "output:" language:YAML',
    '"stdin:" "pipeline:" "stdout:" language:YAML',
    '"http_server:" "pipeline:" "mapping:" language:YAML',
    '"redpanda" "pipeline:" "processors:" "mapping:" language:YAML',
    '"unit tests" OR "tests:" "target_processors:" language:YAML',
    '"input_batch:" "output_conditions:" language:YAML',
    '"switch:" "check:" "pipeline:" "processors:" language:YAML',
    '"root = this" "pipeline:" language:YAML',
    '"meta" "root." "processors:" "input:" language:YAML',
]
STREAM_KEYS = {"input", "pipeline", "output", "buffer", "input_resources", "processor_resources",
               "output_resources", "cache_resources", "rate_limit_resources", "resources"}
SKIP_PATH = re.compile(r"(^|/)(\.github|helm|charts?|templates|k8s|kubernetes|docker-compose[^/]*)(/|$)", re.I)


def run(*cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw).stdout


def gh_json(path):
    for attempt in range(6):
        p = subprocess.run(["gh", "api", "-H", "Accept: application/vnd.github+json", path],
                           capture_output=True, text=True)
        if p.returncode == 0:
            return json.loads(p.stdout)
        if "rate limit" in (p.stdout + p.stderr).lower() or "403" in p.stderr:
            time.sleep(15 * (attempt + 1))
            continue
        return None
    return None


def normalized(text):
    lines = [re.sub(r"\s+#.*$", "", l).rstrip() for l in text.splitlines()]
    return "\n".join(l for l in lines if l.strip() and not l.lstrip().startswith("#"))


def stream_config(text):
    """True when the text parses as a YAML map with a stream key (multi-document files: the first document)."""
    try:
        doc = next(yaml.safe_load_all(text), None)
    except Exception:
        return False
    return isinstance(doc, dict) and bool(STREAM_KEYS & set(doc))


class Corpus:
    def __init__(self):
        self.rows, self.seen = [], set()
        os.makedirs(FILES, exist_ok=True)

    def add(self, text, source, repo, path, commit, licence):
        if len(text) > 200_000 or not stream_config(text):
            return
        key = hashlib.sha256(normalized(text).encode()).hexdigest()
        if key in self.seen:
            return
        self.seen.add(key)
        sha = hashlib.sha256(text.encode()).hexdigest()
        with open(os.path.join(FILES, sha[:16] + ".yaml"), "w") as f:
            f.write(text)
        self.rows.append({"id": sha[:16], "source": source, "repo": repo, "path": path,
                          "commit": commit, "licence": licence})


def local_repos(c):
    os.makedirs(SRC, exist_ok=True)
    for repo, tag, licence in REPOS:
        dest = os.path.join(SRC, repo.replace("/", "_"))
        if not os.path.isdir(dest):
            run("git", "clone", "-q", "--depth", "1", "-b", tag, f"https://github.com/{repo}", dest)
        commit = run("git", "-C", dest, "rev-parse", "HEAD").strip()
        for base, _, names in sorted(os.walk(dest)):
            if "/.git" in base:
                continue
            for n in sorted(names):
                full = os.path.join(base, n)
                rel = os.path.relpath(full, dest)
                text = open(full, errors="replace").read() if n.endswith((".yaml", ".yml", ".md", ".mdx")) else ""
                if n.endswith((".yaml", ".yml")):
                    c.add(text, "repo", repo, rel, commit, licence)
                elif text:
                    for i, block in enumerate(re.findall(r"```(?:yaml|yml)[^\n]*\n(.*?)```", text, re.S)):
                        c.add(block, "docs", repo, f"{rel}#yaml{i}", commit, licence)


def search(c, queries_log):
    licences, fetched = {}, set()
    today = datetime.date.today().isoformat()
    for q in QUERIES:
        hits = 0
        for page in range(1, 11):
            res = gh_json(f"search/code?q={urllib.parse.quote(q)}&per_page=100&page={page}")
            time.sleep(7)  # code search: 10 requests a minute
            items = (res or {}).get("items") or []
            for it in items:
                repo, path = it["repository"]["full_name"], it["path"]
                if SKIP_PATH.search(path):
                    continue
                m = re.search(r"/blob/([0-9a-f]{40})/", it.get("html_url", ""))
                commit = m.group(1) if m else ""
                if repo not in licences:
                    info = gh_json(f"repos/{repo}") or {}
                    licences[repo] = ((info.get("license") or {}).get("spdx_id")) or "NONE"
                if not commit or (repo, path) in fetched:
                    continue
                fetched.add((repo, path))
                url = f"https://raw.githubusercontent.com/{repo}/{commit}/{urllib.parse.quote(path)}"
                p = subprocess.run(["curl", "-sfL", "--max-time", "20", "--max-filesize", "200000", url],
                                   capture_output=True)
                if p.returncode != 0:
                    continue
                text = p.stdout.decode("utf-8", "replace")
                c.add(text, "search", repo, path, commit, licences[repo])
                hits += 1
            if len(items) < 100:
                break
        queries_log.append((today, q, hits))
        print(f"{q!r}: {hits} hits, corpus {len(c.rows)}", file=sys.stderr)


def drop_leaks(c):
    report = os.path.join(CORPUS, "gitleaks.json")
    subprocess.run(["gitleaks", "detect", "--no-git", "--source", FILES, "--report-format", "json",
                    "--report-path", report, "--no-banner", "--exit-code", "0"], capture_output=True)
    flagged = {os.path.basename(f["File"])[:16] for f in json.load(open(report))}
    for r in c.rows:
        if r["id"] in flagged:
            os.remove(os.path.join(FILES, r["id"] + ".yaml"))
    c.rows = [r for r in c.rows if r["id"] not in flagged]
    print(f"gitleaks dropped {len(flagged)} files", file=sys.stderr)


def main():
    c = Corpus()
    local_repos(c)
    queries_log = []
    if "--no-search" not in sys.argv:
        search(c, queries_log)
    drop_leaks(c)
    os.makedirs(os.path.dirname(MANIFEST), exist_ok=True)
    cols = ["id", "source", "repo", "path", "commit", "licence"]
    with open(MANIFEST, "w") as f:
        f.write("\t".join(cols) + "\n")
        for r in sorted(c.rows, key=lambda r: (r["source"], r["repo"], r["path"])):
            f.write("\t".join(r[k] for k in cols) + "\n")
    if queries_log:
        with open(QUERIES_OUT, "w") as f:
            f.write("date\tquery\tfiles_fetched\n")
            for d, q, n in queries_log:
                f.write(f"{d}\t{q}\t{n}\n")
    print(f"{len(c.rows)} configs", file=sys.stderr)


if __name__ == "__main__":
    main()
