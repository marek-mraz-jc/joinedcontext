#!/usr/bin/env python3
"""Validate the dev cluster against main and the rules it has to hold (T-2799, OPS-35, SEC-01).

    scripts/validate-dev.py --out <summary.json> [--skip drift,images,expiry,kyverno,probes]

The hourly batch runs it after `just dev-apply` (`just dev-validate`) and hands the summary to
`tasks/file-failures`, which files one task per new failure. It reads the cluster and changes
nothing but the probe pod it starts and deletes:

- drift: `helmfile diff --three-way-merge` of the assembled environment is empty, one result
  per release; `--env smartcity` (production) runs this check alone (`just drift <env>`);
- images: every container of a platform pod is pinned by digest, and every image of ours
  passes `cosign verify` for the identity the image workflows sign with (keyless);
- probes: from the internet, the node's internal ports are closed and 443 answers; from a
  pod that carries an App's NetworkPolicy selector, Postgres and another App are refused
  and DNS answers;
- expiry: every TLS certificate in the platform namespaces runs longer than 21 days, and a
  Postgres cluster with a backup configured had a successful one in the last 26 hours;
- kyverno: every policy is ready and enforces, and no policy report of a platform namespace
  holds a failure.

Exit 0 when nothing failed, 1 otherwise; the summary is written either way. A check that
cannot run is an `error` result, never a pass. No secret reaches the summary: only the
certificate half of a TLS secret is read, and helmfile diff runs with --suppress-secrets.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import shutil
import socket
import subprocess
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parent.parent
CHECKS = ("drift", "images", "expiry", "kyverno", "probes")
# smartcity is the production environment (`just prod-apply`).
ENVS = ("dev", "smartcity")
PLATFORM_NS = re.compile(r"^(dev|dev-[a-z0-9-]+|jc-operators)$")
TLS_NS = re.compile(r"^(dev|dev-[a-z0-9-]+|jc-operators|linkerd)$")
CERT_DAYS = 21
BACKUP_HOURS = 26
# The images our workflows sign (.github/workflows/reusable-container-sign.yml).
SIGNED_PREFIXES = ("ghcr.io/marek-mraz-jc/", "ghcr.io/marek-mraz/")
SIGNER_ISSUER = "https://token.actions.githubusercontent.com"
SIGNER_IDENTITY = r"^https://github\.com/marek-mraz(-jc)?/"
# Ports of the node that answer inside the cluster only: Postgres, APISIX data and admin, the
# Linkerd inbound proxy, the kubelet, etcd, and the usual app port.
CLOSED_PORTS = (5432, 9080, 9180, 4143, 10250, 2379, 2380, 8080)
OPEN_PORT = 443
PROBE_IMAGE = "docker.io/busybox:1.38@sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616"
DRIFT = {"changed": "differs from main", "been added": "is in main but not on {env}",
         "been removed": "is on {env} but not in main"}
COMPARING = re.compile(r"^Comparing release=([^,\s]+)")
DIFF_HEADER = re.compile(r"^(\S+), (\S+), (\S+) \(([^)]*)\) has (changed|been added|been removed):")


def result(key: str, verdict: str, title: str, detail: str = "", evidence: str = "") -> dict:
    return {"key": key, "verdict": verdict, "title": title, "detail": detail, "evidence": evidence}


def short(image: str) -> str:
    """An image reference with its digest cut to 12 hex, so the board's redaction keeps it."""
    return re.sub(r"@sha256:([0-9a-f]{12})[0-9a-f]{52}", r"@sha256:\1…", image)


def repository(image: str) -> str:
    """`ghcr.io/o/x:1@sha256:…` → `ghcr.io/o/x`: the key of an image across its versions."""
    name = image.split("@", 1)[0]
    head, _, tail = name.rpartition("/")
    return f"{head}/{tail.split(':', 1)[0]}" if head else tail.split(":", 1)[0]


# --- drift ------------------------------------------------------------------------------------


def drift_results(output: str, code: int, env: str = "dev") -> list[dict]:
    """`helmfile diff --detailed-exitcode`: 0 is no change, 2 is a change, anything else broke.

    One result per release helmfile compared (T-4956): tasks/file-failures files one task per
    drifting release and closes it when that release compares clean again.
    """
    releases: dict[str, set[str]] = {}
    current = ""
    for line in output.splitlines():
        if m := COMPARING.match(line):
            current = m[1]
            releases.setdefault(current, set())
        elif (m := DIFF_HEADER.match(line)) and current:
            releases[current].add(f"{m[3]} {m[1]}/{m[2]} {DRIFT[m[5]].format(env=env)}")
    drifting = {name: objects for name, objects in releases.items() if objects}
    if code not in (0, 2) or (code == 2 and not drifting):
        tail = " ".join(output.strip().splitlines()[-3:])
        return [result(f"drift/{env}", "error", f"helmfile diff of {env} did not run", f"exit {code}: {tail}")]
    if not releases:
        return [result(f"drift/{env}", "pass", f"{env} matches main")]
    return [
        result(
            f"drift/{env}/{name}",
            "fail" if objects else "pass",
            f"drift on {env}: release {name} differs from main ({len(objects)} objects)" if objects
            else f"release {name} on {env} matches main",
            "helmfile diff --three-way-merge against main is not empty: a hand change on the cluster, "
            f"or a merge not applied. `just {'dev-apply' if env == 'dev' else 'prod-apply'}` restores main; "
            "a change that belongs goes into Git. Objects: " + "; ".join(sorted(objects)) if objects else "",
        )
        for name, objects in sorted(releases.items())
    ]


# --- images -----------------------------------------------------------------------------------


def pod_images(pods: dict) -> dict[str, list[str]]:
    """image → the platform pods (ns/name) that run it, init containers included."""
    found: dict[str, list[str]] = {}
    for pod in pods.get("items", []):
        ns = pod["metadata"]["namespace"]
        if not PLATFORM_NS.match(ns):
            continue
        spec = pod.get("spec", {})
        for container in spec.get("initContainers", []) + spec.get("containers", []):
            found.setdefault(container["image"], []).append(f"{ns}/{pod['metadata']['name']}")
    return found


def image_results(images: dict[str, list[str]], verify) -> list[dict]:
    """Unpinned images fail per repository; pinned images of ours go through `verify`."""
    if not images:
        return [result("image/pods", "error", "no platform pod was found on dev, so no image was checked")]
    unpinned: dict[str, list[str]] = {}
    unsigned: dict[str, list[str]] = {}
    for image, pods in sorted(images.items()):
        if "@sha256:" not in image:
            unpinned.setdefault(repository(image), []).extend(f"{p} ({image})" for p in pods)
            continue
        if not image.startswith(SIGNED_PREFIXES):
            continue
        ok, why = verify(image)
        if ok is None:
            return [result("image/cosign", "error", "cosign is not available, so no signature was verified", why)]
        # Two digests of one repository are one result; one unsigned digest fails it.
        found = unsigned.setdefault(repository(image), [])
        if not ok:
            found.append(f"{short(image)} runs in {', '.join(pods[:5])}: {why}")
    results = [
        result(
            f"image/signature/{repo}",
            "fail" if problems else "pass",
            f"{repo} on dev is {'not signed by our image workflow' if problems else 'signed'}",
            "; ".join(problems),
        )
        for repo, problems in sorted(unsigned.items())
    ]
    for repo, where in sorted(unpinned.items()):
        results.append(result(
            f"image/unpinned/{repo}",
            "fail",
            f"{repo} runs on dev by tag, not by digest",
            f"{len(where)} container(s): {', '.join(where[:5])}. Pin the image by digest where it is rendered or injected.",
        ))
    return results


def cosign_verify(image: str) -> tuple[bool | None, str]:
    if shutil.which("cosign") is None:
        return None, "install cosign on the host that runs the hourly batch"
    run = subprocess.run(
        ["cosign", "verify", "--certificate-oidc-issuer", SIGNER_ISSUER,
         "--certificate-identity-regexp", SIGNER_IDENTITY, image],
        capture_output=True, text=True, timeout=120,
    )
    lines = run.stderr.strip().splitlines()
    return run.returncode == 0, lines[-1] if lines else f"exit {run.returncode}"


# --- expiry -----------------------------------------------------------------------------------


def not_after(openssl_output: str) -> datetime:
    """`notAfter=Dec  4 10:00:00 2026 GMT` → an aware datetime."""
    value = openssl_output.strip().removeprefix("notAfter=")
    return datetime.strptime(value, "%b %d %H:%M:%S %Y GMT").replace(tzinfo=UTC)


def cert_results(certs: list[tuple[str, str, datetime | None]], now: datetime) -> list[dict]:
    results = []
    for ns, name, end in certs:
        key = f"expiry/certificate/{ns}/{name}"
        if end is None:
            results.append(result(key, "error", f"certificate {ns}/{name} could not be read"))
            continue
        days = (end - now) / timedelta(days=1)
        ok = days > CERT_DAYS
        results.append(result(
            key, "pass" if ok else "fail",
            f"certificate {ns}/{name} expires in {days:.0f} days",
            "" if ok else f"it expires {end:%Y-%m-%d %H:%M} UTC, inside the {CERT_DAYS}-day floor: "
                          "cert-manager should have renewed it (kubectl describe certificate).",
        ))
    return results


def backup_results(clusters: dict, now: datetime) -> list[dict]:
    results = []
    for cluster in clusters.get("items", []):
        ns, name = cluster["metadata"]["namespace"], cluster["metadata"]["name"]
        if not PLATFORM_NS.match(ns):
            continue
        key = f"expiry/backup/{ns}/{name}"
        if not cluster.get("spec", {}).get("backup"):
            results.append(result(key, "skip", f"Postgres {ns}/{name} has no backup configured (T-0046)"))
            continue
        last = cluster.get("status", {}).get("lastSuccessfulBackup")
        age = (now - datetime.fromisoformat(last.replace("Z", "+00:00"))) / timedelta(hours=1) if last else None
        ok = age is not None and age <= BACKUP_HOURS
        results.append(result(
            key, "pass" if ok else "fail",
            f"Postgres {ns}/{name}: " + (f"last backup {age:.0f} h ago" if age is not None else "no successful backup"),
            "" if ok else f"a backup is due every {BACKUP_HOURS} h: kubectl get backups -n {ns}",
        ))
    return results


# --- kyverno ----------------------------------------------------------------------------------


def policy_results(policies: list[dict]) -> list[dict]:
    """Every validate rule enforces, and every policy is ready."""
    results = []
    for policy in policies:
        meta, spec = policy["metadata"], policy.get("spec", {})
        name = f"{meta['namespace']}/{meta['name']}" if meta.get("namespace") else meta["name"]
        audit = [
            rule["name"] for rule in spec.get("rules", []) if "validate" in rule
            and (rule["validate"].get("failureAction") or spec.get("validationFailureAction") or "Audit").lower() != "enforce"
        ]
        ready = any(c.get("type") == "Ready" and c.get("status") == "True"
                    for c in policy.get("status", {}).get("conditions", []))
        problems = ([f"rules in Audit: {', '.join(audit)}"] if audit else []) + ([] if ready else ["not Ready"])
        results.append(result(
            f"kyverno/policy/{name}", "fail" if problems else "pass",
            f"Kyverno policy {name} " + ("; ".join(problems) if problems else "enforces"),
            "global.runtimePolicies.failureAction is Enforce on dev; a rule in Audit lets the "
            "violation through (kubectl describe clusterpolicy)." if problems else "",
        ))
    return results


def report_results(reports: list[dict]) -> list[dict]:
    """One result per failing policy rule, naming the platform resources it failed on."""
    failed: dict[tuple[str, str], list[str]] = {}
    for report in reports:
        ns = report["metadata"].get("namespace", "")
        if not PLATFORM_NS.match(ns):
            continue
        for item in report.get("results", []):
            if item.get("result") not in ("fail", "error"):
                continue
            where = [f"{ns}/{r.get('kind')}/{r.get('name')}" for r in item.get("resources", [])]
            if not where and report.get("scope"):
                where = [f"{ns}/{report['scope'].get('kind')}/{report['scope'].get('name')}"]
            failed.setdefault((item.get("policy", "?"), item.get("rule", "?")), []).extend(where or [ns])
    if not failed:
        return [result("kyverno/reports", "pass", "no Kyverno violation in the platform namespaces")]
    return [
        result(
            f"kyverno/violation/{policy}/{rule}", "fail",
            f"Kyverno {policy}/{rule} fails on {len(where)} platform resource(s)",
            ", ".join(sorted(set(where))[:10]),
        )
        for (policy, rule), where in sorted(failed.items())
    ]


# --- probes -----------------------------------------------------------------------------------


def edge_results(host: str, connect) -> list[dict]:
    """From outside the cluster: internal ports closed, 443 open (else nothing was measured)."""
    if not connect(host, OPEN_PORT):
        return [result("probe/internet/443", "error", f"{host}:443 did not answer, so the closed ports prove nothing")]
    reached = [port for port in CLOSED_PORTS if connect(host, port)]
    return [result(
        "probe/internet/internal-ports", "fail" if reached else "pass",
        f"the node answers from the internet on {', '.join(map(str, reached))}" if reached
        else "the node's internal ports are closed to the internet",
        f"{host}: {', '.join(map(str, reached))} must be closed by the node firewall" if reached else "",
    )]


def tcp_open(host: str, port: int) -> bool:
    try:
        with socket.create_connection((host, port), timeout=5):
            return True
    except OSError:
        return False


def probe_script(targets: dict[str, tuple[str, int]], settle: tuple[str, int]) -> str:
    """The probe pod's shell: wait for the policy to hold (T-0459), then try every target."""
    ip, port = settle
    lines = [
        f"i=0; while [ $i -lt 18 ] && nc -w 3 -z {ip} {port} >/dev/null 2>&1; do i=$((i+1)); sleep 4; done",
        "echo SETTLED-AFTER=$((i*4))s",
        "timeout 8 nslookup kubernetes.default.svc.cluster.local >/dev/null 2>&1 && echo REACHED-dns",
    ]
    lines += [f"nc -w 5 -z {ip} {port} >/dev/null 2>&1 && echo REACHED-{label}" for label, (ip, port) in targets.items()]
    lines.append("echo PROBE-RAN")
    return "; ".join(lines)


def probe_results(output: str, denied: list[str]) -> list[dict]:
    """DNS must answer (the positive control); every label in `denied` must not be reached."""
    lines = {line.strip() for line in output.splitlines()}
    if "PROBE-RAN" not in lines:
        return [result("probe/app", "error", "the App probe pod never ran, so no pair was measured", output[-300:])]
    if "REACHED-dns" not in lines:
        return [result("probe/app/dns", "fail", "a pod under an App's NetworkPolicy cannot resolve names, so its refusals prove nothing")]
    return [
        result(
            f"probe/app/{label}", "fail" if f"REACHED-{label}" in lines else "pass",
            f"a pod under an App's NetworkPolicy {'reached' if f'REACHED-{label}' in lines else 'is refused'} {label}",
        )
        for label in denied
    ]


def app_probe(kubectl) -> list[dict]:
    pods = json.loads(kubectl("get", "pods", "-A", "-l", "joinedcontext.com/app=true",
                              "--field-selector=status.phase=Running", "-o", "json"))["items"]
    pods = [p for p in pods if PLATFORM_NS.match(p["metadata"]["namespace"]) and p["status"].get("podIP")]
    if not pods:
        return [result("probe/app", "skip", "no App pod runs on dev, so no App NetworkPolicy was probed")]
    app = pods[0]
    ns, labels = app["metadata"]["namespace"], app["metadata"].get("labels", {})
    policies = json.loads(kubectl("get", "networkpolicies", "-n", ns, "-o", "json"))["items"]
    selector = next((p["spec"]["podSelector"]["matchLabels"] for p in policies
                     if p["spec"].get("podSelector", {}).get("matchLabels")
                     and p["spec"]["podSelector"]["matchLabels"].items() <= labels.items()), None)
    if selector is None:
        return [result("probe/app/policy", "fail", f"App pod {ns}/{app['metadata']['name']} has no NetworkPolicy of its own")]
    primaries = json.loads(kubectl("get", "pods", "-A", "-l", "cnpg.io/instanceRole=primary", "-o", "json"))["items"]
    postgres = next((p["status"]["podIP"] for p in primaries if p["status"].get("podIP")), None)
    if postgres is None:
        return [result("probe/app", "error", "no Postgres primary pod on dev, so the App probe has no target")]
    other = next((p for p in pods if p["metadata"].get("labels", {}).get("app.kubernetes.io/name")
                  != labels.get("app.kubernetes.io/name")), app)["status"]["podIP"]
    targets = {"postgres": (postgres, 5432), "other-app": (other, 8080), "other-app-mesh": (other, 4143)}
    name = f"validate-app-probe-{datetime.now(UTC):%H%M%S}"
    overrides = {
        "metadata": {
            "labels": selector,
            "annotations": {
                "linkerd.io/inject": "disabled",
                "mesh.joinedcontext.com/opt-out-reason": "validation probe: an unmeshed pod is the point of the NetworkPolicy check",
            },
        },
        "spec": {
            "securityContext": {"runAsNonRoot": True, "runAsUser": 65534, "seccompProfile": {"type": "RuntimeDefault"}},
            "automountServiceAccountToken": False,
            "containers": [{
                "name": name, "image": PROBE_IMAGE,
                "command": ["sh", "-c", probe_script(targets, targets["postgres"])],
                # Never Ready, so the App's Service never sends a person's request to the probe.
                "readinessProbe": {"exec": {"command": ["false"]}, "periodSeconds": 60},
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True,
                                    "capabilities": {"drop": ["ALL"]}},
                "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "64Mi"}},
            }],
        },
    }
    try:
        output = kubectl("run", name, "-n", ns, "--rm", "--attach", "--restart=Never", "--quiet",
                         "--timeout=180s", f"--image={PROBE_IMAGE}", f"--overrides={json.dumps(overrides)}",
                         check=False)
    finally:
        kubectl("delete", "pod", name, "-n", ns, "--ignore-not-found", "--wait=false", check=False)
    return probe_results(output, list(targets))


# --- the run ----------------------------------------------------------------------------------


def kubectl(*args: str, check: bool = True) -> str:
    run = subprocess.run(["kubectl", *args], capture_output=True, text=True, timeout=300)
    if check and run.returncode != 0:
        raise RuntimeError(f"kubectl {' '.join(args[:3])}: {run.stderr.strip()[-300:]}")
    return run.stdout


def tls_certs() -> list[tuple[str, str, datetime | None]]:
    # Only the certificate half is read; the key never leaves the cluster.
    rows = kubectl("get", "secrets", "-A", "--field-selector", "type=kubernetes.io/tls", "-o",
                   r"jsonpath={range .items[*]}{.metadata.namespace}{'\t'}{.metadata.name}{'\t'}{.data.tls\.crt}{'\n'}{end}")
    certs = []
    for row in rows.splitlines():
        ns, name, crt = (row.split("\t") + ["", ""])[:3]
        if not TLS_NS.match(ns):
            continue
        end = None
        if crt:
            run = subprocess.run(["openssl", "x509", "-noout", "-enddate"], input=base64.b64decode(crt),
                                 capture_output=True, timeout=30)
            if run.returncode == 0:
                end = not_after(run.stdout.decode())
        certs.append((ns, name, end))
    return certs


def guarded(name: str, check) -> list[dict]:
    try:
        return check()
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        return [result(f"{name}/run", "error", f"the {name} check did not run", str(error)[-300:])]


def run_checks(skip: set[str], env: str = "dev") -> list[dict]:
    now = datetime.now(UTC)
    checks = {
        "drift": lambda: drift_results(*helmfile_diff(env), env),
        "images": lambda: image_results(pod_images(json.loads(kubectl("get", "pods", "-A", "-o", "json"))), cosign_verify),
        "expiry": lambda: cert_results(tls_certs(), now) + backup_results(
            json.loads(kubectl("get", "clusters.postgresql.cnpg.io", "-A", "-o", "json")), now),
        "kyverno": lambda: policy_results(
            json.loads(kubectl("get", "clusterpolicies", "-o", "json"))["items"]
            + json.loads(kubectl("get", "policies", "-A", "-o", "json"))["items"]
        ) + report_results(json.loads(kubectl("get", "policyreports", "-A", "-o", "json"))["items"]),
        "probes": lambda: edge_results(node_host(), tcp_open) + app_probe(kubectl),
    }
    return [r for name, check in checks.items() if name not in skip for r in guarded(name, check)]


def helmfile_diff(env: str) -> tuple[str, int]:
    # --three-way-merge compares with the live objects, not only with Helm's stored manifest:
    # without it a `kubectl edit` on the cluster never shows up as drift (T-4956).
    # kube-system is left out (T-5702): the drift identity reads Secrets only in the platform's
    # release namespaces, and kube-system's hold the cluster's own (node passwords, bootstrap
    # tokens). Its one release, node-storage-local-path, is the local-path provisioner's config.
    # cert-manager likewise: it holds the ACME account keys and the DNS-01 provider token, and its
    # one release, apisix-dns01, is installed: false on dev and smartcity, which helmfile diff
    # still looks up (helm list reads that namespace's release Secrets) to report a deletion.
    run = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", env, "--selector", "namespace!=kube-system,namespace!=cert-manager", "diff",
         "--detailed-exitcode", "--suppress-secrets", "--context", "0",
         "--diff-args", "--three-way-merge"],
        cwd=ROOT, capture_output=True, text=True, timeout=900, check=False,
    )
    return run.stdout + run.stderr, run.returncode


def node_host() -> str:
    server = kubectl("config", "view", "--minify", "-o", "jsonpath={.clusters[0].cluster.server}")
    host = urlparse(server.strip()).hostname
    if not host:
        raise ValueError("the kube context names no API server")
    return host


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path, help="where the summary JSON goes")
    parser.add_argument("--skip", default="", help=f"comma-separated checks to leave out: {','.join(CHECKS)}")
    parser.add_argument("--env", default="dev", choices=ENVS,
                        help="the environment to read; any but dev runs the drift check alone")
    args = parser.parse_args(argv)
    skip = {s for s in args.skip.split(",") if s}
    if unknown := skip - set(CHECKS):
        parser.error(f"unknown check(s): {', '.join(sorted(unknown))}")
    if args.env != "dev":
        # The namespace patterns of the other checks are dev's; prod is read for drift only.
        skip |= set(CHECKS) - {"drift"}
    results = run_checks(skip, args.env)
    summary = {
        "check": "deployment",
        "repo": "joinedcontext-deployment",
        "run": datetime.now(UTC).strftime("%Y-%m-%dT%H:%MZ"),
        "requirements": ["OPS-35", "SEC-01"],
        "results": results,
    }
    args.out.write_text(json.dumps(summary, indent=2) + "\n")
    bad = [r for r in results if r["verdict"] in ("fail", "error")]
    for r in results:
        print(f"{r['verdict']:5} {r['key']}: {r['title']}")
    print(f"{len(results)} results, {len(bad)} failing; summary in {args.out}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
