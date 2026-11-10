"""Selftests of minted_cleanup.py against a stub gateway (stdlib unittest; ci.yml runs them):
python3 -m unittest discover -s bins/jc/deploy/minted-cleanup"""

from __future__ import annotations

import http.server
import io
import json
import re
import subprocess
import sys
import tempfile
import threading
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import minted_cleanup as mc

P = "portal.smartcity.joinedcontext.com"
TOKEN = "stub-token-6b1f0c2e9d"
SLUG = "si6epqkx364lprho5uaigutk274r5grb"


def urn(t: str, n: int | str) -> str:
    return f"urn:ngsi-ld:{t}:{P}:helsinki:{n}"


class Stub:
    """A gateway holding entities with the references between them. A delete of an entity another
    stored entity names is refused (the gateway's HeldDelete with a deleteBatch-only grant); an
    absent id answers ResourceNotFound; `fail` names ids answered with another member error."""

    def __init__(self, entities: dict[str, set[str]]):
        self.entities = dict(entities)
        self.named_by: dict[str, set[str]] = {}
        for o, refs in entities.items():
            for r in refs:
                self.named_by.setdefault(r, set()).add(o)
        self.fail: set[str] = set()
        self.status: int | None = None
        self.calls: list[dict] = []
        self.child_pid: int | None = None
        self.cmdlines: list[bytes] = []
        stub = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                ids = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                stub.calls.append({"path": self.path, "ids": ids, "auth": self.headers["Authorization"],
                                   "reason": self.headers["JC-Reason"]})
                if stub.child_pid:
                    stub.cmdlines.append(Path(f"/proc/{stub.child_pid}/cmdline").read_bytes())
                if stub.status:
                    self.send_response(stub.status)
                    self.end_headers()
                    return
                ok, errors = [], []
                for i in ids:
                    held = any(o != i and o in stub.entities for o in stub.named_by.get(i, ()))
                    if i in stub.fail:
                        errors.append({"entityId": i, "error": {"type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", "title": "bad"}})
                    elif i not in stub.entities:
                        errors.append({"entityId": i, "error": {"type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound", "title": "Not found"}})
                    elif held:
                        errors.append({"entityId": i, "error": {"type": "https://uri.etsi.org/ngsi-ld/errors/Conflict", "title": "kept"}})
                    else:
                        ok.append(i)
                for i in ok:
                    del stub.entities[i]
                body = json.dumps({"success": ok, "errors": errors}).encode() if errors else b""
                self.send_response(207 if errors else 204)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class Case(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.state = self.dir / "state"
        self.token = self.dir / "token"
        self.token.write_text(TOKEN + "\n")
        self.token.chmod(0o600)
        self.mem = self.dir / "mem"
        self.mem.write_text("41.5\n")
        self.endpoints(f"helsinki\t{SLUG}\thelsinki\thelsinki\n")
        self.stubs: list[Stub] = []

    def tearDown(self):
        for s in self.stubs:
            s.close()

    def endpoints(self, text: str):
        (self.dir / "e.tsv").write_text(text)

    def listing(self, rows: list[tuple]):
        (self.dir / "l.tsv").write_text("".join("\t".join(r) + "\n" for r in rows))

    def stub(self, rows: list[tuple]) -> Stub:
        s = Stub({r[1]: set(r[4].split()) for r in rows})
        self.stubs.append(s)
        return s

    def args(self, *extra: str, apply_to: Stub | None = None) -> list[str]:
        a = ["run", "--list", str(self.dir / "l.tsv"), "--endpoints", str(self.dir / "e.tsv"),
             "--state", str(self.state), *extra]
        if apply_to:
            a += ["--apply", "--gateway", apply_to.url, "--token-file", str(self.token),
                  "--mem-cmd", f"cat {self.mem}"]
        return a

    def main(self, argv: list[str]) -> tuple[int, str]:
        out = io.StringIO()
        with redirect_stdout(out):
            code = mc.main(argv)
        return code, out.getvalue()


def row(t: str, n, types: str | None = None, expires: str = "", refs: str = "", tenant="helsinki"):
    return (tenant, urn(t, n), types or t, expires, refs)


class Refusals(Case):
    def refused(self, bad: tuple, needle: str):
        rows = [row("Tree", i) for i in range(3)] + [bad]
        self.listing(rows)
        s = self.stub(rows)
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 1, out)
        self.assertIn(needle, out)
        self.assertIn("stopped before any delete", out)
        self.assertEqual(s.calls, [], "a failing row stops the group before any delete")

    def test_an_id_outside_the_prefix_stops_the_group_before_any_delete(self):
        self.refused(("helsinki", "urn:ngsi-ld:Tree:hel.fi:helsinki:1", "Tree", "", ""), "not under")

    def test_an_id_under_a_longer_domain_is_outside_the_prefix(self):
        self.refused(("helsinki", f"urn:ngsi-ld:Tree:{P}.evil:x", "Tree", "", ""), "not under")

    def test_a_type_mismatch_stops_the_group_before_any_delete(self):
        self.refused(("helsinki", urn("Tree", 9), "https://smartdatamodels.org/NoiseZone", "", ""), "is not the row's")

    def test_a_tenant_mismatch_stops_the_group_before_any_delete(self):
        self.endpoints(f"helsinki\t{SLUG}\thelsinki\thelsinki\npraha-mesto\tjw7tffshzip7igmzcszgffkkkuetnc6v\thelsinki\tpraha\n")
        self.refused(row("Tree", 9, tenant="praha-mesto"), "serves the space helsinki")

    def test_a_tenant_without_an_endpoint_stops(self):
        self.refused(row("Tree", 9, tenant="barcelona"), "no Endpoint")

    def test_an_expanded_type_iri_matches_its_short_name(self):
        rows = [row("Tree", 1, types="https://smartdatamodels.org/dataModel.ParksAndGardens/Tree")]
        self.listing(rows)
        self.assertEqual(self.main(self.args())[0], 0)

    def test_a_cycle_stops_before_any_delete(self):
        rows = [row("Tree", 1, refs=urn("Tree", 2)), row("Tree", 2, refs=urn("Tree", 1))]
        self.listing(rows)
        s = self.stub(rows)
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual((code, s.calls), (1, []), out)
        self.assertIn("cycle", out)


class Apply(Case):
    def test_dry_run_sends_nothing_and_counts_the_plan(self):
        rows = [row("Tree", i) for i in range(1201)]
        self.listing(rows)
        code, out = self.main(self.args())
        self.assertEqual(code, 0, out)
        self.assertIn("1201 to delete in 1 layer(s), 3 batch(es)", out)
        self.assertIn("dry run: nothing sent", out)

    def test_batches_hold_at_most_500_ids_and_every_copy_goes(self):
        rows = [row("Tree", i) for i in range(1201)]
        self.listing(rows)
        s = self.stub(rows)
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 0, out)
        self.assertEqual([len(c["ids"]) for c in s.calls], [500, 500, 201])
        self.assertEqual(s.entities, {})
        c = s.calls[0]
        self.assertEqual(c["path"], f"/api/endpoint/{SLUG}/ngsi-ld/v1/entityOperations/delete")
        self.assertEqual(c["auth"], f"Bearer {TOKEN}")
        self.assertEqual(c["reason"], mc.REASON)
        self.assertIn("done: 1201 deleted, 0 already gone", out)

    def test_referencing_copies_go_before_the_copies_they_name(self):
        # 3 -> 2 -> 1 and 4 -> 1: the stub refuses a delete of a still-named entity
        rows = [row("Tree", 1), row("Vehicle", 2, refs=urn("Tree", 1)),
                row("KeyPerformanceIndicator", 3, refs=urn("Vehicle", 2)), row("Tree", 4, refs=urn("Tree", 1))]
        self.listing(rows)
        s = self.stub(rows)
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 0, out)
        order = [c["ids"] for c in s.calls]
        self.assertEqual(order, [[urn("KeyPerformanceIndicator", 3), urn("Tree", 4)], [urn("Vehicle", 2)], [urn("Tree", 1)]])
        self.assertEqual(s.entities, {})

    def test_a_207_member_error_stops_the_run(self):
        rows = [row("Tree", i) for i in range(1200)]
        self.listing(rows)
        s = self.stub(rows)
        s.fail = {urn("Tree", 10)}
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 1, out)
        self.assertEqual(len(s.calls), 1, "no batch after the member error")
        self.assertIn(f"member error on {urn('Tree', 10)}", out)
        done = (self.state / "done.txt").read_text().split()
        self.assertEqual(len(done), 499)
        self.assertNotIn(urn("Tree", 10), done)

    def test_resume_after_a_stop_skips_deleted_ids(self):
        rows = [row("Tree", i) for i in range(1200)]
        self.listing(rows)
        s = self.stub(rows)
        s.fail = {urn("Tree", 10)}
        self.assertEqual(self.main(self.args(apply_to=s))[0], 1)
        s.fail = set()
        s.calls.clear()
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 0, out)
        sent = [i for c in s.calls for i in c["ids"]]
        self.assertEqual(len(sent), 701)
        self.assertIn(urn("Tree", 10), sent)
        self.assertEqual(s.entities, {})
        self.assertIn("499 done before, 701 to delete", out)

    def test_an_expired_copy_answering_not_found_counts_as_done(self):
        rows = [row("Vehicle", 1, expires="2026-11-10T12:00:00Z"), row("Vehicle", 2)]
        self.listing(rows)
        s = self.stub(rows)
        del s.entities[urn("Vehicle", 1)]  # the broker answers 404 for an expired entity
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 0, out)
        self.assertIn("1 deleted, 1 already gone", out)

    def test_a_non_expired_copy_answering_not_found_stops(self):
        # e.g. an Endpoint whose view leaves the type out: Antares answers it as absent
        rows = [row("Tree", 1), row("Tree", 2)]
        self.listing(rows)
        s = self.stub(rows)
        del s.entities[urn("Tree", 1)]
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 1, out)
        self.assertIn("not found though not expired", out)

    def test_after_a_timeout_the_resent_batch_may_answer_not_found(self):
        rows = [row("Tree", 1), row("Tree", 2)]
        self.listing(rows)
        s = self.stub(rows)
        s.status = 502
        self.assertEqual(self.main(self.args(apply_to=s))[0], 1)
        self.assertEqual(sorted((self.state / "inflight.txt").read_text().split()), [urn("Tree", 1), urn("Tree", 2)])
        s.status = None
        del s.entities[urn("Tree", 1)]  # the edge timed out after the broker deleted it
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 0, out)
        self.assertEqual((self.state / "inflight.txt").read_text(), "")

    def test_a_401_stops_and_counts_nothing(self):
        rows = [row("Tree", 1)]
        self.listing(rows)
        s = self.stub(rows)
        s.status = 401
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual(code, 1)
        self.assertIn("refresh the token file", out)
        self.assertFalse((self.state / "done.txt").read_text())

    def test_memory_at_95_percent_stops_before_the_next_batch(self):
        rows = [row("Tree", i) for i in range(600)]
        self.listing(rows)
        s = self.stub(rows)
        self.mem.write_text("NAME CPU MEM%\nnode 3 95%\n")
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual((code, s.calls), (1, []), out)
        self.assertIn("memory 95 %", out)

    def test_a_failing_memory_check_stops(self):
        rows = [row("Tree", 1)]
        self.listing(rows)
        s = self.stub(rows)
        self.mem.unlink()
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual((code, s.calls), (1, []), out)

    def test_a_token_file_others_can_read_is_refused(self):
        rows = [row("Tree", 1)]
        self.listing(rows)
        s = self.stub(rows)
        self.token.chmod(0o644)
        code, out = self.main(self.args(apply_to=s))
        self.assertEqual((code, s.calls), (1, []), out)

    def test_plain_http_to_a_remote_host_is_refused(self):
        with self.assertRaises(mc.Stop):
            mc.check_gateway("http://jc.example.org")
        self.assertEqual(mc.check_gateway("https://jc.example.org/"), "https://jc.example.org")

    def test_the_token_reaches_neither_argv_nor_the_log(self):
        rows = [row("Tree", i) for i in range(3)]
        self.listing(rows)
        s = self.stub(rows)
        proc = subprocess.Popen([sys.executable, str(HERE / "minted_cleanup.py"), *self.args(apply_to=s)],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        s.child_pid = proc.pid
        out, _ = proc.communicate(timeout=60)
        self.assertEqual(proc.returncode, 0, out)
        self.assertEqual(len(s.cmdlines), 1, "the stub read the tool's /proc/<pid>/cmdline mid-run")
        self.assertNotIn(TOKEN.encode(), s.cmdlines[0])
        self.assertIn(b"--token-file", s.cmdlines[0])
        self.assertNotIn(TOKEN, out)
        for f in self.state.iterdir():
            self.assertNotIn(TOKEN, f.read_text(), f.name)
        self.assertEqual(s.calls[0]["auth"], f"Bearer {TOKEN}")


class Policies(Case):
    def test_one_deletebatch_grant_per_space_with_a_selector_per_minted_type(self):
        self.listing([row("Tree", 1), row("NoiseZone", 2), row("Tree", 3)])
        code, out = self.main(["policies", "--list", str(self.dir / "l.tsv"), "--endpoints", str(self.dir / "e.tsv"),
                               "--state", str(self.state), "--until", "2026-11-10T20:00:00Z"])
        self.assertEqual(code, 0, out)
        doc = json.loads((self.state / "policy-helsinki.json").read_text())
        self.assertEqual(doc["metadata"], {"name": "cleanup-t6676", "namespace": "helsinki", "title": doc["metadata"]["title"]})
        spec = doc["spec"]
        self.assertEqual(spec["operations"], ["deleteBatch"])
        self.assertEqual(spec["assignee"], {"kind": "serviceAccount", "id": "cleanup-t6676"})
        self.assertEqual(spec["validity"], {"to": "2026-11-10T20:00:00Z"})
        sel = spec["information"][0]["entities"]
        self.assertEqual([e["type"] for e in sel], ["NoiseZone", "Tree"])
        pat = re.compile(sel[1]["idPattern"])
        self.assertTrue(pat.match(urn("Tree", 1)))
        self.assertFalse(pat.match("urn:ngsi-ld:Tree:hel.fi:helsinki:1"))
        self.assertFalse(pat.match("urn:ngsi-ld:Tree:portalXsmartcity.joinedcontext.com:1"))


class Verify(Case):
    def verify(self, recount: str, history: str) -> tuple[int, str]:
        (self.dir / "r.tsv").write_text(recount)
        (self.dir / "h.tsv").write_text(history)
        return self.main(["verify", "--recount", str(self.dir / "r.tsv"), "--history", str(self.dir / "h.tsv"),
                          "--state", str(self.state)])

    def test_the_history_step_is_skipped_when_no_history_remains(self):
        code, out = self.verify(f"helsinki\t{urn('Vehicle', 1)}\t2026-11-10T12:00:00Z\n", "")
        self.assertEqual(code, 0, out)
        self.assertIn("history step skipped", out)
        self.assertFalse((self.state / "history-for-sweep.tsv").exists())

    def test_remaining_history_lists_expired_copies_for_the_sweep(self):
        code, out = self.verify(f"helsinki\t{urn('Vehicle', 1)}\t2026-11-10T12:00:00Z\n",
                                f"helsinki\t{urn('Vehicle', 1)}\t1\t12\n")
        self.assertEqual(code, 0, out)
        self.assertEqual((self.state / "history-for-sweep.tsv").read_text(), f"helsinki\t{urn('Vehicle', 1)}\n")
        self.assertEqual((self.state / "history-left-after-delete.tsv").read_text(), "")

    def test_history_of_a_deleted_id_and_live_copies_fail(self):
        code, out = self.verify(f"helsinki\t{urn('Tree', 2)}\t\n", f"helsinki\t{urn('Tree', 3)}\t1\t4\n")
        self.assertEqual(code, 1, out)
        self.assertIn("1 non-expired left", out)
        self.assertIn("1 deleted ids whose history stayed", out)


if __name__ == "__main__":
    unittest.main()
