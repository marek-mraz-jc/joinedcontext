"""OPS-45, T-1719: scripts/rotate-secret.sh rotates an in-cluster credential and MEASURES it.

The script talks to a cluster, to helmfile, to Keycloak, to the forge and to PostgreSQL, so
the suite runs it against stubs on PATH that keep one small state file: which value each
copy of the Secret holds, which value the other side still accepts, and every call made. A
test asserts what the script did, in which order, and that no value ever reached a command
line, the output or the log.
"""

import json
import os
import re
import subprocess
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = PROJECT_ROOT / "scripts" / "rotate-secret.sh"

OLD = "old-Value-9f3a*x"
NEW = "new-Value-7c2b~y"

KUBECTL_STUB = r'''#!/usr/bin/env python3
import base64, json, os, sys
state_path = os.environ["STUB_STATE"]
state = json.load(open(state_path))
args = sys.argv[1:]
with open(os.environ["STUB_CALLS"], "a") as log:
    log.write("kubectl " + " ".join(args) + "\n")

def save():
    json.dump(state, open(state_path, "w"))

ns = args[args.index("-n") + 1] if "-n" in args else None
out = next((a for a in args if a.startswith("jsonpath=")), "")
if args[:3] == ["get", "secret", "-A"]:
    print("\n".join(n for n, v in state["copies"].items() if v is not None))
elif "get" in args and "secret" in args and ns is not None and "json" in args:
    value = state["copies"].get(ns)
    if value is None:
        sys.exit(1)
    sys.stdout.write(json.dumps({"apiVersion": "v1", "kind": "Secret", "type": "Opaque",
        "metadata": {"name": "s", "namespace": ns, "uid": "u", "resourceVersion": "1"},
        "data": {"key": base64.b64encode(value.encode()).decode()}}))
elif "get" in args and "secret" in args and ns is not None:
    value = state["copies"].get(ns)
    if value is None:
        sys.exit(1)
    previous = state.get("previous", {}).get(ns)
    if out == "jsonpath={.data}":
        sys.stdout.write(json.dumps({"key": base64.b64encode(value.encode()).decode()}))
    elif out == "jsonpath={.data.previous}":
        sys.stdout.write(base64.b64encode(previous.encode()).decode() if previous else "")
    elif "release-name" in out:
        sys.stdout.write(state.get("release", ""))
    elif "username" in out:
        sys.stdout.write(base64.b64encode(b"portal").decode())
    else:
        sys.stdout.write(base64.b64encode(value.encode()).decode())
elif "create" in args and "-f" in args:
    secret = json.loads(sys.stdin.read())
    assert "resourceVersion" not in secret["metadata"], "create refuses a resourceVersion"
    state["copies"][ns] = base64.b64decode(secret["data"]["key"]).decode()
    save()
elif "delete" in args and "secret" in args:
    state["copies"][ns] = None
    state.get("previous", {}).pop(ns, None)
    save()
elif "patch" in args and "secret" in args:
    # A value may only arrive on stdin; the json removal carries no value at all.
    if "--patch-file" in args and args[args.index("--patch-file") + 1] == "/dev/stdin":
        data = json.loads(sys.stdin.read())["data"]
        if "previous" in data:
            state.setdefault("previous", {})[ns] = base64.b64decode(data["previous"]).decode()
        else:
            state["copies"][ns] = base64.b64decode(data["key"]).decode()
    elif args[args.index("-p") + 1] == '[{"op":"remove","path":"/data/previous"}]':
        del state["previous"][ns]
    else:
        sys.exit(1)
    save()
elif "get" in args and "serviceaccount" in args:
    sys.stdout.write(state.get("saRelease", ""))
elif "get" in args and "deployment,statefulset,daemonset" in args:
    print(json.dumps({"items": state.get("workloads", {}).get(ns, [])}))
elif "rollout" in args:
    sys.exit(0)
elif "get" in args and "pod" in args:
    sys.stdout.write(state.get("primary", "/"))
elif "exec" in args:
    password = sys.stdin.readline().rstrip("\n")
    sys.exit(0 if password == state["accepted"] else 2)
else:
    sys.exit(1)
'''

HELMFILE_STUB = r'''#!/usr/bin/env python3
import json, os, sys
state_path = os.environ["STUB_STATE"]
state = json.load(open(state_path))
args = sys.argv[1:]
with open(os.environ["STUB_CALLS"], "a") as log:
    log.write("helmfile " + " ".join(args) + "\n")
selector = args[args.index("-l") + 1]
if selector in state.get("syncFails", []):
    sys.exit(1)
if selector.startswith("secret-name=") or selector in ("release=gitea-bootstrap", "release=" + state.get("saRelease", "-")):
    value = state["regenerate"]
    for ns in state["copies"]:
        state["copies"][ns] = value
    # A forge token is deleted in Gitea by the bootstrap itself; a database role follows its Secret.
    if state.get("pushedBy") == selector.split("=")[0] or state.get("pushedBy") == "generator":
        state["accepted"] = value
if selector == "release=keycloak-config" and state.get("configPushes", True):
    state["accepted"] = next(v for v in state["copies"].values() if v is not None)
json.dump(state, open(state_path, "w"))
'''

CURL_STUB = r'''#!/usr/bin/env python3
import json, os, sys
state = json.load(open(os.environ["STUB_STATE"]))
args = sys.argv[1:]
with open(os.environ["STUB_CALLS"], "a") as log:
    log.write("curl " + " ".join(args) + "\n")
stdin = sys.stdin.read()
if "client_secret@-" in args:
    value = stdin
elif "-K" in args:
    value = stdin.split("Authorization: ", 1)[1].rsplit('"', 1)[0]
    for scheme in ("token ", "Bearer "):
        value = value.removeprefix(scheme)
else:
    sys.exit(2)
refusal = "403" if any("api_token_list" in a for a in args) else "401"
accepted = state.get("acceptedAlso", []) + [state["accepted"]]
sys.stdout.write("200" if value in accepted else refusal)
'''


def run(tmp_path: Path, state: dict, *args: str):
    state = {"copies": {"keycloak": OLD, "portal": OLD}, "accepted": OLD, "release": "secrets-x", **state}
    state["regenerate"] = state.get("regenerate", NEW)
    scripts = tmp_path / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)
    (scripts / "rotate-secret.sh").write_text(SCRIPT.read_text())
    (scripts / "rotate-secret.sh").chmod(0o755)
    stub_bin = tmp_path / "bin"
    stub_bin.mkdir(parents=True, exist_ok=True)
    for name, body in (("kubectl", KUBECTL_STUB), ("helmfile", HELMFILE_STUB), ("curl", CURL_STUB)):
        (stub_bin / name).write_text(body)
        (stub_bin / name).chmod(0o755)
    state_file = tmp_path / "state.json"
    state_file.write_text(json.dumps(state))
    calls_file = tmp_path / "calls.log"
    calls_file.write_text("")
    env = dict(os.environ)
    env["PATH"] = f"{stub_bin}:{env['PATH']}"
    env["STUB_STATE"] = str(state_file)
    env["STUB_CALLS"] = str(calls_file)
    result = subprocess.run(
        ["scripts/rotate-secret.sh", "--bound", "2", *args],
        cwd=tmp_path, env=env, capture_output=True, text=True,
    )
    calls = calls_file.read_text().splitlines()
    for text in (result.stdout, result.stderr, *calls):
        assert OLD not in text and NEW not in text, "a credential reached the output or a command line"
    return result, calls, json.loads(state_file.read_text())


def deployment(name: str, secret: str) -> dict:
    return {
        "kind": "Deployment",
        "metadata": {"name": name},
        "spec": {"template": {"spec": {"containers": [
            {"name": "app", "env": [{"name": "S", "valueFrom": {"secretKeyRef": {"name": secret, "key": "k"}}}]},
        ]}}},
    }


CLIENT = ("--instance", "dev", "--secret", "keycloak-client-portal-api", "--idm", "https://idm.x", "--realm", "r")


def test_what_the_platform_does_not_generate_is_refused_and_nothing_is_touched(tmp_path):
    """OPS-45: the administrators and operator-supplied keys have their own runbook."""
    for name in ("gitea-admin-credentials", "keycloak-admin-user", "agent-runner-model-key", "keycloak-smtp"):
        result, calls, _ = run(tmp_path, {}, "--instance", "dev", "--secret", name)
        assert result.returncode == 2, name
        assert "Runbook 5" in result.stderr
        assert calls == []


def test_a_secret_without_a_helm_release_is_never_deleted(tmp_path):
    result, calls, state = run(tmp_path, {"release": ""}, "--instance", "dev", "--secret", "portal-cookie-key")
    assert result.returncode == 2
    assert "no Helm release" in result.stderr
    assert not any("delete" in c for c in calls)
    assert state["copies"] == {"keycloak": OLD, "portal": OLD}


def test_a_client_secret_is_rotated_everywhere_pushed_to_keycloak_and_measured(tmp_path):
    """OPS-45: every copy goes before the apply, Keycloak gets the new one, readers restart."""
    workloads = {"portal": [deployment("portal", "keycloak-client-portal-api"), deployment("other", "unrelated")]}
    result, calls, state = run(tmp_path, {"workloads": workloads}, *CLIENT)
    assert result.returncode == 0, result.stdout + result.stderr
    deletes = [i for i, c in enumerate(calls) if "delete secret" in c]
    generator = next(i for i, c in enumerate(calls) if "secret-name=keycloak-client-portal-api sync" in c)
    config = next(i for i, c in enumerate(calls) if "release=keycloak-config sync" in c)
    assert len(deletes) == 2 and max(deletes) < generator < config
    restarted = [c for c in calls if "rollout restart" in c]
    assert restarted == ["kubectl -n portal rollout restart deployment/portal"]
    assert state["copies"] == {"keycloak": NEW, "portal": NEW}
    assert "the old value is refused" in result.stdout
    assert "rotated" in result.stdout


def test_a_client_secret_keycloak_never_received_fails_the_run(tmp_path):
    """The measurement, not the apply, decides: the old secret still working is a failure."""
    result, _, _ = run(tmp_path, {"configPushes": False}, *CLIENT)
    assert result.returncode == 1
    assert "still accepted" in result.stdout


@pytest.mark.parametrize("failing", ["secret-name=keycloak-client-portal-api", "release=keycloak-config"])
def test_a_failed_apply_puts_the_old_secret_back_in_every_copy(tmp_path, failing):
    """F-R2DEP3-010: every copy was deleted before the apply, so a failed sync left no Secret."""
    result, calls, state = run(tmp_path, {"syncFails": [failing]}, *CLIENT)
    assert result.returncode == 2
    assert "the old keycloak-client-portal-api is back in every copy" in result.stderr
    assert state["copies"] == {"keycloak": OLD, "portal": OLD}
    assert not any("rollout restart" in c for c in calls)


def test_a_copy_that_survives_the_apply_stops_the_run(tmp_path):
    result, _, _ = run(tmp_path, {"regenerate": OLD}, *CLIENT)
    assert result.returncode == 2
    assert "holds the old value" in result.stderr


def test_a_database_password_is_measured_by_a_login_on_the_primary(tmp_path):
    state = {"copies": {"portal": OLD, "postgres": OLD}, "pushedBy": "generator", "primary": "postgres/pg-1"}
    result, calls, _ = run(tmp_path, state, "--instance", "dev", "--secret", "db-portal")
    assert result.returncode == 0, result.stdout + result.stderr
    logins = [c for c in calls if " exec -i pg-1 " in c]
    assert len(logins) == 2, "one login with the old password, one with the new"
    assert all("PGPASSWORD" in c and "-U 'portal'" in c for c in logins)


def test_a_database_with_no_primary_to_ask_fails_rather_than_passes(tmp_path):
    state = {"copies": {"portal": OLD}, "pushedBy": "generator", "primary": "/"}
    result, _, _ = run(tmp_path, state, "--instance", "dev", "--secret", "db-portal")
    assert result.returncode == 1
    assert "no CloudNativePG primary" in result.stdout


def test_a_forge_token_is_minted_again_by_the_bootstrap_and_the_old_one_is_refused(tmp_path):
    state = {"copies": {"portal": OLD}, "release": "", "pushedBy": "release"}
    result, calls, _ = run(tmp_path, state, "--instance", "dev", "--secret", "gitea-token-portal", "--forge", "https://x/git")
    assert result.returncode == 0, result.stdout + result.stderr
    assert any("release=gitea-bootstrap sync" in c for c in calls)
    assert not any("secret-name=" in c for c in calls)


def test_a_session_key_is_rotated_and_its_readers_restart_without_a_measurement(tmp_path):
    workloads = {"portal": [deployment("portal", "portal-cookie-key")]}
    result, calls, _ = run(tmp_path, {"workloads": workloads}, "--instance", "dev", "--secret", "portal-cookie-key")
    assert result.returncode == 0, result.stdout + result.stderr
    assert any("rollout restart deployment/portal" in c for c in calls)
    assert not any(c.startswith("curl") for c in calls)


def test_a_session_key_that_comes_back_unchanged_stops_the_run(tmp_path):
    result, _, _ = run(tmp_path, {"regenerate": OLD}, "--instance", "dev", "--secret", "portal-cookie-key")
    assert result.returncode == 2
    assert "holds the old value" in result.stderr


def test_a_demo_persons_password_is_pushed_to_the_realm(tmp_path):
    result, calls, _ = run(tmp_path, {}, "--instance", "dev", "--secret", "keycloak-user-jana")
    assert result.returncode == 0, result.stdout + result.stderr
    generator = next(i for i, c in enumerate(calls) if "secret-name=keycloak-user-jana sync" in c)
    config = next(i for i, c in enumerate(calls) if "release=keycloak-config sync" in c)
    assert generator < config


SESSION_KEY = ("--instance", "dev", "--secret", "portal-cookie-key")
THIRD = "third-Value-5d1e.z"


def test_a_session_key_rotation_keeps_the_old_key_as_previous_before_any_reader_restarts(tmp_path):
    """T-2842 (OPS-45): the Portal opens cookies sealed with the previous key, so nobody is signed out."""
    workloads = {"portal": [deployment("portal", "portal-cookie-key")]}
    result, calls, state = run(tmp_path, {"workloads": workloads}, *SESSION_KEY)
    assert result.returncode == 0, result.stdout + result.stderr
    assert state["copies"] == {"keycloak": NEW, "portal": NEW}
    assert state["previous"] == {"keycloak": OLD, "portal": OLD}, "every copy keeps the retired key"
    patches = [i for i, c in enumerate(calls) if "patch secret portal-cookie-key --type merge --patch-file /dev/stdin" in c]
    restart = next(i for i, c in enumerate(calls) if "rollout restart" in c)
    assert len(patches) == 2 and max(patches) < restart, "a pod started before the patch would hold the new key alone"
    assert "--drop-previous" in result.stdout


def test_the_next_session_key_rotation_drops_the_key_the_last_one_kept(tmp_path):
    """At most one retired key is ever held: the second run keeps the first run's new key only."""
    state = {"copies": {"portal": NEW}, "previous": {"portal": OLD}, "regenerate": THIRD}
    result, _, state = run(tmp_path, state, *SESSION_KEY)
    assert result.returncode == 0, result.stdout + result.stderr
    assert state["copies"] == {"portal": THIRD}
    assert state["previous"] == {"portal": NEW}


def test_drop_previous_removes_the_retired_key_everywhere_and_restarts_its_readers(tmp_path):
    workloads = {"portal": [deployment("portal", "portal-cookie-key")]}
    state = {"copies": {"portal": NEW, "other": NEW}, "previous": {"portal": OLD, "other": OLD}, "workloads": workloads}
    result, calls, state = run(tmp_path, state, *SESSION_KEY, "--drop-previous")
    assert result.returncode == 0, result.stdout + result.stderr
    assert state["previous"] == {}
    assert state["copies"] == {"portal": NEW, "other": NEW}, "dropping is not a rotation"
    assert not any("delete secret" in c or c.startswith("helmfile") for c in calls)
    assert any("rollout restart deployment/portal" in c for c in calls)


def test_drop_previous_with_no_rotation_open_changes_nothing(tmp_path):
    workloads = {"portal": [deployment("portal", "portal-cookie-key")]}
    result, calls, _ = run(tmp_path, {"copies": {"portal": NEW}, "workloads": workloads}, *SESSION_KEY, "--drop-previous")
    assert result.returncode == 2
    assert "no rotation is open" in result.stderr
    assert not any("patch" in c or "rollout" in c for c in calls)


def test_drop_previous_is_refused_for_a_secret_no_reader_keeps_a_previous_of(tmp_path):
    result, calls, _ = run(tmp_path, {}, *CLIENT, "--drop-previous")
    assert result.returncode == 2
    assert "portal-cookie-key" in result.stderr
    assert calls == []


def test_the_portal_reads_the_retired_session_key_and_starts_without_one(rendered):
    """The render names JC_PORTAL_COOKIE_KEY_PREVIOUS; optional, so a Secret without `previous` starts the pod."""
    portal = next(
        d for d in rendered("local")
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"
    )
    env = {e["name"]: e for e in portal["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_PORTAL_COOKIE_KEY_PREVIOUS"]["valueFrom"]["secretKeyRef"] == {
        "name": "portal-cookie-key", "key": "previous", "optional": True,
    }
    assert "optional" not in env["JC_PORTAL_COOKIE_KEY"]["valueFrom"]["secretKeyRef"], "the active key stays required"


# T-5106: the catalogue token, the model key and the dry run of every class.

CATALOGUE = ("--instance", "dev", "--secret", "ckan-api-token", "--catalogue", "https://opendata.x")


def model_key(tmp_path: Path, value: str = NEW) -> tuple[str, ...]:
    key_file = tmp_path / "new-key"
    key_file.write_text(value + "\n")
    return ("--instance", "dev", "--secret", "agent-runner-model-key", "--from-file", str(key_file),
            "--model-base", "https://openrouter.x/api/v1")


CHANGES = re.compile(r" (delete|patch|rollout) ")


def test_the_catalogue_token_is_minted_again_by_its_release_and_the_old_one_is_refused(tmp_path):
    """The token Job's ServiceAccount names the release; CKAN refuses a revoked token as anonymous (403)."""
    state = {"copies": {"ckan": OLD}, "release": "", "saRelease": "ckan-catalogue", "pushedBy": "release",
             "workloads": {"ckan": [deployment("portal", "ckan-api-token")]}}
    result, calls, after = run(tmp_path, state, *CATALOGUE)
    assert result.returncode == 0, result.stdout + result.stderr
    assert any(c.startswith("helmfile") and "-l release=ckan-catalogue sync" in c for c in calls)
    assert after["copies"] == {"ckan": NEW} and after["accepted"] == NEW
    assert any("rollout restart deployment/portal" in c for c in calls)
    assert any("api_token_list" in c for c in calls)
    assert "the old value is refused" in result.stdout


def test_a_catalogue_token_without_its_release_is_never_deleted(tmp_path):
    state = {"copies": {"ckan": OLD}, "release": "", "saRelease": ""}
    result, calls, after = run(tmp_path, state, *CATALOGUE)
    assert result.returncode == 2 and "no catalogue release" in result.stderr
    assert after["copies"] == {"ckan": OLD}
    assert not any(CHANGES.search(c) or c.startswith("helmfile") for c in calls)


def test_a_catalogue_token_needs_the_catalogue_to_measure_at(tmp_path):
    result, calls, _ = run(tmp_path, {}, "--instance", "dev", "--secret", "ckan-api-token")
    assert result.returncode == 2 and "--catalogue" in result.stderr and calls == []


def test_the_model_key_from_a_file_replaces_every_copy_and_its_readers_restart(tmp_path):
    """The owner's new key is measured at the provider first; the old one stays until the owner deletes it."""
    state = {"copies": {"agent-runner": OLD}, "release": "", "acceptedAlso": [NEW],
             "workloads": {"agent-runner": [deployment("agent-proxy", "agent-runner-model-key")]}}
    result, calls, after = run(tmp_path, state, *model_key(tmp_path))
    assert result.returncode == 0, result.stdout + result.stderr
    assert after["copies"] == {"agent-runner": NEW}
    assert any("rollout restart deployment/agent-proxy" in c for c in calls)
    assert "delete it in the provider's account" in result.stdout
    assert not any(c.startswith("helmfile") for c in calls) and not any(" delete " in c for c in calls)


def test_a_model_key_the_provider_refuses_changes_nothing(tmp_path):
    state = {"copies": {"agent-runner": OLD}, "release": ""}
    result, calls, after = run(tmp_path, state, *model_key(tmp_path))
    assert result.returncode == 2 and "nothing changed" in result.stderr
    assert after["copies"] == {"agent-runner": OLD}
    assert not any("patch" in c or "rollout" in c for c in calls)


def test_a_model_key_file_with_the_key_in_use_changes_nothing(tmp_path):
    state = {"copies": {"agent-runner": OLD}, "release": ""}
    result, _, after = run(tmp_path, state, *model_key(tmp_path, OLD))
    assert result.returncode == 2 and "already in use" in result.stderr
    assert after["copies"] == {"agent-runner": OLD}


def test_from_file_is_refused_for_a_generated_secret(tmp_path):
    key_file = tmp_path / "k"
    key_file.write_text(NEW)
    result, calls, _ = run(tmp_path, {}, "--instance", "dev", "--secret", "db-portal", "--from-file", str(key_file))
    assert result.returncode == 2 and calls == []


DRY_RUNS = {
    "client": ({}, CLIENT),
    "database": ({"pushedBy": "generator"}, ("--instance", "dev", "--secret", "db-portal")),
    "forge": ({}, ("--instance", "dev", "--secret", "gitea-token-gateway", "--forge", "https://x/git")),
    "session key": ({}, ("--instance", "dev", "--secret", "portal-cookie-key")),
    "catalogue": ({"release": "", "saRelease": "ckan-catalogue"}, CATALOGUE),
    "model key": ({"release": ""}, None),
}


@pytest.mark.parametrize("kind", sorted(DRY_RUNS))
def test_a_dry_run_prints_the_plan_and_changes_nothing(tmp_path, kind):
    """T-5106: every recipe's dry run reads the cluster, names what it would do, and writes nothing."""
    state, args = DRY_RUNS[kind]
    args = args or model_key(tmp_path)
    state = {"workloads": {"portal": [deployment("reader", "x")]}, **state}
    result, calls, after = run(tmp_path, state, *args, "--dry-run")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "dry run: nothing changed" in result.stdout
    assert "would" in result.stdout
    assert not any(c.startswith(("helmfile", "curl")) for c in calls)
    assert not any(CHANGES.search(c) for c in calls)
    assert all(v == OLD for v in after["copies"].values()) and after["accepted"] == OLD
