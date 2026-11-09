"""Smoke suite `endpoints`: every Endpoint the export lists, through the gateway paths dev serves
(`/api/endpoint/{slug}/...`), checked against what the export holds.

Per public Endpoint: NGSI-LD types (exactly the types its public Policies open, of those the export
holds rows of), each type's details, the entity count per type (`count=true`, equal to the export),
one entity per type by id, GeoJSON for a type with a location, the temporal API over the last 24 h
(answers) and before it (nothing), OGC API Features collections + items, OData $metadata + one
query per type, SensorThings and iCal where the Endpoint serves them, and an anonymous write that
must be refused. Per closed Endpoint: 401 anonymously; with --person-token-file, 200 where the
token names the Endpoint and a Policy grants its holder, 403 for a write outside the Policy.
Per published Endpoint: its datasets in the dataroom catalogue (`/opendata`), one per Endpoint or
one per `spec.catalog.datasets` entry."""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import datetime as dt
import json
import re
import secrets
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import export as ex  # noqa: E402
from common import Http, Results, as_json, jwt_claims, loopback_only, read_token, short  # noqa: E402

LD = "application/ld+json"
GEO = "application/geo+json"
OUTSIDE = "SmokeOutsidePolicy"


def q(s: str) -> str:
    return urllib.parse.quote(s, safe="")


def iso(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def listed_types(body) -> list[str] | None:
    """NGSI-LD Retrieve Available Entity Types: an EntityTypeList or a list of EntityType."""
    if isinstance(body, dict) and isinstance(body.get("typeList"), list):
        return [ex.short_type(str(t)) for t in body["typeList"]]
    if isinstance(body, list):
        out = []
        for t in body:
            if isinstance(t, dict) and t.get("typeName"):
                out.append(ex.short_type(str(t["typeName"])))
            elif isinstance(t, dict) and t.get("typeList"):
                out += [ex.short_type(str(x)) for x in t["typeList"]]
            elif isinstance(t, str):
                out.append(ex.short_type(t))
        return out
    return None


def instances(entity) -> int:
    """How many temporal instances a temporal entity representation carries."""
    if not isinstance(entity, dict):
        return 0
    n = 0
    for k, v in entity.items():
        if k in ("id", "type", "@context", "scope"):
            continue
        if isinstance(v, list):
            n += sum(1 for x in v if isinstance(x, dict))
        elif isinstance(v, dict):
            n += len(v.get("values") or []) if "values" in v else 1
    return n


def base64_json(d: dict) -> str:
    """A JWT part (tests build tokens with it)."""
    import base64
    return base64.urlsafe_b64encode(json.dumps(d).encode()).rstrip(b"=").decode()


class Person:
    """What --person-token-file is, read from its unverified claims to pick each expectation."""

    def __init__(self, token: str | None):
        self.token = token
        c = jwt_claims(token)
        aud = c.get("aud") or []
        self.aud = set([aud] if isinstance(aud, str) else aud)
        self.sub = c.get("sub")
        self.groups = {str(g).lstrip("/") for g in c.get("groups") or []} | \
            {str(g).rstrip("/").split("/")[-1] for g in c.get("groups") or []}
        self.roles = set((c.get("realm_access") or {}).get("roles") or [])
        self.azp = c.get("azp")

    def holds(self, grant: ex.Grant) -> bool:
        a = grant.assignee
        kind, id = a.get("kind"), str(a.get("id"))
        return (kind == "user" and id in (self.sub, f"service-account-{self.azp}")) \
            or (kind == "group" and id.lstrip("/") in self.groups) \
            or (kind == "role" and id in self.roles) \
            or (kind == "serviceAccount" and id == self.azp)


class Suite:
    def __init__(self, a, index: dict, person: Person):
        self.a, self.index, self.person = a, index, person
        self.http = Http(a.base_url, timeout=a.timeout)
        self.r = Results("endpoints", a.base_url)
        now = dt.datetime.now(dt.timezone.utc)
        # The rehearsal keeps 24 h of history and sweeps hourly: an instance may be up to
        # 24 h + the sweep interval old; anything before that is a leak of older history.
        self.window = now - dt.timedelta(hours=24)
        self.stale = now - dt.timedelta(hours=24, minutes=a.sweep_minutes)

    # ---------------------------------------------------------------- helpers

    def get(self, path, token=None, accept=LD):
        return self.http.call("GET", path, token=token, headers={"Accept": accept})

    def ok_json(self, id, target, path, token=None, accept=LD, expect=200):
        st, h, raw = self.get(path, token, accept)
        body = as_json(raw)
        good = st == expect and body is not None
        self.r.check(id, target, f"GET {path}", f"{expect} JSON", f"{st} {h.get('content-type', '')} {short(raw, 100)}"
                     if not good else f"{st}", good)
        return body if good else None

    # ---------------------------------------------------------------- per Endpoint

    def endpoint(self, e: ex.Endpoint):
        try:
            if e.closed:
                self.closed(e)
            else:
                self.public(e)
            self.catalogue(e)
        except Exception as err:  # noqa: BLE001 - a crash in one Endpoint is a row, never the run
            self.r.add("crash", e.slug, "", "no exception", repr(err)[:200], "fail", "suite error")

    def public(self, e: ex.Endpoint):
        grants = e.public_grants()
        tenant = self.index["tenants"].get(e.tenant, {})
        if not grants:
            st, _, raw = self.get(f"/api/endpoint/{e.slug}/ngsi-ld/v1/types")
            names = listed_types(as_json(raw)) if st == 200 else []
            self.r.check("anon.no-grant", e.slug, f"GET /api/endpoint/{e.slug}/ngsi-ld/v1/types",
                         "no public Policy: no type (200 empty, 401 or 403)", f"{st} {names}",
                         st in (401, 403) or (st == 200 and names == []))
            return
        want, narrowed = ex.expected_types(e, grants, tenant)
        self.reads(e, None, grants, want, narrowed, tenant, "anon")
        self.write_refused(e, None, (401, 403), "anon", sorted(want))

    def closed(self, e: ex.Endpoint):
        path = f"/api/endpoint/{e.slug}/ngsi-ld/v1/types"
        st, _, raw = self.get(path)
        self.r.check("anon.refused", e.slug, f"GET {path}", "401", f"{st} {short(raw, 80)}", st == 401)
        p = self.person
        if not p.token:
            self.r.skip("person.reads", e.slug, f"GET {path}", "200 with a granted person's token",
                        "no --person-token-file given")
            return
        if e.slug not in p.aud:
            st, _, raw = self.get(path, p.token)
            self.r.check("person.other-audience", e.slug, f"GET {path} (person token)",
                         "401: the token does not name this Endpoint", f"{st}", st == 401)
            return
        grants = [g for g in e.grants if g.reads() and (p.holds(g) or e.own_role(g))]
        if not grants:
            st, _, raw = self.get(path, p.token)
            names = listed_types(as_json(raw)) if st == 200 else []
            self.r.check("person.no-grant", e.slug, f"GET {path} (person token)",
                         "403 or no type: no Policy grants the person", f"{st} {names}",
                         st == 403 or (st == 200 and names == []))
            return
        tenant = self.index["tenants"].get(e.tenant, {})
        want, narrowed = ex.expected_types(e, grants, tenant)
        self.reads(e, p.token, grants, want, narrowed, tenant, "person")
        held = [g for g in e.grants if g.writes() and (p.holds(g) or e.own_role(g))]
        if held:
            self.r.skip("person.write-outside", e.slug, "", "403",
                        f"the person holds a write grant ({held[0].name}): no write is outside it by type alone")
        else:
            self.write_refused(e, p.token, (403,), "person", sorted(want))

    def reads(self, e, token, grants, want: dict, narrowed: str, tenant: dict, who: str):
        base = f"/api/endpoint/{e.slug}"
        body = self.ok_json(f"{who}.types", e.slug, f"{base}/ngsi-ld/v1/types", token)
        if body is not None:
            got = set(listed_types(body) or [])
            # A type whose entities all expire (expiresAt) may be gone by now: not missing.
            missing = sorted(t for t in set(want) - got if not tenant[t].get("expires"))
            extra = sorted(got - set(want))
            self.r.check(f"{who}.types.match", e.slug, f"GET {base}/ngsi-ld/v1/types",
                         f"exactly {sorted(want)}", f"missing {missing}, extra {extra}",
                         not missing and not extra,
                         "extra = a type no granting Policy names or the export holds no row of" if extra else "")
        self.ok_json(f"{who}.attributes", e.slug, f"{base}/ngsi-ld/v1/attributes", token)
        temporal = [g for g in grants if g.temporal()]
        for t, count in sorted(want.items()):
            # Retrieve Entity is a grant of its own: a Policy of queryEntity alone finds the
            # entity by query and answers its id 404 (CIM 009 Table 4.20-1; prod answers so).
            retrieve = any(g.operations & ex.RETRIEVE_OPS and (not g.selectors or any(
                s.type in (t, "*") for s in g.selectors)) for g in grants)
            self.one_type(e, token, who, t, count, narrowed, tenant[t], temporal, retrieve)
        self.surfaces(e, token, who, want, narrowed)

    def one_type(self, e, token, who, t, count, narrowed, slot, temporal, retrieve=True):
        base = f"/api/endpoint/{e.slug}/ngsi-ld/v1"
        target = f"{e.slug} {t}"
        path = f"{base}/entities?type={q(t)}&count=true&limit=0"
        st, h, raw = self.get(path, token)
        actual = h.get("ngsild-results-count")
        if count is None:
            self.r.add(f"{who}.count", target, f"GET {path}", "a count (not comparable: " + (narrowed or "overlapping idPatterns") + ")",
                       f"{st} count={actual}", "pass" if st == 200 and actual is not None else "fail")
        elif slot.get("expires"):
            ok = st == 200 and actual is not None and int(actual) <= count
            self.r.check(f"{who}.count", target, f"GET {path}", f"<= {count} (entities expire)",
                         f"{st} count={actual}", ok)
        else:
            ok = st == 200 and actual is not None and int(actual) == count
            self.r.check(f"{who}.count", target, f"GET {path}", f"{count}", f"{st} count={actual} {short(raw, 80) if st != 200 else ''}", ok)
        if slot.get("expires") and st == 200 and actual is not None and int(actual) == 0:
            self.r.skip(f"{who}.type-details", target, f"GET {base}/types/{q(t)}", "200", "every entity of the type expired since the export")
            self.r.skip(f"{who}.by-id", target, "", "", "every entity of the type expired since the export")
            return
        self.ok_json(f"{who}.type-details", target, f"{base}/types/{q(t)}", token)
        sample = slot["sample"]
        sel = [s for s in self.selectors_for(e, t) if s.pattern]
        if count is None:
            self.r.skip(f"{who}.by-id", target, f"GET {base}/entities/{{id}}", "200 the entity",
                        "grant narrowed by " + (narrowed or "overlapping idPatterns") + ": no id known to be readable")
        else:
            if sel:
                sample = (slot.get("psample") or {}).get(sel[0].pattern)
            if not sample:
                self.r.skip(f"{who}.by-id", target, "", "", "no exported id matches the grant")
            elif not retrieve:
                st, _, raw = self.get(f"{base}/entities/{q(sample)}", token)
                self.r.check(f"{who}.by-id", target, f"GET {base}/entities/{q(sample)}",
                             "404, as for an id that does not exist (the grant has queryEntity, no retrieveEntity)",
                             f"{st} {short(raw, 80)}", st == 404)
            elif slot.get("expires"):
                st, _, raw = self.get(f"{base}/entities/{q(sample)}", token)
                self.r.check(f"{who}.by-id", target, f"GET {base}/entities/{q(sample)}", "200 or 404 (expired)",
                             f"{st}", st in (200, 404))
            else:
                ent = self.ok_json(f"{who}.by-id", target, f"{base}/entities/{q(sample)}", token)
                if ent is not None:
                    self.r.check(f"{who}.by-id.same", target, f"GET {base}/entities/{q(sample)}",
                                 f"id {sample}", f"id {ent.get('id')} type {ent.get('type')}",
                                 ent.get("id") == sample and ex.short_type(str(ent.get("type"))) == t)
        if slot.get("location"):
            path = f"{base}/entities?type={q(t)}&limit=1"
            st, h, raw = self.get(path, token, GEO)
            body = as_json(raw)
            self.r.check(f"{who}.geojson", target, f"GET {path} Accept: {GEO}", "200 FeatureCollection",
                         f"{st} {h.get('content-type', '')} {str((body or {}).get('type')) if isinstance(body, dict) else short(raw, 60)}",
                         st == 200 and isinstance(body, dict) and body.get("type") == "FeatureCollection")
        self.temporal(e, token, who, t, temporal)

    def selectors_for(self, e, t):
        return [s for g in e.grants for s in g.selectors if s.type in (t, "*")]

    def temporal(self, e, token, who, t, grants: list):
        base = f"/api/endpoint/{e.slug}/ngsi-ld/v1/temporal/entities"
        target = f"{e.slug} {t}"
        series = (self.index["temporal"].get(e.tenant) or {}).get(t)
        if not series:
            return
        # the grants of a temporal read that name this type, and the idPatterns they narrow it by
        sels = [s for g in grants for s in (g.selectors or [ex.Selector("*", None)]) if s.type in (t, "*")]
        if not sels:
            self.r.skip(f"{who}.temporal", target, f"GET {base}/{{id}}", "history",
                        "no bound Policy grants a temporal read of this type")
            return
        if all(s.pattern for s in sels):
            found = [(series.get("pseries") or {}).get(s.pattern) for s in sels]
            found = [f for f in found if f]
            if not found:
                self.r.skip(f"{who}.temporal", target, f"GET {base}/{{id}}", "history",
                            "no exported history of an id the grant's idPattern opens")
                return
            series = {**series, **max(found, key=lambda f: f["newest"])}
        eid = series["id"]
        recent = ex.parse_time(series["newest"]) >= self.window
        path = f"{base}/{q(eid)}?timerel=after&timeAt={iso(self.window)}"
        # CIM 009 6.3.10: a temporal answer cut at the broker's instance limit is 206 Partial
        # Content with Content-Range, the instances of its first part in the body.
        whole = (200, 206)
        if recent:
            st, _, raw = self.get(path, token)
            n = instances(as_json(raw)) if st in whole else 0
            self.r.check(f"{who}.temporal.last-24h", target, f"GET {path}", ">= 1 instance",
                         f"{st} {n} instances {short(raw, 80) if st not in whole else ''}", st in whole and n >= 1)
        else:
            self.r.skip(f"{who}.temporal.last-24h", target, f"GET {path}", ">= 1 instance",
                        f"the newest exported instance ({series['newest']}) is older than 24 h now")
        path = f"{base}/{q(eid)}?timerel=before&timeAt={iso(self.stale)}"
        st, _, raw = self.get(path, token)
        n = instances(as_json(raw)) if st in whole else 0
        self.r.check(f"{who}.temporal.none-older", target, f"GET {path}",
                     f"no instance before now - 24 h - {self.a.sweep_minutes} min (200 empty or 404)",
                     f"{st} {n} instances", st == 404 or (st in whole and n == 0))

    def surfaces(self, e, token, who, want: dict, narrowed: str):
        base = f"/api/endpoint/{e.slug}"
        reps = set(e.representations)
        if "ogc-features" in reps:
            body = self.ok_json(f"{who}.ogc.collections", e.slug, f"{base}/ogc/features/collections", token, "application/json")
            if body is not None:
                ids = {c.get("id") for c in body.get("collections") or [] if isinstance(c, dict)}
                missing = sorted(set(want) - ids)
                self.r.check(f"{who}.ogc.collections.match", e.slug, f"GET {base}/ogc/features/collections",
                             f"a collection per type {sorted(want)}", f"missing {missing}", not missing)
                for t, count in sorted(want.items()):
                    if t not in ids:
                        continue
                    path = f"{base}/ogc/features/collections/{q(t)}/items?limit=1"
                    st, h, raw = self.get(path, token, GEO)
                    fc = as_json(raw)
                    matched = fc.get("numberMatched") if isinstance(fc, dict) else None
                    ok = st == 200 and isinstance(fc, dict) and fc.get("type") == "FeatureCollection"
                    if ok and count is not None and matched is not None and not narrowed:
                        ok = int(matched) == count or (int(matched) <= count and self.expires(e, t))
                    self.r.check(f"{who}.ogc.items", f"{e.slug} {t}", f"GET {path}",
                                 f"200 FeatureCollection, numberMatched {count}", f"{st} numberMatched={matched} {short(raw, 80) if st != 200 else ''}", ok)
        if "odata" in reps:
            st, h, raw = self.get(f"{base}/odata/v4/$metadata", token, "application/xml")
            ok = st == 200 and b"EntitySet" in raw
            self.r.check(f"{who}.odata.metadata", e.slug, f"GET {base}/odata/v4/$metadata", "200 CSDL with EntitySets",
                         f"{st} {h.get('content-type', '')} {short(raw, 80) if not ok else ''}", ok)
            sets = set(re.findall(rb'<EntitySet Name="([^"]+)"', raw)) if ok else set()
            for t, count in sorted(want.items()) if ok else ():
                if t.encode() not in sets:
                    self.r.check(f"{who}.odata.set", f"{e.slug} {t}", f"GET {base}/odata/v4/$metadata",
                                 f"EntitySet {t}", "absent", False)
                    continue
                path = f"{base}/odata/v4/{q(t)}?$top=1&$count=true"
                st, _, raw = self.get(path, token, "application/json")
                body = as_json(raw)
                n = body.get("@odata.count", body.get("@count")) if isinstance(body, dict) else None
                ok = st == 200 and isinstance(body, dict) and isinstance(body.get("value"), list)
                if ok and count is not None and n is not None:
                    ok = int(n) == count or (int(n) <= count and self.expires(e, t))
                self.r.check(f"{who}.odata.query", f"{e.slug} {t}", f"GET {path}", f"200, @odata.count {count}",
                             f"{st} count={n} {short(raw, 80) if st != 200 else ''}", ok)
        if "sta" in reps:
            body = self.ok_json(f"{who}.sta.root", e.slug, f"{base}/sta/v1.1", token, "application/json")
            if body is not None:
                path = f"{base}/sta/v1.1/Things?$top=1&$count=true"
                st, _, raw = self.get(path, token, "application/json")
                b = as_json(raw)
                self.r.check(f"{who}.sta.things", e.slug, f"GET {path}", "200 value list",
                             f"{st} count={(b or {}).get('@iot.count') if isinstance(b, dict) else short(raw, 80)}",
                             st == 200 and isinstance(b, dict) and isinstance(b.get("value"), list))
        for name in e.ical:
            path = f"{base}/ical/v1/{q(name)}.ics"
            st, h, raw = self.get(path, token, "text/calendar")
            self.r.check(f"{who}.ical", f"{e.slug} {name}", f"GET {path}", "200 text/calendar VCALENDAR",
                         f"{st} {h.get('content-type', '')} {short(raw, 60)}",
                         st == 200 and raw.lstrip().startswith(b"BEGIN:VCALENDAR"))

    def expires(self, e, t) -> bool:
        return bool(((self.index["tenants"].get(e.tenant) or {}).get(t) or {}).get("expires"))

    def write_refused(self, e, token, expect: tuple, who: str, types: list):
        """Writes the caller holds no grant for: a Create Entity of a type the Endpoint serves, and
        one of a type no Policy names. Both refused before anything is read or written."""
        path = f"/api/endpoint/{e.slug}/ngsi-ld/v1/entities"
        # An Endpoint no bound Policy grants Create Entity (createEntity, or its group
        # redirectionOps) serves no POST /entities: its view answers 405 for every caller, which
        # refuses as surely as 401/403. A grant of batch writes alone (a pipeline's upsertBatch)
        # serves only /entityOperations.
        if not any(g.creates() for g in e.grants):
            expect = (*expect, 405)
        want = " or ".join(map(str, expect))
        for id, t in ((f"{who}.write-outside", types[0] if types else OUTSIDE),
                      (f"{who}.write-unknown-type", OUTSIDE)):
            if not self.a.writes:
                self.r.skip(id, e.slug, f"POST {path}", want, "--no-writes")
                continue
            eid = f"urn:ngsi-ld:{t}:smoke-{secrets.token_hex(6)}"
            st, _, raw = self.http.call("POST", path, token=token, headers={"Content-Type": "application/json"},
                                        body={"id": eid, "type": t})
            if st in (200, 201, 204):
                self.http.call("DELETE", f"{path}/{q(eid)}", token=token)
            self.r.check(id, e.slug, f"POST {path} type {t}", want, f"{st} {short(raw, 120)}", st in expect,
                         "a write no Policy grants was accepted" if 200 <= st < 300 else
                         "answered before the caller was refused (it reads the body first)" if st == 400 else "")
            if t == OUTSIDE:
                break

    def catalogue(self, e: ex.Endpoint):
        if not e.datasets:
            return
        if e.closed:
            self.r.skip("dataset", e.slug, "", "", f"closed Endpoint ({e.audience}): its datasets are not public")
            return
        for name in e.datasets:
            path = f"/opendata/api/3/action/package_show?id={q(name)}"
            st, _, raw = self.get(path, accept="application/json")
            body = as_json(raw)
            res = (body or {}).get("result") or {} if isinstance(body, dict) else {}
            n = len(res.get("resources") or []) if isinstance(res, dict) else 0
            self.r.check("dataset", f"{e.slug} {name}", f"GET {path}", "200 success, >= 1 resource",
                         f"{st} resources={n} {short(raw, 100) if st != 200 else ''}",
                         st == 200 and bool((body or {}).get("success")) and n >= 1)
        for name in e.slices:
            self.ok_json("dataset.record", f"{e.slug} {name}", f"/api/endpoint/{e.slug}/datasets/{q(name)}",
                         accept="application/json")

    # ---------------------------------------------------------------- whole run

    def listing(self, eps):
        # The list pages (at most 100 a page) and names the next page's cursor: read every page.
        path, slugs, cursor, pages = "/api/endpoints", set(), None, 0
        while True:
            page = f"{path}?limit=100" + (f"&cursor={q(cursor)}" if cursor else "")
            st, _, raw = self.get(page, accept="application/json")
            slugs |= set(re.findall(r'"slug"\s*:\s*"([a-z2-7]+)"', raw.decode(errors="replace")))
            body, pages = as_json(raw), pages + 1
            cursor = body.get("nextCursor") if st == 200 and isinstance(body, dict) else None
            if not cursor or pages >= 1000:
                break
        want = {e.slug for e in eps if not e.closed and e.public_grants()}
        missing = sorted(want - slugs)
        self.r.check("listing", "gateway", f"GET {path}", f"{len(want)} public Endpoints listed",
                     f"{st} {len(slugs)} listed, missing {len(missing)}: {missing[:8]}", st == 200 and not missing)


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run endpoints", description=__doc__.splitlines()[0])
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--export", required=True, help="the export bundle dir (manifest.json, gitea/, antares/)")
    p.add_argument("--admin-token-file", help="unused by this suite (accepted for the common runner)")
    p.add_argument("--person-token-file", help="a person's access token naming closed Endpoints")
    p.add_argument("--cache", help="where the export index is kept (default <out>/../.cache)")
    p.add_argument("--only", help="regex over slug, name, project or tenant")
    p.add_argument("--max-endpoints", type=int, default=0)
    p.add_argument("--workers", type=int, default=6)
    p.add_argument("--timeout", type=float, default=60)
    p.add_argument("--sweep-minutes", type=int, default=65,
                   help="slack for the hourly 24 h sweep before older history counts as a leak")
    p.add_argument("--no-writes", dest="writes", action="store_false")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    export, out = Path(a.export), Path(a.out)
    eps = ex.load_endpoints(export / "gitea" / "files")
    if a.only:
        rx = re.compile(a.only)
        eps = [e for e in eps if any(rx.search(x) for x in (e.slug, e.name, e.project, e.tenant))]
    if a.max_endpoints:
        eps = eps[:a.max_endpoints]
    index = ex.load_index(export, eps, Path(a.cache) if a.cache else out.parent / ".cache",
                          log=lambda m: print(m, file=sys.stderr))
    s = Suite(a, index, Person(read_token(a.person_token_file)))
    s.listing(eps)
    # Closed Endpoints first: a person token lives minutes, the whole run longer.
    eps = sorted(eps, key=lambda e: not e.closed)
    with cf.ThreadPoolExecutor(max_workers=a.workers) as pool:
        list(pool.map(s.endpoint, eps))
    s.r.write(out, {"export": str(export), "exportedAt": index.get("exportedAt"), "endpoints": len(eps),
                    "closed": sum(1 for e in eps if e.closed),
                    "person token": "given" if s.person.token else "none",
                    "person audience": len(s.person.aud), "person groups": sorted(s.person.groups)[:20]})
    t = s.r.totals()
    print(f"endpoints: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip -> {out}/REPORT.md")
    return 0 if t["fail"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
