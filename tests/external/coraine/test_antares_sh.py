"""Self-check of antares.sh: Coraine's role helpers stubbed, a fake broker that records its env."""
import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

STUB = """
COR_ROLES="
   CB        1026    {d}/CB.pid      corTest
   CP1       1027    {d}/CP1.pid     corTest_cp1
"
corRoleLookup() {{
  local line; line=$(echo "$COR_ROLES" | awk -v r="$1" '$1 == r {{ print $2, $3 }}')
  [ -n "$line" ] || {{ echo "corRoleLookup: unknown role: $1"; return 1; }}
  COR_ROLE_PORT=${{line%% *}}; COR_ROLE_PID_FILE=${{line#* }}
}}
corAwaitPort() {{ local i pid; pid=$(< "$COR_ROLE_PID_FILE"); for i in $(seq 50); do [ -e "{d}/up.$pid" ] && return 0; sleep 0.05; done; return 1; }}
corAwaitPortClosed() {{ return 0; }}
corDbDrop() {{ echo "coraine's corDbDrop ran"; }}
corPidAlive() {{ local _p _c state; read -r _p _c state _ 2>/dev/null < "/proc/$1/stat" || return 1; [ "$state" != Z ]; }}
"""

FAKE = """#!/bin/bash
env | grep '^ANTARES_' | sort > "{d}/env.$ANTARES_HTTP_PORT"
echo "$(date +%s%N)" >> "{d}/started.$ANTARES_HTTP_PORT"
touch "{d}/up.$$"
exec sleep 30
"""


def alive(pid):
    """Running, not a zombie: a container's PID 1 often reaps nothing."""
    try:
        with open(f"/proc/{pid}/stat") as fh:
            return fh.read().split(")")[-1].split()[0] != "Z"
    except FileNotFoundError:
        return False


class Adapter(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        d = self.d = self.tmp.name
        os.makedirs(f"{d}/coraine/test/funcTests")
        with open(f"{d}/coraine/test/funcTests/corTestFunctions.sh", "w") as fh:
            fh.write(STUB.format(d=d))
        with open(f"{d}/antares", "w") as fh:
            fh.write(FAKE.format(d=d))
        os.chmod(f"{d}/antares", 0o755)

    def tearDown(self):
        self.sh("coraineStop -all")
        self.tmp.cleanup()

    def sh(self, script, store="file"):
        env = dict(os.environ, CORAINE_DIR=f"{self.d}/coraine", ANTARES_BIN=f"{self.d}/antares",
                   ANTARES_TEST_STORE=store, ANTARES_TEST_DIR=f"{self.d}/data")
        return subprocess.run(["bash", "-c", f"source {HERE}/antares.sh\n{script}"],
                              env=env, capture_output=True, text=True, timeout=30)

    def env_of(self, port):
        with open(f"{self.d}/env.{port}") as fh:
            return dict(line.rstrip("\n").split("=", 1) for line in fh)

    def starts(self, port):
        try:
            with open(f"{self.d}/started.{port}") as fh:
                return len(fh.read().split())
        except FileNotFoundError:
            return 0

    def test_a_role_starts_on_its_port_and_store_with_ignored_flags(self):
        r = self.sh("coraineStart -role CP1 -dist --troe timescale --troeSync --noSplitEntities")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        env = self.env_of(1027)
        self.assertEqual(env["ANTARES_STORE"], "file")
        self.assertEqual(env["ANTARES_PUBLIC_URL"], "http://localhost:1027")
        self.assertEqual(env["ANTARES_DATA_DIR"], f"{self.d}/data/CP1")
        self.assertNotIn("ANTARES_DATABASE_URL", env)
        # the broker refuses unknown ANTARES_* keys; ANTARES_TEST_* is reserved, this is not
        self.assertNotIn("ANTARES_BIN", env)
        # the spec sets no batch ceiling; Coraine upserts 1101 entities in one batch
        self.assertEqual(env["ANTARES_MAX_BATCH_ITEMS"], "2000")

    def test_every_role_is_its_own_source_alias(self):
        # one shared default alias made every forward between two roles a loop (508)
        self.sh("coraineStart")
        self.sh("coraineStart -role CP1 --csourceAlias cp1-alias")
        self.assertEqual(self.env_of(1026)["ANTARES_HOST_ALIAS"], "antares-cb")
        self.assertEqual(self.env_of(1027)["ANTARES_HOST_ALIAS"], "cp1-alias")

    def test_a_coraine_only_flag_refuses_to_start(self):
        r = self.sh("coraineStart --apiPlugins admin")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stderr, "CORAINE-ONLY --apiPlugins\n")
        self.assertEqual(self.starts(1026), 0)

    def test_an_unknown_role_fails(self):
        self.assertNotEqual(self.sh("coraineStart -role CP9").returncode, 0)

    def test_stop_ends_the_broker_and_forgets_its_pid(self):
        self.sh("coraineStart")
        with open(f"{self.d}/CB.pid") as fh:
            pid = int(fh.read())
        r = self.sh("coraineStop")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertFalse(os.path.exists(f"{self.d}/CB.pid"))
        self.assertFalse(alive(pid))

    def test_a_drop_on_a_running_file_broker_empties_it_and_restarts_it_with_its_flags(self):
        self.sh("coraineStart -role CP1 -dist")
        os.makedirs(f"{self.d}/data/CP1/tenant")
        r = self.sh("corDbDrop -role CP1 -tenant t1")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertNotIn("coraine's", r.stdout)
        self.assertFalse(os.path.exists(f"{self.d}/data/CP1/tenant"))
        self.assertEqual(self.starts(1027), 2)

    def test_a_drop_of_a_stopped_broker_does_not_start_it(self):
        os.makedirs(f"{self.d}/data/CB/x")
        r = self.sh("corDbDrop -db coraine")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertFalse(os.path.exists(f"{self.d}/data/CB"))
        self.assertEqual(self.starts(1026), 0)

    def test_a_restart_after_a_flagless_start_has_no_flags(self):
        self.sh("coraineStart", store="memory")
        r = self.sh("corDbDrop", store="memory")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertEqual(self.starts(1026), 2)

    def test_an_unknown_store_is_refused(self):
        r = self.sh("corDbDrop", store="mongo")
        self.assertEqual(r.returncode, 1)
        self.assertIn("unknown ANTARES_TEST_STORE", r.stderr)

    def test_curl_sends_one_content_type_and_one_accept_the_last_given(self):
        # corCurl adds its default Content-Type and Accept, then a case's own -H; the
        # broker refuses two Content-Types, and two Accepts combine (RFC 9110 5.3)
        os.makedirs(f"{self.d}/bin")
        with open(f"{self.d}/bin/curl", "w") as fh:
            fh.write('#!/bin/bash\nprintf "%s\\n" "$@"\n')
        os.chmod(f"{self.d}/bin/curl", 0o755)
        r = self.sh(f"PATH={self.d}/bin:$PATH; curl -s -H 'Content-Type: application/json' "
                    "-H 'Accept: application/json' --header 'content-type: application/ld+json' "
                    "-H 'Accept: application/geo+json;q=0.5' -d x")
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertEqual(r.stdout.split("\n")[:-1], [
            "-s", "-d", "x", "-H", "Accept: application/geo+json;q=0.5",
            "-H", "content-type: application/ld+json"])

    def test_postgres_needs_its_url(self):
        r = self.sh("coraineStart", store="postgres")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("ANTARES_TEST_PG", r.stderr)


if __name__ == "__main__":
    unittest.main()
