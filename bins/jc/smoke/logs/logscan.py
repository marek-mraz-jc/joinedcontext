"""Reading jc's log: one parsed record per line (text or JSON format), the module a line belongs
to, the secret values of a rehearsal's state, and the scans for secrets and personal data.

Every function here is pure over its inputs (tests/test_logscan.py); suite.py does the calls."""

from __future__ import annotations

import base64
import datetime as dt
import json
import re
import urllib.parse
from dataclasses import dataclass, field
from pathlib import Path

LEVELS = ("TRACE", "DEBUG", "INFO", "WARN", "ERROR")
# tracing-subscriber's text format: `TS  LEVEL span{f=v}:span{f=v}: target: message k=v ...`
TEXT = re.compile(
    r"^(?P<ts>\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z)\s+(?P<level>TRACE|DEBUG|INFO|WARN|ERROR)\s+"
    r"(?P<spans>(?:[\w-]+\{[^{}]*\}:)*)\s*(?P<target>[\w:-]+): (?P<rest>.*)$")
SPAN_ID = re.compile(r"\brequest_id=\"?([A-Za-z0-9-]{8,128})")
SPAN_MODULE = re.compile(r"\bmodule=\"?([a-z]+)")
# The module each log target belongs to; the first matching prefix wins. jc's own targets name the
# module they wire (jc::modules::<m>); jc::access names it in its `module` field.
MODULE_PREFIXES = (
    ("jc::modules::portal", "portal"), ("jc::modules::gateway", "gateway"),
    ("jc::modules::antares", "antares"), ("jc::modules::shako", "shako"),
    ("jc::modules::ekiben", "ekiben"), ("jc::modules::kaisatsu", "kaisatsu"),
    ("jc::modules::daiya", "daiya"), ("jc::modules::dataroom", "dataroom"),
    ("joinedcontext_portal", "portal"), ("context_gateway", "gateway"),
    ("antares", "antares"), ("shako", "shako"), ("ekiben", "ekiben"), ("kaisatsu", "kaisatsu"),
    ("daiya", "daiya"), ("dataroom", "dataroom"), ("jc", "jc"),
)
MODULES = ("portal", "gateway", "antares", "shako", "ekiben", "kaisatsu", "daiya", "dataroom")


@dataclass
class Record:
    n: int                     # line number, 1-based
    ts: dt.datetime | None
    level: str
    target: str
    message: str
    fields: dict = field(default_factory=dict)
    request_id: str | None = None
    raw: str = ""
    structured: bool = True
    span_module: str | None = None  # the module of jc's request span the line was written in

    @property
    def module(self) -> str:
        if self.target == "jc::access" and self.fields.get("module"):
            return str(self.fields["module"])
        own = module_of(self.target)
        # A shared library (jc_core, sqlx, rmcp) logs for the module whose request it serves.
        return self.span_module if own in ("jc", "other") and self.span_module else own


def module_of(target: str) -> str:
    for prefix, module in MODULE_PREFIXES:
        if target == prefix or target.startswith(prefix + "::") or target.startswith(prefix + "_"):
            return module
    return "other"


def parse_time(s: str) -> dt.datetime | None:
    try:
        s = s.replace("Z", "+00:00")
        if "." in s:  # Python takes at most 6 fraction digits
            head, frac = s.split(".", 1)
            digits = re.match(r"\d+", frac).group(0)
            s = f"{head}.{digits[:6]}{frac[len(digits):]}"
        return dt.datetime.fromisoformat(s)
    except (ValueError, AttributeError):
        return None


FIELD = re.compile(r'(?:^|\s)([A-Za-z_][\w.]*)=("(?:[^"\\]|\\.)*"|\S+)')


def text_fields(rest: str) -> tuple[str, dict]:
    """The message and the `k=v` fields after it (a field value may be quoted)."""
    m = FIELD.search(rest)
    msg = rest[:m.start()].strip() if m else rest.strip()
    fields = {}
    for k, v in FIELD.findall(rest[m.start():] if m else ""):
        fields[k] = v[1:-1] if v.startswith('"') else v
    return msg, fields


def parse_line(n: int, line: str) -> Record:
    line = line.rstrip("\n")
    if line.startswith("{"):
        try:
            doc = json.loads(line)
        except ValueError:
            doc = None
        if isinstance(doc, dict) and doc.get("level") and doc.get("target"):
            f = dict(doc.get("fields") or {})
            msg = str(f.pop("message", ""))
            rid, mod = None, None
            for span in list(doc.get("spans") or []) + [doc.get("span") or {}]:
                if isinstance(span, dict) and span.get("request_id"):
                    rid = str(span["request_id"])
                if isinstance(span, dict) and span.get("module"):
                    mod = str(span["module"])
            rid = rid or (str(f["request_id"]) if f.get("request_id") else None)
            return Record(n, parse_time(str(doc.get("timestamp", ""))), str(doc["level"]).upper(),
                          str(doc["target"]), msg, f, rid, line, span_module=mod)
    m = TEXT.match(line)
    if not m:
        return Record(n, None, "", "", "", {}, None, line, structured=False)
    msg, fields = text_fields(m.group("rest"))
    rid = None
    for found in SPAN_ID.finditer(m.group("spans")):
        rid = found.group(1)  # the innermost request span names it
    rid = rid or fields.get("request_id")
    mods = SPAN_MODULE.findall(m.group("spans"))
    return Record(n, parse_time(m.group("ts")), m.group("level"), m.group("target"), msg, fields, rid, line,
                  span_module=mods[-1] if mods else None)


def read_records(path: Path, start: int = 0) -> tuple[list[Record], int]:
    """The records from byte offset `start` on, and the offset after the last full line."""
    with open(path, "rb") as f:
        f.seek(start)
        data = f.read()
    end = data.rfind(b"\n") + 1
    out = []
    for i, line in enumerate(data[:end].decode(errors="replace").split("\n")[:-1], 1):
        if line.strip():
            out.append(parse_line(i, line))
    return out, start + end


# ---------------------------------------------------------------- secrets and personal data

MIN_SECRET = 12
SECRET_KEYS = re.compile(r"secret|token|password|passwd|key|credential", re.I)
DB_URL_PW = re.compile(r"^[a-z][a-z0-9+.-]*://[^:/@\s]+:([^@\s]+)@")


@dataclass(frozen=True)
class Secret:
    name: str   # the file (and key) it came from, never the value
    value: str


def secrets_of_file(path: Path, label: str) -> list[Secret]:
    try:
        raw = path.read_bytes()
    except OSError:
        return []
    if len(raw) > 65536:
        return []
    text = raw.decode(errors="replace").strip()
    out: list[Secret] = []
    try:
        doc = json.loads(text)
    except ValueError:
        doc = None
    if isinstance(doc, (dict, list)):
        def walk(v, key=""):
            if isinstance(v, dict):
                for k, x in v.items():
                    walk(x, str(k))
            elif isinstance(v, list):
                for x in v:
                    walk(x, key)
            elif isinstance(v, str) and SECRET_KEYS.search(key) and len(v) >= MIN_SECRET:
                out.append(Secret(f"{label}:{key}", v))
        walk(doc)
        return out
    for line in text.splitlines():
        line = line.strip()
        m = DB_URL_PW.match(line)
        if m:
            out.append(Secret(label, urllib.parse.unquote(m.group(1))))
        elif "://" in line:
            continue  # an address without a password is no secret
        elif len(line) >= MIN_SECRET and not line.startswith("-----"):
            out.append(Secret(label, line))
        elif line.startswith("AGE-SECRET-KEY-") or line.startswith("-----BEGIN"):
            out.append(Secret(label, line))
    # An age identity file: the key line, not the comments.
    return [s for s in out if not s.value.startswith("#")]


def collect_secrets(dirs: list[Path]) -> list[Secret]:
    """The secret values of the files directly in each dir (a state dir's subdirectories hold
    checkouts and caches, not secrets)."""
    found: dict[str, Secret] = {}
    for d in dirs:
        if not d.is_dir():
            continue
        for p in sorted(d.iterdir()):
            if p.is_file() and not p.is_symlink():
                for s in secrets_of_file(p, f"{d.name}/{p.relative_to(d)}"):
                    found.setdefault(s.value, s)
    return list(found.values())


def secret_forms(value: str) -> set[str]:
    forms = {value}
    q = urllib.parse.quote(value, safe="")
    if q != value:
        forms.add(q)
    b = value.encode()
    if len(b) >= 12:
        forms.add(base64.b64encode(b).decode().rstrip("="))
        forms.add(base64.urlsafe_b64encode(b).decode().rstrip("="))
    return {f for f in forms if len(f) >= MIN_SECRET}


class SecretIndex:
    """Finds any form of any secret in a text in one pass: a dict from each form's first K
    characters to the forms, probed at every position of every run of token characters."""

    RUN = re.compile(r"[A-Za-z0-9+/=_.~%-]{12,}")

    def __init__(self, secrets: list[Secret]):
        self.by_prefix: dict[str, list[tuple[str, Secret]]] = {}
        self.k = MIN_SECRET
        for s in secrets:
            for f in secret_forms(s.value):
                self.by_prefix.setdefault(f[:self.k], []).append((f, s))

    def find(self, text: str) -> list[Secret]:
        hits: dict[str, Secret] = {}
        if not self.by_prefix:
            return []
        for run in self.RUN.finditer(text):
            r = run.group(0)
            for i in range(len(r) - self.k + 1):
                for form, s in self.by_prefix.get(r[i:i + self.k], ()):
                    if r.startswith(form, i):
                        hits[s.name] = s
        return list(hits.values())


# A bearer credential printed anywhere: a JWT, an `Authorization: Bearer` value, a password field.
JWT = re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")
# A challenge's auth-param (`401 Bearer resource_metadata="…"`, RFC 6750 and RFC 9728) is a name, not a
# value: only those names, each followed by `=`, are skipped.
BEARER = re.compile(r"(?i)\bbearer\s+(?!(?:realm|scope|error|error_description|error_uri|resource_metadata)=)"
                    r"[A-Za-z0-9._~+/-]{16,}")
PW_FIELD = re.compile(r"(?i)\b(?:password|passwd|client_secret|secret|token)\s*[=:]\s*\"?([^\s\",}]{8,})")
REDACTED = re.compile(r"(?i)^(?:\*+|<redacted>|\[redacted\]|redacted|xxx+|\.\.\.)$")
EMAIL = re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b")
IPV4 = re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b")
# Addresses a log may name: services, not people.
SERVICE_EMAIL = re.compile(r"(?i)^(?:noreply|no-reply|postmaster|admin|root|git)@")
# Package versions (`crate@1.2.3`) and similar are not addresses.
NOT_EMAIL = re.compile(r"@\d")


def credential_shapes(text: str) -> list[str]:
    """What kind of credential a text prints, by shape (never the value)."""
    kinds = []
    if JWT.search(text):
        kinds.append("jwt")
    if BEARER.search(text):
        kinds.append("bearer")
    for m in PW_FIELD.finditer(text):
        if not REDACTED.match(m.group(1)) and not m.group(1).startswith("$("):
            kinds.append("password-field")
            break
    return kinds


def mask_email(e: str) -> str:
    local, _, domain = e.partition("@")
    return f"{local[:1]}***@{domain}"


def personal_data(text: str) -> list[str]:
    """E-mail addresses and non-local IPv4 addresses a text names, masked."""
    out = []
    for m in EMAIL.finditer(text):
        e = m.group(0)
        if SERVICE_EMAIL.match(e) or NOT_EMAIL.search(e):
            continue
        out.append("email " + mask_email(e))
    for m in IPV4.finditer(text):
        parts = [int(p) for p in m.group(0).split(".")]
        if any(p > 255 for p in parts):
            continue
        a, b = parts[0], parts[1]
        private = a in (0, 10, 127) or (a == 172 and 16 <= b <= 31) or (a == 192 and b == 168) \
            or (a == 169 and b == 254) or a >= 224
        if not private:
            out.append(f"ip {a}.{b}.x.x")
    return out


# ---------------------------------------------------------------- volume

def volume(records: list[Record], minutes: float | None = None) -> dict:
    """Per module: lines, bytes, minutes covered, lines and bytes per minute, the most repeated
    message (as `target: message`) and how often it came. `minutes` is the window the records
    were read over; without it, the span from the first line to the last."""
    per: dict[str, dict] = {}
    for r in records:
        if not r.structured or r.ts is None:
            continue
        v = per.setdefault(r.module, {"lines": 0, "bytes": 0, "first": r.ts, "last": r.ts, "msgs": {}})
        v["lines"] += 1
        v["bytes"] += len(r.raw.encode()) + 1
        v["first"], v["last"] = min(v["first"], r.ts), max(v["last"], r.ts)
        key = f"{r.target}: {r.message[:80]}"
        v["msgs"][key] = v["msgs"].get(key, 0) + 1
    span_all = [r.ts for r in records if r.ts]
    total_min = max((max(span_all) - min(span_all)).total_seconds() / 60, 1 / 60) if span_all else 0
    if minutes:
        total_min = minutes
    out = {}
    for m, v in per.items():
        top = max(v["msgs"].items(), key=lambda kv: kv[1])
        out[m] = {"lines": v["lines"], "bytes": v["bytes"], "minutes": round(total_min, 2),
                  "linesPerMinute": round(v["lines"] / total_min, 1) if total_min else 0,
                  "bytesPerMinute": round(v["bytes"] / total_min) if total_min else 0,
                  "topMessage": top[0], "topCount": top[1]}
    return out


def request_lines(records: list[Record], bound: int) -> dict:
    """Lines per request id across every module: how many requests, the mean, p95 and max, how
    many wrote more than `bound`, and the most common shape of those (`module target` per line)."""
    per: dict[str, list[Record]] = {}
    for r in records:
        if r.structured and r.request_id:
            per.setdefault(r.request_id, []).append(r)
    counts = sorted(len(v) for v in per.values())
    if not counts:
        return {"requests": 0, "mean": 0, "p95": 0, "max": 0, "over": 0, "shape": ""}
    shapes: dict[str, int] = {}
    for rs in per.values():
        if len(rs) > bound:
            key = ", ".join(sorted(f"{r.module} {r.target}" for r in rs))
            shapes[key] = shapes.get(key, 0) + 1
    return {"requests": len(counts), "mean": round(sum(counts) / len(counts), 2),
            "p95": counts[int(len(counts) * 0.95)], "max": counts[-1],
            "over": sum(shapes.values()), "shape": max(shapes.items(), key=lambda kv: kv[1])[0] if shapes else ""}

