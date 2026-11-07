#!/usr/bin/env python3
"""E2/E4 gate (surfaces-conformance.md 2.3, T-3501): the ETSI suite through a
view answers TP by TP as the plain root does (E1), except where the view owes
a refusal.

A refusal is owed, not hand-listed: a TP whose body sent a request the view
does not offer (its operation, from operation-matrix.yaml's NGSI-LD binding,
is not in the view's `operations`; ADR-0023 rule 4), a request no operation
binds, a request naming an NGSILD-Tenant (rule 10) or an entity map (threat
V22), and, for a view that does not show every name, a request naming a type
outside its selector (rule 3) or an Attribute no rule shows (rule 2) in its
`type`, `attrs`, `pick`, `omit`, `orderBy` or `q`, in the JSON body it sent
(T-3787), in its path (`/types/{type}`, `/attributes/{attr}`) or as the type
of an entity id it reads (the suite's ids are `urn:ngsi-ld:{Type}:…`, T-3980).
Such a view also owes its narrowing (T-3980): a discovery listing names only
what it shows, a join follows Relationships only into entities it shows, a
query that names no type answers only the view's types, and
an answer may lack an Attribute it does not show, when the TP's failure
message (failures.csv) names only such Attributes as missing;
and an Entity write or a Subscription in a TP whose fixtures registered a
Context Source, which no write through a view reaches (rule 7 is checked on the
local store, so the write stays local; rule 9: a view's Subscription has no
distributed half; T-3945); and a request naming an Entity that an earlier
TP's body created only through a request the view owes a refusal, so it never
existed through the view (the restricted view refuses 422_01_05's create, and
422_01_06 reads that Building; T-3982).
Names are expanded as the suite sends them: its own @context, then the core
one, then the default vocabulary. The body requests come from the run's
requests.jsonl (dev/etsi_setup_url.py records them, and, marked, the fixtures').

Usage:
  etsi-view-diff.py <view.json> <e1-dir> <e2-dir>

Every other difference fails, unless dev/etsi-view-known.json names the TP
under the view's id with the task that fixes it; a named TP that no longer
differs fails too (remove it). The owed refusals are written to
<e2-dir>/owed-refusals.json. KNOWN overrides the known file (the selftest).
"""
import csv
import glob
import json
import os
import re
import sys
from urllib.parse import parse_qs, unquote, urlsplit

HERE = os.path.dirname(os.path.abspath(__file__))
MATRIX = f"{HERE}/../docs/analysis/operation-matrix.yaml"
KNOWN = os.environ.get("KNOWN") or f"{HERE}/etsi-view-known.json"
ROOT = "/ngsi-ld/v1"
CORE_CONTEXT = f"{HERE}/../crates/antares-jsonld/contexts/core-v1.9.jsonld"
DEFAULT_VOCAB = "https://uri.etsi.org/ngsi-ld/default-context/"
# ngsi-ld-test-suite resources/jsonld-contexts/ngsi-ld-test-suite.jsonld, the
# terms of the @context every TP sends (through -compound.jsonld, before core)
SUITE_CONTEXT = {t: f"https://ngsi-ld-test-suite/context#{t}" for t in (
    "Building", "Vehicle", "OffStreetParking", "airQualityLevel", "almostFull",
    "availableSpotsNumber", "brandName", "fuelLevel", "isParked", "name",
    "reliability", "source", "speed", "totalSpotsNumber", "subCategory")}
# members of an Entity or a request body that are no Attribute (CIM 009 5.2)
NOT_ATTRIBUTES = {"id", "@id", "type", "@type", "@context", "scope", "createdAt",
                  "modifiedAt", "deletedAt", "expiresAt"}
# the discovery listings a view narrows to the names it shows (ADR-0023 rules 2 and 3)
LISTINGS = {"retrieve_entity_types", "retrieve_entity_type_details", "retrieve_entity_type_info",
            "retrieve_attributes", "retrieve_attribute_details", "retrieve_attribute_info"}
# the queries a view narrows to its selector's types when they name none (ADR-0023 rule 3)
QUERIES = {"query_entities", "query_entities_post", "temporal_query", "temporal_query_post"}
# the type segment of the suite's entity ids; an id that says nothing names no type
ID_TYPE = re.compile(r"urn:ngsi-ld:([A-Za-z][\w-]*):")
# what a failure message says is missing from an answer: a DeepDiff removal, a key not found
MISSING = re.compile(r"Item root\[(?:'[^']*'\]\[)*'([^']+)'\] removed from dictionary"
                     r"|has no key '([^']+)'|KeyError: '([^']+)'|does not contain '([^']+)'")
# request bodies that name entities and attributes rather than being one
SELECTORS = {"Query", "Subscription", "ContextSourceRegistration", "CSourceRegistration"}

sys.path.insert(0, HERE)
from importlib import import_module  # noqa: E402

baseline = import_module("etsi-baseline")


def bindings(matrix_path=MATRIX):
    """[(op, methods, segments, query)] from the matrix's NGSI-LD bindings;
    broker-initiated ones (notifications) bind no request the suite sends."""
    import yaml

    out = []
    for op in yaml.safe_load(open(matrix_path))["operations"]:
        b = (op.get("surfaces") or {}).get("ngsi_ld_http") or {}
        text = b.get("binding") or ""
        if b.get("status") != "supported" or "{notification" in text \
                or "notification endpoint" in text:
            continue
        methods, _, paths = text.partition(" ")
        first = paths.split(" and ")[0]
        for path in paths.split(" and "):
            path = path.replace("...", first.rsplit("/attrs", 1)[0]) if path.startswith("...") else path
            path, _, query = path.partition("?")
            segs = path.removeprefix(ROOT).strip("/").split("/")
            out.append((op["id"], set(methods.split("|")), segs, dict(q.split("=") for q in query.split("&") if q)))
    return out


def operation(method, url, binds):
    """The operation a request is, or None. Literal segments and a matched
    query beat placeholders (`/types?details=true` over `/types`)."""
    parts = urlsplit(url)
    path = parts.path
    if ROOT not in path:
        return None
    segs = path.split(ROOT, 1)[1].strip("/").split("/")
    query = {k: v[-1] for k, v in parse_qs(parts.query).items()}
    best, score = None, -1
    for op, methods, pattern, wants in binds:
        if method not in methods or len(pattern) != len(segs):
            continue
        if any(not p.startswith("{") and p != s for p, s in zip(pattern, segs)):
            continue
        if any(query.get(k) != v for k, v in wants.items()):
            continue
        s = sum(not p.startswith("{") for p in pattern) + 2 * len(wants)
        if s > score:
            best, score = op, s
    return best


# A view serves only the Subscriptions made through it (ADR-0023 rule 9).
OWN_SUBSCRIPTIONS = {"retrieve_subscription", "query_subscriptions",
                     "update_subscription", "delete_subscription"}


def core_terms(path=CORE_CONTEXT):
    """term -> IRI of the core @context, its compact IRIs resolved."""
    ctx = json.load(open(path))["@context"]
    plain = {k: v if isinstance(v, str) else v.get("@id") if isinstance(v, dict) else None
             for k, v in ctx.items()}
    out = {}
    for term, iri in plain.items():
        if not iri or term.startswith("@"):
            continue
        prefix, sep, rest = iri.partition(":")
        out[term] = plain[prefix] + rest if sep and prefix in plain and not rest.startswith("//") else iri
    return out


def expand(name, core):
    """The IRI the broker reads `name` as under the suite's @context."""
    if "://" in name or name.startswith("urn:"):
        return name
    return SUITE_CONTEXT.get(name) or core.get(name) or DEFAULT_VOCAB + name


def q_attributes(q):
    """The Attribute names a `q` (CIM 009 4.9) reads: the first path segment
    of every term, before its operator."""
    names = set()
    for term in re.split(r"[;|()]", q or ""):
        m = re.match(r"\s*!?\s*([^=!<>~\[.\s]+)", term)
        if m and not m.group(1).startswith(("'", '"')):
            names.add(m.group(1))
    return names


def named(url, body):
    """(types, attributes) a request names, in its query and its body."""
    query = parse_qs(urlsplit(url).query)
    first = lambda key: ",".join(query.get(key, []))
    types = {t for t in re.split(r"[,;|()]", first("type")) if t.strip()}
    attrs = {a for key in ("attrs", "pick", "omit") for a in first(key).split(",") if a.strip()}
    attrs |= {re.split(r"[;\s]", a.strip())[0] for a in first("orderBy").split(",") if a.strip()}
    attrs |= q_attributes(first("q"))
    segs = [unquote(s) for s in urlsplit(url).path.split(ROOT, 1)[-1].strip("/").split("/")]
    if len(segs) == 2 and segs[0] == "types":
        types.add(segs[1])
    if len(segs) == 2 and segs[0] == "attributes":
        attrs.add(segs[1])
    ids = query.get("id", [""])[0].split(",")
    ids += [segs[i + 1] for i, s in enumerate(segs[:-1]) if s == "entities"]
    types |= {m.group(1) for m in map(ID_TYPE.match, ids) if m}
    for item in (body if isinstance(body, list) else [body]):
        if not isinstance(item, dict):
            continue
        kind = item.get("type")
        if kind in SELECTORS:
            for e in item.get("entities") or []:
                if isinstance(e, dict) and isinstance(e.get("type"), str):
                    types |= {t for t in re.split(r"[,;|()]", e["type"]) if t.strip()}
            for key in ("attrs", "watchedAttributes"):
                attrs |= {a for a in item.get(key) or [] if isinstance(a, str)}
            attrs |= {a for a in (item.get("notification") or {}).get("attributes") or [] if isinstance(a, str)}
            attrs |= q_attributes(item.get("q") if isinstance(item.get("q"), str) else "")
        else:
            kinds = kind if isinstance(kind, list) else [kind]
            types |= {t for t in kinds if isinstance(t, str)}
            attrs |= {k for k in item if k not in NOT_ATTRIBUTES}
    return {t.strip() for t in types}, {a.strip() for a in attrs}


def hidden_by(view, core):
    """A function naming what of a request the view hides, or None for a view
    that shows every name."""
    selector = set(view.get("selector") or [])
    shown = {a for rule in view.get("rules") or [] for a in rule.get("attributes") or []}
    if "*" in selector and "*" in shown:
        return None

    def hides(attribute):
        return "*" not in shown and expand(attribute, core) not in shown

    def hidden(url, body, op=None):
        types, attrs = named(url, body)
        why = [f"a query naming no type answers only the view's types (ADR-0023 rule 3)"] \
            if op in QUERIES and not types and "*" not in selector else []
        why += [f"type {t} outside the view (ADR-0023 rule 3)"
               for t in sorted(types) if "*" not in selector and expand(t, core) not in selector]
        why += [f"Attribute {a} the view does not show (ADR-0023 rule 2)"
                for a in sorted(attrs) if hides(a)]
        return why
    hidden.hides = hides
    return hidden


# The Entity writes a registration can forward (TS 104 175 4.3.6.2); through
# a view they stay local and no Context Source receives them, since rule 7
# is checked on the local store alone (ADR-0023 rule 7,
# antares-api federation::write_regs).
LOCAL_WRITES = {"create_entity", "replace_entity", "merge_entity", "delete_entity",
                "purge_entities", "append_attributes", "update_attributes",
                "partial_attribute_update", "replace_attribute", "set_attribute_value",
                "delete_attribute", "batch_create", "batch_update", "batch_upsert",
                "batch_merge", "batch_delete"}


def lacking(message, hidden):
    """Why an answer may lack what the failure message says it lacks: every
    name it says is missing is an Attribute the view does not show; else []."""
    names = {next(g for g in m.groups() if g) for m in MISSING.finditer(message or "")}
    if not hidden or not names or not all(hidden.hides(n) for n in names):
        return []
    return [f"the answer lacks Attribute {n} the view does not show (ADR-0023 rule 2)" for n in sorted(names)]


def owed(requests, offered, binds, hidden=None, fixtures=()):
    """Why the view refuses some request of a TP's body, or []."""
    why = []
    made = False
    registered = any(operation(f["method"], f["url"], binds) == "register_context_source" for f in fixtures)
    for r in requests:
        url, headers = r["url"], set(r.get("headers") or ())
        op = operation(r["method"], url, binds)
        made = made or op == "create_subscription"
        if "ngsild-tenant" in headers:
            why.append(f"{r['method']} {op or url}: NGSILD-Tenant (ADR-0023 rule 10)")
        elif "ngsild-entitymap" in headers or "entityMap" in parse_qs(urlsplit(url).query):
            why.append(f"{r['method']} {op or url}: an entity map (threat V22)")
        elif op is None:
            why.append(f"{r['method']} {urlsplit(url).path}: no operation binds it (ADR-0023 rule 4)")
        elif op not in offered:
            why.append(f"{r['method']} {op}: not offered (ADR-0023 rule 4)")
        elif op in OWN_SUBSCRIPTIONS and not made:
            why.append(f"{r['method']} {op}: a Subscription made at the root, not through the view (ADR-0023 rule 9)")
        elif registered and op in LOCAL_WRITES:
            why.append(f"{r['method']} {op}: a fixture registered a Context Source, "
                       "and a write through a view stays local (ADR-0023 rule 7)")
        elif registered and op == "create_subscription":
            why.append(f"{r['method']} {op}: a fixture registered a Context Source, "
                       "and a Subscription through a view stays local (ADR-0023 rule 9)")
        elif hidden:
            why += [f"{r['method']} {op}: {w}" for w in hidden(url, r.get("body"), op)]
            if op in LISTINGS:
                why.append(f"{r['method']} {op}: lists only the names the view shows (ADR-0023 rules 2 and 3)")
            if "join" in parse_qs(urlsplit(url).query):
                why.append(f"{r['method']} {op}: a join follows only into entities the view shows (ADR-0023 rule 3)")
    return why


CREATES = {"create_entity", "batch_create", "batch_upsert"}


def never_made(d, offered, binds, hidden=None):
    """TP -> why it names an Entity no request made: a TP body's create the view
    owes a refusal leaves the Entity out of the store, and a later TP naming it
    reads 404 where the root found it (T-3982). Requests are taken in the order
    the run sent them; a create that went through, fixtures' included, makes the
    Entity again."""
    why = {}
    for path in sorted(glob.glob(f"{d}/**/requests.jsonl", recursive=True)):
        unmade = set()
        for line in open(path, encoding="utf-8"):
            r = json.loads(line)
            op = operation(r["method"], r["url"], binds)
            if op in CREATES:
                body = r.get("body")
                ids = {e["id"] for e in (body if isinstance(body, list) else [body])
                       if isinstance(e, dict) and isinstance(e.get("id"), str)}
                refused = not r.get("fixture") and owed([r], offered, binds, hidden)
                unmade = unmade | ids if refused else unmade - ids
                continue
            m = re.search(r"/entities/([^/?]+)", urlsplit(r["url"]).path)
            eid = unquote(m.group(1)) if m else None
            if eid in unmade and not r.get("fixture") and r.get("test"):
                why.setdefault(r["test"], []).append(
                    f"{r['method']} {op or urlsplit(r['url']).path}: names {eid}, which an earlier "
                    "create the view refused never made (ADR-0023 rule 4)")
    return why


def requests_of(d):
    """(body requests by TP, fixture requests by TP and by suite file)."""
    by_tp, fixtures = {}, {}
    for path in glob.glob(f"{d}/**/requests.jsonl", recursive=True):
        for line in open(path, encoding="utf-8"):
            r = json.loads(line)
            if r.get("fixture"):
                fixtures.setdefault(r["test"] or r.get("suite", ""), []).append(r)
            else:
                by_tp.setdefault(r["test"], []).append(r)
    return by_tp, fixtures


def messages_of(d):
    """{"<test name>": failure message} from the failures.csv under `d`."""
    out = {}
    for path in glob.glob(f"{d}/**/failures.csv", recursive=True):
        with open(path, newline="", encoding="utf-8") as fh:
            for row in csv.DictReader(fh):
                out[row["test"]] = row.get("message") or ""
    return out


def diff(view_path, e1_dir, e2_dir, binds=None):
    view = json.load(open(view_path))
    known = json.load(open(KNOWN)).get(view["id"], {}) if os.path.exists(KNOWN) else {}
    binds = bindings() if binds is None else binds
    e1, e2 = baseline.run_of(e1_dir), baseline.run_of(e2_dir)
    red = [f"{d}: no TP ran (tp-status.csv missing or empty)" for d, r in ((e1_dir, e1), (e2_dir, e2)) if not r]
    if red:
        print("\n".join(red))
        return 1
    reqs, fixtures = requests_of(e2_dir)
    messages = messages_of(e2_dir)
    offered = set(view.get("operations") or [])
    hidden = hidden_by(view, core_terms())
    unmade = never_made(e2_dir, offered, binds, hidden)
    refusals, explained = [], 0
    for tp in sorted(e1.keys() | e2.keys()):
        a, b = e1.get(tp, "did not run"), e2.get(tp, "did not run")
        if a == b:
            if tp in known:
                red.append(f"stale: {tp} answers as E1 now; remove it from {os.path.basename(KNOWN)}")
            continue
        made = fixtures.get(tp, []) + fixtures.get(tp.split("::")[0], [])
        why = (owed(reqs.get(tp, []), offered, binds, hidden, made) + unmade.get(tp, [])
               if "did not run" not in (a, b) else [])
        if not why and "did not run" not in (a, b):
            why = lacking(messages.get(tp.split("::", 1)[-1]), hidden)
        if why:
            refusals.append({"tp": tp, "e1": a, "view": b, "owed": why})
            if tp in known:
                red.append(f"stale: {tp} is an owed refusal now; remove it from {os.path.basename(KNOWN)}")
        elif tp in known:
            explained += 1
            print(f"known ({known[tp]}): E1 {a}, view {b}: {tp}")
        else:
            red.append(f"E1 {a}, view {b}, no refusal owed: {tp}")
    with open(f"{e2_dir}/owed-refusals.json", "w") as fh:
        json.dump({"view": view["id"], "refusals": refusals}, fh, indent=1)
    print(f"{len(e1)} TPs E1, {len(e2)} through view {view['id']}: "
          f"{len(refusals)} owed refusals, {explained} known, {len(red)} red")
    for line in red:
        print(line)
    return 1 if red else 0


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    sys.exit(diff(*sys.argv[1:]))
