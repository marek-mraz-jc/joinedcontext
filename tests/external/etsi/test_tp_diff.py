"""Self-check of tp-diff.py."""
import importlib.util
import os
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "tp_diff", os.path.join(os.path.dirname(os.path.abspath(__file__)), "tp-diff.py"))
tp_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tp_diff)


def csv_file(d, name, rows):
    path = os.path.join(d, name)
    with open(path, "w") as fh:
        fh.write("suite,source,test,status,tags\n")
        for test, status in rows:
            fh.write(f"S,s.robot,{test},{status},\n")
    return path


class TpDiff(unittest.TestCase):
    def test_counts_and_every_difference(self):
        with tempfile.TemporaryDirectory() as d:
            a = tp_diff.load(csv_file(d, "a", [("001_01 Create", "PASS"), ("001_02 X", "PASS"), ("002_01 Y", "FAIL")]))
            b = tp_diff.load(csv_file(d, "b", [("001_01 Create retitled", "PASS"), ("001_02 X", "FAIL"),
                                               ("003_01 New", "PASS")]))
            self.assertEqual(tp_diff.diff(a, b), [("001_02", "PASS", "FAIL"), ("002_01", "FAIL", "absent"),
                                                 ("003_01", "absent", "PASS")])
            text = tp_diff.report(a, b)
            self.assertIn("| pinned (a) | 3 | 2 | 1 | 0 | 0 |", text)
            self.assertIn("TPs that differ from (a): 3", text)
            self.assertIn("| 002_01 | FAIL | absent |", text)

    def test_a_missing_run_is_reported_not_raised(self):
        a = {"001_01": "PASS"}
        text = tp_diff.report(a, tp_diff.load("/nonexistent/tp-status.csv"))
        self.assertIn("| latest (b) | no results |", text)
        self.assertIn("TPs that differ from (a): 1", text)

    def test_equal_runs_have_no_table(self):
        text = tp_diff.report({"001_01": "PASS"}, {"001_01": "PASS"})
        self.assertIn("TPs that differ from (a): 0", text)
        self.assertNotIn("| TP |", text)


if __name__ == "__main__":
    unittest.main()
