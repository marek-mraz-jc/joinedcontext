"""FederationReset.py names the suite context the runner serves, never forge, when one is set (T-3678)."""
import importlib.util
import os
import sys
import types
import unittest
from unittest import mock

LIBRARY = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../../ngsi-ld-test-suite/libraries/FederationReset.py")


def load():
    # the reset's HTTP client is not needed to read its Link header
    with mock.patch.dict(sys.modules, {"requests": types.ModuleType("requests")}):
        spec = importlib.util.spec_from_file_location("federation_reset", LIBRARY)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return module


class FederationResetContext(unittest.TestCase):
    def test_the_served_context_is_the_one_in_the_link_header(self):
        local = "http://localhost:8079/ngsi-ld-test-suite-compound.jsonld"
        with mock.patch.dict(os.environ, {"SUITE_CONTEXT": local}):
            link = load()._LINK["Link"]
        self.assertTrue(link.startswith(f"<{local}>;"), link)
        self.assertNotIn("forge.etsi.org", link)

    def test_without_a_runner_it_names_the_suites_own_url(self):
        with mock.patch.dict(os.environ, {"SUITE_CONTEXT": ""}):
            link = load()._LINK["Link"]
        self.assertIn("forge.etsi.org", link)
        self.assertIn("ngsi-ld-test-suite-compound.jsonld", link)


if __name__ == "__main__":
    unittest.main()
