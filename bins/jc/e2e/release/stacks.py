"""Release e2e stacks (T-7156): N throwaway full jc stacks on this host, each seeded from the smoke
gate's synthetic bundle, for the real-click journeys of crates/portal/ui/e2e/release.

    stacks.py up --stacks 4 --slots 2 --run <dir>     start N stacks, write <run>/stacks.json
    stacks.py persons --slots 2 --run <dir>           new passwords, second factors reset (a rerun)
    stacks.py down --run <dir>                        stop them, delete every container and file
    stacks.py run --stacks 4 --slots 2 --runs 3 --run <dir>   up, every journey 3 times, measure, down
    stacks.py selftest --run <dir>                    up 2, one journey on each, down, no leftovers

A stack is deploy/rehearsal's `jc --role all` (kaisatsu, Antares, the Portal, shako, daiya, dataroom,
ekiben, the gateway in one process) on PostgreSQL+PostGIS in docker, imported by
deploy/rehearsal/import.sh from bins/jc/smoke/ci/synth-export.tgz moved to now. The binaries are
copied out of the release image by digest (--image, jc and kaisatsu; --platform-image, jcctl), so the
journeys judge the image prod would run. kaisatsu asks every person for a second factor
(`second_factor = "required"`, as jc-prod).

Isolation: every stack binds loopback only (up.sh refuses another bind for the import) and has its
own Postgres container, ports, state and secrets under <run>/home/rel<i>. Persons: per stack and
per Playwright slot one `admin` and one `approver` (both /platform-admins), one `viewer` (zilina's
viewer group) and one `outsider` (no group), each with a random password made here and kept only in <run>/stacks.json
(mode 0600, deleted by `down`); their TOTP seeds are enrolled by the browser and kept in
<run>/seed-<person> (0600) for a Playwright worker that replaces a failed one. Nothing here or
there prints a password, seed or token.

Python 3 stdlib only, plus the smoke suites' modules (bins/jc/smoke)."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
SMOKE = REPO / "bins/jc/smoke"
REHEARSAL = REPO / "deploy/rehearsal"
sys.path.insert(0, str(SMOKE))
sys.path.insert(0, str(SMOKE / "ui"))

import personas as ps
from common import Http, loopback_only, realm_of

# deployment components/jc/images.yaml serverJcDev and components/functions/images.yaml (jcctl).
JC_IMAGE = "ghcr.io/marek-mraz-jc/jc@sha256:2bce2ff55915494fa3aa6fc8c1754986a3fa15f77ec08d1909864acb176873fb"
PLATFORM_IMAGE = ("ghcr.io/marek-mraz-jc/joinedcontext-platform"
                  "@sha256:76602eb8c85b6adbf9f557e5f20bf5f514dfff1f6d7534eeffc5da045edbe2be")
# Stack i: public PORT0+10i (functions +1); Postgres PG0+2i, and up.sh's +100 (internal), +200 (admin),
# +300/+301 (Prometheus, its proxy): even spacing keeps every stack's ports apart up to 16 stacks.
# All below 32768, the start of Linux' ephemeral range: a client socket took 55604 once and stack 2's
# Postgres could not bind.
PORT0, PG0 = 30500, 31000
PERSONAS = ("admin", "approver", "viewer", "outsider")
PREFIX = "rel"              # env names rel0..; containers rh-pg-rel<i>, rh-functions-<port>


def log(msg: str) -> None:
    print(f"stacks: {msg}", flush=True)


def ports(i: int) -> tuple[int, int]:
    return PORT0 + 10 * i, PG0 + 2 * i


def copy_out(image: str, files: list[str], dest: Path) -> None:
    """Copies `files` out of `image` (pulled by digest) into `dest`."""
    if "@sha256:" not in image:
        raise SystemExit(f"{image}: a release stack runs an image by digest only")
    subprocess.run(["docker", "pull", "-q", image], check=True, stdout=subprocess.DEVNULL)
    cid = subprocess.run(["docker", "create", image], check=True, capture_output=True, text=True).stdout.strip()
    try:
        for f in files:
            subprocess.run(["docker", "cp", f"{cid}:{f}", str(dest)], check=True)
    finally:
        subprocess.run(["docker", "rm", cid], check=True, stdout=subprocess.DEVNULL)


def binaries(run: Path, image: str, platform_image: str, daiya: str | None) -> Path:
    """<run>/bin: jc and kaisatsu of the release image, jcctl of the platform image, daiya (the
    import's dashboard check; the image runs daiya inside jc and ships no CLI) from --daiya-bin."""
    b = run / "bin"
    b.mkdir(parents=True, exist_ok=True)
    copy_out(image, ["/usr/local/bin/jc", "/usr/local/bin/kaisatsu"], b)
    copy_out(platform_image, ["/usr/local/bin/jcctl"], b)
    src = Path(daiya or Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target")) / "debug/daiya")
    if not os.access(src, os.X_OK):
        raise SystemExit(f"no daiya at {src}: cargo build -p daiya-server, or --daiya-bin")
    shutil.copy2(src, b / "daiya")
    (b / "image").write_text(image)  # up.sh: this jc embeds the image's UI (no ui-fresh check)
    return b


def export(run: Path) -> Path:
    """The synthetic bundle moved to now and stamped, as the smoke gate prepares it."""
    ex = run / "export"
    shutil.rmtree(ex, ignore_errors=True)
    ex.mkdir(parents=True)
    subprocess.run(["tar", "-xzf", str(SMOKE / "ci/synth-export.tgz"), "-C", str(ex)], check=True)
    subprocess.run([sys.executable, "-I", str(SMOKE / "ci/redate.py"), str(ex)], check=True,
                   stdout=subprocess.DEVNULL)
    subprocess.run([sys.executable, "-I", str(REHEARSAL / "rehearsal.py"), "digest", "--export", str(ex)],
                   check=True, stdout=subprocess.DEVNULL)
    return ex


def env_for(run: Path, b: Path, image: str) -> dict:
    extra = run / "jc-extra.toml"
    extra.write_text('[kaisatsu]\nsecond_factor = "required"\n')
    return {**os.environ, "REHEARSAL_HOME": str(run / "home"), "JC_BIN": str(b / "jc"),
            "JCCTL_BIN": str(b / "jcctl"), "REHEARSAL_JC_IMAGE": image,
            "REHEARSAL_JC_TOML_EXTRA": str(extra), "REHEARSAL_BIND": "127.0.0.1",
            "REHEARSAL_PG_LISTEN": "127.0.0.1"}


def converge(log_file: Path, since: int, secs: int = 300) -> bool:
    """Two reconciler passes logged after line `since` (gate.sh converge): the views, schemas and
    policies of the import are in place."""
    for _ in range(secs):
        lines = log_file.read_text(errors="replace").splitlines()[since:]
        if sum("sync run finished" in x and "leader=true" in x for x in lines) >= 2:
            return True
        time.sleep(1)
    return False


def start(run: Path, env: dict, ex: Path, i: int) -> None:
    port, pg = ports(i)
    name = f"{PREFIX}{i}"
    out = run / f"{name}.import.log"
    t0 = time.monotonic()
    with out.open("w") as f:
        r = subprocess.run([str(REHEARSAL / "import.sh"), name, str(port), str(pg), str(ex)],
                           env=env, stdout=f, stderr=subprocess.STDOUT, check=False)
    if r.returncode != 0:
        raise RuntimeError(f"{name}: import failed, see {out}")
    if not converge(run / "home" / name / "jc.log", 0):
        raise RuntimeError(f"{name}: the reconciler did not converge in 300 s")
    log(f"{name}: up on http://127.0.0.1:{port} in {time.monotonic() - t0:.0f} s")


def people(run: Path, ex: Path, i: int, slots: int) -> dict:
    """The persons of stack i: per slot one of each persona, a fresh random password each."""
    port, _ = ports(i)
    base = f"http://127.0.0.1:{port}"
    loopback_only(base)
    state = run / "home" / f"{PREFIX}{i}" / "state"
    realm = realm_of(state)
    h = Http(base)
    admin = ps.Admin(h, ps.admin_token(h, (state / "portal-admin-client-secret").read_text().strip(), realm), realm)
    plan = {x["name"]: x for x in ps.plan(ex, None)}
    # A second administrator: nobody approves their own change (CC-34), so a journey's proposal
    # needs another person of /platform-admins to approve it.
    if "admin" in plan:
        plan["approver"] = {**plan["admin"], "name": "approver"}
    missing = [p for p in PERSONAS if p not in plan]
    if missing:
        raise RuntimeError(f"the bundle grants no {missing}: no persona to make")
    slots_out = []
    for s in range(slots):
        row = {}
        for p in PERSONAS:
            user = f"rel-{p}-s{s}"
            row[p] = {"user": user, "password": admin.person(user, plan[p]["groups"]),
                      "project": plan[p]["project"]}
        slots_out.append(row)
    return {"name": f"{PREFIX}{i}", "baseURL": base, "slots": slots_out}


def rss_kb(run: Path, i: int) -> int:
    pid = (run / "home" / f"{PREFIX}{i}" / "jc.pid").read_text().strip()
    with open(f"/proc/{pid}/status") as f:
        return next(int(x.split()[1]) for x in f if x.startswith("VmRSS:"))


def up(a) -> int:
    run = a.run.resolve()
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    t0 = time.monotonic()
    b = binaries(run, a.image, a.platform_image, a.daiya_bin)
    ex = export(run)
    env = env_for(run, b, a.image)
    # Imports run two at a time: each holds a jc on allow-all, a Postgres restore and jcctl.
    with ThreadPoolExecutor(max_workers=a.parallel) as pool:
        list(pool.map(lambda i: start(run, env, ex, i), range(a.stacks)))
    write_people(run, ex, a.stacks, a.slots, a.image)
    meas = {"up_secs": round(time.monotonic() - t0), "stacks": a.stacks,
            "rss_mb": [rss_kb(run, i) // 1024 for i in range(a.stacks)]}
    (run / "up.json").write_text(json.dumps(meas))
    log(f"{a.stacks} stacks up in {meas['up_secs']} s, jc RSS MB {meas['rss_mb']}")
    return 0


def write_people(run: Path, ex: Path, n: int, slots: int, image: str) -> None:
    """(Re)makes every person: a new password and no second factor, so the next run's browsers
    enrol fresh seeds. <run>/stacks.json, 0600."""
    for f in run.glob("seed-*"):
        f.unlink()
    stacks = [people(run, ex, i, slots) for i in range(n)]
    f = run / "stacks.json"
    fd = os.open(f, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump({"stacks": stacks, "image": image}, out)
    log(f"persons of {n} stacks x {slots} slots in {f} (0600)")


def persons(a) -> int:
    run = a.run.resolve()
    n = len(list((run / "home").glob(f"{PREFIX}*/jc.pid")))
    write_people(run, run / "export", n, a.slots, a.image)
    return 0


def ours(kind: str) -> list[str]:
    """Names of this harness's docker containers or volumes: rh-pg-, rh-prom-, rh-promproxy-,
    rh-front-rel<i> and rh-functions-<port>."""
    if kind == "container":
        out = subprocess.run(["docker", "ps", "-a", "--format", "{{.Names}}"], capture_output=True, text=True, check=False).stdout
        fn = [f"rh-functions-{PORT0 + 10 * i}" for i in range(32)]
        mine = tuple(f"rh-{k}-{PREFIX}" for k in ("pg", "prom", "promproxy", "front"))
        return [n for n in out.split() if n.startswith(mine) or n in fn]
    out = subprocess.run(["docker", "volume", "ls", "-q"], capture_output=True, text=True, check=False).stdout
    return [v for v in out.split() if PREFIX in v]


def down(a) -> int:
    run = a.run.resolve()
    home = run / "home"
    for pid_file in sorted(home.glob("*/*.pid")) if home.is_dir() else []:
        pid = pid_file.read_text().strip()
        if pid.isdigit():
            subprocess.run([str(REHEARSAL / "stop-jc.sh"), pid], check=False)
    names = ours("container")
    if names:
        subprocess.run(["docker", "rm", "-f", "-v", *names], check=False, stdout=subprocess.DEVNULL)
    # pgdata belongs to the container's postgres user: emptied from inside a container (gate.sh fresh).
    if home.is_dir():
        img = "postgis/postgis:17-3.5@sha256:01a6a70e41e6c4467c8f55f6063555ed72db2d6662cd0d571040d42eadaeb6f6"
        subprocess.run(["docker", "run", "--rm", "-v", f"{home}:/h", "--entrypoint", "sh", img, "-c",
                        "rm -rf /h/*/pgdata"], check=False, stdout=subprocess.DEVNULL)
    if a.keep_traces:
        for p in run.iterdir():
            if p.name not in ("results", "measure.json", "up.json") and not p.name.endswith(".import.log"):
                shutil.rmtree(p) if p.is_dir() else p.unlink()
    else:
        shutil.rmtree(run, ignore_errors=True)
    left = ours("container") + ours("volume")
    if left:
        log(f"left behind: {left}")
        return 1
    log("down: no stack container or volume left")
    return 0


def playwright(run: Path, grep: str | None, workers: int, extra_env: dict | None = None) -> int:
    ui = REPO / "crates/portal/ui"
    cmd = ["pnpm", "exec", "playwright", "test", "--config", "playwright.release.config.ts", f"--workers={workers}"]
    if grep:
        cmd.append(f"--grep={grep}")
    env = {**os.environ, "RELEASE_STACKS": str(run / "stacks.json"), "RELEASE_OUT": str(run / "results"),
           **(extra_env or {})}
    return subprocess.run(cmd, cwd=ui, env=env, check=False).returncode


def results(report: Path) -> dict:
    """One Playwright JSON report folded: per journey its outcome (passed, flaky: green on a retry,
    failed) and the duration of its last attempt."""
    rows = {}

    def walk(suite):
        for spec in suite.get("specs", []):
            for t in spec.get("tests", []):
                res = t.get("results", [])
                if t.get("projectName") == "lint" or not res:
                    continue
                last = res[-1]
                status = ("passed" if last["status"] == "passed" and len(res) == 1 else
                          "flaky" if last["status"] == "passed" else last["status"])
                rows[spec["title"]] = {"status": status, "ms": last.get("duration", 0)}
        for child in suite.get("suites", []):
            walk(child)

    for suite in json.loads(report.read_text()).get("suites", []):
        walk(suite)
    return rows


def run(a) -> int:
    """up, then --runs full runs of every journey (persons reset between runs), the measurements,
    then down (--keep-traces keeps the failed journeys' traces and shots)."""
    try:
        up(a)
    except (RuntimeError, subprocess.CalledProcessError) as e:
        log(f"up failed: {e}; tearing down, the import logs kept")
        a.keep_traces = True
        down(a)
        return 1
    run_dir = a.run.resolve()
    workers = a.stacks * a.slots
    meas = json.loads((run_dir / "up.json").read_text())
    meas["runs"] = []
    rc = 0
    for n in range(a.runs):
        if n:
            write_people(run_dir, run_dir / "export", a.stacks, a.slots, a.image)
        t0 = time.monotonic()
        out = run_dir / "results" / f"run{n}"
        rc_n = playwright(run_dir, a.grep, workers, {"RELEASE_OUT": str(out)})
        rows = results(out / "report.json") if (out / "report.json").exists() else {}
        rss = [rss_kb(run_dir, i) // 1024 for i in range(a.stacks)]
        counts = {k: sum(r["status"] == k for r in rows.values()) for k in ("passed", "flaky", "failed", "timedOut")}
        ms = sorted(r["ms"] for r in rows.values())
        meas["runs"].append({"wall_secs": round(time.monotonic() - t0), "journeys": len(rows), **counts,
                             "p50_ms": ms[len(ms) // 2] if ms else 0, "max_ms": ms[-1] if ms else 0,
                             "sum_ms": sum(ms), "rss_mb_after": rss,
                             "not_green": sorted(t for t, r in rows.items() if r["status"] != "passed")})
        log(f"run {n}: {counts} in {meas['runs'][-1]['wall_secs']} s, jc RSS MB {rss}")
        rc = rc or rc_n
    (run_dir / "measure.json").write_text(json.dumps(meas, indent=1))
    log(f"measurements in {run_dir / 'measure.json'}")
    if a.keep_traces or rc:
        a.keep_traces = True
    down(a)
    return rc


def selftest(a) -> int:
    a.stacks, a.slots = 2, 1
    up(a)
    # Two journeys, two workers: worker p runs on stack p % 2, so each stack serves one.
    rc = playwright(a.run.resolve(), "J010 |J011 ", 2)
    a.keep_traces = False
    left = down(a)
    log(f"selftest: journeys {'green' if rc == 0 else 'RED'}, teardown {'clean' if left == 0 else 'LEFT FILES'}")
    return rc or left


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("command", choices=["up", "persons", "run", "down", "selftest"])
    p.add_argument("--run", type=Path, required=True, help="the run's directory (made 0700)")
    p.add_argument("--stacks", type=int, default=4)
    p.add_argument("--slots", type=int, default=2, help="Playwright workers per stack")
    p.add_argument("--parallel", type=int, default=2, help="imports at once")
    p.add_argument("--image", default=os.environ.get("RELEASE_JC_IMAGE", JC_IMAGE))
    p.add_argument("--platform-image", default=os.environ.get("RELEASE_PLATFORM_IMAGE", PLATFORM_IMAGE))
    p.add_argument("--daiya-bin")
    p.add_argument("--keep-traces", action="store_true",
                   help="down keeps <run>/results (traces, shots), the measurements and the import logs")
    p.add_argument("--runs", type=int, default=1, help="run: full runs of the journeys")
    p.add_argument("--grep", help="run: only the journeys whose title matches")
    a = p.parse_args(argv)
    if not 1 <= a.stacks <= 16 or not 1 <= a.slots <= 8:
        p.error("--stacks 1..16, --slots 1..8")
    return {"up": up, "persons": persons, "run": run, "down": down, "selftest": selftest}[a.command](a)


if __name__ == "__main__":
    sys.exit(main())
