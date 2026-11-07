"""Self-check of forge-free.sh on a throwaway git checkout (no network)."""
import os
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "forge-free.sh")
FORGE = "https://forge.etsi.org/rep/cim/ngsi-ld-test-suite/-/raw/develop/"
GITHUB = "https://raw.githubusercontent.com/joinedcontext/Antares-NGSI-LD-Context-Broker/dev/ngsi-ld-test-suite/"


def git(d, *args):
    return subprocess.run(["git", "-C", d, "-c", "user.name=t", "-c", "user.email=t@t", *args],
                          capture_output=True, text=True, check=True).stdout


class ForgeFree(unittest.TestCase):
    def suite(self, d, files):
        os.makedirs(f"{d}/resources/jsonld-contexts")
        for name, body in files.items():
            os.makedirs(os.path.dirname(f"{d}/{name}"), exist_ok=True)
            with open(f"{d}/{name}", "w") as fh:
                fh.write(body)
        git(d, "init", "-q")
        git(d, "add", "-A")
        git(d, "commit", "-qm", "suite")

    def run_it(self, d):
        return subprocess.run(["bash", SCRIPT, d, "http://127.0.0.1:8099"], capture_output=True, text=True)

    def test_every_remote_prefix_is_rewritten_and_committed(self):
        with tempfile.TemporaryDirectory() as d:
            self.suite(d, {
                "resources/variables.py": f"ctx = '{GITHUB}resources/jsonld-contexts/c.jsonld'\n",
                "data/e.jsonld": f'{{"@context": ["{FORGE}resources/jsonld-contexts/c.jsonld"]}}\n',
                "libraries/Reset.py": f'_C = (\n    "{FORGE}"\n    "resources/jsonld-contexts/c.jsonld"\n)\n',
                "README.md": "https://forge.etsi.org/rep/cim/ngsi-ld-test-suite/-/blob/master/TP\n",
            })
            r = self.run_it(d)
            self.assertEqual(r.returncode, 0, r.stderr)
            with open(f"{d}/resources/variables.py") as fh:
                self.assertEqual(fh.read(), "ctx = 'http://127.0.0.1:8099/resources/jsonld-contexts/c.jsonld'\n")
            with open(f"{d}/libraries/Reset.py") as fh:
                self.assertIn('"http://127.0.0.1:8099/"', fh.read())
            with open(f"{d}/README.md") as fh:
                self.assertIn("/-/blob/master/TP", fh.read())
            self.assertEqual(git(d, "status", "--porcelain"), "")
            self.assertIn("forge-free", git(d, "log", "-1", "--format=%s"))
            # restoring a file keeps the rewrite
            git(d, "checkout", "--", "resources/variables.py")
            with open(f"{d}/resources/variables.py") as fh:
                self.assertIn("127.0.0.1:8099", fh.read())

    def test_a_suite_without_remote_references_is_left_alone(self):
        with tempfile.TemporaryDirectory() as d:
            self.suite(d, {"resources/variables.py": "ctx = 'http://x/c.jsonld'\n"})
            head = git(d, "rev-parse", "HEAD")
            r = self.run_it(d)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(git(d, "rev-parse", "HEAD"), head)

    def test_a_directory_that_is_not_a_suite_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_it(d)
            self.assertEqual(r.returncode, 1)
            self.assertIn("no resources/jsonld-contexts", r.stderr)


if __name__ == "__main__":
    unittest.main()
