"""Self-check of ratchet.py and the committed forge-latest list."""
import importlib.util
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("ratchet", os.path.join(HERE, "ratchet.py"))
ratchet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ratchet)

FORGE_LIST = os.path.join(HERE, "forge-latest-ratchet.txt")


def write(d, name, text):
    path = os.path.join(d, name)
    with open(path, "w") as fh:
        fh.write(text)
    return path


def run_csv(d, rows):
    body = "suite,source,test,status,tags\n" + "".join(f"S,s.robot,{t},{s},\n" for t, s in rows)
    return write(d, "tp-status.csv", body)


class Ratchet(unittest.TestCase):
    LIST = "# comment\n\n001_01  test bug: x; forge\n002_01  test bug: y; forge\n"

    def verdict(self, rows, partial=False):
        with tempfile.TemporaryDirectory() as d:
            listed = ratchet.load_list(write(d, "list", self.LIST))
            return ratchet.check(listed, ratchet.load_run(run_csv(d, rows)), partial)[0]

    def test_the_listed_failures_alone_are_green(self):
        self.assertEqual(self.verdict([("001_01 A", "FAIL"), ("002_01 B", "FAIL"), ("003_01 C", "PASS")]), [])

    def test_a_planted_regression_is_red(self):
        red = self.verdict([("001_01 A", "FAIL"), ("002_01 B", "FAIL"), ("003_01 C", "FAIL")])
        self.assertEqual(red, ["regression: 003_01 FAIL and is not on the list"])

    def test_a_skip_or_not_run_counts_as_not_passing(self):
        red = self.verdict([("001_01 A", "FAIL"), ("002_01 B", "FAIL"), ("003_01 C", "NOT RUN")])
        self.assertEqual(red, ["regression: 003_01 NOT RUN and is not on the list"])

    def test_a_fixed_tp_must_leave_the_list(self):
        red = self.verdict([("001_01 A", "PASS"), ("002_01 B", "FAIL")])
        self.assertEqual(red, ["fixed: 001_01 passes; take it off the list"])

    def test_a_listed_tp_that_did_not_run_is_red_unless_partial(self):
        rows = [("001_01 A", "FAIL")]
        self.assertEqual(self.verdict(rows),
                         ["did not run: 002_01 is on the list; take it off if the suite dropped it"])
        self.assertEqual(self.verdict(rows, partial=True), [])

    def test_an_empty_run_is_red(self):
        self.assertEqual(self.verdict([]), ["no TP ran (tp-status.csv missing or empty)"])

    def test_a_missing_run_file_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            argv = [write(d, "list", self.LIST), os.path.join(d, "absent.csv")]
            self.assertEqual(ratchet.main(argv), 1)

    def test_a_line_without_triage_or_twice_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaisesRegex(ValueError, "no triage"):
                ratchet.load_list(write(d, "a", "001_01\n"))
            with self.assertRaisesRegex(ValueError, "listed twice"):
                ratchet.load_list(write(d, "b", "001_01  x\n001_01  y\n"))


class ForgeList(unittest.TestCase):
    def test_holds_98_or_fewer_each_with_its_triage(self):
        listed = ratchet.load_list(FORGE_LIST)
        self.assertLessEqual(len(listed), 98)
        self.assertTrue(listed)
        for tp, why in listed.items():
            self.assertRegex(why, r"^(test bug|Antares bug|spec unclear|suites disagree|harness): .+; .+",
                             f"{tp}: a triage line names its bucket, clause and report")

    def test_a_case_flipped_against_the_committed_list_is_red(self):
        listed = ratchet.load_list(FORGE_LIST)
        run = {tp: "FAIL" for tp in listed}
        run["001_01_01"] = "PASS"
        self.assertEqual(ratchet.check(listed, run)[0], [])
        run["001_01_01"] = "FAIL"
        self.assertEqual(ratchet.check(listed, run)[0], ["regression: 001_01_01 FAIL and is not on the list"])


if __name__ == "__main__":
    unittest.main()
