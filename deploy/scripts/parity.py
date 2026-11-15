#!/usr/bin/env python3
"""Does the production instance carry what dev carries (T-4678)? Read-only on both clusters.

    DEV_KUBECONFIG=... PROD_KUBECONFIG=... scripts/parity.py            # snapshot both, compare
    scripts/parity.py snapshot dev|smartcity > snap.json                 # one side (its *_KUBECONFIG)
    scripts/parity.py seeds > seeds.json                                 # both rendered seeds
    scripts/parity.py compare dev.json smartcity.json [seeds.json]       # recorded snapshots

A snapshot holds, per instance:
- every object of the forge's configuration repositories (the Portal's source of truth), by
  repository path, with a hash of its text after the seed rewrites are undone (hostnames, the
  owner account, the demo users), so a changed schedule, sampling or setting shows as "differs";
- the App repositories;
- the CKAN catalogue: datasets, organizations, groups and the resource count;
- per dataset with an Endpoint, whether its data model is served: `schema/index.json` answers 200
  with at least one model (the gateway answers an empty list when the broker's index fails);
- the broker's entities, counted per tenant and type;
- the pipelines that have reported a status to the Portal.

The comparison lists every difference by name and exits 1 when one is unexplained. With the two
rendered seeds (the live run renders them; `compare` takes them as a third file), an object is
judged by where it comes from: one prod's seed carries must be on prod and equal to dev's; one
only dev's seed carries is left out of prod by its seed filter (a test object, the demo feed);
one in no seed was made on that instance at runtime. Those two kinds are allowed and listed. Live
counts may differ within `live_tolerance`. A test object (TEST_OBJECTS) on prod is a failure.
"""

import base64
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

INSTANCES = {
    "dev": {"namespace": "dev", "domain": "dev.joinedcontext.com", "kubeconfig": "DEV_KUBECONFIG"},
    "smartcity": {"namespace": "smartcity", "domain": "smartcity.joinedcontext.com", "kubeconfig": "PROD_KUBECONFIG"},
}
# The seed files production leaves out (the gitea seed filter of T-4678, same expression).
TEST_OBJECTS = re.compile(r"/(deployments|sources)/[^/]*eval[^/]*\.yaml$|/serviceaccounts/conformance\.yaml$|/policies/conformance-[^/]*\.yaml$")
REPOS = "/data/git/gitea-repositories"


def live_tolerance(a: int, b: int) -> bool:
    """Live data drifts: ±10 % or ±50, whichever is larger (T-4678)."""
    return abs(a - b) <= max(0.10 * max(a, b), 50)


def normalize(text: str, domain: str) -> str:
    """Undo what the production seed rewrites change, so equal seeds hash equal."""
    text = re.sub(r"(^|[^a-z0-9.-])data\." + re.escape(domain), r"\1opendata." + domain, text)
    text = text.replace(domain, "{domain}")
    # What each forge writes for itself: an App's own repository commit and its build status.
    text = re.sub(r"(?m)^(\s*ref:\s*)[0-9a-f]{40}$", r"\1{commit}", text)
    text = re.sub(r"(?m)^status:\n(?:[ \t].*(?:\n|$))*", "", text)
    text = re.sub(r"(?m)^\s*-\s*user:\s*demo\.(?!steward@)\S+\n", "", text)
    return re.sub(r"\bdemo\.steward@[A-Za-z0-9.-]+|\b[A-Za-z0-9._%+-]+@joinedcontext\.com\b", "{owner}", text)


def run(env: str, *args: str, stdin: str | None = None) -> str:
    kubeconfig = os.environ.get(INSTANCES[env]["kubeconfig"])
    if not kubeconfig:
        sys.exit(f"{env}: set {INSTANCES[env]['kubeconfig']}")
    return subprocess.run(["kubectl", "--kubeconfig", kubeconfig, "-n", INSTANCES[env]["namespace"], *args],
                          input=stdin, check=True, capture_output=True, text=True).stdout


def pod(env: str, prefix: str) -> str:
    names = run(env, "get", "pods", "-o", "name").split()
    return next(n for n in names if re.match(rf"pod/{prefix}", n))


def forge(env: str) -> tuple[dict[str, str], list[str]]:
    """Every configuration object by repository path, and the App repositories."""
    script = f"""cd {REPOS}
for r in joinedcontext/*.git; do
  git --git-dir="$r" ls-tree -r --name-only HEAD 2>/dev/null | grep -E '\\.ya?ml$' | while read -r f; do
    printf '%s\\0%s\\0' "$r:$f" "$(git --git-dir="$r" show "HEAD:$f" | base64 -w0)"
  done
done"""
    out = run(env, "exec", pod(env, "gitea-[0-9a-f]+-"), "-c", "gitea", "--", "sh", "-c", script)
    parts = out.split("\0")
    domain = INSTANCES[env]["domain"]
    objects = {
        path: hashlib.sha256(normalize(base64.b64decode(blob).decode(errors="replace"), domain).encode()).hexdigest()[:16]
        for path, blob in zip(parts[0::2], parts[1::2]) if path
    }
    apps = run(env, "exec", pod(env, "gitea-[0-9a-f]+-"), "-c", "gitea", "--", "ls", f"{REPOS}/joinedcontext-apps").split()
    return objects, sorted(apps)


def psql(env: str, db: str, sql: str) -> list[list[str]]:
    out = run(env, "exec", "-i", "postgres-cluster-1", "-c", "postgres", "--",
              "psql", "-X", "-q", "-A", "-t", "-F", "\t", "-d", db, stdin=sql)
    return [line.split("\t") for line in out.splitlines() if line]


def ckan(env: str) -> dict:
    base = f"https://opendata.{INSTANCES[env]['domain']}/api/3/action/"

    def call(action: str):
        for attempt in range(3):
            try:
                with urllib.request.urlopen(base + action, timeout=60) as r:
                    return json.load(r)["result"]
            except urllib.error.HTTPError as e:
                # The edge limits the catalogue's API: wait as it says, then ask again.
                if e.code != 429 or attempt == 2:
                    raise
                time.sleep(min(int(e.headers.get("Retry-After") or 10), 60))

    datasets = call("package_search?rows=1000")["results"]
    return {
        "datasets": sorted(d["name"] for d in datasets),
        "resources": sum(d.get("num_resources", 0) for d in datasets),
        "organizations": sorted(call("organization_list")),
        "groups": sorted(call("group_list")),
        "models": models(env, datasets),
    }


def endpoint_of(dataset: dict) -> str | None:
    """The Endpoint slug a dataset's resources point at, or None for a dataset without one."""
    for resource in dataset.get("resources") or []:
        if m := re.search(r"/api/endpoint/([a-z0-9]+)/", resource.get("url") or ""):
            return m[1]
    return None


def served(index: dict) -> bool:
    return bool(index.get("models"))


def models(env: str, datasets: list[dict]) -> dict[str, bool]:
    """Per dataset with an Endpoint: is its data model served (T-4678, the owner's catalogue)?"""
    out = {}
    for dataset in datasets:
        if not (slug := endpoint_of(dataset)):
            continue
        url = f"https://api.{INSTANCES[env]['domain']}/api/endpoint/{slug}/schema/index.json"
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                out[dataset["name"]] = served(json.load(r))
        except (OSError, ValueError):
            out[dataset["name"]] = False
    return out


def snapshot(env: str) -> dict:
    objects, apps = forge(env)
    entities = psql(env, "antares", "set statement_timeout='300s'; select tenant_id || '|' || t, count(*) "
                    "from entities, unnest(types) t group by 1;")
    pipelines = psql(env, "portal", "select project || '/' || pipeline from pipeline_status;")
    return {
        "objects": objects,
        "apps": apps,
        "ckan": ckan(env),
        "entities": {k: int(v) for k, v in entities},
        "pipelines": sorted(p[0] for p in pipelines),
    }


def seed_path(configmap: str, key: str) -> str | None:
    """The forge path a gitea seed ConfigMap key lands on, or None for an App template's file."""
    parts = key.split("__")
    if configmap.startswith("gitea-seed-") and parts[0] == "projects" and len(parts) > 2:
        return f"joinedcontext/{parts[1]}.git:" + "/".join(parts[2:])
    if configmap.startswith("gitea-bootstrap"):
        return "joinedcontext/configuration.git:" + "/".join(parts)
    return None


def seeds(env: str) -> list[str]:
    """Every forge path the environment's rendered gitea seed writes (needs `just _dev-assemble`)."""
    import yaml
    out = subprocess.run(["helmfile", "-f", "deployment/helmfile.yaml", "-e", env, "-l", "component=gitea",
                          "template", "-q"], check=True, capture_output=True, text=True).stdout
    return sorted({
        p for d in yaml.safe_load_all(out) if d and d.get("kind") == "ConfigMap"
        for k in (d.get("data") or {}) if (p := seed_path(d["metadata"]["name"], k))
        and re.search(r"\.ya?ml$", p)
    })


def compare(dev: dict, prod: dict, seeded: dict | None = None) -> tuple[list[str], list[str]]:
    """(failures, allowed) differences, each one line naming the object. `seeded` maps "dev" and
    "smartcity" to their rendered seed paths; without it every missing object fails."""
    fail, allowed = [], []
    dev_seed, prod_seed = (set(seeded[k]) for k in ("dev", "smartcity")) if seeded else (None, None)
    for path in sorted(set(dev["objects"]) | set(prod["objects"])):
        test = bool(TEST_OBJECTS.search(path))
        if test and path in prod["objects"]:
            fail.append(f"test object on prod: {path}")
        elif path not in prod["objects"]:
            if test:
                allowed.append(f"test object, dev only: {path}")
            elif prod_seed is None or path in prod_seed:
                fail.append(f"missing on prod: {path}")
            elif path in dev_seed:
                allowed.append(f"left out of prod's seed: {path}")
            else:
                allowed.append(f"made on dev: {path}")
        elif path not in dev["objects"]:
            if prod_seed is not None and path not in prod_seed:
                allowed.append(f"made on prod: {path}")
            else:
                fail.append(f"only on prod: {path}")
        elif dev["objects"][path] != prod["objects"][path]:
            fail.append(f"differs: {path}")
    # An App repository or a pipeline's status follows its manifest: one prod does not carry
    # (made on dev, left out of its seed) is no difference of its own.
    def manifest(kind: str, name: str) -> str:
        if kind == "App":
            project, _, app = name.removesuffix(".git").partition("_")
            return f"joinedcontext/{project}.git:apps/{app}/app.yaml"
        project, _, pipeline = name.partition("/")
        return f"joinedcontext/{project}.git:pipelines/{pipeline}/pipeline.yaml"

    for kind, a, b in (("App", dev["apps"], prod["apps"]), ("pipeline reporting", dev["pipelines"], prod["pipelines"])):
        for n in sorted(set(a) - set(b)):
            if manifest(kind, n) in prod["objects"]:
                fail.append(f"{kind} missing on prod: {n}")
            else:
                allowed.append(f"{kind} without its manifest on prod: {n}")
        fail += [f"{kind} only on prod: {n}" for n in sorted(set(b) - set(a))]
    for kind, a, b in (("CKAN dataset", dev["ckan"]["datasets"], prod["ckan"]["datasets"]),
                       ("CKAN organization", dev["ckan"]["organizations"], prod["ckan"]["organizations"]),
                       ("CKAN group", dev["ckan"]["groups"], prod["ckan"]["groups"])):
        fail += [f"{kind} missing on prod: {n}" for n in sorted(set(a) - set(b))]
        fail += [f"{kind} only on prod: {n}" for n in sorted(set(b) - set(a))]
    for name, ok in sorted(dev["ckan"].get("models", {}).items()):
        if ok and not prod["ckan"].get("models", {}).get(name, False) and name in prod["ckan"]["datasets"]:
            fail.append(f"data model not served on prod: {name}")
    if not live_tolerance(dev["ckan"]["resources"], prod["ckan"]["resources"]):
        fail.append(f"CKAN resources: dev {dev['ckan']['resources']}, prod {prod['ckan']['resources']}")
    for key in sorted(set(dev["entities"]) | set(prod["entities"])):
        a, b = dev["entities"].get(key, 0), prod["entities"].get(key, 0)
        if not live_tolerance(a, b):
            fail.append(f"entities {key}: dev {a}, prod {b}")
    return fail, allowed


def main(argv: list[str]) -> int:
    if argv[:1] == ["snapshot"] and len(argv) == 2 and argv[1] in INSTANCES:
        json.dump(snapshot(argv[1]), sys.stdout, indent=1, sort_keys=True)
        return 0
    seeded = None
    if argv[:1] == ["compare"] and len(argv) in (3, 4):
        dev, prod = (json.load(open(p)) for p in argv[1:3])
        seeded = json.load(open(argv[3])) if len(argv) == 4 else None
    elif argv == ["seeds"]:
        json.dump({env: seeds(env) for env in INSTANCES}, sys.stdout, indent=1)
        return 0
    elif not argv:
        dev, prod = snapshot("dev"), snapshot("smartcity")
        seeded = {env: seeds(env) for env in INSTANCES}
    else:
        print(__doc__, file=sys.stderr)
        return 2
    fail, allowed = compare(dev, prod, seeded)
    for line in allowed:
        print(f"allowed  {line}")
    for line in fail:
        print(f"FAIL     {line}")
    print(f"{len(dev['objects'])} objects on dev, {len(prod['objects'])} on prod: "
          f"{len(fail)} unexplained, {len(allowed)} allowed differences")
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
