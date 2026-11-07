"""Self-check of compare.py (run: python3 -m unittest discover -s tests/external -p 'test_*.py')."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import compare  # noqa: E402

EXPECT = """01. GET the entity
==================
HTTP/1.1 200 OK
Date: REGEX(.*)
Content-Type: application/json
Content-Length: REGEX(\\d+)

{
  "id": "urn:ngsi-ld:Spot:1",
  "location": {"type": "GeoProperty", "value": {"coordinates": [-3.803561677, 43.462966417], "type": "Point"}},
  "n": REGEX(\\d+),
  "type": "Spot"
}


02. error
=========
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "detail": "Coraine wording",
  "title": "Entity Not Found",
  "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
}
"""

ACTUAL = """01. GET the entity
==================
HTTP/1.1 200 OK
date: Sun, 11 Oct 2026 02:00:00 GMT
content-type: application/json; charset=utf-8
vary: accept
content-length: 999

{
  "id": "urn:ngsi-ld:Spot:1",
  "location": {"type": "GeoProperty", "value": {"coordinates": [-3.80356167695194, 43.46296641666926], "type": "Point"}},
  "n": 42,
  "type": "Spot"
}

02. error
=========
HTTP/1.1 404 Not Found
content-type: application/json

{
  "detail": "entity urn:ngsi-ld:Spot:2 not found",
  "title": "Not found",
  "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
}
"""


class Normalised(unittest.TestCase):
    def test_coraine_only_text_is_set_aside(self):
        self.assertIsNone(compare.normalised_diff(EXPECT, ACTUAL))

    def test_a_different_status_is_a_failure(self):
        self.assertIn('"status"', compare.normalised_diff(EXPECT, ACTUAL.replace("404 Not Found", "400 Bad Request")))

    def test_a_different_problem_type_is_a_failure(self):
        self.assertIsNotNone(compare.normalised_diff(EXPECT, ACTUAL.replace("ResourceNotFound", "BadRequestData")))

    def test_detail_counts_outside_a_problem_details_body(self):
        exp = 'HTTP/1.1 200 OK\n\n{"detail": "a", "type": "Thing"}\n'
        self.assertIsNotNone(compare.normalised_diff(exp, exp.replace('"a"', '"b"')))

    def test_a_missing_or_extra_member_is_a_failure(self):
        self.assertIsNotNone(compare.normalised_diff(EXPECT, ACTUAL.replace('"n": 42,', "")))
        self.assertIsNotNone(compare.normalised_diff(EXPECT, ACTUAL.replace('"n": 42,', '"n": 42, "x": 1,')))

    def test_a_bare_regex_does_not_match_a_non_number(self):
        self.assertIsNotNone(compare.normalised_diff(EXPECT, ACTUAL.replace('"n": 42', '"n": "x"')))

    def test_arrays_match_in_any_order_but_not_by_count(self):
        exp = 'HTTP/1.1 200 OK\n\n[{"id": "a"}, {"id": "b"}]\n'
        self.assertIsNone(compare.normalised_diff(exp, exp.replace('"a"}, {"id": "b"', '"b"}, {"id": "a"')))
        self.assertIsNotNone(compare.normalised_diff(exp, 'HTTP/1.1 200 OK\n\n[{"id": "a"}]\n'))
        self.assertIsNotNone(compare.normalised_diff(exp, 'HTTP/1.1 200 OK\n\n[{"id": "a"}, {"id": "a"}]\n'))
        twice = 'HTTP/1.1 200 OK\n\n[{"id": "a"}, {"id": "a"}]\n'
        self.assertIsNotNone(compare.normalised_diff(twice, 'HTTP/1.1 200 OK\n\n[{"id": "b"}, {"id": "a"}]\n'))

    def test_a_missing_or_extra_kept_header_is_a_failure(self):
        exp = "HTTP/1.1 201 Created\nLocation: /ngsi-ld/v1/entities/urn:a\n\n"
        self.assertIsNone(compare.normalised_diff(exp, exp.replace("/ngsi", "http://localhost:1026/ngsi")))
        self.assertIsNotNone(compare.normalised_diff(exp, "HTTP/1.1 201 Created\n\n"))
        self.assertIsNotNone(compare.normalised_diff("HTTP/1.1 201 Created\n\n", exp))

    def test_text_lines_and_empty_outputs(self):
        self.assertIsNone(compare.normalised_diff("", ""))
        self.assertIsNotNone(compare.normalised_diff("count: 3\n", "count: 4\n"))
        self.assertIsNone(compare.normalised_diff("count: REGEX([0-9]+)\n", "count: 4\n"))
        self.assertIsNotNone(compare.normalised_diff("a\n", ""))

    def test_a_kept_header_printed_as_text_ignores_the_name_case(self):
        # RFC 9110 5.1: field names are case-insensitive; the value still counts
        exp = "NGSILD-Results-Count: 50\n"
        self.assertIsNone(compare.normalised_diff(exp, "Ngsild-Results-Count: 50\n"))
        self.assertIsNotNone(compare.normalised_diff(exp, "Ngsild-Results-Count: 51\n"))
        self.assertIsNotNone(compare.normalised_diff("Count: 3\n", "count: 3\n"))

    def test_a_list_representation_id_is_the_implementations(self):
        # TS 104 175 5.2.6.10: "URI that is unique within the system scope"
        exp = '{"id": "urn:ngsi-ld:EntityTypeList:local", "type": "EntityTypeList", "typeList": ["Car"]}\n'
        act = '{"id": "urn:ngsi-ld:EntityTypeList:9b1c", "type": "EntityTypeList", "typeList": ["Car"]}\n'
        self.assertIsNone(compare.normalised_diff(exp, act))
        self.assertIsNotNone(compare.normalised_diff(exp, act.replace("Car", "Bus")))
        self.assertIsNotNone(compare.normalised_diff(exp.replace("EntityTypeList", "T"), act.replace("EntityTypeList", "T")))

    def test_a_body_that_is_not_json_is_compared_as_text(self):
        self.assertIsNone(compare.normalised_diff("HTTP/1.1 200 OK\n\n{not json\n", "HTTP/1.1 200 OK\n\n{not json\n"))
        self.assertIsNotNone(compare.normalised_diff("HTTP/1.1 200 OK\n\n{not json\n", "HTTP/1.1 200 OK\n\n{not jsox\n"))

    def test_booleans_are_not_numbers(self):
        self.assertFalse(compare.json_equal(1, True))
        self.assertFalse(compare.json_equal(True, 1))
        self.assertTrue(compare.json_equal(False, False))


class Regex(unittest.TestCase):
    def test_balanced_parentheses_classes_and_escapes(self):
        self.assertTrue(compare.text_matches("id REGEX((a|b)[)]\\)) end", "id b)) end"))
        self.assertFalse(compare.text_matches("REGEX(a", "a"))
        self.assertFalse(compare.text_matches("REGEX([)", "x"))
        self.assertTrue(compare.text_matches("x.y", "x.y"))
        self.assertFalse(compare.text_matches("x.y", "xzy"))

    def test_a_regex_inside_a_json_string_is_kept(self):
        self.assertEqual(compare.quote_bare_regex('{"a": "REGEX(.*)"}'), '{"a": "REGEX(.*)"}')
        self.assertEqual(json.loads(compare.quote_bare_regex('{"a": REGEX(\\d+)}'))["a"], compare.RX + "\\d+")
        self.assertEqual(compare.quote_bare_regex('{"a": "\\"REGEX("}'), '{"a": "\\"REGEX("}')

    def test_a_received_request_compares_its_ngsi_ld_headers_only(self):
        # a dump of what a receiver got: the client's own transport headers
        # (User-Agent, Host, Content-Length, Accept) are the implementation's
        exp = {"verb": "POST", "url": "/n", "body": {}, "headers": {
            "Content-Type": "application/json", "NGSILD-Tenant": "t",
            "User-Agent": "REGEX(Coraine/.*)", "Host": "localhost:7701",
            "Content-Length": "REGEX(\\d+)", "Accept": "application/json"}}
        act = {"verb": "POST", "url": "/n", "body": {}, "headers": {
            "content-type": "application/json", "ngsild-tenant": "t", "via": "1.1 antares"}}
        self.assertTrue(compare.json_equal(exp, act))
        act["headers"]["ngsild-tenant"] = "u"
        self.assertFalse(compare.json_equal(exp, act))
        del act["headers"]["ngsild-tenant"]
        self.assertFalse(compare.json_equal(exp, act))

    def test_link_headers_are_a_set(self):
        # RFC 8288 orders no links; the query of a next link is the broker's to order
        exp = ('HTTP/1.1 200 OK\nLink: </e?limit=1&offset=1>;rel="next"\n'
               'Link: <https://c>; rel="ctx"\n\n')
        act = ('HTTP/1.1 200 OK\nLink: <https://c>; rel="ctx"\n'
               'Link: </e?offset=1&limit=1>; rel="next"\n\n')
        self.assertIsNone(compare.normalised_diff(exp, act))
        self.assertIsNotNone(compare.normalised_diff(exp, act.replace("offset=1", "offset=2")))

    def test_one_link_header_may_join_several_link_values(self):
        # RFC 8288 3: Link = #link-value, so one header line is a comma list
        exp = ('HTTP/1.1 200 OK\nLink: </e?offset=0>; rel="prev"\n'
               'Link: </e?offset=2>; rel="next"\n\n')
        act = 'HTTP/1.1 200 OK\nLink: </e?offset=2>; rel="next", </e?offset=0>; rel="prev"\n\n'
        self.assertIsNone(compare.normalised_diff(exp, act))
        self.assertIsNotNone(compare.normalised_diff(exp, act.replace("offset=0", "offset=1")))

    def test_a_bare_regex_glued_to_digits_or_ending_a_member(self):
        body = compare.quote_bare_regex('{"d": 85REGEX(\\.\\d+),\n "e": REGEX(.*)\n "f": 1}')
        obj = json.loads(body)
        self.assertTrue(compare.json_equal(obj["d"], 85.25))
        self.assertFalse(compare.json_equal(obj["d"], 86.25))
        self.assertTrue(compare.json_equal(obj["e"], "2026-01-01T00:00:00Z"))
        self.assertEqual(obj["f"], 1)
        self.assertEqual(body.count("\n"), 2)

    def test_a_bare_regex_with_quotes_matches_the_json_text(self):
        obj = json.loads(compare.quote_bare_regex('{"q": REGEX("speed>.*")}'))
        self.assertTrue(compare.json_equal(obj["q"], "speed>5"))
        self.assertFalse(compare.json_equal(obj["q"], "speed<5"))

    def test_a_case_pinned_to_one_coraine_backend(self):
        one = "--NAME--\nx\n# REQUIRE_DB: corDB\n--TAGS--\nentity\n"
        v, why = compare.verdict("/nonexistent/x", one, {}, {})
        self.assertEqual(v, "not-applicable")
        self.assertIn("corDB", why)
        both = one.replace("corDB", "mongoc corDB")
        self.assertNotEqual(compare.verdict("/nonexistent/x", both, {}, {})[0], "not-applicable")

    def test_a_not_updated_reason_is_free_text(self):
        # 5.2.19 NotUpdatedDetails "reason": "Reason for not having changed such Attribute"
        exp = {"attributeName": "p", "reason": "Attribute exists"}
        self.assertTrue(compare.json_equal(exp, {"attributeName": "p", "reason": "noOverwrite"}))
        self.assertFalse(compare.json_equal(exp, {"attributeName": "q", "reason": "x"}))

    def test_a_regex_backslash_inside_a_json_string_parses(self):
        # Coraine writes "REGEX(\d+)" inside strings: not a JSON escape
        body = compare.quote_bare_regex('{"a": "REGEX(\\d+)", "b": "x\\ny"}')
        self.assertEqual(json.loads(body), {"a": "REGEX(\\d+)", "b": "x\ny"})
        self.assertTrue(compare.json_equal(json.loads(body)["a"], "12"))


class Verdicts(unittest.TestCase):
    def case(self, d, name, tags="entity", **files):
        with open(os.path.join(d, name + ".test"), "w") as fh:
            fh.write(f"--NAME--\nx\n\n--TAGS--\n{tags}\n\n--RUN--\necho\n--EXPECT--\nx\n--TEARDOWN--\n")
        for ext, body in files.items():
            with open(os.path.join(d, f"{name}.{ext.replace('_', '.')}"), "w") as fh:
                fh.write(body)

    def test_every_verdict(self):
        with tempfile.TemporaryDirectory() as d:
            cases = os.path.join(d, "cases")
            os.mkdir(cases)
            na = os.path.join(d, "na.txt")
            with open(na, "w") as fh:
                fh.write("# comment\n\ntag:bridge its bridges\nname:only_coraine its CLI\n"
                         "tag@memory:persist restarts\ntag@file:entity never here\n")
            self.case(cases, "a_pass", expect="x\n", out="x\n", diff="")
            self.case(cases, "b_norm", expect="HTTP/1.1 200 OK\nDate: x\n\n", out="HTTP/1.1 200 OK\nDate: y\n\n", diff="d")
            self.case(cases, "c_fail", expect="HTTP/1.1 200 OK\n\n", out="HTTP/1.1 500 Oops\n\n", diff="d")
            self.case(cases, "d_bridge", tags="bridge entity")
            self.case(cases, "only_coraine")
            self.case(cases, "e_flag", init="x", init_stderr="CORAINE-ONLY --apiPlugins\n")
            self.case(cases, "f_err", init="x", out="", run_stderr="boom\n")
            self.case(cases, "g_skip")
            self.case(cases, "h_initfail", init="x")
            self.case(cases, "i_persist", tags="persist", expect="x\n", out="x\n", diff="")
            counts = compare.main(cases, na, "memory", os.path.join(d, "out"))
            self.assertEqual(counts, {"pass": 1, "pass-normalised": 1, "fail": 1,
                                      "not-applicable": 4, "error": 2, "skipped": 1})
            with open(os.path.join(d, "out", "coraine-report.json")) as fh:
                rows = {r["case"]: r for r in json.load(fh)["cases"]}
            self.assertIn("--apiPlugins", rows["e_flag"]["reason"])
            self.assertIn('"500"', rows["c_fail"]["reason"])
            with open(os.path.join(d, "out", "coraine-report.md")) as fh:
                md = fh.read()
            self.assertIn("| c_fail | fail |", md)
            self.assertNotIn("a_pass", md)
            self.assertIn("restarts", rows["i_persist"]["reason"])
            self.assertEqual(compare.main(cases, na, "postgres", os.path.join(d, "pg"))["pass"], 2)

    def test_a_malformed_list_line_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "na.txt")
            with open(p, "w") as fh:
                fh.write("bridge no kind\n")
            with self.assertRaises(ValueError):
                compare.load_na(p, "file")


if __name__ == "__main__":
    unittest.main()
