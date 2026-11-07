"""Self-check of refusals.py."""
import importlib.util
import json
import os
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "refusals", os.path.join(os.path.dirname(os.path.abspath(__file__)), "refusals.py"))
refusals = importlib.util.module_from_spec(spec)
spec.loader.exec_module(refusals)

CLEAN = {"type": "urn:antares:error:AccessDenied", "title": "AccessDenied", "status": 403,
         "detail": "a policy prohibits this operation"}


def log(d, bodies):
    text = "".join(f"Response ->\n{json.dumps({'url': 'u', 'status_code': s, 'body': b}, indent=4)}\n"
                   for s, b in bodies)
    with open(os.path.join(d, "Consumption-console.log"), "w") as fh:
        fh.write("Request ->\n{}\n" + text)
    return d


class Refusals(unittest.TestCase):
    def test_a_clean_403_is_green_and_other_statuses_are_not_read(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(refusals.main(log(d, [(403, CLEAN), (404, {"id": "urn:ngsi-ld:Vehicle:1"})])), 0)

    def test_a_403_naming_an_entity_is_red(self):
        leak = {**CLEAN, "detail": "urn:ngsi-ld:Vehicle:1 is prohibited"}
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(refusals.main(log(d, [(403, leak)])), 1)
        extra = {**CLEAN, "entity": {"id": "urn:ngsi-ld:Vehicle:1"}}
        self.assertTrue(refusals.problems({"body": extra}))

    def test_a_run_without_any_refusal_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(refusals.main(log(d, [(200, {"id": "x"})])), 1)


if __name__ == "__main__":
    unittest.main()
