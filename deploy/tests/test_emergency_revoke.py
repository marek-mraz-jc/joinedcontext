"""T-0048: scripts/emergency-revoke.sh must revoke and then MEASURE.

The script talks to a cluster and to Keycloak, so the suite runs it against stub
`kubectl` and `curl` on PATH inside a throwaway copy of `scripts/`, the way
test_smoke_script.py does. Every stub call is recorded, so a test can assert both what
the script did and what it never did (OPS-45, R48, PF-38).
"""

import base64
import json
import os
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = PROJECT_ROOT / "scripts" / "emergency-revoke.sh"

FORGE_TOKEN = "forge.write.token"
CHECKOUTS_ARGS = [
    "--org-dir", "/repo/current", "--projects-dir", "/projects",
    "--forge", "http://gitea-http.dev.svc.cluster.local:3000/joinedcontext",
    "--token-file", "/var/run/forge/token", "--interval", "10",
]

KUBECTL_STUB = '''#!/usr/bin/env python3
import base64, json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
args = sys.argv[1:]
with open(os.environ["STUB_CALLS"], "a") as log:
    log.write("kubectl " + " ".join(args) + "\\n")

if "get" in args and "secret" in args and "-A" in args:
    if not spec.get("listOk", True):
        sys.exit(1)
    sys.stdout.write("".join(ns + "\\n" for ns in spec.get("secretCopies", [])))
elif "patch" in args and "secret" in args:
    ns = args[args.index("-n") + 1]
    with open(os.environ["STUB_PATCHES"], "a") as patches:
        patches.write(ns + " " + sys.stdin.read().strip() + "\\n")
    sys.exit(1 if ns in spec.get("patchFails", []) else 0)
elif "get" in args and "deployments,statefulsets" in args:
    ns = args[args.index("-n") + 1]
    sys.stdout.write("".join(w + "\\n" for w in spec.get("workloads", {}).get(ns, [])))
elif "get" in args and "secret" in args:
    token = spec.get("forgeToken", "forge.write.token")
    sys.stdout.write(base64.b64encode(token.encode()).decode() if token else "")
elif "get" in args and "deployment" in args:
    sys.stdout.write("app.kubernetes.io/instance=context-gateway,app.kubernetes.io/name=context-gateway,")
elif "get" in args and "pods" in args:
    sys.stdout.write("".join(p + "\\n" for p in spec.get("pods", ["context-gateway-0"])))
elif "get" in args and "pod" in args:
    sys.stdout.write("".join(a + "\\n" for a in spec.get("checkoutsArgs", [])))
elif "exec" in args:
    sys.exit(0 if spec.get("execOk", True) else 1)
elif "rollout" in args and "restart" in args:
    sys.exit(0)
else:
    sys.exit(1)
'''

# The verify loop asks the same URL repeatedly; `refusedAfter` is how many calls the
# endpoint keeps answering 200 before the revocation reaches it.
CURL_STUB = '''#!/usr/bin/env python3
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
line = " ".join(sys.argv[1:])
with open(os.environ["STUB_CALLS"], "a") as log:
    log.write("curl " + line + "\\n")
# A secret must come on stdin (a -K - config or an @- field), never in argv.
config = sys.stdin.read() if "-K" in sys.argv or "@-" in line else ""
if config:
    with open(os.environ["STUB_CALLS"], "a") as log:
        log.write(("forge-config " if "/api/v1/repos/" in line else "stdin ") + config.strip() + "\\n")

if "/api/v1/repos/" in line:
    if "-X DELETE" in line:
        sys.stdout.write("{}\\n" + str(spec.get("deleteStatus", 200)))
    elif spec.get("policyAt", True):
        sys.stdout.write('{"name":"public-read.yaml","sha":"0123abcd","last_commit_sha":"ffff","type":"file"}\\n200')
    else:
        sys.stdout.write('{"message":"not found"}\\n404')
elif "/protocol/openid-connect/token" in line:
    sys.stdout.write('{"access_token":"admin.token.value"}')
elif "/admin/realms/" in line and "/users?" in line:
    sys.stdout.write(spec.get("userLookup", '[{"id":"user-uuid","username":"jana.kovacova"}]'))
elif "/admin/realms/" in line and "/clients?" in line:
    sys.stdout.write(spec.get("clientLookup", '[{"id":"client-uuid","clientId":"conformance"}]'))
elif "/client-secret" in line:
    sys.stdout.write(spec.get("rotatedAnswer", '{"type":"secret","value":"rotated.client.secret"}'))
    sys.stdout.write("\\n" + str(spec.get("adminWriteStatus", 200)))
elif "/logout" in line:
    sys.stdout.write(str(spec.get("adminWriteStatus", 204)))
else:
    calls = sum(1 for l in open(os.environ["STUB_CALLS"]) if l.startswith("curl ") and "/api/endpoint/" in l)
    still_open = spec.get("refusedAfter", 1)
    sys.stdout.write("200" if calls <= still_open - 1 else str(spec.get("refusedStatus", 403)))
'''


def run(tmp_path: Path, spec: dict, *args: str) -> tuple[subprocess.CompletedProcess, list[str]]:
    scripts = tmp_path / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)
    (scripts / "emergency-revoke.sh").write_text(SCRIPT.read_text())
    (scripts / "emergency-revoke.sh").chmod(0o755)

    stub_bin = tmp_path / "bin"
    stub_bin.mkdir(parents=True, exist_ok=True)
    for name, body in (("kubectl", KUBECTL_STUB), ("curl", CURL_STUB)):
        (stub_bin / name).write_text(body)
        (stub_bin / name).chmod(0o755)

    spec_file = tmp_path / "spec.json"
    spec_file.write_text(json.dumps(spec))
    calls_file = tmp_path / "calls.log"
    calls_file.write_text("")

    env = dict(os.environ)
    env["PATH"] = f"{stub_bin}:{env['PATH']}"
    env["STUB_SPEC"] = str(spec_file)
    env["STUB_CALLS"] = str(calls_file)
    env["STUB_PATCHES"] = str(tmp_path / "patches.log")
    env.setdefault("KEYCLOAK_ADMIN_TOKEN", "admin.token.value")

    result = subprocess.run(
        ["scripts/emergency-revoke.sh", *args],
        cwd=tmp_path, env=env, capture_output=True, text=True,
    )
    return result, calls_file.read_text().splitlines()


def token_file(tmp_path: Path) -> str:
    path = tmp_path / "compromised-token"
    path.write_text("compromised.jwt.value")
    return str(path)


def test_nothing_to_revoke_is_refused(tmp_path):
    result, calls = run(tmp_path, {}, "--instance", "dev")
    assert result.returncode == 2, result.stdout
    assert "nothing to revoke" in result.stderr
    assert calls == [], "a refused invocation must not touch the cluster"


VERIFY = ("--verify-url", "https://host/api/endpoint/slug/ngsi-ld/v1/entities?type=X")


def revoke_policy(tmp_path: Path, spec: dict, *extra: str):
    return run(
        tmp_path, spec,
        "--instance", "dev", "--namespace", "dev", "--policy", "public-read",
        "--project", "helsinki", "--space", "air", "--forge", "https://dev.example.test/git",
        *VERIFY, "--verify-token-file", token_file(tmp_path), *extra,
    )


def test_the_policy_leaves_the_forge_and_each_gateway_pod_fetches_it_now(tmp_path):
    # T-5650: the gateway reads the forge through its checkouts sidecar, never a ConfigMap; the
    # Policy goes with a commit on main and one --only pass per pod, and nothing restarts.
    result, calls = revoke_policy(tmp_path, {
        "pods": ["context-gateway-a", "context-gateway-b"], "checkoutsArgs": CHECKOUTS_ARGS,
    })
    assert result.returncode == 0, result.stdout + result.stderr
    contents = "https://dev.example.test/git/api/v1/repos/joinedcontext/helsinki/contents/spaces/air/policies/public-read.yaml"
    delete = next(c for c in calls if "-X DELETE" in c)
    assert contents in delete and '"sha":"0123abcd"' in delete and '"branch":"main"' in delete
    execs = [c for c in calls if " exec " in f" {c} "]
    assert len(execs) == 2
    assert execs[0].startswith("kubectl -n dev exec context-gateway-a -c checkouts -- /usr/local/bin/jcctl checkouts")
    assert all("--projects-dir /projects" in c and c.endswith("--once --only helsinki") for c in execs)
    assert not any("--interval" in c for c in execs), "the loop's own interval is not passed to one pass"
    assert calls.index(delete) < calls.index(execs[0]), "the pods fetch after the commit"
    assert not any("rollout" in c or "configmap" in c for c in calls), "no restart, no ConfigMap"


def test_the_forge_token_never_rides_on_a_command_line(tmp_path):
    result, calls = revoke_policy(tmp_path, {"checkoutsArgs": CHECKOUTS_ARGS})
    assert result.returncode == 0, result.stdout + result.stderr
    argv = [c for c in calls if not c.startswith("forge-config ")]
    assert not any(FORGE_TOKEN in c for c in argv)
    assert 'forge-config header = "Authorization: token forge.write.token"' in calls
    assert FORGE_TOKEN not in result.stdout + result.stderr


def test_a_policy_the_forge_does_not_carry_fails_loudly(tmp_path):
    result, calls = revoke_policy(tmp_path, {"policyAt": False, "checkoutsArgs": CHECKOUTS_ARGS})
    assert result.returncode == 1
    assert "is not at joinedcontext/helsinki:spaces/air/policies/public-read.yaml" in result.stderr
    assert not any("-X DELETE" in c or " exec " in c for c in calls), "nothing to remove, nothing fetched"


def test_a_refused_commit_fetches_nothing_and_fails(tmp_path):
    result, calls = revoke_policy(tmp_path, {"deleteStatus": 403, "checkoutsArgs": CHECKOUTS_ARGS})
    assert result.returncode == 1
    assert "the forge refused to remove" in result.stderr and "still grants" in result.stderr
    assert not any(" exec " in c for c in calls)


def test_no_forge_token_removes_nothing(tmp_path):
    result, calls = revoke_policy(tmp_path, {"forgeToken": ""})
    assert result.returncode == 1
    assert "no forge token in Secret gitea-token-portal of dev" in result.stderr
    assert not any("/api/v1/repos/" in c for c in calls)


def test_a_pod_that_cannot_fetch_fails_the_run(tmp_path):
    result, _ = revoke_policy(tmp_path, {"execOk": False, "checkoutsArgs": CHECKOUTS_ARGS})
    assert result.returncode == 1
    assert "context-gateway-0 could not fetch helsinki" in result.stderr


def test_a_policy_is_never_removed_without_measuring_the_refusal(tmp_path):
    result, calls = run(
        tmp_path, {},
        "--instance", "dev", "--policy", "public-read", "--project", "helsinki",
        "--space", "air", "--forge", "https://dev.example.test/git",
    )
    assert result.returncode == 2
    assert "--verify-url and --verify-token-file" in result.stderr
    assert calls == []


def test_a_path_that_walks_out_of_the_repository_is_refused(tmp_path):
    result, calls = revoke_policy(tmp_path, {}, "--forge-repo", "joinedcontext/../admin")
    assert result.returncode == 2
    assert calls == []


def test_a_user_is_logged_out_and_a_service_account_secret_is_rotated(tmp_path):
    result, calls = revoke_policy(
        tmp_path, {"checkoutsArgs": CHECKOUTS_ARGS},
        "--user", "jana.kovacova", "--service-account", "conformance",
        "--idm", "https://idm.example.test",
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert any("/admin/realms/dev/users/user-uuid/logout" in c for c in calls)
    assert any("/admin/realms/dev/clients/client-uuid/client-secret" in c for c in calls)


def test_the_refusal_is_measured_and_passes_inside_the_bound(tmp_path):
    result, calls = revoke_policy(tmp_path, {"refusedAfter": 1, "checkoutsArgs": CHECKOUTS_ARGS}, "--bound", "5")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "is refused (403)" in result.stdout
    assert any(c.startswith("curl ") and "/api/endpoint/" in c for c in calls)
    assert "stdin header = \"Authorization: Bearer compromised.jwt.value\"" in calls


def test_a_credential_still_accepted_after_the_bound_fails_the_run(tmp_path):
    result, calls = revoke_policy(tmp_path, {"refusedAfter": 99, "checkoutsArgs": CHECKOUTS_ARGS}, "--bound", "2")
    assert result.returncode == 1
    assert "still answers 200" in result.stderr
    assert "OPS-45" in result.stderr
    assert not any("rollout" in c for c in calls), "the bound is measured at the gateway, not by a restart"


def test_no_token_or_password_is_ever_printed(tmp_path):
    env_secret = "sup3r-admin-password"
    os.environ["KEYCLOAK_ADMIN_PASSWORD"] = env_secret
    try:
        result, _ = revoke_policy(
            tmp_path, {"refusedAfter": 1, "checkoutsArgs": CHECKOUTS_ARGS},
            "--user", "jana.kovacova", "--idm", "https://idm.example.test",
        )
    finally:
        del os.environ["KEYCLOAK_ADMIN_PASSWORD"]
    printed = result.stdout + result.stderr
    for secret in (env_secret, "admin.token.value", "compromised.jwt.value", FORGE_TOKEN):
        assert secret not in printed, f"{secret} reached the console"



def test_no_password_or_token_rides_on_a_command_line(tmp_path):
    # F-R2DEP3-004: the admin password, the admin bearer and the compromised token went in
    # curl argv, readable in /proc by any process on the operator's host.
    os.environ["KEYCLOAK_ADMIN_TOKEN"] = ""
    os.environ["KEYCLOAK_ADMIN_USER"] = "admin"
    os.environ["KEYCLOAK_ADMIN_PASSWORD"] = "sup3r-admin-password"
    try:
        result, calls = revoke_policy(
            tmp_path, {"refusedAfter": 1, "checkoutsArgs": CHECKOUTS_ARGS},
            "--user", "jana.kovacova", "--service-account", "conformance",
            "--idm", "https://idm.example.test",
        )
    finally:
        for key in ("KEYCLOAK_ADMIN_TOKEN", "KEYCLOAK_ADMIN_USER", "KEYCLOAK_ADMIN_PASSWORD"):
            del os.environ[key]
    assert result.returncode == 0, result.stdout + result.stderr
    argv = [c for c in calls if c.startswith(("curl ", "kubectl "))]
    for secret in ("sup3r-admin-password", "admin.token.value", "compromised.jwt.value"):
        assert not any(secret in c for c in argv), f"{secret} rode on a command line"
    assert any(c.startswith("stdin ") and "admin.token.value" in c for c in calls)
    assert any(c.startswith("stdin ") and "compromised.jwt.value" in c for c in calls)
    assert "stdin sup3r-admin-password" in calls, "the password goes on stdin, without a newline"
    assert any("/admin/realms/dev/users/user-uuid/logout" in c for c in calls)


def test_the_client_secret_rotated_is_the_clients_own_not_a_mappers(tmp_path):
    # F-R2DEP3-001: Keycloak answers a client on one line with its protocolMappers after it; a
    # greedy sed took the last "id", a mapper's, and the rotation answered 404.
    lookup = ('[{"id":"client-uuid","clientId":"conformance","protocolMappers":'
              '[{"id":"mapper-uuid","name":"audience"}]}]')
    users = ('[{"id":"user-uuid","username":"jana.kovacova",'
             '"federatedIdentities":[{"id":"idp-link-uuid"}]}]')
    result, calls = run(
        tmp_path, {"clientLookup": lookup, "userLookup": users},
        "--instance", "dev", "--user", "jana.kovacova",
        "--service-account", "conformance", "--idm", "https://idm.example.test",
    )
    assert any("/admin/realms/dev/clients/client-uuid/client-secret" in c for c in calls), calls
    assert any("/admin/realms/dev/users/user-uuid/logout" in c for c in calls), calls
    assert not any("mapper-uuid" in c or "idp-link-uuid" in c for c in calls)


# T-2841: a platform client's secret lives in the generated Secret `keycloak-client-<client>`,
# which keycloak-config-cli imports on every apply. Rotated in Keycloak alone, the next apply
# wrote the compromised secret back.
EDGE_READER = 'Deployment/apisix {"containers":[{"env":[{"name":"OIDC_SECRET","valueFrom":{"secretKeyRef":{"key":"client-secret","name":"keycloak-client-edge"}}}]}]}'
PROXY_READER = 'Deployment/bikes-agent-proxy {"volumes":[{"name":"s","secret":{"secretName":"keycloak-client-edge-agent-proxy"}}]}'


def revoke_client(tmp_path: Path, spec: dict, client: str = "edge"):
    return run(
        tmp_path, spec,
        "--instance", "dev",
        "--service-account", client, "--idm", "https://idm.example.test",
    )


def patches(tmp_path: Path) -> list[str]:
    path = tmp_path / "patches.log"
    return path.read_text().splitlines() if path.exists() else []


def test_a_platform_clients_new_secret_reaches_every_copy_and_its_readers_restart(tmp_path):
    result, calls = revoke_client(tmp_path, {
        "secretCopies": ["keycloak", "apisix"],
        "workloads": {"apisix": [EDGE_READER, PROXY_READER]},
    })
    assert result.returncode == 0, result.stdout + result.stderr
    written = base64.b64encode(b"rotated.client.secret").decode()
    assert patches(tmp_path) == [
        f'keycloak {{"data":{{"client-secret":"{written}"}}}}',
        f'apisix {{"data":{{"client-secret":"{written}"}}}}',
    ], "Keycloak and every copy of the Secret must hold the same new secret"
    rotated = next(i for i, c in enumerate(calls) if "/client-secret" in c)
    first_patch = next(i for i, c in enumerate(calls) if "patch secret keycloak-client-edge" in c)
    assert rotated < first_patch
    assert any("-n apisix rollout restart deployment/apisix" in c for c in calls)
    assert not any("bikes-agent-proxy" in c and "restart" in c for c in calls), "only the readers of this Secret restart"
    printed = result.stdout + result.stderr
    assert "rotated.client.secret" not in printed and written not in printed
    assert not any("rotated.client.secret" in c or written in c for c in calls), "the secret never rides on a command line"


def test_a_client_the_portal_created_is_rotated_in_keycloak_only(tmp_path):
    result, calls = revoke_client(tmp_path, {"secretCopies": []}, client="bikes-sensors")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "rotated in Keycloak only" in result.stdout
    assert patches(tmp_path) == []
    assert any("/client-secret" in c for c in calls)


def test_a_copy_left_with_the_revoked_secret_fails_the_run(tmp_path):
    result, _ = revoke_client(tmp_path, {
        "secretCopies": ["keycloak", "apisix"],
        "patchFails": ["apisix"],
    })
    assert result.returncode == 1
    assert "Secret keycloak-client-edge in apisix still holds the revoked secret" in result.stderr


def test_no_new_secret_in_keycloaks_answer_leaves_the_secret_alone_and_fails(tmp_path):
    result, _ = revoke_client(tmp_path, {"secretCopies": ["keycloak"], "rotatedAnswer": "{}"})
    assert result.returncode == 1
    assert "did not answer the new secret" in result.stderr
    assert patches(tmp_path) == []


def test_copies_that_cannot_be_listed_fail_the_run(tmp_path):
    result, _ = revoke_client(tmp_path, {"listOk": False})
    assert result.returncode == 1
    assert "cannot list the copies of Secret keycloak-client-edge" in result.stderr
