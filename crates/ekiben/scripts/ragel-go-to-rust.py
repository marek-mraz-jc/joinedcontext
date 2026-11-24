#!/usr/bin/env python3
"""Translates the exec block of a Ragel 6 goto-style Go machine (`ragel -Z -G2`) into Rust.

Used for github.com/influxdata/go-syslog/v3 v3.0.0's rfc5424/machine.go and rfc3164/machine.go, which
Bento 1.22.0's `parse_log` calls. Every Go label becomes an arm of `loop { match lbl { .. } }`, every
`goto` sets `lbl` and continues, fall-through goes to the next label. Action code is translated by the
ACTIONS table below (each line the generator copied from machine.go.rl); an unknown line stops the
translation, so a new go-syslog version cannot slip through half translated.

usage: ragel-go-to-rust.py <machine.go> <out.rs>
"""
import hashlib
import re
import sys
from pathlib import Path

ERR = r"m\.err = fmt\.Errorf\((\w+)(?: ?\+ ?ColumnPositionTemplate)?, m\.p\)"

# Go action line -> Rust statement (None: the line opens a construct handled in parse()).
ACTIONS = {
    "m.pb = m.p": "m.pb = m.p;",
    "m.msgat = m.p": "m.msgat = m.p;",
    "(m.p)--": "m.p -= 1;",
    "output.priority = uint8(common.UnsafeUTF8DecimalCodePointsToInt(m.text()))": "out.priority = decimal(m.text()) as u8;",
    "output.version = uint16(common.UnsafeUTF8DecimalCodePointsToInt(m.text()))": "out.version = decimal(m.text()) as u16;",
    "output.prioritySet = true": "out.priority_set = true;",
    "output.timestampSet = true": "out.timestamp_set = true;",
    "output.timestamp = t": "out.timestamp = Some(t);",
    "output.timestamp = t.AddDate(m.yyyy, 0, 0)": "out.timestamp = Some(add_years(t, m.yyyy));",
    "output.timestamp = output.timestamp.In(m.loc)": "out.timestamp = out.timestamp.take().map(|t| in_location(t, loc));",
    "output.hostname = string(m.text())": "out.hostname = go_string(m.text());",
    "output.appname = string(m.text())": "out.appname = go_string(m.text());",
    "output.procID = string(m.text())": "out.proc_id = go_string(m.text());",
    "output.msgID = string(m.text())": "out.msg_id = go_string(m.text());",
    "output.tag = string(m.text())": "out.tag = go_string(m.text());",
    "output.content = string(m.text())": "out.content = go_string(m.text());",
    "output.message = string(m.text())": "out.message = go_string(m.text());",
    "output.message = string(m.data[m.msgat:m.p])": "out.message = go_string(m.slice(m.msgat, m.p));",
    "output.structuredData = map[string]map[string]string{}": "out.structured_data = BTreeMap::new();",
    "id := string(m.text())": "let id = go_string(m.text());",
    "output.structuredData[id] = map[string]string{}": "out.structured_data.insert(id.clone(), BTreeMap::new());",
    "output.hasElements = true": "out.has_elements = true;",
    "output.hasElements = false": "out.has_elements = false;",
    "m.currentelem = id": "m.currentelem = id;",
    "m.backslashat = []int{}": "m.backslashat.clear();",
    "m.backslashat = append(m.backslashat, m.p)": "m.backslashat.push(m.p);",
    "m.currentparam = string(m.text())": "m.currentparam = go_string(m.text());",
    "text := m.text()": "let mut text = m.text().to_vec();",
    "text = common.RemoveBytes(text, m.backslashat, m.pb)": "text = remove_bytes(&text, &m.backslashat, m.pb);",
    "output.structuredData[m.currentelem][m.currentparam] = string(text)": "if let Some(e) = out.structured_data.get_mut(&m.currentelem) { e.insert(m.currentparam.clone(), go_string(&text)); }",
    "delete(output.structuredData, m.currentelem)": "out.structured_data.remove(&m.currentelem);",
    "delete(output.structuredData[m.currentelem], m.currentparam)": "if let Some(e) = out.structured_data.get_mut(&m.currentelem) { e.remove(&m.currentparam); }",
    "m.err = fmt.Errorf(\"%s [col %d]\", e, m.p)": "m.err = Some(format!(\"{e} [col {}]\", m.p));",
    "m.cs = start": "m.cs = START;",
    "var _widec int16": "let mut widec: i16;",
    "_widec += 256": "widec += 256;",
}

CONDS = {
    "m.rfc3339": "m.rfc3339",
    "m.loc != nil": "let Some(loc) = &m.loc",
    "(m.p) == (m.pe)": "m.p == m.pe",
    "(m.p) == (m.eof)": "m.p == m.eof",
    "output.hasElements": "out.has_elements",
    "len(m.backslashat) > 0": "!m.backslashat.is_empty()",
    "len(output.structuredData) == 0": "out.structured_data.is_empty()",
    "len(output.structuredData) > 0": "!out.structured_data.is_empty()",
    "m.msgat > 0": "m.msgat > 0",
    "_, ok := output.structuredData[string(m.text())]; ok": "out.structured_data.contains_key(&go_string(m.text()))",
}
PARSES = {
    "t, e := time.Parse(time.Stamp, string(m.text())); e != nil": "parse_stamp(m.text())",
    "t, e := time.Parse(time.RFC3339, string(m.text())); e != nil": "parse_rfc3339(m.text())",
    "t, e := time.Parse(RFC3339MICRO, string(m.text())); e != nil": "parse_rfc3339(m.text())",
}

C = "(m.data)[(m.p)]"


def expr(e):
    e = e.strip()
    e = e.replace(C, "m.c()")
    e = re.sub(r"int16\(m\.c\(\)\)", "i16::from(m.c())", e)
    e = e.replace("_widec", "widec")
    e = e.replace(" - 0)", ")")
    # a <= x && x <= b  ->  (a..=b).contains(&x)
    m = re.fullmatch(r"(\d+) <= (\S+) && \2 <= (\d+)", e)
    if m:
        return f"({m.group(1)}..={m.group(3)}).contains(&{m.group(2)})"
    return e


class P:
    def __init__(self, lines):
        self.lines = lines
        self.i = 0

    def block(self):
        """Statements up to the closing `}` of the current block (consumed) or a case/default line (not)."""
        out = []
        while self.i < len(self.lines):
            s = self.lines[self.i].strip()
            if s == "" or s.startswith("//"):
                self.i += 1
                continue
            if s == "}" or s == "} else {":
                return out
            if re.match(r"^(case .*|default):$", s):
                return out
            self.i += 1
            out.append(self.stmt(s))
        return out

    def stmt(self, s):
        if re.fullmatch(r"[A-Za-z_]\w*:", s):
            return ("label", s[:-1])
        if s.startswith("goto "):
            return ("goto", s[5:])
        if s == "{":
            b = self.block()
            self.close()
            return ("block", b)
        if s == "if (m.p)++; (m.p) == (m.pe) {":
            return ("seq", [("raw", "m.p += 1;"), self.if_("m.p == m.pe")])
        m = re.fullmatch(r"if (.*) \{", s)
        if m:
            c = m.group(1)
            if c in PARSES:
                return self.if_(c, parse=PARSES[c])
            return self.if_(CONDS.get(c) or expr(c))
        m = re.fullmatch(r"switch(.*)\{", s)
        if m:
            return self.switch(m.group(1).strip())
        m = re.fullmatch(r"_widec = (.*)", s)
        if m:
            return ("raw", f"widec = {expr(m.group(1))};")
        m = re.fullmatch(r"m\.cs = (\d+)", s)
        if m:
            return ("raw", f"m.cs = {m.group(1)};")
        m = re.fullmatch(ERR, s)
        if m:
            name = re.sub(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])", "_", m.group(1)).upper()
            return ("raw", f"m.err = Some(col({name}, m.p));")
        if s in ACTIONS:
            return ("raw", ACTIONS[s])
        raise SystemExit(f"untranslated line {self.i}: {s}")

    def close(self):
        s = self.lines[self.i].strip()
        assert s == "}", (self.i, s)
        self.i += 1

    def if_(self, cond, parse=None):
        then = self.block()
        els = None
        if self.lines[self.i].strip() == "} else {":
            self.i += 1
            els = self.block()
        self.close()
        return ("if", cond, then, els, parse)

    def switch(self, subj):
        cases = []
        while True:
            s = self.lines[self.i].strip()
            if s == "" or s.startswith("//"):
                self.i += 1
                continue
            if s == "}":
                self.i += 1
                break
            m = re.fullmatch(r"case (.*):", s)
            self.i += 1
            if m:
                cases.append((m.group(1), self.block()))
            else:
                assert s == "default:", s
                cases.append((None, self.block()))
        return ("switch", subj, cases)


def terminates(st):
    k = st[0]
    if k == "goto":
        return True
    if k in ("block", "seq"):
        return bool(st[1]) and terminates(st[-1][-1] if k == "seq" else st[1][-1])
    return False


def runs(vals):
    """Consecutive integers as `a..=b`, as clippy's manual_range_patterns asks."""
    if not all(v.isdigit() for v in vals):
        return vals
    nums = sorted(int(v) for v in vals)
    out, a = [], 0
    for i in range(1, len(nums) + 1):
        if i == len(nums) or nums[i] != nums[i - 1] + 1:
            out.append(str(nums[a]) if i - a == 1 else f"{nums[a]}..={nums[i - 1]}")
            a = i
    return out


BYTES = frozenset(range(256))
BYTE_TEST = re.compile(r"(\d+) <= m\.c\(\)|m\.c\(\) (==|<=|>=|<|>) (\d+)|\((\d+)\.\.=(\d+)\)\.contains\(&m\.c\(\)\)")


def byte_test(cond):
    """The bytes for which `cond` holds, or None when it is not a plain test of the current byte."""
    m = BYTE_TEST.fullmatch(cond)
    if not m:
        return None
    lo, op, n, a, b = m.groups()
    if lo:
        return frozenset(range(int(lo), 256))
    if a:
        return frozenset(range(int(a), int(b) + 1))
    n = int(n)
    return frozenset(v for v in BYTES if {"==": v == n, "<=": v <= n, ">=": v >= n, "<": v < n, ">": v > n}[op])


def case_bytes(subj, vals):
    """The bytes a `case` of a switch on the current byte takes (every byte for another subject)."""
    if subj != "m.c()":
        return BYTES
    return frozenset(int(v) for v in vals.split(",") if v.strip().isdigit()) | (
        BYTES if not all(v.strip().isdigit() for v in vals.split(",")) else frozenset())


def after(st, known):
    """`known` past a statement that did not jump: a byte test or a switch on the byte whose taken
    side always jumps leaves only the bytes of the side that falls through; moving `m.p` forgets."""
    k = st[0]
    if k == "raw" and "m.p" in st[1]:
        return BYTES
    if k == "if" and not st[4] and byte_test(st[1]) is not None:
        held, then, els = byte_test(st[1]), st[2], st[3]
        if then and terminates(then[-1]) and not els:
            return known - held
        return known
    if k == "switch" and not st[1].strip():
        for cond, body in st[2]:
            held = BYTES if cond is None else byte_test(expr(cond))
            if held is None or not (body and terminates(body[-1])):
                return known
            known = known - held
        return known
    if k == "switch" and st[1].strip() and expr(st[1]) == "m.c()":
        for vals, body in st[2]:
            if vals is None or not (body and terminates(body[-1])):
                return known
            known = known - case_bytes("m.c()", vals)
        return known
    if k in ("seq", "block"):
        for s in st[1]:
            known = after(s, known)
    return known


class E:
    def __init__(self, labels):
        self.labels = labels

    def goto(self, name):
        return f"lbl = {self.labels[name]}; continue;"

    def seq(self, sts, ind, known=BYTES):
        """`known` is the set of values the current byte can still have here: Ragel's binary search
        over a byte repeats the tests an earlier `switch` already settled, and a test whose answer
        is fixed would leave a branch nothing can take, so it is emitted as its one live side."""
        out, todo = [], list(sts)
        while todo:
            st = todo.pop(0)
            if st[0] == "if" and not st[4] and byte_test(st[1]) is not None:
                held = byte_test(st[1])
                if known <= held or not known & held:
                    todo[:0] = st[2] if known <= held else st[3] or []
                    continue
            out += self.st(st, ind, known)
            if terminates(st):
                break
            known = after(st, known)
        return out

    def st(self, st, ind, known=BYTES):
        p = "    " * ind
        k = st[0]
        if k == "raw":
            return [p + st[1]]
        if k == "goto":
            return [p + self.goto(st[1])]
        if k == "seq":
            return self.seq(st[1], ind, known)
        if k == "block":
            if not st[1]:
                return []
            return [p + "{"] + self.seq(st[1], ind + 1, known) + [p + "}"]
        if k == "if":
            _, cond, then, els, parse = st
            held = byte_test(cond)
            if held is None:
                yes = no = BYTES
            else:
                yes, no = known & held, known - held
            if parse:
                return ([p + f"match {parse} {{", p + "    Err(e) => {"] + self.seq(then, ind + 2)
                        + [p + "    }", p + "    Ok(t) => {"] + self.seq(els or [], ind + 2) + [p + "    }", p + "}"])
            if cond == "m.rfc3339" and then == [("raw", "widec += 256;")] and not els:
                # Ragel's condition on a flag fixed for the whole run, in states only that flag
                # value reaches: as an expression it has no branch whose other side is dead.
                return [p + "widec += 256 * i16::from(m.rfc3339);"]
            o = [p + f"if {cond} {{"] + self.seq(then, ind + 1, yes)
            if els:
                o += [p + "} else {"] + self.seq(els, ind + 1, no)
            return o + [p + "}"]
        if k == "switch":
            _, subj, cases = st
            if subj == "":
                o, rest = [], known
                for cond, body in cases:
                    held = BYTES if cond is None else byte_test(expr(cond))
                    if held is not None and not rest & held:
                        continue
                    if held is not None and rest <= held:
                        # the last live side: an `else`, or the whole chain when it is the only one
                        if not o:
                            return self.seq(body, ind, rest)
                        return o + [p + "} else {"] + self.seq(body, ind + 1, rest) + [p + "}"]
                    kw = "if" if not o else "} else if"
                    here = rest & held if held is not None else rest
                    o += [p + f"{kw} {expr(cond)} {{"] + self.seq(body, ind + 1, here)
                    if held is None:
                        rest = BYTES
                    else:
                        rest = rest - held
                return o + [p + "}"] if o else []
            subj = {"m.cs": "m.cs"}.get(subj) or expr(subj)
            o = [p + f"match {subj} {{"]
            default, rest = [], known if subj == "m.c()" else BYTES
            for vals, body in cases:
                if vals is None:
                    default = body
                    continue
                pat = " | ".join(runs([v.strip() for v in vals.split(",")]))
                here = case_bytes(subj, vals)
                rest = rest - here
                o += [p + f"    {pat} => {{"] + self.seq(body, ind + 2, known & here) + [p + "    }"]
            o += [p + "    _ => {"] + self.seq(default, ind + 2, rest) + [p + "    }"]
            return o + [p + "}"]
        raise SystemExit(f"cannot emit {k}")


def reachable(out):
    """The emitted arms without those no `lbl = N` reaches from the entry (every jump and every
    fall-through is an explicit `lbl = N`, so the text holds the whole graph)."""
    head = re.compile(r"            (\d+) => \{ // ")
    starts = [i for i, l in enumerate(out) if head.match(l)]
    spans = {int(head.match(out[a]).group(1)): (a, b) for a, b in zip(starts, starts[1:] + [len(out)])}
    seen, todo = {0}, [0]
    while todo:
        a, b = spans[todo.pop()]
        for t in re.findall(r"lbl = (\d+)", "\n".join(out[a:b])):
            if int(t) not in seen:
                seen.add(int(t))
                todo.append(int(t))
    keep = out[:starts[0]]
    for n, (a, b) in spans.items():
        if n in seen:
            keep += out[a:b]
    return keep


def main():
    src = open(sys.argv[1]).read().splitlines()
    start = next(i for i, l in enumerate(src) if l.strip() == "m.cs = start")
    # `{ m.cs = start }` then the exec block `{ var _widec ... _out: {} }`.
    begin = next(i for i in range(start + 1, len(src)) if src[i].strip() == "{")
    p = P(src)
    p.i = begin + 1
    body = p.block()
    p.close()
    # split at labels
    arms = [("__entry", [])]
    for st in body:
        if st[0] == "label":
            arms.append((st[1], []))
        else:
            arms[-1][1].append(st)
    # exec always runs from the start state (parse sets cs = START and calls it once), so the
    # entry's resume dispatch over every state keeps only the start state's jump: the other arms
    # are code nothing can reach.
    start_state = next(re.fullmatch(r"const start int = (\d+)", l.strip()).group(1)
                       for l in src if l.strip().startswith("const start int = "))
    entry = arms[0][1]
    at = next(i for i, st in enumerate(entry) if st[0] == "switch" and st[1] == "m.cs")
    entry[at:at + 1] = next(body for vals, body in entry[at][2]
                            if vals and start_state in [v.strip() for v in vals.split(",")])
    labels = {name: n for n, (name, _) in enumerate(arms)}
    e = E(labels)
    out = [
        "//! Generated by scripts/ragel-go-to-rust.py from go-syslog v3.0.0 "
        + "/".join(sys.argv[1].split("/")[-2:]) + "; do not edit.",
        "//! Source sha256 " + hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest() + ".",
        "",
        "use super::*;",
        "",
        "#[rustfmt::skip]",
        "pub(super) fn exec(m: &mut Machine<'_>, out: &mut Output) {",
        "    let mut lbl: u16 = 0;",
        "    loop {",
        "        match lbl {",
    ]
    for n, (name, sts) in enumerate(arms):
        body = e.seq([s for s in sts if s != ("raw", "let mut widec: i16;")], 4)
        out.append(f"            {n} => {{ // {name}")
        out += body
        if not (sts and any(terminates(s) for s in sts)):
            if name == "_out":
                out.append("                break;")
            else:
                out.append(f"                lbl = {n + 1};")
        out.append("            }")
    out = reachable(out)
    # `_out` is the last arm and every other label is matched above it, so it takes the
    # catch-all the match needs instead of an arm of its own.
    last = max(i for i, l in enumerate(out) if re.match(r"            \d+ => \{ // ", l))
    assert out[last].endswith("// _out"), out[last]
    out[last] = "            _ => { // _out"
    out += ["        }", "    }", "}"]
    if any("widec" in l for l in out):
        out.insert(out.index("    let mut lbl: u16 = 0;"), "    let mut widec: i16;")
    open(sys.argv[2], "w").write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
