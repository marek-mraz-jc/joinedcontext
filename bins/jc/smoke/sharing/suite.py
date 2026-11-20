"""Smoke suite `sharing`: Organization A shares an Endpoint with Organization B, measured cell by cell.

    bins/jc/smoke/run sharing --base-url http://localhost:42101 --export <the gate's bundle>
        --state <rehearsal state> --out <dir> [--lane] [--provider hel] [--consumer zilina]
        [--outsider barcelona]

The positive half of multiorg (which proves a foreign read is refused). A = provider, B = consumer,
C = an Organization that got nothing. Rows are the three ways B reads A (ARCHITECTURE-FEDERATION.md
section 45, docs Architecture/04 sections 5 and 5a, Architecture/18 section 3):

- w1 direct: A's Endpoint `sh-shared` has `audience: project-list` naming B's project `sh-b`, which
  holds a SharedSpaceReference to it; B's member calls A's Endpoint with B's own token.
- w2-public / w2-protected live federation: a space of B carries a ContextSourceRegistration for A's
  public Endpoint `sh-open` / for `sh-shared`; B's member queries B's Endpoint over that space.
- w3 replicate: B's Pipeline `sh-copy` reads A's `sh-shared` and writes into B's space `sh-copy`;
  B's member queries B's Endpoint over it.

Columns are what B's member reads on each way: NGSI-LD (query by type, retrieve by id, temporal,
a websocket subscription notified after A writes), MCP (initialize, tools/list, query_entities,
get_entity, describe_schema on the Endpoint the way gives B) and LinkML (the schema surface in
format linkml, the @context of the shared type, the foreign DataModel mirrored into B's project
with its sha256 (DM-48), describe_schema format linkml naming the same class and attributes).

Every cell is paired with a control: A's own admin makes the same call on A's Endpoint, and a
cell whose control did not answer is `blind` (red), so a 404 "no such route" never passes or fails
for the wrong reason. Per way the run also checks that the attribute A's Policy hides (`secret`)
is absent on every surface at B while the control shows it, that a write by B's member is refused
and B's admin's write A granted lands (w1), that C reads nothing on any surface, and, after A
revokes (sh-b out of allowedProjects, the share and public Policies ended), how long each w1 and
w2 surface takes to refuse; the w3 copy that remains is recorded as such.

The verdict: a cell on known.json (built later, each with its spec reference) that fails is
`not built` and keeps the run green; any other failing cell turns it red. --lane judges only the
cells of lane.json (those green on the last measured run, the train's smoke lane) and reports the
others as skips. An instance whose forge refuses the fixture repositories (403 or 405, e.g. a local
jc of a deployed export) gets one skip row naming that and no matrix, before any persona exists;
in --lane mode the same refusal fails, since the train's gate jc allows them.

Security: the personas (`sh-*`) get random passwords kept in memory and are deleted after the run,
also a failed one, with the fixture projects; the report holds statuses, counts and attribute
names, never a token, a password or an entity value. Loopback only: a run against a deployment
needs fixtures an approved Operator Change makes, which do not exist yet."""
import argparse
import base64
import importlib.util
import json
import re
import secrets
import sys
import threading
import time
import urllib.parse
from datetime import UTC, datetime, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("smoke_multiorg_for_sharing", HERE.parent / "multiorg" / "suite.py")
mo = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mo)
R = mo.R
sys.path.insert(0, str(HERE.parent / "subscriptions"))
import wsclient

KNOWN = HERE / "known.json"
LANE = HERE / "lane.json"
TYPE = "ShareCheck"
HIDDEN = "secret"  # the attribute A's share and public Policies leave out
SHOWN = ("level", "note")
URN = f"urn:ngsi-ld:{TYPE}:sh-1"
MODEL = "sh-model"
PA, PB = "sh-a", "sh-b"  # the fixture projects of A and B
HUB_ACCOUNT = "sh-fedprot-reader"  # B's ServiceAccount its protected hub forwards as (w2-protected)

WAYS = ("w1", "w2-public", "w2-protected", "w3")
SURFACES = {
    "ngsi-ld": ("query", "retrieve", "temporal", "subscription"),
    "mcp": ("initialize", "tools-list", "query-entities", "get-entity", "describe-schema"),
    "linkml": ("schema", "context", "mirror", "describe-linkml"),
}
# Checks over a whole surface of a way: the hidden attribute, the outsider, the revocation.
ACROSS = ("hidden", "outsider", "revoked")
# Checks of a way: a write by B's member refused, B's admin's granted write (w1 alone allows one).
WRITES = {"w1": ("write-refused", "write-granted"), "w2-public": ("write-refused",),
          "w2-protected": ("write-refused",), "w3": ("write-refused",)}
# A forge that refuses the fixture repositories (403, 405: a deployed export's jc, jc-dev 2026-11-19):
# outside --lane one skip row; in the lane (the train's gate jc, which allows them) a fail.
FIXTURE_REFUSED = (403, 405)
NO_CONTROL = {"mirror", "outsider", "write-refused"}  # judged by themselves, see cells()


def cells() -> list[dict]:
    """The matrix: way x surface x check, each with whether it needs A's control answer."""
    out = []
    for way in WAYS:
        for surface, checks in SURFACES.items():
            for check in (*checks, *ACROSS):
                if check == "revoked" and way == "w3":
                    check = "copy-remains"
                out.append({"id": f"{way}.{surface}.{check}", "way": way, "surface": surface, "check": check,
                            "control": check not in NO_CONTROL and check != "copy-remains"})
        for check in WRITES[way]:
            out.append({"id": f"{way}.write.{check}", "way": way, "surface": "write", "check": check,
                        "control": check == "write-granted"})
    # SHR-03: a second copy Pipeline whose account A grants no `store` is refused at plan time
    out.append({"id": "w3.plan.store-refused", "way": "w3", "surface": "plan", "check": "store-refused",
                "control": False})
    return out


def judge(cell: dict, ok, actual: str, control=None, reason: str = "") -> dict:
    """One matrix row. `control` (ok, actual) of A's admin making the same call: a cell that needs
    one and has none answering is blind, never a pass or a fail of B."""
    row = {**cell, "actual": actual, "reason": reason, "controlActual": None}
    if cell["control"]:
        if control is None or not control[0]:
            row["controlActual"] = control[1] if control else "not made"
            row["result"] = "blind"
            row["reason"] = f"control failed: {row['controlActual']}" + (f"; {reason}" if reason else "")
            return row
        row["controlActual"] = control[1]
    row["result"] = "pass" if ok else "fail"
    return row


def load_known(path: Path = KNOWN) -> dict:
    """known.json: {regex over the cell id: the spec reference saying it is not built yet}."""
    return json.loads(path.read_text())["notBuilt"] if path.exists() else {}


def known_for(cell_id: str, known: dict) -> str | None:
    return next((ref for pat, ref in known.items() if re.fullmatch(pat, cell_id)), None)


def settle(rows: list[dict], known: dict, lane: set | None = None) -> list[dict]:
    """Marks the failing rows a known entry names `not built`, and with a lane the rows outside it
    `outside lane`; answers the rows with `red` set where the verdict turns red."""
    for r in rows:
        ref = known_for(r["id"], known)
        r["known"] = ref
        if lane is not None and r["id"] not in lane:
            r["red"] = False
            r["status"] = "outside lane"
            continue
        if r["result"] == "pass":
            r["status"] = "pass" + (" (known list stale: drop the entry)" if ref else "")
            r["red"] = False
        elif ref:
            r["status"] = "not built"
            r["red"] = False
        else:
            r["status"] = r["result"]
            r["red"] = True
    return rows


def keys_in(doc) -> set:
    """Every object key of a JSON document, at any depth."""
    if isinstance(doc, dict):
        return set(doc) | {k for v in doc.values() for k in keys_in(v)}
    if isinstance(doc, list):
        return {k for v in doc for k in keys_in(v)}
    return set()


def names_attribute(doc, name: str) -> bool:
    """Whether a JSON answer (or the text of one, an MCP result, a LinkML YAML) names attribute
    `name`: as a key, as a quoted string or as a YAML key."""
    if name in keys_in(doc):
        return True
    text = doc if isinstance(doc, str) else json.dumps(doc, ensure_ascii=False)
    return bool(re.search(rf'(["\'/#:]{re.escape(name)}["\']|^\s*{re.escape(name)}\s*:|\\"{re.escape(name)}\\")', text, re.MULTILINE))


def mcp_text(res) -> str:
    """The text an MCP tools/call answered: its content texts and structured content."""
    r = (res or {}).get("result") if isinstance(res, dict) else None
    if not isinstance(r, dict):
        return json.dumps(res, ensure_ascii=False)[:20000]
    parts = [c.get("text", "") for c in r.get("content") or [] if isinstance(c, dict)]
    if r.get("structuredContent") is not None:
        parts.append(json.dumps(r["structuredContent"], ensure_ascii=False))
    return "\n".join(parts)


def mcp_ok(st, res) -> bool:
    r = (res or {}).get("result") if isinstance(res, dict) else None
    return st == 200 and isinstance(r, dict) and not r.get("isError")


def owned_urn(domain: str) -> str:
    """The shared entity's id naming the Organization that owns the space (ADR-N-068:
    jc_core::names::org_id, the domain's labels reversed and joined by `_`)."""
    return f"urn:ngsi-ld:{TYPE}:{'_'.join(reversed(domain.split('.')))}:sh-1"


def now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def b32() -> str:
    return base64.b32encode(secrets.token_bytes(20)).decode().lower()


def head(kind, name, ns) -> str:
    return f"apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: {ns}\n"


def linkml(full: bool = True) -> str:
    """The shared model: A's in full; B's (`full` False) only what A shares, as B gets it from A's
    schema surface. The same IRI per term on both sides: a CSR matches expanded IRIs
    (ARCHITECTURE-FEDERATION.md 45.1 point 1)."""
    slots = "".join(f"  {s}:\n    description: {d}\n    annotations: {{ ngsi_ld_kind: Property }}\n" for s, d in (
        ("level", "A number A shares."), ("note", "A text A shares."), (HIDDEN, "A text A keeps to itself."))
        if full or s != HIDDEN)
    return (f"id: https://joinedcontext.com/models/smoke/{MODEL}\nname: {MODEL}\ntitle: sharing smoke model\n"
            "license: https://joinedcontext.com/licenses/cc-by-4.0\nprefixes:\n  linkml: https://w3id.org/linkml/\n"
            "  jc: https://joinedcontext.com/models/\n  ngsi-ld: https://uri.etsi.org/ngsi-ld/\ndefault_prefix: jc\n"
            f"default_range: string\nimports:\n  - linkml:types\n  - ngsi-ld-core\nclasses:\n  {TYPE}:\n"
            "    is_a: Entity\n    description: The entity A shares with B.\n"
            f"    slots: [{', '.join((*SHOWN, HIDDEN) if full else SHOWN)}]\nslots:\n{slots}")


def space_files(project: str, space: str) -> list:
    """A space over the shared model; a DataModel's name is unique within its project, its LinkML
    (and so every IRI) the same in each."""
    sp, dm = f"spaces/{space}", f"{space}-model"
    full = project == PA
    art = MODEL if full else f"{MODEL}-b"
    return [(f"{sp}/space.yaml", head("ContextSpace", space, project)
             + f"spec:\n  isSandbox: false\n  defaultLocale: en\n  dataModelRef: {{ kind: DataModel, name: {dm} }}\n"
             # every write a history row, kept a day: the temporal reads have instances to answer
             f"  retention:\n    - entityType: {TYPE}\n      history: P1D\n      record: every\n"),
            (f"{sp}/datamodels/{dm}.yaml", head("DataModel", dm, project)
             + f"spec:\n  contextSpaceRef: {space}\n  linkml: ./{dm}.linkml.yaml\n  version: 1.0.0\n"
             f"  lifecycle: published\n  classes: [{TYPE}]\n  artifacts:\n"
             + "".join(f"    {k}: ./{art}.v1.{ext}\n" for k, ext in ARTIFACTS)),
            (f"{sp}/datamodels/{dm}.linkml.yaml", linkml(full)),
            # a published model commits its four generated artifacts (DM-02): `jcctl model generate`
            # of linkml(), committed beside this file (model/README.md)
            *((f"{sp}/datamodels/{art}.v1.{ext}", (HERE / "model" / f"{art}.v1.{ext}").read_text())
              for _, ext in ARTIFACTS)]


def endpoint(project, space, name, slug, audience, extra="") -> tuple:
    return (f"spaces/{space}/endpoints/{name}.yaml", head("Endpoint", name, project)
            + f"spec:\n  contextSpaceRef: {space}\n  slug: {slug}\n  audience: {audience}\n{extra}"
            "  enabledRepresentations: [ngsi-ld, mcp, websocket]\n")


def policy(project, space, name, kind, pid, ops, attrs=None, until=None, actions=None) -> tuple:
    who = f"{{ kind: {kind}, id: {pid} }}" if pid else f"{{ kind: {kind} }}"
    acts = f"  actions: [{', '.join(actions)}]\n" if actions else ""
    names = f"      propertyNames: [{', '.join(attrs)}]\n" if attrs else ""
    valid = f"  validity: {{ to: '{until}' }}\n" if until else ""
    return (f"spaces/{space}/policies/{name}.yaml", head("Policy", name, project)
            + f"spec:\n  contextSpaceRef: {{ kind: ContextSpace, name: {space} }}\n  assigner: did:web:{{orgDomain}}\n"
            f"  assignee: {who}\n  operations: [{', '.join(ops)}]\n{acts}{valid}  information:\n"
            f"    - entities:\n        - type: {TYPE}\n{names}")


ARTIFACTS = (("jsonSchema", "schema.json"), ("context", "context.jsonld"), ("docs", "md"), ("example", "example.jsonld"))
READS = ["retrieveOps", "retrieveTemporal", "queryTemporal", "createSubscription", "deleteSubscription"]
WRITE = ["upsertBatch", "createBatch", "queryBatch"]


class Suite(mo.Suite):
    urn = URN  # setup sets the owner's form (ADR-N-068)

    def __init__(self, a, lane: set | None = None):
        a.jc_config, a.live = None, False
        super().__init__(a)
        self.lane = lane
        self.cells = {c["id"]: c for c in cells()}
        self.matrix: list[dict] = []
        self.slugs = {n: b32() for n in ("sh-shared", "sh-open", "sh-fedpub", "sh-fedprot", "sh-copy")}
        self.made = False
        self.skipped = False  # the forge refused the fixtures: one skip row, no matrix
        self.tok: dict = {}  # (persona, slug) -> exchanged Endpoint token, never written anywhere
        self.revoked_at: float | None = None

    # ---- the matrix
    def put(self, cid, ok, actual, control=None, reason=""):
        self.matrix.append(judge(self.cells[cid], ok, actual, control, reason))

    def lacking(self, why: str):
        """Every cell setup left unmeasured: failed with the reason, never silently missing."""
        done = {r["id"] for r in self.matrix}
        for cid, c in self.cells.items():
            if cid not in done:
                self.matrix.append({**c, "actual": "not measured", "reason": why, "controlActual": None,
                                    "result": "fail"})

    def wanted(self, part: str) -> bool:
        """Whether a cell id holding `part` is judged: always, or in --lane mode a lane cell. The
        waits of a part no lane cell needs (copy, mirror, revocation) are left out of the train."""
        return self.lane is None or any(part in c for c in self.lane)

    # ---- fixtures
    def ids(self, key):
        return self.people[key]["id"]

    def a_files(self, revoke: str | None = None) -> list:
        s = self.slugs
        allowed = "[]" if revoke else f"[{PB}]"
        return [(".jc/layout", "2\n"), ("project.yaml", head("Project", PA, "org") + f"spec:\n  organizationRef: {self.org_a}\n"),
                *space_files(PA, "sh-src"),
                endpoint(PA, "sh-src", "sh-shared", s["sh-shared"], "project-list", f"  allowedProjects: {allowed}\n"),
                # open data is read-only (EP-187): sh-open binds the public Policy alone
                endpoint(PA, "sh-src", "sh-open", s["sh-open"], "public",
                         f"  policyRefs: [urn:ngsi-ld:Policy:{self.domain_a}:sh-src:sh-public]\n"),
                policy(PA, "sh-src", "sh-admin", "user", self.ids("a-admin"), READS + WRITE),
                policy(PA, "sh-src", "sh-share-member", "user", self.ids("b-member"), READS, SHOWN, revoke),
                policy(PA, "sh-src", "sh-share-admin", "user", self.ids("b-admin"), READS + WRITE, SHOWN, revoke),
                # the copy is materializing: A permits `store` to that one Pipeline's account (SHR-03)
                policy(PA, "sh-src", "sh-share-pipeline", "serviceAccount", f"{PB}/pl-sh-copy", ["retrieveOps", "queryBatch"],
                       SHOWN, revoke, ["store"]),
                # w2-protected: B's hub reads as B's ServiceAccount, which A grants by name (FED-15)
                policy(PA, "sh-src", "sh-share-hub", "serviceAccount", f"{PB}/{HUB_ACCOUNT}", READS, SHOWN, revoke),
                policy(PA, "sh-src", "sh-public", "role", "public", ["retrieveOps"], SHOWN, revoke)]

    def b_files(self) -> list:
        s, files = self.slugs, [(".jc/layout", "2\n"), ("project.yaml", head("Project", PB, "org")
                                                       + f"spec:\n  organizationRef: {self.org_b}\n")]
        files.append((f"access/serviceaccounts/{HUB_ACCOUNT}.yaml", head("ServiceAccount", HUB_ACCOUNT, PB)
                      + f"spec:\n  owner: {{ user: {self.ids('b-admin')} }}\n"
                      "  purpose: reads A's shared Endpoint for B's hub space sh-fedprot\n"
                      # a role is required; reading its own hub space is the least it can hold
                      "  roles: [{ role: space-reader, scope: { contextSpace: sh-fedprot } }]\n"
                      # an account is a Keycloak client; the in-process read signs in with none of it
                      "  credentials: [{ kind: oauth-client, name: hub }]\n"))
        identities = {"sh-open": "{ identity: anonymous }",
                      "sh-shared": f"{{ identity: serviceAccount, serviceAccountRef: {{ kind: ServiceAccount, name: {HUB_ACCOUNT} }} }}"}
        for space, door, csr in (("sh-fedpub", "sh-fedpub", "sh-open"), ("sh-fedprot", "sh-fedprot", "sh-shared"),
                                 ("sh-copy", "sh-copy", None)):
            files += [*space_files(PB, space), endpoint(PB, space, door, s[door], "organization"),
                      policy(PB, space, f"{space}-read", "organization", None, READS)]
            if csr:
                files.append((f"spaces/{space}/registrations/{csr}.yaml", head("ContextSourceRegistration", csr, PB)
                              + f"spec:\n  contextSpaceRef: {space}\n  endpointRef: {{ kind: Endpoint, name: {csr}, namespace: {PA} }}\n"
                              # read in process on one broker (SHR-17): the public share as the
                              # anonymous caller, the protected one as B's ServiceAccount
                              f"  federation: {identities[csr]}\n"
                              f"  information:\n    - entities:\n        - type: {TYPE}\n"))
        files += [policy(PB, "sh-copy", "sh-copy-pipelines", "serviceAccount", "pipelines", WRITE),
                  ("pipelines/sh-copy/pipeline.yaml", head("Pipeline", "sh-copy", PB)
                   + "spec:\n  class: auto\n  period: 30s\n  source:\n"
                   f"    endpointRef: {{ kind: Endpoint, name: sh-shared, namespace: {PA} }}\n"
                   f"    query:\n      type: {TYPE}\n"
                   # an Endpoint-sourced stream runs a mapping (streams::eligible) over each page read;
                   # the copy keeps every attribute A's grant shows and takes an id under B's
                   # Organization (ADR-N-068)
                   "  compute:\n    kind: bloblang\n    bloblang: |\n"
                   "      root = this.map_each(this.assign({\"id\": "
                   f"\"{self.copy_urn.rsplit(':', 1)[0]}:\" + this.id.split(\":\").index(-1)}}))\n"
                   "  targetEndpoint: urn:ngsi-ld:Endpoint:{orgDomain}:sh-copy:sh-copy\n"
                   f"  output: {{ type: {TYPE}, mode: upsert }}\n"),
                  (f"shared/{PA}.yaml", head("SharedSpaceReference", PA, PB)
                   + f"spec:\n  alias: {PA}\n  endpointSlug: {s['sh-shared']}\n  schedule: {{ interval: 1m }}\n")]
        return files

    def domain_of(self, org: str) -> str:
        """The Organization's domain, from its manifest in the bundle's configuration repository."""
        path = self.export / "gitea" / "files" / mo.CONFIG[0] / mo.CONFIG[1] / "organizations" / f"{org}.yaml"
        m = re.search(r"^\s*domain:\s*(\S+)", path.read_text(), re.MULTILINE) if path.exists() else None
        if not m:
            raise RuntimeError(f"no domain for Organization {org} in {path}")
        return m.group(1).strip("'\"")

    def setup(self):
        self.org_a, self.org_b, self.org_c = self.a.provider, self.a.consumer, self.a.outsider
        self.domain_a = self.domain_of(self.org_a)
        self.urn = owned_urn(self.domain_a)
        # the copy is B's entity: B's Endpoint refuses a URN naming A (ADR-N-068), so the Pipeline
        # re-mints it under B's Organization
        self.domain_b = self.domain_of(self.org_b)
        self.copy_urn = owned_urn(self.domain_b)
        h = self.client()
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        # the repositories before any persona: a forge that refuses them leaves nothing to remove
        self.made = True
        for project in (PA, PB):
            st, _body = h.json("POST", f"/git/api/v1/orgs/{mo.CONFIG[0]}/repos", headers=auth, body={
                "name": project, "private": True, "auto_init": True, "default_branch": "main"})
            if st in FIXTURE_REFUSED and project == PA and self.lane is None:
                self.made = False
                self.skipped = True
                self.row("setup.fixtures", f"{mo.CONFIG[0]}/{PA}", f"POST /git/api/v1/orgs/{mo.CONFIG[0]}/repos",
                         "201", str(st), None, "no fixture rights on this instance: the suite measures a jc that "
                         "lets it make its fixture projects (the train's gate jc)")
                return False
            if st not in (201, 409):
                raise RuntimeError(f"POST /git/api/v1/orgs/{mo.CONFIG[0]}/repos {project}: {st}")
        tok = R.admin_token(h, self.state)
        for key in ("a-admin", "b-admin", "b-member", "c-member"):
            uid, pw = R.make_person(h, tok, self.realm, f"sh-{key}")
            self.people[key] = {"username": f"sh-{key}", "id": uid, "password": pw}
        org = {"a": self.org_a, "b": self.org_b, "c": self.org_c}
        files = [(f"users/assignments/sharing-{key}.yaml", head("RoleBinding", f"sharing-{key}", "org")
                  + f"spec:\n  subjects:\n    - user: {self.ids(key)} # sh-{key}\n"
                  f"  role: {'org-admin' if key.endswith('admin') else 'viewer'}\n  scope: {{ organization: {org[key[0]]} }}\n")
                 for key in self.people]
        for project, repo in ((PA, self.a_files()), (PB, self.b_files())):
            self.commit(repo, f"sharing smoke: fixture {project}", repo=(mo.CONFIG[0], project))
            files.append((f"projects/{project}.yaml", head("Project", project, "org")
                          + f"spec:\n  organizationRef: {org['a' if project == PA else 'b']}\n"
                          f"  repository:\n    name: {project}\n  ref: main\n"))
        self.commit(files, "sharing smoke: personas' RoleBindings and the fixture projects")
        return self.ready()

    def ready(self) -> bool:
        """Waits until A's admin writes the entity through sh-shared (the Policy reached the
        gateway); writes it twice, so the temporal reads have two instances."""
        deadline = time.time() + self.a.sync_timeout
        why = ""
        while time.time() < deadline:
            st, raw = self.upsert("a-admin", "sh-shared", {"level": "7", "note": "first", HIDDEN: "kept"})
            if st in (200, 201, 204):
                time.sleep(1)
                st, _ = self.upsert("a-admin", "sh-shared", {"level": "8", "note": "second", HIDDEN: "kept"})
                self.row("setup.seed", f"{self.org_a}/{PA}", "A's admin upserts the entity twice", "204", str(st),
                         st in (200, 201, 204))
                return st in (200, 201, 204)
            why = f"{st} {raw[:400].decode(errors='replace') if isinstance(raw, bytes) else str(raw)[:400]}"
            for p in self.people.values():
                p.pop("tokens", None)  # a token taken before the binding synced carries no group
            self.tok.clear()
            time.sleep(5)
        self.row("setup.seed", f"{self.org_a}/{PA}", "A's admin upserts the entity", "204 within the sync timeout",
                 why, False)
        return False

    def cleanup(self):
        """The fixture projects out of the configuration, their repositories and the personas
        deleted, also after a failed run; what is left is a failing row naming it."""
        if not self.made:
            return
        h = self.client()
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        left, ops = [], []
        for path in [f"projects/{PA}.yaml", f"projects/{PB}.yaml",
                     *(f"users/assignments/sharing-{k}.yaml" for k in self.people)]:
            st, cur = h.json("GET", f"/git/api/v1/repos/{mo.CONFIG[0]}/{mo.CONFIG[1]}/contents/{path}?ref=main", headers=auth)
            if st == 200 and isinstance(cur, dict):
                ops.append({"operation": "delete", "path": path, "sha": cur["sha"]})
        try:
            if ops:
                R.merge_change(lambda m, p, body: h.json(m, f"/git/api/v1{p}", headers=auth, body=body),
                               *mo.CONFIG, ops, "sharing smoke: fixtures removed")
        except RuntimeError as x:
            left.append(f"configuration ({str(x)[:120]})")
        for project in (PA, PB):
            st, _, _ = h.call("DELETE", f"/git/api/v1/repos/{mo.CONFIG[0]}/{project}", headers=auth)
            if st not in (204, 404):
                left.append(f"{project} ({st})")
        admin = {"Authorization": f"Bearer {R.admin_token(h, self.state)}"} if self.people else {}
        for key, p in self.people.items():
            st, _, _ = h.call("DELETE", f"/login/admin/realms/{self.realm}/users/{p['id']}", headers=admin)
            if st not in (204, 404):
                left.append(f"persona {key} ({st})")
            p.pop("password", None)
        self.made = False
        self.row("cleanup", "all", "remove fixture projects, repositories and personas", "none left",
                 f"{len(left)} left {left[:4]}", not left)

    # ---- calls
    def token(self, key, door):
        """The person's exchanged token for Endpoint `door` (cached for the run)."""
        slug = self.slugs[door]
        if (key, slug) not in self.tok:
            st, t = self.exchange(key, slug)
            self.tok[(key, slug)] = (st, t)
        return self.tok[(key, slug)]

    def get(self, key, door, path):
        """(status, JSON or text) of a GET on Endpoint `door` as `key` (None: anonymous)."""
        hdr = {}
        if key:
            st, t = self.token(key, door)
            if not t:
                return st or 0, f"exchange refused {st}"
            hdr["Authorization"] = f"Bearer {t}"
        st, _, raw = self.client().call("GET", f"/api/endpoint/{self.slugs[door]}{path}", headers=hdr)
        try:
            return st, json.loads(raw)
        except ValueError:
            return st, raw.decode(errors="replace")

    def upsert(self, key, door, attrs):
        st, t = self.token(key, door)
        if not t:
            return st or 0, b"exchange refused"
        st, _, raw = self.client().call(
            "POST", f"/api/endpoint/{self.slugs[door]}/ngsi-ld/v1/entityOperations/upsert?options=update",
            headers={"Authorization": f"Bearer {t}"},
            body=[{"id": self.urn, "type": TYPE, **{k: {"type": "Property", "value": v, "observedAt": now()}
                                                     for k, v in attrs.items()}}])
        return st, raw

    def mcp(self, key, door, method, params):
        st, t = self.token(key, door)
        if not t:
            return st or 0, {"error": f"exchange refused {st}"}
        return self.client().json("POST", f"/api/endpoint/{self.slugs[door]}/mcp", headers={
            "Authorization": f"Bearer {t}", "Accept": "application/json, text/event-stream"},
            body={"jsonrpc": "2.0", "id": 1, "method": method, "params": params})

    def reads(self, key, door) -> dict:
        """Every read of the matrix on one Endpoint as one person: name -> (ok, actual, document)."""
        out = {}
        since = (datetime.now(UTC) - timedelta(days=1)).strftime("%Y-%m-%dT%H:%M:%SZ")
        urn = self.urn_for(door)
        q = urllib.parse.quote(urn, safe="")
        for name, path, want in (("query", f"/ngsi-ld/v1/entities?type={TYPE}&limit=100", "list"),
                                 ("retrieve", f"/ngsi-ld/v1/entities/{q}", "entity"),
                                 ("temporal", f"/ngsi-ld/v1/temporal/entities?type={TYPE}&timerel=after&timeAt={since}&lastN=5",
                                  "list")):
            st, doc = self.get(key, door, path)
            ok = st == 200 and (any(isinstance(e, dict) and e.get("id") == urn for e in doc) if want == "list" and isinstance(doc, list)
                                else isinstance(doc, dict) and doc.get("id") == urn)
            out[("ngsi-ld", name)] = (ok, f"{st}" + ("" if ok else f", {self.brief(doc)}"), doc)
        for name, method, params in (
                ("initialize", "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                              "clientInfo": {"name": "sharing-smoke", "version": "1"}}),
                ("tools-list", "tools/list", {}),
                ("query-entities", "tools/call", {"name": "query_entities", "arguments": {"type": [TYPE]}}),
                ("get-entity", "tools/call", {"name": "get_entity", "arguments": {"id": urn}}),
                ("describe-schema", "tools/call", {"name": "describe_schema", "arguments": {"entityType": TYPE}}),
                ("describe-linkml", "tools/call", {"name": "describe_schema", "arguments": {"format": "linkml", "entityType": TYPE}})):
            st, res = self.mcp(key, door, method, params)
            text = mcp_text(res)
            if method == "initialize":
                ok = st == 200 and isinstance(res, dict) and "result" in res
            elif method == "tools/list":
                tools = {t.get("name") for t in ((res or {}).get("result") or {}).get("tools", [])} if isinstance(res, dict) else set()
                ok = st == 200 and {"query_entities", "get_entity", "describe_schema"} <= tools
            elif name in ("query-entities", "get-entity"):
                ok = mcp_ok(st, res) and urn in text
            else:
                ok = mcp_ok(st, res) and TYPE in text and all(names_attribute(text, s) for s in SHOWN)
            surface = "linkml" if name == "describe-linkml" else "mcp"
            out[(surface, name)] = (ok, f"{st}" + ("" if ok else f", {self.brief(res)}"), text if method == "tools/call" else res)
        st, idx = self.get(key, door, "/schema/index.json")
        major = next(iter(re.findall(r"/schema/(v\d+)/", json.dumps(idx))), "v1")
        for name, art in (("schema", "model.linkml.yaml"), ("context", "context.jsonld")):
            st, doc = self.get(key, door, f"/schema/{major}/{art}")
            ok = st == 200 and TYPE in (doc if isinstance(doc, str) else json.dumps(doc)) and \
                all(names_attribute(doc, s) for s in SHOWN)
            out[("linkml", name)] = (ok, f"index {st if idx else '-'}, {art} {st}" + ("" if ok else f", {self.brief(doc)}"), doc)
        return out

    @staticmethod
    def as_json(raw):
        try:
            return json.loads(raw)
        except ValueError:
            return raw.decode(errors="replace") if isinstance(raw, bytes) else raw

    @staticmethod
    def brief(doc) -> str:
        """Status-shaped detail of a refusal or an odd answer: a problem's type/title or the
        shape of a list, never an entity value."""
        if isinstance(doc, dict):
            if "error" in doc:
                e = doc["error"]
                return f"error {e.get('code') if isinstance(e, dict) else str(e)[:80]}"
            r = doc.get("result")
            if isinstance(r, dict) and r.get("isError"):
                sc = r.get("structuredContent") or {}
                why = sc.get("title") or sc.get("detail") or next((c.get("text") for c in r.get("content") or []
                                                                   if isinstance(c, dict)), "")
                return f"isError {sc.get('status', '')} {str(why)[:100]}"
            # a refusal's detail names which gate refused (no Policy, a read-only token, a
            # reason, another owner's URN), the one thing a 403 title cannot (T-7424)
            if isinstance(doc.get("title"), str) and isinstance(doc.get("detail"), str):
                return f"title: {doc['title'][:100]}, detail: {doc['detail'][:160]}"
            for k in ("title", "type", "detail"):
                if isinstance(doc.get(k), str) and k != "type" or (k == "type" and "problem" in str(doc.get(k, ""))):
                    return f"{k}: {doc[k][:100]}"
            return f"object with keys {sorted(doc)[:6]}"
        if isinstance(doc, list):
            return f"list of {len(doc)}"
        return str(doc)[:100]

    # ---- the ways
    def urn_for(self, door) -> str:
        """The shared entity's id as a door serves it: B's copy under B's Organization."""
        return self.copy_urn if door == "sh-copy" else self.urn

    def door(self, way) -> str:
        return {"w1": "sh-shared", "w2-public": "sh-fedpub", "w2-protected": "sh-fedprot", "w3": "sh-copy"}[way]

    def control_door(self, way) -> str:
        """A's admin reads the same thing on A's own Endpoint (sh-open is read-only and hides `secret`)."""
        return "sh-shared"

    def check_way(self, way, control: dict):
        door = self.door(way)
        b = self.reads("b-member", door)
        for (surface, name), (ok, actual, _) in b.items():
            self.put(f"{way}.{surface}.{name}", ok, actual, control.get((surface, name), (False, "not made"))[:2])
        # the hidden attribute: absent at B on each surface while A's own read shows it
        for surface in SURFACES:
            seen = sorted(n for (s, n), (_, _, doc) in b.items() if s == surface and doc is not None and names_attribute(doc, HIDDEN)
                          and n not in ("initialize", "tools-list"))
            answered = [n for (s, n), (ok, _, _) in b.items() if s == surface and ok]
            ctl = any(names_attribute(doc, HIDDEN) for (s, n), (ok, _, doc) in control.items() if s == surface and ok and doc is not None)
            self.put(f"{way}.{surface}.hidden", not seen and bool(answered),
                     f"'{HIDDEN}' in {seen}" if seen else f"absent from {len(answered)} answers",
                     (ctl, f"A's admin {'sees' if ctl else 'does not see'} '{HIDDEN}'"),
                     "" if answered else "B got no answer to judge")
        # the outsider: C's member reads nothing through B's or A's door
        c = self.reads("c-member", door)
        for surface in SURFACES:
            got = sorted(n for (s, n), (ok, _, _) in c.items() if s == surface and ok and n not in ("initialize", "tools-list"))
            leaked = [n for (s, n), (_, _, doc) in c.items() if s == surface and doc is not None
                      and self.urn_for(door) in (doc if isinstance(doc, str) else json.dumps(doc))]
            self.put(f"{way}.{surface}.outsider", not got and not leaked,
                     f"answered {got}, entity named in {leaked}" if got or leaked else "nothing answered")
        # a write by B's member: refused (A granted none, B's own doors grant reads only)
        st, raw = self.upsert("b-member", door, {"note": "b-member"})
        self.put(f"{way}.write.write-refused", st in (401, 403, 404, 422) or (st == 0),
                 f"{st}" + ("" if st in (401, 403, 404, 422) else f" {str(raw)[:80]}"))
        if way == "w1":
            st, raw = self.upsert("b-admin", door, {"note": "b-admin"})
            ok_b = st in (200, 201, 204)
            got = self.get("a-admin", "sh-shared", f"/ngsi-ld/v1/entities/{urllib.parse.quote(self.urn, safe='')}")[1]
            landed = isinstance(got, dict) and ((got.get("note") or {}).get("value") == "b-admin")
            self.put("w1.write.write-granted", ok_b and landed, f"{st} {self.brief(self.as_json(raw)) if not ok_b else ''}, "
                     f"read back by A {'equal' if landed else 'unchanged'}",
                     control.get(("write", "a-admin"), (False, "not made")))
        return b

    def subscription(self, way):
        """B's member subscribes on the way's door; A's admin writes; a notification within the
        wait. The control: A's admin's own subscription on A's door, notified by the same write."""
        def sock(key, door):
            st, t = self.token(key, door)
            if not t:
                return None, f"exchange {st}"
            try:
                s = wsclient.Ws(self.base, f"/api/endpoint/{self.slugs[door]}/ws", token=t, timeout=10)
            except wsclient.Refused as r:
                return None, f"upgrade {r.status}"
            except (OSError, wsclient.Closed) as x:
                return None, f"no socket: {type(x).__name__}"
            if not s.until(lambda m: wsclient.op(m) == "x-welcome", 10):
                s.close()
                return None, "no x-welcome"
            s.send({"metadata": {"x-op": "x-subscribe", "x-id": "s"}, "body": {"type": "Subscription",
                                                                              "entities": [{"type": TYPE}]}})
            m = s.until(lambda m: wsclient.op(m) == "x-result", 10)
            status = int(((m or {}).get("metadata") or {}).get("x-status") or 0)
            if status != 201:
                s.close()
                return None, f"x-subscribe {status or 'no answer'}"
            return s, None

        def notified(s, box, key):
            try:
                box[key] = bool(s.until(lambda m: wsclient.op(m) == "x-notification"
                                         and any(u in json.dumps(m) for u in (self.urn, self.copy_urn)), self.a.wait))
            except (OSError, wsclient.Closed):
                box[key] = False

        socks = {"b": sock("b-member", self.door(way)), "a": sock("a-admin", self.control_door(way))}
        box: dict = {}
        threads = [threading.Thread(target=notified, args=(s, box, k)) for k, (s, _) in socks.items() if s]
        for t in threads:
            t.start()
        time.sleep(2)  # the hub's broker subscription is made after the 201
        st, _ = self.upsert("a-admin", "sh-shared", {"level": "9", "note": f"notify {way}", HIDDEN: "kept"})
        for t in threads:
            t.join()
        for s, _ in socks.values():
            if s:
                s.close()
        ctl = (box.get("a", False), f"A's write {st}; A's socket {socks['a'][1] or ('notified' if box.get('a') else 'nothing')}")
        b_why = socks["b"][1] or ("notified" if box.get("b") else f"nothing within {self.a.wait:g} s")
        self.put(f"{way}.ngsi-ld.subscription", bool(box.get("b")), b_why, ctl)

    def mirror(self):
        """DM-48: the foreign DataModel mirrored into B's project, proposed as a merge request whose
        file pins the peer's sha256. The same row for every way: one mirror serves all three."""
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        h = self.client()
        deadline = time.time() + self.a.mirror_wait
        found = "no mirror merge request"
        while time.time() < deadline:
            st, pulls = h.json("GET", f"/git/api/v1/repos/{mo.CONFIG[0]}/{PB}/pulls?state=all&limit=50", headers=auth)
            pr = next((p for p in pulls or [] if isinstance(p, dict) and str(p.get("title", "")).startswith("mirror")), None) \
                if st == 200 and isinstance(pulls, list) else None
            if pr:
                ref = (pr.get("head") or {}).get("ref", "")
                st, tree = h.json("GET", f"/git/api/v1/repos/{mo.CONFIG[0]}/{PB}/git/trees/{urllib.parse.quote(ref, safe='')}?recursive=true",
                                  headers=auth)
                paths = [e["path"] for e in (tree or {}).get("tree", []) if isinstance(e, dict) and "datamodels" in e.get("path", "")
                         and e["path"].endswith(".yaml")] if st == 200 else []
                pinned = any(re.search(r"sha256:\s*\S{16,}", self.branch_file(h, auth, ref, p) or "") for p in paths)
                found = f"merge request on {ref}, {len(paths)} model files, sha256 {'pinned' if pinned else 'missing'}"
                ok = pinned
                break
            time.sleep(10)
        else:
            ok = False
        for way in WAYS:
            self.put(f"{way}.linkml.mirror", ok, found)

    @staticmethod
    def branch_file(h, auth, ref, path):
        st, cur = h.json("GET", f"/git/api/v1/repos/{mo.CONFIG[0]}/{PB}/contents/{path}?ref={urllib.parse.quote(ref, safe='')}",
                         headers=auth)
        return base64.b64decode(cur.get("content", "")).decode() if st == 200 and isinstance(cur, dict) else None

    def store_refused(self):
        """SHR-03 at plan time: B's admin's dry run of a second copy Pipeline, whose account
        `pl-sh-nostore` A grants no `store`, is refused on its source with a reason naming it."""
        body = {"apiVersion": "joinedcontext.com/v1alpha1", "kind": "Pipeline",
                "metadata": {"name": "sh-nostore", "namespace": PB},
                "spec": {"class": "auto", "period": "30s",
                         "source": {"endpointRef": {"kind": "Endpoint", "name": "sh-shared", "namespace": PA},
                                    "query": {"type": TYPE}},
                         "compute": {"kind": "bloblang", "bloblang": "root = this"},
                         "targetEndpoint": f"urn:ngsi-ld:Endpoint:{self.domain_b}:sh-copy:sh-copy",
                         "output": {"type": TYPE, "mode": "upsert"}}}
        st, doc = self.session("b-admin").json("POST", f"/api/v1/projects/{PB}/pipelines?dryRun=All", body=body)
        text = json.dumps(doc, ensure_ascii=False) if doc is not None else ""
        refused = isinstance(doc, dict) and doc.get("valid") is False or st == 422
        # the door judges the second shape: `spec.source` reads as `spec.sources[0]` (PL-54)
        found = re.search(r"spec\.sources?(\[0\])?\.endpointRef[^}]*", text)
        named = bool(found) and "store" in found.group(0) and f"{PB}/pl-sh-nostore" in found.group(0)
        self.put("w3.plan.store-refused", refused and named, f"{st}, {(found.group(0) if found else text)[:400]}")

    def w3_arrived(self) -> bool:
        """The Pipeline's copy reached B's space (the A-admin control read of sh-copy is not one:
        A's admin has no right in B), polled up to --copy-wait."""
        deadline = time.time() + self.a.copy_wait
        while time.time() < deadline:
            st, doc = self.get("b-member", "sh-copy", f"/ngsi-ld/v1/entities?type={TYPE}")
            if st == 200 and isinstance(doc, list) and any(isinstance(e, dict) and e.get("id") == self.copy_urn for e in doc):
                return True
            time.sleep(10)
        return False

    def revoke(self):
        """A revokes: sh-b out of allowedProjects, the share and public Policies ended. Measures
        per w1/w2 surface the seconds until B's member is refused; the w3 copy is recorded."""
        until = (datetime.now(UTC) - timedelta(minutes=1)).strftime("%Y-%m-%dT%H:%M:%SZ")
        files = [f for f in self.a_files(revoke=until) if "/endpoints/sh-shared" in f[0] or "/policies/sh-share" in f[0]
                 or "/policies/sh-public" in f[0]]
        self.commit(files, "sharing smoke: A revokes the share", repo=(mo.CONFIG[0], PA))
        start = time.time()
        probes = {"ngsi-ld": lambda k, d: self.get(k, d, f"/ngsi-ld/v1/entities?type={TYPE}"),
                  "mcp": lambda k, d: self.mcp(k, d, "tools/call", {"name": "query_entities", "arguments": {"type": [TYPE]}}),
                  "linkml": lambda k, d: self.get(k, d, "/schema/v1/model.linkml.yaml")}
        # w2 serves B's own model (B's DataModel, B's Policy): A's revocation does not reach it
        pending = {(w, s) for w in WAYS if w != "w3" for s in SURFACES if not (w.startswith("w2") and s == "linkml")}
        took: dict = {}
        while pending and time.time() - start < self.a.revoke_window:
            self.tok.clear()  # a fresh exchange each pass: an old token proves nothing about the grant
            for way, surface in sorted(pending):
                st, doc = probes[surface]("b-member", self.door(way))
                text = doc if isinstance(doc, str) else json.dumps(doc)
                if surface == "mcp":
                    gone = not mcp_ok(st, doc) or self.urn not in mcp_text(doc)
                elif surface == "ngsi-ld":
                    gone = st != 200 or self.urn not in text
                else:
                    gone = st != 200
                if gone:
                    took[(way, surface)] = (time.time() - start, st)
                    pending.discard((way, surface))
            time.sleep(2)
        for way in WAYS:
            if way == "w3":
                continue
            for surface in SURFACES:
                if way.startswith("w2") and surface == "linkml":
                    self.put(f"{way}.linkml.revoked", True, "B's own model and schema surface: A's revocation does not reach them",
                             (True, "not a read of A"))
                    continue
                t = took.get((way, surface))
                # a read that never answered proves nothing about a revocation: blind
                before = next((r for r in self.matrix if r["id"] == f"{way}.{surface}.{'query' if surface == 'ngsi-ld' else 'query-entities' if surface == 'mcp' else 'schema'}"), None)
                self.put(f"{way}.{surface}.revoked", t is not None,
                         f"refused after {t[0]:.0f} s ({t[1]})" if t else f"still answering after {self.a.revoke_window} s",
                         (bool(before and before["result"] == "pass"), f"before the revocation: {before['actual'] if before else 'not read'}"))
        st, doc = self.get("b-member", "sh-copy", f"/ngsi-ld/v1/entities?type={TYPE}")
        remains = st == 200 and self.copy_urn in json.dumps(doc)
        for surface in SURFACES:
            self.put(f"w3.{surface}.copy-remains", True,
                     f"the copy {'remains' if remains else 'is gone'} in B ({st}): a replica outlives the grant by design")

    # ---- the run
    def run(self):
        try:
            if not self.setup():
                if self.skipped:
                    return
                self.lacking("setup: A's admin could not write the entity within the sync timeout")
                return
            control = {}
            for way in WAYS:
                ctl = self.reads("a-admin", self.control_door(way))
                st, _raw = self.upsert("a-admin", "sh-shared", {"level": "8", "note": "second", HIDDEN: "kept"})
                ctl[("write", "a-admin")] = (st in (200, 201, 204), f"A's own write {st}", None)
                control[way] = ctl
            data = any(self.wanted(f"w3.{c}") for c in ("ngsi-ld.query", "ngsi-ld.retrieve", "mcp.get-entity"))
            copied = data and self.w3_arrived()
            if data and not copied:
                st, pl = self.session("b-admin").json("GET", f"/api/v1/projects/{PB}/pipelines/sh-copy")
                why = json.dumps((pl or {}).get("status"), ensure_ascii=False)[:300] if isinstance(pl, dict) else str(st)
                self.row("setup.w3-copy", f"{self.org_b}/{PB}", "B's Pipeline copies A's entity", "copied", "nothing within "
                         f"{self.a.copy_wait} s; pipeline {st} status {why}", False, "every w3 read below fails for this reason")
            if self.wanted("w3.plan"):
                self.store_refused()
            for way in WAYS:
                self.check_way(way, control[way])
                self.subscription(way)
            if self.wanted(".linkml.mirror"):
                self.mirror()
            if self.wanted(".revoked") or self.wanted(".copy-remains"):
                self.revoke()
        except (RuntimeError, OSError) as x:
            self.row("run", "all", "the run", "completes", f"{type(x).__name__}: {str(x)[:160]}", False)
            self.lacking(f"the run stopped: {type(x).__name__}")
        finally:
            try:
                self.cleanup()
            except (RuntimeError, OSError) as x:
                self.row("cleanup", "all", "cleanup", "none left", f"{type(x).__name__}: {str(x)[:160]}", False)
            self.tok.clear()

    def write(self, out: Path, lane: set | None) -> int:
        rows = [] if self.skipped else settle(self.matrix, load_known(), lane)
        checks = [{"id": r["id"], "target": r["way"], "call": f"{r['surface']} {r['check']}",
                   "expected": "pass" + (" (control answered)" if r["control"] else ""),
                   "actual": r["actual"] + (f"; control {r['controlActual']}" if r["controlActual"] else ""),
                   "result": "pass" if r["status"].startswith("pass") else ("fail" if r["red"] else "skip"),
                   "reason": r["known"] or ("outside lane" if r["status"] == "outside lane" else r["reason"])}
                  for r in rows] + self.rows
        out.mkdir(parents=True, exist_ok=True)
        (out / "rows.json").write_text(json.dumps(rows, indent=1))
        (out / "results.json").write_text(json.dumps({"suite": "sharing", "base": self.base, "checks": checks}, indent=1))
        (out / "REPORT.md").write_text(report(rows, self.rows))
        return sum(1 for c in checks if c["result"] == "fail")


def report(rows: list[dict], setup: list[dict]) -> str:
    """The matrix (way x surface: one column per check) and the red cells with their answers."""
    lines = ["# sharing smoke", "", "| way | surface | " + "check | status | actual" + " |", "|---|---|---|---|---|"]
    for r in rows:
        lines.append(f"| {r['way']} | {r['surface']} | {r['check']} | {r['status']} | "
                     f"{str(r['actual']).replace('|', '/')[:160]}{'; control ' + str(r['controlActual'])[:80] if r['controlActual'] else ''} |")
    lines += ["", "## Setup and cleanup", ""] + [f"- {r['id']}: {r['result']} ({r['actual']})" for r in setup]
    return "\n".join(lines) + "\n"


def main(argv):
    ap = argparse.ArgumentParser(prog="run sharing")
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--export", required=True, help="the gate's bundle (A, B and C are Organizations of it)")
    ap.add_argument("--state", required=True, help="the rehearsal's state dir (realm admin client, forge token)")
    ap.add_argument("--admin-token-file", help="unused: persons are made through --state")
    ap.add_argument("--person-token-file", help="unused: persons are made through --state")
    ap.add_argument("--provider", default="hel", help="Organization A (default hel, the gate bundle's)")
    ap.add_argument("--consumer", default="zilina", help="Organization B (default zilina)")
    ap.add_argument("--outsider", default="barcelona", help="Organization C, which gets nothing (default barcelona)")
    ap.add_argument("--lane", action="store_true", help="judge only the cells of lane.json (the train's smoke lane)")
    ap.add_argument("--sync-timeout", type=int, default=300)
    ap.add_argument("--copy-wait", type=int, default=150, help="seconds for B's Pipeline to copy A's entity")
    ap.add_argument("--mirror-wait", type=int, default=150, help="seconds for the DM-48 mirror merge request")
    ap.add_argument("--revoke-window", type=int, default=120, help="seconds a revocation may take (Portal sync + PDP)")
    ap.add_argument("--wait", type=float, default=15, help="seconds a notification may take")
    a = ap.parse_args(argv)
    R.loopback_only(a.base_url)
    lane = set(json.loads(LANE.read_text())["cells"]) if a.lane else None
    s = Suite(a, lane)
    s.run()
    fails = s.write(a.out, lane)
    red = [r["id"] for r in s.matrix if r.get("red")]
    print(json.dumps({"cells": len(s.matrix), "red": len(red), "fail": fails}))
    return 1 if fails else 0
