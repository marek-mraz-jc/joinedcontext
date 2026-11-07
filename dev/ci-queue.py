#!/usr/bin/env python3
"""Queue and run time of every GitHub Actions job of the account's repositories
(T-6046): the account runs at most 20 jobs at once, so a job's queue time is
what the other repositories held. Prints one Markdown row per repository and
workflow: runs, jobs that took a runner, jobs per run, median and longest
queue, runner minutes, and how many jobs ran under 60 s.

    dev/ci-queue.py 2026-10-28T10:00:00Z [repo ...]
    dev/ci-queue.py --from jobs.json      # table of a saved collection
    dev/ci-queue.py --selftest

Collecting reads `gh api` (GH_TOKEN), one call per run; --save keeps the jobs.
"""
import collections
import datetime as dt
import json
import statistics
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

OWNER = "marek-mraz-jc"
REPOS = ["Antares-NGSI-LD-Context-Broker", "joinedcontext-portal", "joinedcontext-platform",
         "joinedcontext-deployment", "joinedcontext-docs", "joinedcontext-conformance"]


def api(path):
    out = subprocess.run(["gh", "api", "--paginate", "--slurp", path],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def collect(since, repos):
    jobs = []
    for repo in repos:
        runs = [r for page in api(f"repos/{OWNER}/{repo}/actions/runs?per_page=100&created=>={since}")
                for r in page["workflow_runs"]]

        def of(run, repo=repo):
            pages = api(f"repos/{OWNER}/{repo}/actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")
            return [{"repo": repo, "wf": run["name"], "run": run["id"], "job": j["name"],
                         "conclusion": j["conclusion"], "runner": j.get("runner_name"),
                         "created": j["created_at"], "started": j["started_at"], "completed": j["completed_at"]}
                    for page in pages for j in page["jobs"]]

        with ThreadPoolExecutor(8) as pool:
            for js in pool.map(of, runs):
                jobs.extend(js)
    return jobs


def seconds(a, b):
    return (dt.datetime.fromisoformat(b)
            - dt.datetime.fromisoformat(a)).total_seconds()


def table(jobs):
    """Markdown rows, busiest first; a skipped job never took a runner."""
    groups = collections.defaultdict(list)
    for j in jobs:
        if j["conclusion"] != "skipped" and j["runner"] and j["started"] and j["completed"]:
            groups[(j["repo"], j["wf"])].append(j)
    rows = []
    for (repo, wf), js in groups.items():
        queue = [seconds(j["created"], j["started"]) for j in js]
        ran = [seconds(j["started"], j["completed"]) for j in js]
        runs = len({j["run"] for j in js})
        rows.append((len(js), (f"| {repo} | {wf} | {runs} | {len(js)} | {len(js) / runs:.1f} | "
                              f"{statistics.median(queue):.0f} | {max(queue):.0f} | {sum(ran) / 60:.0f} | "
                              f"{sum(1 for r in ran if r < 60)} |")))
    head = ["| repo | workflow | runs | jobs | jobs/run | queue median s | queue max s | runner min | jobs < 60 s |",
            "|---|---|---|---|---|---|---|---|---|"]
    return head + [r for _, r in sorted(rows, key=lambda r: -r[0])]


def selftest():
    t = "2026-10-29T10:00:00Z"
    job = {"repo": "r", "wf": "fast", "run": 1, "job": "a", "conclusion": "success", "runner": "x",
               "created": t, "started": "2026-10-29T10:01:00Z", "completed": "2026-10-29T10:01:30Z"}
    rows = table([job, dict(job, job="b", completed="2026-10-29T10:03:00Z"),
                  dict(job, job="c", conclusion="skipped", runner=None)])
    assert rows[2] == "| r | fast | 1 | 2 | 2.0 | 60 | 60 | 2 | 1 |", rows
    print("ci-queue selftest: ok")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args == ["--selftest"]:
        selftest()
    elif args[:1] == ["--from"] and len(args) == 2:
        with open(args[1]) as f:
            print("\n".join(table(json.load(f))))
    elif args and not args[0].startswith("-"):
        save = "--save" in args
        rest = [a for a in args[1:] if a != "--save"]
        jobs = collect(args[0], rest or REPOS)
        if save:
            with open(f"ci-jobs-{args[0][:13]}.json", "w") as f:
                json.dump(jobs, f)
        print("\n".join(table(jobs)))
    else:
        sys.exit(__doc__)
