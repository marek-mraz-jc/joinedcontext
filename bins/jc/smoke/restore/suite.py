"""Smoke suite `restore`: back up one rehearsal environment, restore it into a fresh PostgreSQL and
state dir, start jc on the copy, and compare.

   bins/jc/smoke/run restore --base-url <url> --out <dir> --home $REHEARSAL_HOME/<env>
       [--export e] [--state s] [--log-file f] [--admin-url u] [--admin-token-file f]
       [--suites endpoints,names] [--with <suite>=<args>]... [--backup-dir d] [--restore-env name]
       [--restore-port p] [--restore-pg-port p] [--jc-bin f] [--keep] [--converge s]

Steps, each timed (the dev move's rollback is exactly this): `before` runs the compared suites
against the source; `dump` takes pg_dumpall --roles-only and a pg_dump -Fc of every database of the
environment's PostgreSQL (container rh-pg-<env>) in one exported snapshot per database, with each
table's row count read in that snapshot; `files` archives the state dir (shako's Git data, the
gateway checkout, realms), secrets, repos, files, dashboards and the runner's resources and
gtfs-realtime.proto; `fresh` starts an empty PostgreSQL
of the same image for <restore env>; `restore` loads roles and databases; `start` runs up.sh on the
copy (no export: the realms come from the restored state dir) on the source jc's --role; `converge` waits for a reconciler
pass; `after` reruns the suites on the copy.

Pass: every check id of the compared suites has the same pass/fail/skip on the copy as before the
dump (not an absolute pass), and the controls hold: the fresh PostgreSQL held none of the
databases, each restored table has the row count of the snapshot, every compared suite passed at
least one check before, an unknown Endpoint slug is refused on both, the copy is another jc
process on its own port on the source's role, and the runner's files are the source's. A row
`control.held` is a skip when either side runs ekiben (pipelines write during the compare, so the
run is no count proof); run the suite on a source started with REHEARSAL_HOLD_PIPELINES=1 for one.
The suite never stops, restarts or re-roles the source.

The dumps hold realm and client secrets: the backup dir must lie under $REHEARSAL_HOME (default
<home>/..), never under an evidence/ dir, and is made 0700 with 0600 files. The copy is torn down at
the end (--keep leaves it running); the backup stays (sizes in results.json)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
REPO = SMOKE.parent.parent.parent
sys.path.insert(0, str(SMOKE))
from common import FAIL, PASS, Http, Results, loopback_only

# What the backup holds beside the databases (dirs of <home>; dashboards may be a link), and the
# runner's two files up.sh reads from <home> (its resources file and gtfs-realtime.proto).
HOME_FILES = ("ekiben-resources.yaml", "gtfs-realtime.proto")
HOME_PARTS = ("state", "secrets", "repos", "files", "dashboards", *HOME_FILES)
# The source's realm lines are rendered over an export; the copy has none, and up.sh refuses them
# without one (up.sh "REHEARSAL_REALM_LINES_* given but ... holds no keycloak/").
COPY_ENV_DROPPED = ("REHEARSAL_REALM_LINES_FROM", "REHEARSAL_REALM_LINES_TO", "REHEARSAL_HOLD_PIPELINES",
                    "REHEARSAL_IMPORT_POLICY")
# No --person-token-file: a token carries the source's issuer (localhost:<port>), which the copy on
# its own port refuses, so every closed-Endpoint check would differ for a reason outside the backup.
PATH_FLAGS = ("--export", "--state", "--log-file", "--admin-token-file")
COUNT_END = "-- end of counts --"


class Refused(Exception):
    """A step that cannot start (disk, a missing container): a named skip, not a jc defect."""


# ---------------------------------------------------------------- paths and arguments


def backup_dir_ok(backup: Path, rehearsal_home: Path) -> str:
    """'' when the backup dir may hold secrets, else why not."""
    b, h = backup.resolve(), rehearsal_home.resolve()
    if "evidence" in b.parts:
        return f"{b} lies under an evidence/ dir: dumps hold realm and client secrets"
    if b != h and h not in b.parents:
        return f"{b} is not under $REHEARSAL_HOME ({h})"
    return ""


def private_dir(p: Path) -> Path:
    p.mkdir(parents=True, exist_ok=True)
    os.chmod(p, 0o700)
    return p


def map_path(p: str, src: Path, dst: Path) -> str:
    """A path under the source home names the same file under the copy's home."""
    pp = Path(p)
    try:
        return str(dst / pp.relative_to(src))
    except ValueError:
        return p


def remap_view(src_view: Path, dst_view: Path, src: Path, dst: Path) -> None:
    """A smoke-state view (links into <home>/state and secrets) rebuilt for the copy's home."""
    if dst_view.exists():
        shutil.rmtree(dst_view)
    private_dir(dst_view)
    for f in sorted(src_view.iterdir()):
        target = os.readlink(f) if f.is_symlink() else str(f)
        target = target if os.path.isabs(target) else str(f.parent / target)
        os.symlink(map_path(target, src, dst), dst_view / f.name)


def copied(p: Path, home: Path) -> bool:
    """True when p lies in a part of <home> the backup holds (else it is a view to rebuild)."""
    try:
        rel = p.resolve().relative_to(home)
    except ValueError:
        return True
    return bool(rel.parts) and rel.parts[0] in HOME_PARTS


def rewrite_argv(argv: list[str], ports: dict[int, int], src: Path, dst: Path) -> list[str]:
    """The inner suites' flags for the copy: paths moved to its home, ports to its ports."""
    out = []
    for i, a in enumerate(argv):
        prev = argv[i - 1] if i else ""
        if prev in PATH_FLAGS:
            a = map_path(a, src, dst)
        elif prev in ("--base-url", "--admin-url"):
            for old, new in ports.items():
                a = re.sub(rf":{old}(?=/|$)", f":{new}", a)
        out.append(a)
    return out


def free_ports(n_extra: tuple[int, ...] = ()) -> int:
    """A loopback port p with p + each offset free too (up.sh uses pg + 100 and pg + 200)."""
    for _ in range(200):
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            p = s.getsockname()[1]
        if p + max(n_extra or (0,)) > 65000:
            continue
        if all(port_free(p + k) for k in (0, *n_extra)):
            return p
    raise Refused("no free loopback port")


def port_free(p: int) -> bool:
    with socket.socket() as s:
        try:
            s.bind(("127.0.0.1", p))
            return True
        except OSError:
            return False


def role_of(pid) -> str:
    """The --role a running jc was started with ('' when the process or the flag is not there)."""
    try:
        argv = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
    except OSError:
        return ""
    for i, x in enumerate(argv):
        if x == b"--role" and i + 1 < len(argv):
            return argv[i + 1].decode()
        if x.startswith(b"--role="):
            return x[len(b"--role="):].decode()
    return ""


def runs_ekiben(role: str) -> bool:
    return role == "all" or "ekiben" in role.split(",")


def sha256_of(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else ""


def pg_port_of(home: Path) -> int:
    m = re.search(r"@127\.0\.0\.1:(\d+)/", (home / "jc.toml").read_text())
    if not m:
        raise Refused(f"{home}/jc.toml names no PostgreSQL port")
    return int(m.group(1))


# ---------------------------------------------------------------- comparison


def load_checks(out: Path) -> dict | None:
    f = out / "results.json"
    if not f.exists():
        return None
    # A check id repeats per target (an Endpoint slug, a kind): the key is target/id, with #n for a
    # pair that repeats in one run.
    out: dict = {}
    for r in json.loads(f.read_text()).get("checks", []):
        key = f"{r.get('target', '')}/{r['id']}"
        n = 1
        while (key if n == 1 else f"{key}#{n}") in out:
            n += 1
        out[key if n == 1 else f"{key}#{n}"] = r
    return out


def compare(res: Results, suite: str, before: dict | None, after: dict | None, call: str) -> None:
    """One row per check id: the copy must answer as the source did before the dump."""
    if before is None or after is None:
        side = "before" if before is None else "after"
        res.check(f"{suite}.results", suite, call, "results.json before and after", f"none {side}", False)
        return
    passed = sum(1 for r in before.values() if r["result"] == PASS)
    res.check(f"control.before-passes.{suite}", suite, call, ">= 1 pass before the dump (else nothing is compared)",
              f"{passed} pass of {len(before)}", passed > 0)
    for id in sorted(set(before) | set(after)):
        b, a = before.get(id), after.get(id)
        exp = f"before: {b['result']}" if b else "before: absent"
        act = f"after: {a['result']}" if a else "after: absent"
        ok = bool(a and b and a["result"] == b["result"])
        why = "" if ok else (a or b or {}).get("actual", "")[:200]
        res.check(f"{suite}/{id}", suite, call, exp, act, ok, why)


def parse_counts(text: str) -> dict[str, int]:
    out = {}
    for line in text.splitlines():
        name, _, n = line.strip().rpartition("|")
        if name and n.isdigit():
            out[name] = int(n)
    return out


def count_diff(before: dict[str, int], after: dict[str, int]) -> list[str]:
    return [f"{t}: {before.get(t, 'absent')} -> {after.get(t, 'absent')}"
            for t in sorted(set(before) | set(after)) if before.get(t) != after.get(t)]


# ---------------------------------------------------------------- PostgreSQL in docker

COUNTS_SQL = (
    "SELECT format('SELECT %L || ''|'' || count(*) FROM %I.%I', n.nspname || '.' || c.relname, n.nspname, c.relname)"
    " FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace"
    " WHERE c.relkind = 'r' AND n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%'"
    " ORDER BY 1 \\gexec\n")


class Pg:
    """One rehearsal PostgreSQL, reached as its superuser over the container's local socket."""

    def __init__(self, container: str, port: int):
        self.container, self.port = container, port

    def exec_argv(self, *argv: str, stdin: bool = False) -> list[str]:
        return ["docker", "exec", *(["-i"] if stdin else []), "-u", "postgres", self.container, *argv]

    def psql(self, db: str, sql: str) -> str:
        p = subprocess.run(self.exec_argv("psql", "-X", "-q", "-tA", "-v", "ON_ERROR_STOP=1", "-p", str(self.port),
                                          "-d", db, stdin=True), input=sql, capture_output=True, text=True, timeout=3600, check=False)
        if p.returncode:
            raise RuntimeError(f"psql {db}: {p.stderr.strip()[:300]}")
        return p.stdout

    def databases(self) -> list[str]:
        return [d for d in self.psql("postgres", "SELECT datname FROM pg_database WHERE NOT datistemplate "
                                                 "AND datname <> 'postgres' ORDER BY 1").split() if d]

    def size(self, dbs: list[str]) -> int:
        if not dbs:
            return 0
        names = ",".join("'" + d.replace("'", "''") + "'" for d in dbs)
        return int(self.psql("postgres", f"SELECT sum(pg_database_size(datname)) FROM pg_database WHERE datname IN ({names})")
                   .strip() or 0)

    def counts(self, db: str) -> dict[str, int]:
        return parse_counts(self.psql(db, COUNTS_SQL))

    def to_file(self, argv: list[str], path: Path) -> None:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "wb") as f:
            p = subprocess.run(self.exec_argv(*argv), stdout=f, stderr=subprocess.PIPE, timeout=6 * 3600, check=False)
        if p.returncode:
            raise RuntimeError(f"{argv[0]}: {p.stderr.decode(errors='replace').strip()[:300]}")

    def dump_roles(self, path: Path) -> None:
        self.to_file(["pg_dumpall", "--roles-only", "-p", str(self.port)], path)

    def dump(self, db: str, path: Path) -> dict[str, int]:
        """pg_dump -Fc in a snapshot the row counts are read in, so the control compares like with like."""
        p = subprocess.Popen(self.exec_argv("psql", "-X", "-q", "-tA", "-v", "ON_ERROR_STOP=1", "-p", str(self.port),
                                            "-d", db, stdin=True),
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            p.stdin.write("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;\nSELECT pg_export_snapshot();\n")
            p.stdin.flush()
            snapshot = p.stdout.readline().strip()
            if not snapshot:
                raise RuntimeError(f"no snapshot from {db}: {p.stderr.read()[:300] if p.poll() is not None else ''}")
            p.stdin.write(COUNTS_SQL + f"SELECT '{COUNT_END}';\n")
            p.stdin.flush()
            lines = []
            for line in p.stdout:
                if line.strip() == COUNT_END:
                    break
                lines.append(line)
            self.to_file(["pg_dump", "-Fc", "-p", str(self.port), f"--snapshot={snapshot}", "-d", db], path)
            p.stdin.write("COMMIT;\n")
            p.stdin.close()
            p.wait(timeout=60)
            return parse_counts("".join(lines))
        finally:
            if p.poll() is None:
                p.kill()

    def restore_roles(self, path: Path) -> None:
        with path.open("rb") as f:
            p = subprocess.run(self.exec_argv("psql", "-X", "-q", "-p", str(self.port), "-d", "postgres", stdin=True),
                               stdin=f, capture_output=True, timeout=600, check=False)
        errors = [ln for ln in p.stderr.decode(errors="replace").splitlines()
                  if "ERROR" in ln and "already exists" not in ln]
        if p.returncode or errors:
            raise RuntimeError(f"roles: {' / '.join(errors)[:300] or p.returncode}")

    def restore(self, db: str, path: Path) -> None:
        with path.open("rb") as f:
            p = subprocess.run(self.exec_argv("pg_restore", "--create", "--exit-on-error", "-p", str(self.port),
                                              "-d", "postgres", stdin=True),
                               stdin=f, capture_output=True, timeout=6 * 3600, check=False)
        if p.returncode:
            raise RuntimeError(f"pg_restore {db}: {p.stderr.decode(errors='replace').strip()[:300]}")

    def image(self) -> str:
        return run(["docker", "inspect", "-f", "{{.Config.Image}}", self.container]).strip()

    def running(self) -> bool:
        p = subprocess.run(["docker", "inspect", "-f", "{{.State.Running}}", self.container],
                           capture_output=True, text=True, check=False)
        return p.stdout.strip() == "true"


def relocate(rhome: Path, home: Path) -> list[str]:
    old, new = str(home) + "/", str(rhome) + "/"
    found = []
    for f in sorted(rhome.rglob("*")):
        rel = f.relative_to(rhome)
        if rel.parts[0] not in HOME_PARTS:
            continue
        if f.is_symlink():
            target = os.readlink(f)
            if target.startswith(old):
                found.append(f"{rel} -> {target}")
                f.unlink()
                os.symlink(new + target[len(old):], f)
        elif f.is_file() and f.stat().st_size < (8 << 20):
            text = f.read_bytes()
            if old.encode() in text:
                found.append(f"{rel}: {text.count(old.encode())}x {old}")
                f.write_bytes(text.replace(old.encode(), new.encode()))
    return found


def run(argv: list[str], env: dict | None = None, timeout: float = 3600, cwd: str | None = None) -> str:
    p = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=timeout, cwd=cwd, check=False)
    if p.returncode:
        raise RuntimeError(f"{Path(argv[0]).name} exit {p.returncode}: {(p.stderr or p.stdout).strip()[-400:]}")
    return p.stdout


# ---------------------------------------------------------------- the steps


class Ops:
    """Everything that touches docker, files and processes, so the selftest can stand in for it."""

    def __init__(self, a, home: Path, rhome: Path, backup: Path):
        self.a, self.home, self.rhome, self.backup = a, home, rhome, backup
        self.src = Pg(f"rh-pg-{home.name}", a.src_pg_port)
        self.dst = Pg(f"rh-pg-{rhome.name}", a.restore_pg_port)
        self.counts: dict[str, dict[str, int]] = {}

    def check_disk(self) -> dict:
        if not self.src.running():
            raise Refused(f"container {self.src.container} does not run: this suite backs up a rehearsal up.sh started")
        dbs = self.src.databases()
        need = int(self.src.size(dbs) * 1.5)  # the dumps (< 0.3x) and the restored copy (1x), plus slack
        st = shutil.disk_usage(self.backup)
        used_after = (st.total - st.free + need) / st.total
        if used_after > self.a.max_disk:
            raise Refused(f"needs ~{need >> 20} MiB; the disk would be {used_after:.1%} full (> {self.a.max_disk:.0%}: "
                          f"the host sweeper deletes build dirs at 93 %)")
        return {"databases": dbs, "needBytes": need}

    def suite(self, name: str, argv: list[str], out: Path) -> tuple[int, str]:
        call = [sys.executable, str(SMOKE / "run"), name, *argv, "--out", str(out)]
        out.mkdir(parents=True, exist_ok=True)
        with (out / "run.log").open("wb") as fo:
            p = subprocess.Popen(call, stdout=fo, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = p.wait(timeout=self.a.suite_timeout)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, 9)
                p.wait()
                code = -9
        return code, "run " + " ".join(shlex.quote(x) for x in [name, *argv])

    def dump_roles(self) -> None:
        self.src.dump_roles(self.backup / "roles.sql")

    def dump(self, db: str) -> None:
        self.counts[db] = self.src.dump(db, self.backup / f"{db}.dump")
        (self.backup / f"{db}.counts.json").write_text(json.dumps(self.counts[db], indent=1))

    def files(self) -> None:
        parts = [p for p in HOME_PARTS if (self.home / p).exists() and p != "dashboards"]
        run(["tar", "-cf", str(self.backup / "home.tar"), "-C", str(self.home), *parts])
        dash = self.home / "dashboards"
        if dash.exists():  # a link to the dashboards dir on the rehearsal: archived as its content
            run(["tar", "-cf", str(self.backup / "dashboards.tar"), "-C", str(dash.resolve()), "."])
        for f in self.backup.iterdir():
            os.chmod(f, 0o600)

    def fresh(self) -> list[str]:
        """An empty PostgreSQL of the source's image on the copy's port; answers the databases it holds."""
        private_dir(self.rhome)
        run(["tar", "-xf", str(self.backup / "home.tar"), "-C", str(self.rhome)])
        if (self.backup / "dashboards.tar").exists():
            private_dir(self.rhome / "dashboards")
            run(["tar", "-xf", str(self.backup / "dashboards.tar"), "-C", str(self.rhome / "dashboards")])
        subprocess.run(["docker", "rm", "-f", self.dst.container], capture_output=True, check=False)
        run(["docker", "run", "-d", "--name", self.dst.container, "--network", "host", "--shm-size=512m",
             "-e", f"PGPORT={self.dst.port}", "-e", "POSTGRES_PASSWORD_FILE=/run/rh/superuser",
             "-v", f"{self.rhome / 'secrets' / 'superuser'}:/run/rh/superuser:ro",
             "-v", f"{self.rhome / 'pgdata'}:/var/lib/postgresql/data", self.src.image()])
        for _ in range(120):
            if subprocess.run(["pg_isready", "-q", "-h", "127.0.0.1", "-p", str(self.dst.port)], check=False).returncode == 0:
                # the image's init restarts the server once: wait until psql gets through
                try:
                    return self.dst.databases()
                except RuntimeError:
                    pass
            time.sleep(1)
        raise RuntimeError(f"no PostgreSQL on {self.dst.port} after 120 s")

    def relocate(self) -> list[str]:
        """Paths of the source home inside the restored files, rewritten to the copy's home.

        A backup restored at another path (a new volume, a second host) only starts when nothing in
        it names the old path: answers what did, each rewritten so the rest can still be compared."""
        return relocate(self.rhome, self.home)

    def restore_roles(self) -> None:
        self.dst.restore_roles(self.backup / "roles.sql")

    def restore(self, db: str) -> dict[str, int]:
        self.dst.restore(db, self.backup / f"{db}.dump")
        return self.dst.counts(db)

    def start(self, role: str) -> int:
        """up.sh on the copy, on the source's role (a held source gives a held copy)."""
        env = {k: v for k, v in os.environ.items() if k not in COPY_ENV_DROPPED}
        env.update({"REHEARSAL_HOME": str(self.rhome.parent), "JC_BIN": self.a.jc_bin,
                    # no export: up.sh keeps the realms of the restored state dir
                    "REHEARSAL_EXPORT": str(self.rhome / "no-export"),
                    "REHEARSAL_PEOPLE": str(self.rhome / "no-people"),
                    "REHEARSAL_DEMO_USERS": str(self.rhome / "no-demo-users.yaml")})
        if role:
            env["REHEARSAL_ROLE"] = role
        else:
            env.pop("REHEARSAL_ROLE", None)
        try:
            run([str(REPO / "deploy" / "rehearsal" / "up.sh"), self.rhome.name, str(self.a.restore_port),
                 str(self.dst.port)], env=env, timeout=900)
        except RuntimeError as e:
            log = self.rhome / "jc.log"
            errors = [ln for ln in (log.read_text(errors="replace").splitlines() if log.exists() else [])
                      if " ERROR " in ln or "panicked" in ln]
            raise RuntimeError(" / ".join(errors[-3:])[-600:] or str(e)) from e
        return int((self.rhome / "jc.pid").read_text().strip())

    def role(self, pid) -> str:
        return role_of(pid)

    def converge(self, timeout: float) -> int:
        log = self.rhome / "jc.log"
        end = time.time() + timeout
        n = 0
        while time.time() < end:
            n = len(re.findall(r"sync run finished.*leader=true", log.read_text(errors="replace")))
            if n >= 1:
                return n
            time.sleep(2)
        return n

    def teardown(self) -> None:
        for f in ("jc.pid", "retain.pid"):
            pid = self.rhome / f
            if pid.exists():
                subprocess.run([str(REPO / "deploy" / "rehearsal" / "stop-jc.sh"), pid.read_text().strip()],
                               capture_output=True, check=False)
        # the copy's jc-functions (apps.sh, named by its public port), Prometheus and label proxy
        # (prometheus.sh, named by its env)
        for name in (self.dst.container, f"rh-functions-{self.a.restore_port}", f"rh-prom-{self.rhome.name}",
                     f"rh-promproxy-{self.rhome.name}"):
            subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
        # pgdata belongs to the container's postgres user: emptied from inside a container
        if (self.rhome / "pgdata").exists():
            subprocess.run(["docker", "run", "--rm", "-v", f"{self.rhome}:/h", "--entrypoint", "rm", self.src.image(),
                            "-rf", "/h/pgdata"], capture_output=True, check=False)
        shutil.rmtree(self.rhome, ignore_errors=True)


class Steps:
    """Times each step into a row step.<name> and the timings dict."""

    def __init__(self, res: Results):
        self.res, self.seconds = res, {}

    def run(self, name: str, what: str, fn, *args):
        t = time.monotonic()
        try:
            out = fn(*args)
        except Refused:
            raise
        except Exception as e:
            s = round(time.monotonic() - t, 1)
            self.seconds[name] = s
            self.res.check(f"step.{name}", "restore", what, "done", f"failed after {s} s", False, str(e)[:300])
            raise StepFailed(name) from e
        s = round(time.monotonic() - t, 1)
        self.seconds[name] = s
        self.res.check(f"step.{name}", "restore", what, "done", f"{s} s", True)
        return out


class StepFailed(Exception):
    pass


def inner_argv(a, suite: str) -> list[str]:
    argv = ["--base-url", a.base_url]
    src = (SMOKE / suite / "suite.py").read_text()
    for f in PATH_FLAGS + ("--admin-url",):
        v = getattr(a, f[2:].replace("-", "_"))
        if v and f'"{f}"' in src:
            argv += [f, v]
    return argv + shlex.split(a.extra.get(suite, ""))


def unknown_slug(base: str) -> tuple[int, str]:
    path = f"/api/endpoint/smoke-restore-{secrets.token_hex(6)}/ngsi-ld/v1/types"
    status, _, _ = Http(base, timeout=30).call("GET", path)
    return status, path


def pipeline(a, res: Results, ops: Ops, out: Path) -> dict:
    steps = Steps(res)
    meta: dict = {"steps": steps.seconds}
    suites = [s for s in a.suites.split(",") if s]
    before_dir, after_dir = out / "before", out / "after"

    def before():
        for s in suites:
            ops.suite(s, inner_argv(a, s), before_dir / s)
    try:
        info = ops.check_disk()
    except Refused as e:
        res.skip("step.dump", "restore", "pg_dump of every database", "done", str(e))
        return meta
    dbs = info["databases"]
    meta["databases"] = dbs
    try:
        steps.run("before", f"run {','.join(suites)} on the source", before)
        steps.run("dump.roles", "pg_dumpall --roles-only", ops.dump_roles)
        for db in dbs:
            steps.run(f"dump.{db}", f"pg_dump -Fc {db} (exported snapshot)", ops.dump, db)
        steps.run("files", f"tar {', '.join(HOME_PARTS)}", ops.files)
        meta["backupBytes"] = {f.name: f.stat().st_size for f in sorted(ops.backup.iterdir())}
        held = steps.run("fresh", "an empty PostgreSQL of the source's image", ops.fresh)
        res.check("control.fresh-empty", "restore", f"databases of {ops.dst.container} before the restore",
                  f"none of {', '.join(dbs)}", ", ".join(held) or "none", not set(held) & set(dbs))
        moved = steps.run("relocate", "the source home's path in the restored files", ops.relocate)
        res.check("control.relocatable", "restore", f"files of the copy naming {ops.home}",
                  "none (a backup restores at any path)", f"{len(moved)} rewritten" if moved else "none",
                  not moved, "; ".join(moved[:6])[:600])
        held_files = [f for f in HOME_FILES if (ops.home / f).is_file()]
        if held_files:
            differ = [f for f in held_files if sha256_of(ops.home / f) != sha256_of(ops.rhome / f)]
            res.check("control.home-files", "restore", f"{', '.join(held_files)} of the copy",
                      "the source's (sha256)", f"{len(differ)} differ: {', '.join(differ)}" if differ else "equal",
                      not differ)
        else:
            res.skip("control.home-files", "restore", f"{', '.join(HOME_FILES)} of the copy", "the source's (sha256)",
                     f"the source home has none of {', '.join(HOME_FILES)}: streams naming the runner's resources "
                     "are refused on both")
        steps.run("restore.roles", "psql < roles.sql", ops.restore_roles)
        for db in dbs:
            got = steps.run(f"restore.{db}", f"pg_restore --create {db}", ops.restore, db)
            diff = count_diff(ops.counts.get(db, {}), got)
            res.check(f"control.data.{db}", "restore", f"row count of each table of {db}",
                      f"{len(got)} tables, {sum(ops.counts.get(db, {}).values())} rows as in the dump's snapshot",
                      "equal" if not diff else f"{len(diff)} tables differ", not diff, "; ".join(diff[:5]))
        src_pid = (ops.home / "jc.pid").read_text().strip() if (ops.home / "jc.pid").exists() else "?"
        src_role = ops.role(src_pid)
        pid = steps.run("start", f"up.sh on the copy (no export, role {src_role or 'unknown'})", ops.start, src_role)
        res.check("control.own-process", "restore", "jc pid and port of the copy",
                  f"another process than {src_pid}, port {a.restore_port}", f"pid {pid}",
                  str(pid) != src_pid and a.restore_port != a.src_port)
        copy_role = ops.role(pid)
        res.check("control.role", "restore", "jc --role of the source and the copy",
                  f"the source's ({src_role or 'unknown'})", copy_role or "unknown",
                  bool(src_role) and copy_role == src_role)
        live = [side for side, r in (("source", src_role), ("copy", copy_role)) if runs_ekiben(r)]
        if live:
            res.skip("control.held", "restore", "pipelines held on both sides during the compare",
                     "no ekiben on either side",
                     f"ekiben runs on the {' and the '.join(live)}: pipelines write while the suites run, so an "
                     "equal row is no count proof and a flipped one may be a stream's write")
        else:
            res.check("control.held", "restore", "pipelines held on both sides during the compare",
                      "no ekiben on either side", "held on both", True)
        n = steps.run("converge", "a reconciler pass on the copy", ops.converge, a.converge)
        res.check("converge", "restore", "sync run finished leader=true in the copy's log", ">= 1", str(n), n >= 1)
        rbase = re.sub(rf":{a.src_port}(?=/|$)", f":{a.restore_port}", a.base_url.rstrip("/"))
        ports = {a.src_port: a.restore_port, a.src_pg_port + 200: a.restore_pg_port + 200}
        if a.state and Path(a.state).is_dir() and not copied(Path(a.state), ops.home):
            remap_view(Path(a.state), Path(map_path(a.state, ops.home, ops.rhome)), ops.home, ops.rhome)

        def after():
            for s in suites:
                ops.suite(s, rewrite_argv(inner_argv(a, s), ports, ops.home, ops.rhome), after_dir / s)
        steps.run("after", f"run {','.join(suites)} on the copy", after)
        for s in suites:
            compare(res, s, load_checks(before_dir / s), load_checks(after_dir / s), f"run {s} before vs after")
        (s1, path), (s2, _) = unknown_slug(a.base_url), unknown_slug(rbase)
        res.check("control.unknown-slug", "restore", f"GET {path} on source and copy",
                  "the same refusal (4xx) on both", f"source {s1}, copy {s2}", s1 == s2 and 400 <= s1 < 500)
    except StepFailed:
        pass
    meta["steps"] = steps.seconds
    meta["rollbackSeconds"] = round(sum(v for k, v in steps.seconds.items()
                                        if k.startswith(("fresh", "restore", "start", "converge"))), 1)
    return meta


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run restore", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--home", help="the rehearsal environment dir to back up ($REHEARSAL_HOME/<env>)")
    for f in PATH_FLAGS + ("--admin-url",):
        p.add_argument(f, help="handed to the compared suites (moved to the copy's home and ports after)")
    p.add_argument("--suites", default="endpoints,names", help="the suites compared before and after")
    p.add_argument("--with", dest="extra_args", action="append", default=[], metavar="SUITE=ARGS")
    p.add_argument("--backup-dir", help="default $REHEARSAL_HOME/backups/<env>-<utc time>")
    p.add_argument("--restore-env", help="the copy's environment name (default <env>-restore)")
    p.add_argument("--restore-port", type=int, default=0, help="the copy's public port (default: a free one)")
    p.add_argument("--restore-pg-port", type=int, default=0, help="the copy's PostgreSQL port (default: a free one)")
    p.add_argument("--jc-bin", help="default: the source jc's executable, else $JC_BIN")
    p.add_argument("--converge", type=float, default=300)
    p.add_argument("--max-disk", type=float, default=0.92, help="refuse when the disk would be fuller (0..1)")
    p.add_argument("--suite-timeout", type=float, default=3600)
    p.add_argument("--keep", action="store_true", help="leave the copy running")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    a.extra = {}
    for x in a.extra_args:
        k, _, v = x.partition("=")
        a.extra[k] = (a.extra.get(k, "") + " " + v).strip()
    res = Results("restore", a.base_url)
    out = Path(a.out)
    meta = execute(a, res, out, Ops)
    res.write(out, meta)
    return 1 if res.totals()[FAIL] else 0


def execute(a, res: Results, out: Path, ops_class) -> dict:
    if not a.home:
        res.skip("run", "restore", "back up and restore", "done", "no --home: name the rehearsal env dir to back up")
        return {}
    home = Path(a.home).resolve()
    if not (home / "jc.toml").exists():
        res.skip("run", "restore", "back up and restore", "done", f"{home} holds no jc.toml: not an up.sh environment")
        return {}
    rh_home = Path(os.environ.get("REHEARSAL_HOME") or home.parent).resolve()
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    backup = Path(a.backup_dir or rh_home / "backups" / f"{home.name}-{stamp}")
    why = backup_dir_ok(backup, rh_home)
    if why:
        res.check("backup-dir", "restore", "where the dumps go", "under $REHEARSAL_HOME, never evidence/", why, False)
        return {}
    private_dir(backup.parent)
    private_dir(backup)
    try:
        a.src_port = int(re.search(r":(\d+)", a.base_url.split("//", 1)[1]).group(1))
        a.src_pg_port = pg_port_of(home)
        a.restore_port = a.restore_port or free_ports((1,))  # apps.sh: jc-functions on port + 1
        a.restore_pg_port = a.restore_pg_port or free_ports((100, 200))
    except (Refused, AttributeError) as e:
        res.skip("run", "restore", "back up and restore", "done", str(e) or "no port in --base-url")
        return {}
    if not a.jc_bin:
        pid_file = home / "jc.pid"
        exe = Path(f"/proc/{pid_file.read_text().strip()}/exe") if pid_file.exists() else None
        a.jc_bin = os.path.realpath(exe) if exe and exe.exists() else os.environ.get("JC_BIN", "")
    rhome = rh_home / (a.restore_env or f"{home.name}-restore")
    if rhome == home:
        res.check("restore-env", "restore", "the copy's dir", "not the source's", str(rhome), False)
        return {}
    ops = ops_class(a, home, rhome, backup)
    meta = {"source": str(home), "copy": str(rhome), "backup": str(backup), "jcBin": a.jc_bin,
            "restorePort": a.restore_port, "restorePgPort": a.restore_pg_port}
    try:
        meta.update(pipeline(a, res, ops, out))
    finally:
        log = rhome / "jc.log"
        if log.exists():  # the copy's log outlives the copy (the logs suite scans it with the results)
            out.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(log, out / "copy-jc.log")
        if not a.keep:
            ops.teardown()
    return meta


if __name__ == "__main__":
    sys.exit(main())
