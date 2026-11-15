"""T-5899: the release gate on dev and the prod-apply step that refuses a candidate without its record."""

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "standards-gate.py"
spec = importlib.util.spec_from_file_location("standards_gate", SCRIPT)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
JUSTFILE = (ROOT / "justfile").read_text()


def recipe(name: str) -> str:
    m = re.search(rf"^{re.escape(name)}\b[^\n]*:\n((?:\t.*\n|\n)+)", JUSTFILE, re.MULTILINE)
    assert m, f"{name} recipe not found in the justfile"
    return m.group(1)


def test_the_self_test_passes():
    # a red suite blocks, a stale record for another sha (other code or other pins) blocks
    run = subprocess.run([sys.executable, str(SCRIPT), "--self-test"], capture_output=True, text=True, check=False)
    assert run.returncode == 0, run.stdout + run.stderr


def test_prod_apply_checks_the_record_after_the_tag_and_before_anything_changes():
    body = recipe("prod-apply")
    verify = body.index("./scripts/release.py verify HEAD")
    check = body.index("./scripts/standards-gate.py check HEAD")
    assert verify < check < body.index("just _dev-assemble") < body.index(" sync ")


def test_check_refuses_head_without_a_record_and_passes_with_a_green_one(tmp_path, capsys):
    assert gate.main(["check", "HEAD", "--records", str(tmp_path)]) == 1
    assert "has no green release gate record" in capsys.readouterr().err
    cand = gate.candidate("HEAD")
    folder = tmp_path / cand["sha"]
    folder.mkdir()
    rec = {**cand, "green": False, "finished": "2026-10-28T09:00:00Z"}
    (folder / "record.json").write_text(json.dumps(rec))
    assert gate.main(["check", "HEAD", "--records", str(tmp_path)]) == 1
    (folder / "record.json").write_text(json.dumps({**rec, "green": True}))
    assert gate.main(["check", "HEAD", "--records", str(tmp_path)]) == 0


def test_every_stage_targets_dev_only(tmp_path):
    args = argparse.Namespace(conformance=tmp_path, portal=tmp_path, requests=None, rate=1000,
                              duration="5m", only=None)
    plan = gate.stages(args, tmp_path)
    assert [name for name, *_ in plan] == list(gate.REQUIRED)
    urls = [v for _, argv, env, _ in plan for v in [*argv, *env.values()] if "https://" in v]
    assert urls and all(re.search(r"https://[a-z.]*dev\.joinedcontext\.com", u) for u in urls), urls
    assert "smartcity" not in SCRIPT.read_text()
    assert "just _dev-guard" in recipe("release-gate")


def test_a_partial_run_is_never_green():
    stages = [{"name": n, "verdict": "pass"} for n in gate.REQUIRED if n != "load"]
    assert not gate.verdict(stages, [])


def test_the_committed_known_reds_park_todays_dev_smoke_reds_and_nothing_else(tmp_path):
    # the lines dev-smoke prints on every run since 2026-10-27 (deployer, T-5899 09:4xZ), as the scripts print them;
    # the source links budget line left with T-5903
    log = (
        "  FAIL  catalogue catalog (wna4brinxxxxxxxxxxxxxxxxxxxx): count 0 entities of Catalog\n"
        "  FAIL  catalogue (no dataset) (emzask3oxxxxxxxxxxxxxxxxxxxx): ckan no dataset lists this Endpoint\n"
        "  FAIL  catalogue (no dataset) (y5jpwu6fxxxxxxxxxxxxxxxxxxxx): ckan no dataset lists this Endpoint\n"
        "  FAIL  catalogue: 626 datasets, 625 complete, 1 ko, 2 public Endpoints without a dataset; 482 requests\n"
    )
    known = gate.known_red()
    assert all(task.startswith("T-") for task, _ in known)

    def smoke(text):
        argv = ["bash", "-c", f"printf %s {gate.quote(text)}; exit 1"]
        return gate.run_stage("dev-smoke", argv, {}, tmp_path, tmp_path / "out", known)

    held = smoke("  ok    portal: 200\n" + log)
    assert held["verdict"] == "pass", held
    assert len(held["known_red"]) == 4
    # T-5903: the sweep finishes, so a budget cut is a red again
    assert smoke(log + "  FAIL  source links: the run's budget ended after 482 requests\n")["verdict"] == "fail"
    # a catalogue outage or another Endpoint's red is not parked
    assert smoke(log + "  FAIL  catalogue: package_search answered 503\n")["verdict"] == "fail"
    assert smoke(log + "  FAIL  catalogue bbsk-x (abcdefgh): count 0 entities\n")["verdict"] == "fail"
    assert smoke(log + "  FAIL  source link e1 T: resolved a, expected b\n")["verdict"] == "fail"


def test_the_first_live_runs_reds_are_parked_by_case_and_a_suite_row_does_not_hide_them(tmp_path):
    # c97c4488 on dev (deployer 2026-10-28T14:1xZ): the helsinki-feedback catalogue row is parked
    known = gate.known_red()
    feedback = ("  FAIL  catalogue helsinki-feedback (gv24bicwway4foznuszsagmzydzk3e2z): data urn:ngsi-ld:ServiceRequest:"
                "hel.fi:helsinki:open311-3379243 has no name; urn:ngsi-ld:ServiceRequest:hel.fi:helsinki:open311-1 has no name"
                " (+17 more)\n")
    argv = ["bash", "-c", f"printf %s {gate.quote(feedback)}; exit 1"]
    assert gate.run_stage("dev-smoke", argv, {}, tmp_path, tmp_path / "s", known)["verdict"] == "pass"
    other = feedback.replace("has no name", "has no location", 1)
    argv = ["bash", "-c", f"printf %s {gate.quote(other)}; exit 1"]
    assert gate.run_stage("dev-smoke", argv, {}, tmp_path, tmp_path / "s2", known)["verdict"] == "fail"

    def ws(reds: dict[str, list[str]]):
        out = tmp_path / f"ws{len(list(tmp_path.iterdir()))}"
        rows = ["endpoint\tsuite\tverdict\tdetail\tlog"]
        for ep, names in reds.items():
            (out / "ws" / ep / "ws").mkdir(parents=True)
            body = "".join(f'<testcase classname="tests.ws.test_gateway_ws" name="{n}"><failure/></testcase>' for n in names)
            (out / "ws" / ep / "ws" / "pytest.xml").write_text(f'<testsuite><testcase classname="c" name="ok"/>{body}</testsuite>')
            rows.append(f"{ep}\tws\t{'fail' if names else 'pass'}\tx\ty")
        (out / "ws" / "results.tsv").write_text("\n".join(rows) + "\n")
        return gate.run_stage("ws", ["false"], {}, tmp_path, out, known)

    a, b = "hqgmj6ieuq57y37g5g4ckz5eyhebklox", "mluyob4nz52lok3ssk7pgn5vwt"
    flood = "test_a_subscribe_flood_closes_with_1008_and_another_socket_stays"
    upgrade = "test_a_plain_get_answers_426_naming_the_upgrade"
    # T-5931 fixed both ws cases (the edge advertises the upgrade, the gateway ends a flood with a close
    # handshake): neither is parked any more, on either Endpoint.
    assert ws({a: [upgrade]})["verdict"] == "fail", "the 426 without Upgrade is red again"
    assert ws({b: [flood]})["verdict"] == "fail", "the flood without its 1008 is red again"


def test_the_roundtrip_stage_writes_only_on_the_seeded_conformance_endpoint(tmp_path, monkeypatch):
    # T-5914: the write-here read-everywhere stage targets the conformance space's own Endpoint
    args = argparse.Namespace(conformance=tmp_path, portal=tmp_path, requests=None, rate=1000,
                              duration="5m", only=["roundtrip"])
    monkeypatch.delenv("WRITE_ENDPOINT", raising=False)
    [(name, argv, env, cwd)] = gate.stages(args, tmp_path)
    seeded = re.search(r"slug: (\w+)", gate.CONFORMANCE_ENDPOINT.read_text()).group(1)
    assert name == "roundtrip" and cwd == tmp_path and env == {}
    assert argv[argv.index("--endpoint") + 1] == f"https://api.dev.joinedcontext.com/api/endpoint/{seeded}"
    assert "contextSpaceRef: conformance" in gate.CONFORMANCE_ENDPOINT.read_text()
    monkeypatch.setenv("WRITE_ENDPOINT", "abc")
    assert gate.stages(args, tmp_path)[0][1][argv.index("--endpoint") + 1].endswith("/api/endpoint/abc")
    monkeypatch.setenv("WRITE_ENDPOINT", "../x")
    try:
        gate.stages(args, tmp_path)
    except gate.Refused as e:
        assert "not an Endpoint slug" in str(e)
    else:
        raise AssertionError("a WRITE_ENDPOINT that is no slug must refuse")


def test_a_red_roundtrip_case_blocks_the_record(tmp_path):
    # the deliberate break: the suite's red line, as roundtrip.py prints it, turns the stage and the record red
    line = "  FAIL  current OData: OData: pm10.value: want 21.5 got 22"
    script = f"print('  ok  current NGSI-LD retrieve'); print({line!r}); raise SystemExit(1)"
    stage = gate.run_stage("roundtrip", [sys.executable, "-c", script], {}, tmp_path, tmp_path, known=[])
    assert stage["verdict"] == "fail"
    assert stage["cases"]["roundtrip::current OData"] == "fail"
    assert stage["cases"]["roundtrip::current NGSI-LD retrieve"] == "pass"
    green = [{"name": n, "verdict": "pass"} for n in gate.REQUIRED if n != "roundtrip"]
    assert not gate.verdict([*green, stage], [])
    assert gate.verdict([*green, {**stage, "verdict": "pass"}], [])


def test_the_sweep_leaves_the_critic_walks_out_and_gets_its_own_cap(tmp_path, monkeypatch):
    # first record b629a881: the sweep hit the 3600 s stage cap at spec 79 of 330, 1,619 s of it critic-* walks
    args = argparse.Namespace(conformance=tmp_path, portal=tmp_path, requests=None, rate=8,
                              duration="5m", only=["portal-sweep", "ws"], stage_timeout=3600,
                              sweep_timeout=18000, rev="HEAD", records=tmp_path)
    env = {name: env for name, _, env, _ in gate.stages(args, tmp_path)}["portal-sweep"]
    skip = re.compile(env["SWEEP_SKIP"])
    assert env["SWEEP_WORKERS"] == str(gate.SWEEP_WORKERS)
    assert skip.search("e2e/live/critic-policy.spec.ts") and not skip.search("e2e/live/mqtt-device.spec.ts")
    seen = {}
    monkeypatch.setattr(gate, "candidate", lambda rev: {"sha": "a" * 40, "images": {}})
    monkeypatch.setattr(gate, "preflight", lambda rev, names: {})
    monkeypatch.setattr(gate, "dev_commit", lambda: "a" * 40)
    monkeypatch.setattr(gate, "moved", lambda *a: [])
    monkeypatch.setattr(gate, "run_stage", lambda name, *a: seen.setdefault(name, a[-1]) and
                        {"name": name, "verdict": "pass", "problems": [], "cases": {}})
    monkeypatch.setattr(gate, "record_of", lambda *a: {"green": False, "sha": "a" * 40})
    monkeypatch.setattr(gate, "write", lambda rec, root: tmp_path)
    gate.run(args)
    assert seen == {"portal-sweep": 18000, "ws": 3600}, seen
    # without --sweep-timeout the stage gets the cap of its counted tests
    seen.clear()
    monkeypatch.setattr(gate, "sweep_tests", lambda *a: 330)
    gate.run(argparse.Namespace(**{**vars(args), "sweep_timeout": None}))
    # tmp_path holds no sweep.sh that reads SWEEP_WORKERS, so the stage is sized for one worker
    assert seen == {"portal-sweep": gate.sweep_cap(330, 1), "ws": 3600}, seen


def test_the_sweep_cap_holds_the_measured_sweep_with_a_quarter_to_spare():
    # T-6354: gate 39a90ab9 ran 291 of 324 tests in 17,304 s at one worker and hit the fixed 18,000 s cap
    assert 17304 / 291 <= gate.SWEEP_TEST_S
    cap = gate.sweep_cap(324, 1)
    assert 324 * gate.SWEEP_TEST_S <= 0.75 * cap, cap
    assert not 324 * gate.SWEEP_TEST_S <= 0.75 * 18000  # the old cap, red by the same rule
    assert gate.sweep_cap(400, 1) > cap  # it grows with the sweep


def test_the_parallel_sweep_cap_spreads_the_measured_work_over_the_workers():
    # T-6404: gate f1d9eceb listed 328 tests and held dev's pins for its 26,240 s one-worker cap (~7.3 h)
    assert gate.sweep_cap(328, 1) == 26240
    cap = gate.sweep_cap(328)
    assert 328 * gate.SWEEP_TEST_S / gate.SWEEP_WORKERS <= 0.75 * cap, cap
    assert cap < 2 * 3600, cap
    # the longest spec runs on one worker whatever the count: walk-dialogs took 2,298 s in gate 39a90ab9
    assert cap > 2298


def test_the_gate_counts_the_sweep_before_it_sizes_the_stage(tmp_path):
    listing = tmp_path / "sweep.sh"
    listing.write_text('#!/bin/sh\n[ -n "$SWEEP_LIST" ] || exit 9\necho "  [chromium] > a.spec.ts:1:1 > x"\n'
                       'echo "Total: 330 tests in 129 files"\n')
    listing.chmod(0o755)
    assert gate.sweep_tests([str(listing)], {}, tmp_path, tmp_path / "out/list.txt") == 330
    assert gate.sweep_stage_cap([str(listing)], {}, tmp_path, tmp_path / "list.txt") == gate.sweep_cap(330, 1)
    # a sweep.sh that reads SWEEP_WORKERS (T-6404) gets the cap of that many workers
    parallel = tmp_path / "parallel.sh"
    parallel.write_text(listing.read_text().replace("exit 9\n", "exit 9\n: \"${SWEEP_WORKERS:-4}\"\n"))
    parallel.chmod(0o755)
    assert gate.sweep_stage_cap([str(parallel)], {}, tmp_path, tmp_path / "par.txt") == gate.sweep_cap(330)
    assert gate.sweep_cap(330) < gate.sweep_cap(330, 1)
    # a sweep.sh that knows no SWEEP_LIST would run the whole sweep: it is never started
    old = tmp_path / "old.sh"
    old.write_text(f"#!/bin/sh\ntouch {tmp_path / 'ran'}\n")
    old.chmod(0o755)
    assert gate.sweep_tests([str(old)], {}, tmp_path, tmp_path / "old.txt") is None
    assert not (tmp_path / "ran").exists()
    assert gate.sweep_stage_cap([str(old)], {}, tmp_path, tmp_path / "old.txt") == gate.sweep_cap(gate.SWEEP_TESTS, 1)
    failing = tmp_path / "failing.sh"
    failing.write_text('#!/bin/sh\n# SWEEP_LIST\necho "Total: 3 tests in 1 file"\nexit 1\n')
    failing.chmod(0o755)
    assert gate.sweep_tests([str(failing)], {}, tmp_path, tmp_path / "f.txt") is None


def test_the_per_apply_smoke_leaves_the_two_data_sweeps_to_the_hourly_recipe_and_the_gate():
    # T-6148: the catalogue (~15 min) and source-link (~5 min) sweeps held every apply ~20 min;
    # the per-apply smoke keeps every other check, the release gate and prod-smoke keep the sweeps.
    sweeps = ("smoke-catalogue.py", "smoke-source-links.py")
    per_apply = recipe("_smoke") + recipe("dev-smoke")
    assert not [s for s in sweeps if s in per_apply]
    for check in ("scripts/smoke.sh", "smoke-public-apps.py", "failed-jobs.py", "smoke-edge-wave.py",
                  "smoke-forge-login.sh", "disk-full.py"):
        assert check in per_apply, check
    assert all(s in recipe("_smoke-sweeps") for s in sweeps)
    assert "just _dev-guard" in recipe("dev-smoke-sweeps")
    assert 'just _smoke-sweeps "{{ dev_domain }}" dev' in recipe("dev-smoke-sweeps")
    full = recipe("dev-smoke-full")
    assert "just dev-smoke " in full and 'just _smoke-sweeps "{{ dev_domain }}" dev' in full
    assert 'just _smoke-sweeps "{{ prod_domain }}" smartcity' in recipe("prod-smoke")
    plan = [p for p in gate.stages(argparse.Namespace(conformance=ROOT, portal=ROOT, only=None, requests=None,
                                                             rate=1, duration="1s"), ROOT) if p[0] == "dev-smoke"]
    assert plan[0][1] == ["just", "dev-smoke-full"]


@pytest.mark.parametrize("name", ["dev-smoke", "dev-smoke-sweeps", "dev-smoke-full", "prod-smoke"])
def test_the_smoke_recipes_parse(name):
    run = subprocess.run(["just", "--dry-run", name], cwd=ROOT, capture_output=True, text=True, check=False,
                         env={**os.environ, "JUST_NO_DOTENV": "true"})
    assert run.returncode == 0, run.stderr


def broker_repo(tmp_path, sha="abc1234"):
    """A deployment checkout whose broker pin names Antares commit `sha` (T-6151)."""
    repo = tmp_path / "deploy"
    (repo / "components/context-broker").mkdir(parents=True)
    (repo / gate.BROKER_IMAGES).write_text(f"x:\n  # Antares dev {sha} (dev-120, over aaaaaaa)\n    tag: 'dev-120'\n")
    for args in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "pin"]):
        subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)
    return repo


class FakeGh:
    """gh for the sweep stage: answers the runs list from `pages` (one per call, the last repeats), records calls."""

    def __init__(self, *pages):
        self.pages, self.calls = list(pages), []

    def __call__(self, *args):
        self.calls.append(args)
        if args[0] == "api":
            return json.dumps({"workflow_runs": self.pages.pop(0) if len(self.pages) > 1 else self.pages[0]})
        return ""


def sweep_run(run_id, title, status="completed", conclusion="success"):
    return {"id": run_id, "display_title": title, "status": status, "conclusion": conclusion,
            "created_at": f"2026-10-30T0{run_id % 10}:00:00Z", "html_url": f"https://github.com/x/runs/{run_id}"}


def test_the_antares_sweep_stage_is_last_and_runs_this_script(tmp_path):
    args = argparse.Namespace(conformance=tmp_path, portal=tmp_path, requests=None, rate=1, duration="1s",
                              only=None, rev="v1.2.3")
    name, argv, env, _ = gate.stages(args, tmp_path)[-1]
    assert name == "antares-sweep" == gate.REQUIRED[-1]
    assert argv[1:] == [str(SCRIPT), "antares-sweep", "v1.2.3"] and env == {}


def test_a_green_sweep_of_the_pinned_commit_passes_and_is_named_in_the_record(tmp_path, capsys):
    repo = broker_repo(tmp_path)
    fake = FakeGh([sweep_run(7, "sweep abc0000"), sweep_run(5, "sweep abc1234def0"), sweep_run(3, "sweep nothex1")])
    assert gate.antares_sweep("HEAD", wait=True, call=fake, cwd=repo) == 0
    out = capsys.readouterr().out
    assert "  ok  sweep run 5 of Antares abc1234: https://github.com/x/runs/5" in out
    assert [c[0] for c in fake.calls] == ["api"], "a green run dispatches nothing"
    # the run lands in the record as a case of the stage
    assert "antares-sweep::sweep run 5 of Antares abc1234" in gate.cases("antares-sweep", tmp_path, out)


def test_a_red_sweep_refuses_and_says_how_to_go_on(tmp_path, capsys):
    repo = broker_repo(tmp_path)
    fake = FakeGh([sweep_run(9, "sweep abc1234", conclusion="failure"), sweep_run(4, "sweep abc1234", conclusion="cancelled")])
    assert gate.antares_sweep("HEAD", wait=True, call=fake, cwd=repo) == 1
    out = capsys.readouterr().out
    assert "FAIL  sweep run 9 of Antares abc1234: failure" in out and "gh run rerun 9" in out
    assert gate.cases("antares-sweep", tmp_path, out) == {"antares-sweep::sweep run 9 of Antares abc1234": "fail"}


def test_without_a_sweep_the_gate_dispatches_one_and_waits_for_it(tmp_path, capsys):
    repo = broker_repo(tmp_path)
    fake = FakeGh([], [], [sweep_run(11, "sweep abc1234", status="in_progress", conclusion=None)],
                  [sweep_run(11, "sweep abc1234")])
    naps = []
    assert gate.antares_sweep("HEAD", wait=True, call=fake, sleep=naps.append, cwd=repo) == 0
    dispatches = [c for c in fake.calls if c[0] == "workflow"]
    assert dispatches == [("workflow", "run", "sweep.yml", "-R", gate.ANTARES_REPO, "--ref", "dev", "-f", "sha=abc1234")]
    assert len(naps) == 3 and "ok  sweep run 11" in capsys.readouterr().out
    # started by `run` without waiting: one dispatch, then back
    fake = FakeGh([])
    assert gate.antares_sweep("HEAD", wait=False, call=fake, cwd=repo) == 0
    assert [c[0] for c in fake.calls] == ["api", "workflow"]


def test_a_running_sweep_is_never_dispatched_twice(tmp_path):
    repo = broker_repo(tmp_path)
    fake = FakeGh([sweep_run(12, "sweep abc1234", status="queued", conclusion=None)])
    assert gate.antares_sweep("HEAD", wait=False, call=fake, cwd=repo) == 0
    assert [c[0] for c in fake.calls] == ["api"]


# ---------------------------------------------------------------------------- T-6318: the hold on dev


def pins_repo(tmp_path):
    """A deployment checkout with three commits: base, a gateway pin, then a Portal pin."""
    repo = tmp_path / "pins"
    git = lambda *a: subprocess.run(["git", *a], cwd=repo, check=True, capture_output=True, text=True).stdout.strip()
    shas = []
    for portal, gateway in (("1", "1"), ("1", "2"), ("2", "2")):
        for component, digest in (("portal", portal), ("context-gateway", gateway)):
            path = repo / "components" / component / "images.yaml"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"image: {{repository: ghcr.io/x/{component}, tag: a, digest: sha256:{digest * 64}}}\n")
        if not shas:
            git("init", "-q")
        git("add", "-A")
        git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "pin")
        shas.append(git("rev-parse", "HEAD"))
    return repo, shas


def hold(root, components, until="2999-01-01T00:00:00Z"):
    root.mkdir(parents=True, exist_ok=True)
    (root / gate.HOLD).write_text(json.dumps({"sha": "c" * 40, "stage": "x", "components": components,
                                              "until": until}))


def test_the_hold_only_shrinks_and_frees_the_pins_after_the_last_stage_that_measures_them():
    order = list(gate.REQUIRED)
    held = [gate.holding(order[i:]) for i in range(len(order))]
    assert held[0] == ["*"] and held[order.index("portal-sweep")] == ["*"]
    assert held[order.index("load")] == sorted(gate.DATA_PLANE)
    assert held[order.index("antares-sweep")] == [], "the GitHub sweep holds no dev pod"
    assert set(gate.MEASURES) == set(gate.REQUIRED), "every stage says what it measures"


def test_a_pin_range_is_held_only_where_it_moves_a_held_component(tmp_path):
    repo, (base, gw, portal) = pins_repo(tmp_path)
    root = tmp_path / "records"
    assert gate.held(base, portal, root, repo) == [], "no running gate holds nothing"
    hold(root, ["portal"])
    assert gate.held(base, gw, root, repo) == [], "a gateway pin passes a Portal hold"
    assert gate.held(gw, portal, root, repo) == ["portal"]
    hold(root, ["*"])
    assert gate.held(base, gw, root, repo) == ["context-gateway"]
    hold(root, [])
    assert gate.held(base, portal, root, repo) == []
    hold(root, ["*"], until="2026-11-01T07:00:00Z")
    assert gate.held(base, portal, root, repo) == [], "a dead gate's hold expires"
    (root / gate.HOLD).write_text("{not json")
    with pytest.raises(gate.Refused, match="unreadable hold"):
        gate.held(base, portal, root, repo)
    hold(root, "*")
    with pytest.raises(gate.Refused, match="no component list"):
        gate.held(base, portal, root, repo)


def test_held_exits_one_naming_the_held_components_and_zero_when_free(tmp_path, monkeypatch, capsys):
    repo, (base, gw, portal) = pins_repo(tmp_path)
    root = tmp_path / "records"
    hold(root, ["portal", "context-broker"])
    monkeypatch.setattr(gate, "ROOT", repo)
    monkeypatch.setattr(gate.held, "__defaults__", (gate.RECORDS, repo, None))
    assert gate.main(["held", gw, portal, "--records", str(root)]) == 1
    assert "held by the release gate of cccccccccccc (x, until 2999-01-01T00:00:00Z): portal" in capsys.readouterr().out
    assert gate.main(["held", base, gw, "--records", str(root)]) == 0
    assert capsys.readouterr().out.strip() == "free"


def gate_args(tmp_path, only):
    return argparse.Namespace(conformance=tmp_path, portal=tmp_path, requests=None, rate=8, duration="5m",
                              only=only, stage_timeout=60, sweep_timeout=60, rev="HEAD", records=tmp_path / "rec")


def test_a_stage_under_which_dev_moved_a_component_it_measures_is_red_and_the_hold_is_released(tmp_path, monkeypatch):
    repo, (base, gw, portal) = pins_repo(tmp_path)
    monkeypatch.setattr(gate, "ROOT", repo)
    monkeypatch.setattr(gate.moved, "__defaults__", (repo,))
    monkeypatch.setattr(gate, "candidate", lambda rev: {"sha": base, "images": []})
    monkeypatch.setattr(gate, "preflight", lambda rev, names: {})
    monkeypatch.setattr(gate, "antares_sweep", lambda *a, **k: 0)
    # dev runs the candidate through the preflight and ws; the Portal pin lands while antares-sweep waits,
    # the gateway pin under mcp (a hand apply the hold refused)
    readings = iter([base, base, base, base, gw, portal, portal])
    monkeypatch.setattr(gate, "dev_commit", lambda: next(readings))
    holds = {}

    def stage(name, *a):
        holds[name] = json.loads((tmp_path / "rec" / gate.HOLD).read_text())["components"]
        return {"name": name, "verdict": "pass", "problems": [], "cases": {name: "pass"}, "known_red": {},
                "seconds": 0}

    monkeypatch.setattr(gate, "run_stage", stage)
    assert gate.run(gate_args(tmp_path, ["ws", "mcp", "antares-sweep"])) == 1
    rec = json.loads((tmp_path / "rec" / base / "record.json").read_text())
    verdicts = {s["name"]: (s["verdict"], s["dev"], s["problems"]) for s in rec["stages"]}
    assert verdicts["ws"] == ("pass", [base, base], [])
    assert verdicts["mcp"][0] == "fail" and verdicts["mcp"][1] == [base, gw]
    assert verdicts["mcp"][2] == [f"dev ran other pins of context-gateway (deployment {gw[:12]}) under this stage"]
    assert verdicts["antares-sweep"] == ("pass", [portal, portal], []), "a Portal pin lands beside the GitHub sweep"
    assert holds == {"ws": sorted(gate.DATA_PLANE), "mcp": sorted(gate.DATA_PLANE), "antares-sweep": []}
    assert not (tmp_path / "rec" / gate.HOLD).exists(), "the finished run holds nothing"
    assert rec["sha"] == base, "the record still names the candidate it judged"


def test_the_gate_refuses_in_seconds_when_dev_runs_other_pins_or_records_none(tmp_path, monkeypatch):
    repo, (base, gw, _) = pins_repo(tmp_path)
    monkeypatch.setattr(gate.moved, "__defaults__", (repo,))
    monkeypatch.setattr(gate, "candidate", lambda rev: {"sha": base, "images": []})
    monkeypatch.setattr(gate, "preflight", lambda rev, names: {})
    monkeypatch.setattr(gate, "dev_commit", lambda: gw)
    with pytest.raises(gate.Refused, match=r"dev runs other pins than .* for context-gateway: `just dev-apply"):
        gate.run(gate_args(tmp_path, ["ws"]))
    monkeypatch.setattr(gate, "dev_commit", lambda: (_ for _ in ()).throw(gate.Refused("dev records no applied commit")))
    with pytest.raises(gate.Refused, match="no applied commit"):
        gate.run(gate_args(tmp_path, ["ws"]))
    assert not (tmp_path / "rec" / gate.HOLD).exists()
