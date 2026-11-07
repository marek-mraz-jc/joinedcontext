#!/usr/bin/env python3
"""Verdict per Coraine functional case run against Antares (T-3549).

corTest compares a case's output byte for byte with its --EXPECT-- (REGEX()
aside). The expectations were captured from Coraine, so they carry its
Date, Content-Length, header set and ProblemDetails wording. This script
reads what `corTest --keep` left beside each case and gives one verdict:

  pass             corTest's own diff is empty
  pass-normalised  equal once Coraine-only text is set aside: the status code,
                   Location, Content-Type media type, NGSILD-Results-Count and
                   Link headers, every JSON body member (ProblemDetails detail
                   and title excepted, the id of an EntityTypeList or AttributeList
                   excepted, a NotUpdatedDetails reason excepted, numbers to 1e-6,
                   arrays in any order; in a request a receiver dumped, the
                   client's transport headers excepted) and every other output
                   line are compared; Link headers as a set
  fail             anything else (the first difference is reported)
  not-applicable   named in not-applicable.txt by tag or name, or the case
                   started the broker with a Coraine-only flag (antares.sh
                   prints "CORAINE-ONLY <flag>"), or the case pins one Coraine
                   storage backend (REQUIRE_DB)
  error            INIT or RUN wrote stderr or exited non-zero
  skipped          corTest did not run it (its own SKIP_ or skip list)

Usage: compare.py <cases-dir> <not-applicable.txt> <store> <out-dir>
Writes <out-dir>/coraine-report.json and coraine-report.md; exits 0.
"""
import json
import os
import re
import sys

KEEP_HEADERS = ("location", "content-type", "ngsild-results-count", "link")
PROBLEM_PREFIX = "https://uri.etsi.org/ngsi-ld/errors/"
RX = "\x00RX:"  # an expected JSON value that was a bare REGEX(...)
MARKER = "CORAINE-ONLY "
LIST_TYPES = ("EntityTypeList", "AttributeList")
HEADER_LINE = re.compile(r"([A-Za-z][A-Za-z0-9-]*):\s*(.*)")
# what an HTTP client adds on its own to a request a receiver dumps (RFC 9110)
TRANSPORT_HEADERS = ("user-agent", "host", "content-length", "accept", "accept-encoding",
                     "connection", "ngsild-attribute-format")
ONE_BACKEND = re.compile(r"^# REQUIRE_DB:\s*(\S+)\s*$", re.M)


def regex_end(s, start):
    """Index of the ')' closing a REGEX( whose pattern starts at s[start]; -1 if none."""
    depth, i, n = 1, start, len(s)
    while i < n:
        c = s[i]
        if c == "\\":
            i += 2
            continue
        if c == "[":
            i += 1
            if i < n and s[i] == "^":
                i += 1
            if i < n and s[i] == "]":
                i += 1
            while i < n and s[i] != "]":
                i += 2 if s[i] == "\\" else 1
            i += 1
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def text_matches(expected, actual):
    """corDiff's rule: literal text, with any number of REGEX(pattern) parts."""
    if "REGEX(" not in expected:
        return expected == actual
    parts, pos = [], 0
    while True:
        k = expected.find("REGEX(", pos)
        if k < 0:
            break
        end = regex_end(expected, k + 6)
        if end < 0:
            return False
        parts.append(re.escape(expected[pos:k]))
        parts.append("(?:" + expected[k + 6:end] + ")")
        pos = end + 1
    parts.append(re.escape(expected[pos:]))
    try:
        return re.fullmatch("".join(parts), actual, re.S) is not None
    except re.error:
        return False


def quote_bare_regex(body):
    """JSON text in which every REGEX(...) outside a string becomes a marked string."""
    out, i, n, in_str = [], 0, len(body), False
    while i < n:
        c = body[i]
        if in_str:
            out.append(c)
            if c == "\\" and i + 1 < n:
                # a regex escape such as \d is no JSON escape: keep it literal
                if body[i + 1] not in '"\\/bfnrtu':
                    out.append("\\")
                out.append(body[i + 1])
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
        elif body.startswith("REGEX(", i):
            end = regex_end(body, i + 6)
            if end > 0:
                # digits written before the REGEX are part of the same number
                glued = ""
                while out and (out[-1].isdigit() or out[-1] == "."):
                    glued = out.pop() + glued
                out.append(json.dumps(RX + re.escape(glued) + body[i + 6:end]))
                i = end + 1
                # REGEX(.*) at a line end matched the member's comma as well
                rest = body[i:]
                if re.match(r"[ \t]*\n\s*\"", rest):
                    out.append(",")
                continue
        out.append(c)
        i += 1
    return "".join(out)


def items(text, expected):
    """The comparable items of one output: status, kept headers, JSON bodies, text lines."""
    lines = text.split("\n")
    res, i, n = [], 0, len(lines)
    decoder = json.JSONDecoder()
    while i < n:
        line = lines[i].rstrip("\r")
        if re.match(r"HTTP/[0-9.]+ \d{3}", line):
            res.append(("status", line.split()[1]))
            i += 1
            links = []
            while i < n and lines[i].strip():
                name, _, value = lines[i].partition(":")
                lname = name.strip().lower()
                if lname == "link":
                    # RFC 8288 3: one header may join several link-values with commas
                    links += [("header", "link", link_value(v)) for v in re.split(r",\s*(?=<)", value)]
                elif lname in KEEP_HEADERS:
                    res.append(("header", lname, header_value(name, value)))
                i += 1
            res.extend(sorted(links))  # RFC 8288 orders no links
            continue
        if line.lstrip()[:1] in ("{", "["):
            rest = "\n".join(lines[i:])
            src = quote_bare_regex(rest) if expected else rest
            try:
                obj, end = decoder.raw_decode(src.lstrip())
            except ValueError:
                obj = None
            if obj is not None:
                # quote_bare_regex never adds or removes a newline
                res.append(("json", obj))
                i += src.lstrip()[:end].count("\n") + 1
                continue
        if line.strip():
            res.append(("text", line.rstrip()))
        i += 1
    return res


def header_value(name, value):
    value = value.strip()
    lname = name.strip().lower()
    if lname == "content-type":
        return value.split(";")[0].strip().lower()
    if lname == "location":
        return re.sub(r"^[a-z]+://[^/]+", "", value)
    return value


def link_value(value):
    """One Link header value with RFC 8288 spacing, and the query of its target in a fixed order."""
    value = re.sub(r"\s*;\s*", "; ", value.strip())
    m = re.match(r"<([^>?]*)\?([^>]*)>(.*)", value)
    if m and "REGEX(" not in value:
        value = f"<{m[1]}?{'&'.join(sorted(m[2].split('&')))}>{m[3]}"
    return value


def json_equal(exp, act):
    if isinstance(exp, str) and exp.startswith(RX):
        # REGEX("...") was written against the JSON text, quotes included
        quoted = exp[len(RX):].startswith('"')
        shown = act if isinstance(act, str) and not quoted else json.dumps(act)
        return text_matches("REGEX(" + exp[len(RX):] + ")", shown)
    if isinstance(exp, bool) or isinstance(act, bool):
        return exp is act
    if isinstance(exp, (int, float)) and isinstance(act, (int, float)):
        return abs(exp - act) <= 1e-6 * max(1.0, abs(exp))
    if isinstance(exp, str) and isinstance(act, str):
        return text_matches(exp, act)
    if isinstance(exp, dict) and isinstance(act, dict):
        drop = set()
        if str(exp.get("type", "")).startswith(PROBLEM_PREFIX):
            drop = {"detail", "title"}
        elif exp.get("type") in LIST_TYPES:
            drop = {"id"}  # 5.2.6.10: "URI that is unique within the system scope"
        elif "attributeName" in exp and "reason" in exp:
            drop = {"reason"}  # 5.2.19 NotUpdatedDetails: free text
        if "verb" in exp and isinstance(exp.get("headers"), dict) and isinstance(act.get("headers"), dict):
            # a request a receiver dumped: header names are case-insensitive (RFC 9110 5.1)
            ah = {k.lower(): v for k, v in act["headers"].items()}
            for k, v in exp["headers"].items():
                if k.lower() not in TRANSPORT_HEADERS and not (k.lower() in ah and json_equal(v, ah[k.lower()])):
                    return False
            drop = {"headers"}
        ek, ak = set(exp) - drop, set(act) - drop
        return ek == ak and all(json_equal(exp[k], act[k]) for k in ek)
    if isinstance(exp, list) and isinstance(act, list):
        if len(exp) != len(act):
            return False
        if all(json_equal(e, a) for e, a in zip(exp, act)):
            return True
        # ponytail: greedy O(n²) match, arrays in the suite hold a few dozen members at most
        left = list(act)
        for e in exp:
            k = next((j for j, a in enumerate(left) if json_equal(e, a)), None)
            if k is None:
                return False
            left.pop(k)
        return True
    return exp == act


def item_equal(exp, act):
    if exp[0] != act[0]:
        return False
    if exp[0] == "json":
        return json_equal(exp[1], act[1])
    if exp[0] == "header":
        return exp[1] == act[1] and text_matches(exp[2], act[2])
    # a kept header a case prints as text: the name is case-insensitive (RFC 9110 5.1)
    e, a = HEADER_LINE.match(exp[-1]), HEADER_LINE.match(act[-1])
    if e and a and e[1].lower() == a[1].lower() and e[1].lower() in KEEP_HEADERS:
        return text_matches(e[2], a[2])
    return text_matches(exp[-1], act[-1])


def normalised_diff(expected, actual):
    """None when equal, else a one-line description of the first difference."""
    e, a = items(expected, True), items(actual, False)
    for k, (x, y) in enumerate(zip(e, a)):
        if not item_equal(x, y):
            return f"item {k + 1}: expected {short(x)}, got {short(y)}"
    if len(e) != len(a):
        extra = e[len(a)] if len(e) > len(a) else a[len(e)]
        side = "missing" if len(e) > len(a) else "unexpected"
        return f"item {min(len(e), len(a)) + 1}: {side} {short(extra)}"
    return None


def short(item):
    s = json.dumps(list(item), ensure_ascii=False).replace(RX, "REGEX:")
    return s if len(s) <= 160 else s[:157] + "..."


def read(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except FileNotFoundError:
        return None


def section(test_text, name):
    m = re.search(rf"^--{name}--\n(.*?)(?=^--|\Z)", test_text, re.S | re.M)
    return m.group(1).strip() if m else ""


def load_na(path, store):
    """The tag and name entries that apply to this store's cell."""
    tags, names = {}, {}
    for raw in (read(path) or "").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, _, reason = line.partition(" ")
        kind, _, value = key.partition(":")
        kind, _, only = kind.partition("@")
        if only and only != store:
            continue
        if kind == "tag":
            tags[value] = reason.strip()
        elif kind == "name":
            names[value] = reason.strip()
        else:
            raise ValueError(f"{path}: bad line {raw!r}")
    return tags, names


def verdict(prefix, test_text, na_tags, na_names):
    base = os.path.basename(prefix)
    tags = section(test_text, "TAGS").split()
    for t in tags:
        if t in na_tags:
            return "not-applicable", f"tag {t}: {na_tags[t]}"
    if base in na_names:
        return "not-applicable", na_names[base]
    one = ONE_BACKEND.search(test_text)
    if one:
        return "not-applicable", f"pins one Coraine storage backend (REQUIRE_DB: {one[1]})"
    errs = [(p, read(f"{prefix}.{p}.stderr") or "") for p in ("init", "run", "teardown")]
    for _, err in errs:
        m = re.search(MARKER + r"(\S+)", err)
        if m:
            return "not-applicable", f"started with the Coraine-only flag {m.group(1)}"
    out, diff = read(f"{prefix}.out"), read(f"{prefix}.diff")
    for part, err in errs[:2]:
        if err.strip():
            return "error", f"{part.upper()} stderr: {err.strip().splitlines()[0][:160]}"
    if out is None:
        if read(f"{prefix}.init") is not None:
            return "error", "INIT failed before RUN"
        return "skipped", "not run by corTest"
    if diff is not None and not diff.strip():
        return "pass", ""
    d = normalised_diff(read(f"{prefix}.expect") or "", out)
    return ("pass-normalised", "") if d is None else ("fail", d)


def main(cases, na_path, store, out_dir):
    na_tags, na_names = load_na(na_path, store)
    rows = []
    for name in sorted(os.listdir(cases)):
        if not name.endswith(".test"):
            continue
        prefix = os.path.join(cases, name[:-5])
        text = read(prefix + ".test") or ""
        v, why = verdict(prefix, text, na_tags, na_names)
        rows.append({"case": name[:-5], "verdict": v, "reason": why,
                     "tags": section(text, "TAGS").split()})
    counts = {}
    for r in rows:
        counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, "coraine-report.json"), "w") as fh:
        json.dump({"counts": counts, "cases": rows}, fh, indent=1)
    order = ["pass", "pass-normalised", "fail", "error", "not-applicable", "skipped"]
    with open(os.path.join(out_dir, "coraine-report.md"), "w") as fh:
        fh.write(f"cases: {len(rows)}; " +
                 ", ".join(f"{k} {counts.get(k, 0)}" for k in order) + "\n\n")
        fh.write("| case | verdict | reason |\n|---|---|---|\n")
        for r in rows:
            if r["verdict"] in ("fail", "error"):
                reason = r["reason"].replace("|", "\\|")
                fh.write(f"| {r['case']} | {r['verdict']} | {reason} |\n")
    return counts


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    print(json.dumps(main(*sys.argv[1:])))
