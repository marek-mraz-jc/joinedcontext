"""smoke.yml hands each shard the App bundles BuildKit built (brief 120): apps.sh must never fall
back to `docker build --target apps` inside dockerd, and the gate never drops the App rows. The
shards (T-6954) run every suite exactly once, logs on each, and the verdict expects every shard."""
import importlib.util
import pathlib
import re

import yaml

repo = pathlib.Path(__file__).resolve().parents[4]
workflow = repo / ".github/workflows/smoke.yml"
_spec = importlib.util.spec_from_file_location("smoke_all_for_yml", repo / "bins/jc/smoke/all/suite.py")
all_suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(all_suite)


def jobs():
    return yaml.safe_load(workflow.read_text())["jobs"]


def steps(job):
    return jobs()[job]["steps"]


def test_each_shard_runs_the_bundles_image_the_apps_job_builds():
    build = next(s for s in steps("apps") if s.get("uses", "").startswith("docker/build-push-action@"))
    assert build["with"]["target"] == "apps-bundles" and build["with"]["tags"] == "jc-apps:smoke"
    assert build["with"]["outputs"] == "type=docker,dest=${{ runner.temp }}/jc-apps.tar"
    run_all = next(s for s in steps("shard") if s.get("name") == "run all (shard)")
    assert run_all["env"]["REHEARSAL_JC_IMAGE"] == "jc-apps:smoke"
    runs = [s.get("run", "") for s in steps("shard")]
    loaded = next(i for i, r in enumerate(runs) if 'docker load -i "$RUNNER_TEMP/apps/jc-apps.tar"' in r)
    assert loaded < steps("shard").index(run_all) and "apps" in jobs()["shard"]["needs"]
    assert "FROM scratch AS apps-bundles\nCOPY --from=apps /srv/apps /srv/apps\n" in (
        repo / "bins/jc/Dockerfile"
    ).read_text()


def test_jcctl_is_built_beside_jc_and_shipped_to_the_shards_with_the_ui_it_embeds():
    """import.sh refuses to import without JCCTL_BIN, default `jcctl` beside jc (target/debug)."""
    runs = [s.get("run", "") for s in steps("build")]
    jcctl = next(i for i, r in enumerate(runs) if "-p jcctl" in r)
    assert "vendor/joinedcontext-platform/Cargo.toml" in runs[jcctl] and "--target-dir target" in runs[jcctl]
    tar = next(i for i, r in enumerate(runs) if "tar -cf" in r)
    # up.sh and import.sh run kaisatsu, daiya and jcctl beside jc (`dirname "$jc_bin"`)
    shipped = ("target/debug/jc ", "target/debug/kaisatsu ", "target/debug/daiya ", "target/debug/jcctl", "crates/portal/ui/dist")
    assert tar > jcctl and all(f in runs[tar] for f in shipped)
    rehearsal = (repo / "deploy/rehearsal/up.sh").read_text() + (repo / "deploy/rehearsal/import.sh").read_text()
    for b in re.findall(r'dirname "\$jc_bin"\)/(\w+)', rehearsal):
        assert f"target/debug/{b}" in runs[tar], b
    assert "JCCTL_BIN" not in str(jobs())


def test_the_gate_never_turns_the_apps_off():
    doc = yaml.safe_load(workflow.read_text())
    for name, job in doc["jobs"].items():
        for env in (doc.get("env", {}), job.get("env", {}), *(s.get("env", {}) for s in job["steps"])):
            assert "REHEARSAL_APPS" not in env, name
        for s in job["steps"]:
            assert "REHEARSAL_APPS" not in s.get("run", ""), name


def matrix():
    return jobs()["shard"]["strategy"]["matrix"]["include"]


def test_every_suite_runs_on_exactly_one_shard_and_logs_on_each():
    gate = (repo / "bins/jc/smoke/ci/gate.sh").read_text()
    variants = re.findall(r'--with "(\w+@\w+)=', gate)
    plan = [r for s in all_suite.suites() for r in (s, *(v for v in variants if all_suite.program(v) == s))]
    assert "multiorg@path" in plan and "ui" in plan and "restore" in plan
    ran = {r: [] for r in plan}
    for m in matrix():
        for r in plan:
            if (not m.get("only") or all_suite.picked(r, m["only"])) and not all_suite.picked(r, m.get("skip", "")):
                ran[r].append(m["shard"])
    assert ran.pop("logs") == [m["shard"] for m in matrix()]
    split = {"ui"}  # by persona, ui's rows below
    assert {r: len(v) for r, v in ran.items() if r not in split} == {r: 1 for r in ran if r not in split}
    # restore on a shard of its own (T-7313: after pipelines that shard took 22-29 min); the
    # Pipelines' first runs write new types between its before and after runs (runs 43683591053,
    # 43685936569), so the gate makes its before run wait for them, bounded, never a fixed sleep
    assert ran["restore"] == ["restore"] and ran["pipelines"] == ["pipelines"]
    assert next(m for m in matrix() if m["shard"] == "restore")["only"] == "restore,logs"
    line = next(x for x in gate.splitlines() if x.startswith('args+=(--with "restore='))
    drain = re.search(r"--drain (\d+)", line)
    # 25 min at most: green trains drained after 875-888 s, so 900 s failed on slower runners; the
    # wait ends as soon as the queue drains and the shard keeps its 60 min job timeout
    assert "--home $home" in line and drain and 0 < int(drain.group(1)) <= 1500
    assert "sleep" not in next(m for m in matrix() if m["shard"] == "restore").get("with", "")
    ui = [m["with"] for m in matrix() if m["shard"] in ran["ui"]]
    personas = [x for w in ui for x in w.split("--personas ")[1].split()[0].split(",")]
    assert sorted(personas) == ["admin", "outsider", "viewer"]
    assert sum("--no-shared" not in w for w in ui) == 1  # the dataroom and shako walks run once
    run_all = next(s for s in steps("shard") if s.get("name") == "run all (shard)")
    assert set(run_all["env"]) >= {"SMOKE_SHARD", "SMOKE_ONLY", "SMOKE_SKIP", "SMOKE_WITH"}
    assert "SMOKE_E2E" not in str(jobs()["shard"])  # e2e runs in its own job, once


def test_the_verdict_expects_every_shard_and_runs_whatever_they_did():
    v = jobs()["verdict"]
    assert v["if"] == "always()" and set(v["needs"]) == {"runner-tests", "e2e", "shard"}
    merge = next(s for s in v["steps"] if s.get("name") == "merge")["run"]
    expect = merge.split("--expect ")[1].split()[0].split(",")
    assert expect == [m["shard"] for m in matrix()] + ["e2e"]
    assert "--known bins/jc/smoke/ci/known.txt" in merge
    e2e = next(s for s in steps("e2e") if s.get("name") == "run all (e2e)")["run"]
    assert "--shard e2e --only e2e --e2e" in e2e and "--known bins/jc/smoke/ci/known.txt" in e2e
    uploads = [s["with"]["name"] for j in ("e2e", "shard") for s in steps(j) if "upload-artifact" in s.get("uses", "")]
    assert uploads == ["smoke-shard-e2e", "smoke-shard-${{ matrix.shard }}"]
    assert any(s.get("with", {}).get("pattern") == "smoke-shard-*" for s in v["steps"])
    assert any("needs.runner-tests.result" in str(s.get("env", "")) for s in v["steps"])


def test_only_a_green_verdict_on_the_merge_branch_fast_forwards_main_and_only_that_job_writes():
    doc = yaml.safe_load(workflow.read_text())
    ff = doc["jobs"]["ff-main"]
    # needs the verdict and no status function: GitHub's implicit success() keeps a red one out
    assert ff["needs"] == "verdict" and "always()" not in ff["if"] and "failure()" not in ff["if"]
    assert "github.ref == 'refs/heads/jc-portal-1'" in ff["if"]
    assert ff["permissions"] == {"contents": "write", "actions": "write"}
    assert doc["permissions"] == {"contents": "read"}
    assert [n for n, j in doc["jobs"].items() if "permissions" in j] == ["ff-main"]
    assert ff["concurrency"] == {"group": "ff-main", "cancel-in-progress": False}
    checkout = ff["steps"][0]
    assert "checkout" in checkout["uses"] and checkout["with"]["persist-credentials"] is False
    assert ff["steps"][1]["run"] == "sh .github/scripts/ff-main.sh"
    assert ff["steps"][1]["env"]["SHA"] == "${{ github.sha }}"


def test_every_action_is_pinned_by_sha_and_no_secret_reaches_a_job():
    text = workflow.read_text()
    uses = re.findall(r"uses: (\S+)", text)
    assert uses and all(re.fullmatch(r"[\w.-]+/[\w.-]+@[0-9a-f]{40}", u) for u in uses), uses
    assert "secrets." not in text


def test_the_logs_suite_looks_for_the_realm_secrets_and_jcs_state_files():
    # collect_secrets reads plain files directly in each dir: the realm client secrets sit in
    # secrets/realm and the suites' --state is a view of links, so each needs its own flag.
    gate = (repo / "bins/jc/smoke/ci/gate.sh").read_text()
    line = next(x for x in gate.splitlines() if x.startswith('args+=(--with "logs='))
    for d in ("$home/secrets ", "$home/secrets/realm ", "$home/state\""):
        assert f"--secrets-dir {d}" in line, d


def test_the_gate_judges_multiorg_in_host_and_path_mode_on_a_composed_bundle():
    # T-6803: on the one-installation synth the suite skipped its setup and judged nothing.
    gate = (repo / "bins/jc/smoke/ci/gate.sh").read_text()
    assert (repo / "bins/jc/smoke/ci/multiorg-export.tgz").is_file()
    assert "multiorg/compose.py" in gate and all(f"{o}=" in gate for o in ("ops", "hel", "zilina", "barcelona", "ctl-a", "ctl-b"))
    # T-6850: PF-121 keeps one installation Organization, the operators' ops with no data.
    assert "--installation ops" in gate and "JC_ENVIRONMENT=rehearsal" in gate
    withs = [x for x in gate.splitlines() if x.startswith('args+=(--with "multiorg')]
    assert [w.split('"')[1].split("=")[0] for w in withs] == ["multiorg", "multiorg@path"]
    assert all("--composed" in w and "--export $mo_export" in w for w in withs)
    assert "--hosts $mo_hosts" in gate and "REHEARSAL_JC_TOML_EXTRA=$mo_hosts multiorg mo-hosts" in gate
