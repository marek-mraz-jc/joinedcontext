"""T-0231: scripts/smoke.sh must assert exact statuses and fail on anything else.

The script talks to a cluster and to the outside world, so the suite runs it against
stub `curl`/`kubectl`/`wait-rollouts.sh` on PATH inside a throwaway copy of `scripts/`.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SMOKE = PROJECT_ROOT / "scripts" / "smoke.sh"

LIVING = json.dumps([
    {"id": "urn:ngsi-ld:Vehicle:hel.fi:helsinki:22-7", "type": "Vehicle", "expiresAt": "2026-10-20T10:05:00.000Z",
     "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.9, 60.1]},
                  "observedAt": "2026-10-20T10:00:00.000Z"}},
    {"id": "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID1", "type": "Alert", "expiresAt": "2026-10-20T11:00:00Z",
     "alertSource": {"type": "Property", "value": "Fintraffic"},
     "validTo": {"type": "Property", "value": "2026-10-20T11:00:00Z"}},
    {"id": "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-1", "type": "Alert",
     "alertSource": {"type": "Property", "value": "HSL"}},
])

CURL_STUB = '''#!/usr/bin/env -S python3 -S
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
args = sys.argv[1:]
# The method, the headers and the request body all separate two calls to one URL
# (anonymous vs bearer, create vs delete), so the whole command line is the key.
line = " ".join(args)
if os.environ.get("STUB_ARGV"):
    with open(os.environ["STUB_ARGV"], "a") as log:
        log.write(line + "\\n")
# A form field curl reads from a file (`name@/dev/fd/N`, the script's pipes) is part of the request
# as curl sends it, so the body matchers see it as `name=<value>`.
for arg in args:
    name, at, path = arg.partition("@")
    if at and path.startswith("/dev/fd/"):
        line += " %s=%s" % (name, open(path).read())

DEFAULT_HEADERS = (
    "HTTP/2 200\\r\\nserver: APISIX\\r\\n"
    "strict-transport-security: max-age=31536000; includeSubDomains; preload\\r\\n"
    "x-content-type-options: nosniff\\r\\nx-frame-options: SAMEORIGIN\\r\\n"
    "referrer-policy: no-referrer\\r\\n"
)


def lookup(table, default):
    for match, value in spec.get(table, []):
        needles = match if isinstance(match, list) else [match]
        if all(n in line for n in needles):
            return value
    return default


# The write-out that asks for the status (`%{http_code}`, alone or with `%header{...}` beside it).
write_out = next((a for a in args if "%{http_code}" in a), None)


def answer(code):
    # A write-out naming the edge's rate-limit reset gets one (T-4188); its newline is curl's.
    said = str(code)
    if "%header{x-ratelimit-reset}" in write_out:
        said += " " + str(spec.get("rateLimitReset", ""))
    # A write-out naming the redirect target gets the one its URL is moved to (T-4630).
    if "%{redirect_url}" in write_out:
        said += " " + lookup("locations", "")
    return said + ("\\n" if "\\\\n" in write_out else "")


if "--tls-max" in line:
    sys.exit(spec.get("tls11", 1))
elif "-sSI" in args:
    sys.stdout.write(lookup("headers", spec.get("defaultHeaders", DEFAULT_HEADERS)))
elif write_out is not None and any(all(n in line for n in (m if isinstance(m, list) else [m])) for m, _ in spec.get("statusSequences", [])):
    # A status that changes while the script waits (T-3424): each call takes the next answer of
    # the sequence, and the last one stands once it runs out.
    for index, (match, answers) in enumerate(spec["statusSequences"]):
        needles = match if isinstance(match, list) else [match]
        if all(n in line for n in needles):
            counter = os.environ["STUB_SPEC"] + ".seq%d" % index
            taken = int(open(counter).read()) if os.path.exists(counter) else 0
            open(counter, "w").write(str(taken + 1))
            sys.stdout.write(answer(answers[min(taken, len(answers) - 1)]))
            break
elif write_out is not None:
    # A conditional read of a schema artifact is a 304, as the gateway answers it (EP-51).
    sys.stdout.write(answer(lookup("statuses", 304 if "If-None-Match" in line else 200)))
elif "-D" in args:
    # The response headers of one read (T-2262): the gateway's own caching and its ETag.
    sys.stdout.write(lookup("dumpedHeaders", spec.get("schemaHeaders",
        'HTTP/2 200\\r\\ncache-control: private, no-cache\\r\\netag: "e3b0c442"\\r\\n')))
elif "/q/health" in line:
    # The broker's own health over the port-forward (T-3905): the surfaces the deployment mounts.
    surfaces = spec.get("brokerSurfaces", ["views", "mcp", "access", "ws", "ogc", "ical"])
    sys.stdout.write(lookup("bodies", json.dumps({"status": "UP", "surfaces": {s: "UP" for s in surfaces}})))
else:
    sys.stdout.write(lookup("bodies", ""))
'''

KUBECTL_STUB = '''#!/usr/bin/env -S python3 -S
import base64, itertools, json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
args = sys.argv[1:]
line = " ".join(args)
# The pod selector of APISIX's egress policy (components/apisix/networkpolicies.yaml).
EDGE_SELECTOR = [("app.kubernetes.io/name", "apisix"), ("app.kubernetes.io/instance", "apisix-apisix")]
# dev's brokers (T-4857): sensors on 8883 by SNI, vehicles on its dedicated port alone (T-5871),
# and a certificate door beside sensors (T-5875), which the check does not probe or count.
MQTT_BROKERS = ["mqtt-helsinki-sensors 1", "mqtt-helsinki-vehicles 1", "mqtt-helsinki-sensors-cert 1"]
MQTT_ROUTES = ["mqtt-helsinki-sensors mqtts HostSNI(`sensors.helsinki.mqtt.example.test`)",
               "mqtt-helsinki-vehicles-port mqtt-18830 HostSNI(`*`)",
               "mqtt-helsinki-sensors-cert mqtts HostSNI(`cert.sensors.helsinki.mqtt.example.test`)"]
MQTT_CERTS = ["mqtt-helsinki-sensors sensors.helsinki.mqtt.example.test",
              "mqtt-helsinki-vehicles vehicles.helsinki.mqtt.example.test",
              "mqtt-helsinki-sensors-cert cert.sensors.helsinki.mqtt.example.test"]
if args[0] == "get" and args[1] == "deployments" and "app.kubernetes.io/name=mqtt-broker" in line:
    # The brokers the Portal stamped (T-4857): name and ready replicas.
    for broker in spec.get("mqttBrokers", MQTT_BROKERS):
        print(broker)
elif args[0] == "get" and args[1] == "ingressroutetcp":
    # Their routes: name, entrypoint, match.
    for route in spec.get("mqttRoutes", MQTT_ROUTES):
        print(route)
elif args[0] == "get" and args[1] == "certificates":
    # Their certificates: name, first dnsName.
    for cert in spec.get("mqttCerts", MQTT_CERTS):
        print(cert)
elif args[0] == "get" and args[1] == "configmap" and args[2].startswith("gitea-seed-") and "endpoints__helsinki-" in line:
    # The Helsinki seed's slugs, one per endpoint name (T-0478); nothing seeded by default,
    # because the checks behind them wait up to 210 s for data to flow.
    name = "".join(itertools.takewhile(lambda c: c.isalnum() or c == "-", line.split("endpoints__helsinki-", 1)[1]))
    # The OGC rows' Endpoints (T-3957) are seeded beside the ones the residue count lists.
    seeded = spec.get("helsinkiSeed", {}).get(name) or spec.get("ogcSeed", {}).get(name)
    # A grown seed moves a file to a later part of its release (dev 2026-11-02).
    if args[2].endswith("--2") != (name in spec.get("seedInLaterPart", [])):
        seeded = None
    sys.stdout.write("spec:\\n  slug: %s\\n" % seeded if seeded else "")
elif args[0] == "get" and args[1] == "configmap" and args[2].startswith("gitea-seed-") and "jsonpath={.data}" in line:
    # The seed's file names, for the residue count (T-0667): one Helsinki endpoint per seeded slug.
    keys = ["projects__helsinki__spaces__helsinki__endpoints__helsinki-%s.yaml" % n for n in spec.get("helsinkiSeed", {})]
    if spec.get("helsinkiSeed"):
        # Two pipelines and two spaces beside the endpoints (T-0750): counted the same way.
        keys += ["projects__helsinki__pipelines__hel-news__pipeline.yaml",
                 "projects__helsinki__pipelines__citybikes-gbfs__pipeline.yaml",
                 "projects__helsinki__spaces__helsinki__space.yaml",
                 "projects__helsinki__spaces__helsinki-kpi__space.yaml"]
    sys.stdout.write(json.dumps({k: "" for k in keys}))
elif args[0] == "get" and args[1] == "configmap" and args[2].startswith("gitea-seed-") and "spaces__conformance__endpoints__conformance" in line:
    # The conformance space's Endpoint, where the round trip writes (F-BNT1-002).
    sys.stdout.write(spec.get("conformanceEndpoint", "spec:\\n  slug: dv7iyelohbhylxfilexaneobbl\\n"))
elif args[0] == "get" and args[1] == "configmap" and args[2].startswith("gitea-seed-"):
    # The seeded Endpoint manifest the smoke reads the slug from (T-0282, T-0278); "" = nothing seeded.
    sys.stdout.write(spec.get("seedEndpoint", "spec:\\n  slug: mluyob4nz52lok3ssk7pgn5vwt\\n"))
elif args[0] == "get" and args[1] == "configmap" and args[2] == "portal-branding":
    # The block both the Portal and the catalogue theme are served (OPS-46).
    sys.stdout.write(spec.get("branding", "instanceName: Example Context\\ncolours:\\n  primary: '#0000bf'\\n"))
elif args[0] == "get" and args[1] == "configmaps" and "broker-template" in line:
    # The namespace the brokers chart installed its template into (T-4857); "" = not installed.
    print(spec.get("mqttNamespace", "dev"))
elif args[0] == "get" and args[1] == "configmaps" and "==cm" in line:
    # Every ConfigMap's values (T-4857): the seed's MqttBrokers, one per stamped broker by default.
    print("==cm gitea-seed-s4-helsinki")
    for _ in range(spec.get("mqttSeeded", 2)):
        print("apiVersion: joinedcontext.com/v1alpha1\\nkind: MqttBroker")
    print("==cm gitea-sample-apps-helsinki-bikes\\nkind: MqttBroker")
elif args[0] == "get" and args[1] == "configmaps" and "-o name" in line:
    # The sample applications the forge bootstrap seeded (T-2599); `sampleApps: []` is none.
    for app in spec.get("sampleApps", ["helsinki-bikes", "helsinki-events", "helsinki-alerts"]):
        sys.stdout.write("configmap/gitea-sample-apps-%s\\n" % app)
    # The projects' seed, each named by the release the layout packed it into (T-4431).
    for part in ("s1-banskabystrica", "s4-helsinki", "s4-helsinki--2", "s4-helsinki-mobility", "s2-bbsk--2"):
        sys.stdout.write("configmap/gitea-seed-%s\\n" % part)
elif args[0] == "get" and args[1] == "configmap" and args[2].startswith("gitea-sample-apps-") and "go-template" in line:
    # A sample app's file names: its grants (T-2667) when the spec gives it any.
    app = args[2][len("gitea-sample-apps-"):]
    grants = ["grants__projects__helsinki__spaces__helsinki__endpoints__app-%s.yaml" % app] if app in spec.get("appGrants", []) else []
    sys.stdout.write("".join("%s\\n" % k for k in ["app.yaml", "index.html"] + grants))
elif args[0] == "get" and args[1] == "configmap":
    routes = "".join("  - id: %s\\n" % r for r in spec.get("routes", []))
    sys.stdout.write("routes:\\n" + routes)
    # A credential copied into a rendered config is a credential in `kubectl get -o yaml`
    # (T-0935); the default instance has none, and a test that plants one asserts the catch.
    if spec.get("leakedCredential"):
        sys.stdout.write("  password: %s\\n" % spec["leakedCredential"])
elif args[0] == "get" and args[1] == "deployment" and args[2] == "apisix":
    sys.exit(0 if spec.get("edge", True) else 1)
elif args[0] == "logs" and args[1].startswith("smoke-meshforge"):
    if spec.get("meshForgerRan", True):
        sys.stdout.write("PROBE-RAN\\n")
        code = spec.get("meshForgerCode", 401)
        if code is not None and open(os.environ["STUB_SPEC"] + ".meshforge").read() == "fits":
            sys.stdout.write("HTTP-%s\\n" % code)
        sys.stdout.write("PROBE-DONE\\n")
elif args[0] == "logs" and "pipeline-runner-runner" in line:
    # T-4055: the runner's refusals, each `[age in seconds, line]`; `--since=Ns` shows those
    # at most N seconds old.
    since = int(next(a for a in args if a.startswith("--since=")).split("=")[1].rstrip("s"))
    for age, said in spec.get("runnerRefused", []):
        if age <= since:
            sys.stdout.write('{"level":"error","msg":"%s"}\\n' % said)
elif args[0] == "logs":
    # One access-log line for the beacon path, from whoever the edge thinks called (T-0929).
    caller = spec.get("edgeCaller", "203.0.113.7")
    if caller:
        sys.stdout.write('%s - - [17/Sep/2026:08:00:03 +0000] host "GET /smoke-caller HTTP/1.1" 404 0\\n' % caller)
elif args[0] == "get" and args[1] == "statefulset" and args[2] == "artifact-store":
    # The store itself (T-0925, T-0933); an instance without it skips the whole section.
    sys.exit(0 if spec.get("artifactStore", True) else 1)
elif args[0] == "get" and args[1] == "secret" and "artifact-store-role=reader" in line:
    sys.stdout.write("".join("%s " % n for n in spec.get("artifactReaders", ["artifact-store-reader-helsinki"])))
elif args[0] == "get" and args[1] == "secret" and "artifact-store-role=writer" in line:
    # The writer must exist nowhere: the default is an empty list, and a test that hands one
    # over is asserting that the smoke catches it.
    sys.stdout.write("".join("%s " % n for n in spec.get("artifactWriters", [])))
elif args[0] == "get" and args[1] == "secret" and args[2].startswith("artifact-store-reader-"):
    held = spec.get("artifactReaderKeys", {}).get(args[2], ["ACCESS_KEY_ID", "ACCESS_SECRET_KEY"])
    key = line.rsplit(".data.", 1)[1].rstrip("}")
    sys.stdout.write(base64.b64encode(b"a-credential").decode() if key in held else "")
elif args[0] == "get" and args[1] == "secret" and args[2] == "pipeline-secrets":
    # The one Secret the Portal writes for the runner (T-0927, T-0935). `None` is the instance
    # where no pipeline declares a secretRef at all, and the whole section skips.
    resolved = spec.get("pipelineSecrets", {"DEMO_FEED_PASSWORD": "a-credential"})
    if resolved is None:
        sys.exit(1)
    if "go-template" in line:
        sys.stdout.write("".join("%s\\n" % name for name in resolved))
    elif ".data." in line:
        value = resolved.get(line.rsplit(".data.", 1)[1].rstrip("}"), "")
        sys.stdout.write(base64.b64encode(value.encode()).decode() if value else "")
elif args[0] == "exec" and "deploy/gitea-runner" in line:
    # T-1707: the runner's uid and whether it holds a ServiceAccount token.
    if spec.get("runnerWallsRead", True):
        sys.stdout.write("UID=%s\\n" % spec.get("runnerUid", 1000))
        if spec.get("runnerSaToken", False):
            sys.stdout.write("SA-TOKEN\\n")
        sys.stdout.write("WALLS-READ\\n")
elif args[0] == "exec":
    # `sh -c [ -n "$NAME" ]`: the runner has the variable, or it does not. The value is never
    # printed, here or in the script.
    named = next((a.split("$", 1)[1].strip('" ]') for a in args if "$" in a), "")
    sys.exit(1 if named in spec.get("runnerEnvMissing", []) else 0)
elif args[0] == "get" and args[1] == "secret" and args[2].startswith("keycloak-user-"):
    # A demo user's generated password (T-0248); "" = no demo users seeded. The TOTP seed of a
    # holder of mfa-required (T-5120) is its own key, absent unless the spec names one.
    secret = spec.get("demoTotp", "") if ".data.totp" in line else spec.get("demoPassword", "Passw0rd-demo")
    if not secret:
        sys.exit(1)
    sys.stdout.write(base64.b64encode(secret.encode()).decode())
elif args[0] == "get" and args[1] == "secret" and args[2] == "gitea-runner-registration":
    # The organization the runners' token belongs to, recorded by the bootstrap (T-2969).
    owner = spec.get("runnerOwner", "")
    sys.stdout.write(base64.b64encode(owner.encode()).decode() if owner else "")
elif args[0] == "create" and args[1] == "token":
    # A ServiceAccount token, which the smoke presents as a federated client's assertion
    # (PF-47, T-2868); empty when the account may not have one.
    sys.stdout.write(spec.get("serviceAccountToken", "eyJ.serviceaccount.token"))
elif args[0] == "get" and args[1] == "secret":
    secret = spec.get("clientSecret", "s3cr3t")
    if not secret:
        sys.exit(1)
    sys.stdout.write(base64.b64encode(secret.encode()).decode())
elif args[0] == "get" and args[1] == "deployment" and args[2] == "context-broker":
    sys.exit(0 if spec.get("broker", True) else 1)
elif args[0] == "get" and args[1] == "deployment" and args[2] == "jc-functions":
    sys.exit(0 if spec.get("functions", True) else 1)
elif args[0] == "get" and args[1] == "service" and args[2] == "jc-functions":
    sys.stdout.write("10.43.0.99")
elif args[0] == "port-forward":
    sys.exit(0)
elif args[0] == "run" and args[1].startswith("smoke-functions") and args[1].endswith("-portal"):
    # The probe carrying the Portal's label: the policy's own positive control (T-0686).
    sys.exit(0 if spec.get("functionsPortalReaches", True) else 1)
elif args[0] == "run" and args[1].startswith("smoke-functions"):
    sys.exit(0 if spec.get("functionsReachable") else 1)
elif args[0] == "run" and args[1].startswith("smoke-egress"):
    # The egress probe reports by what it prints, not by its exit status, so the stub prints
    # what the spec says it saw: how long the controller took to program the pod's chains
    # (T-0459), then the destinations that still answered. `egressProbeRan: False` is the pod
    # that never started -- no output at all, which must not read as "the policy stopped it".
    settled = spec.get("egressSettled", 4)
    if settled is not None:
        sys.stdout.write("SETTLED-AFTER=%ds\\n" % settled)
    for destination in spec.get("egressReached", []):
        sys.stdout.write("REACHED-%s\\n" % destination)
    if spec.get("egressProbeRan", True):
        sys.stdout.write("PROBE-RAN\\n")
elif args[0] == "get" and args[1] == "deployments" and "metadata.name=gitea-runner" in line:
    # The Actions runner (T-2608); `runnerNamespace: ""` is an instance without one.
    namespace = spec.get("runnerNamespace", "runners")
    if namespace:
        sys.stdout.write("deployment.apps/gitea-runner\\n" if "name" in args[-1:] else namespace)
elif args[0] == "run" and args[1].startswith("smoke-runner"):
    # Like the egress probe, it reports by what it prints (T-2608).
    if spec.get("runnerProbeRan", True):
        sys.stdout.write("SETTLED-AFTER=4s\\n")
        for destination in spec.get("runnerReached", ["DNS"]):
            sys.stdout.write("REACHED-%s\\n" % destination)
        sys.stdout.write("PROBE-RAN\\n")
elif args[0] == "get" and args[1] == "deployments" and "joinedcontext.com/app=true" in line:
    # T-2665: the published pod-backed Apps as `ns/name|ready|wanted`; ready None = no pod ready.
    for name, ready, wanted in spec.get("appDeployments", [["dev/app-air-quality", 1, 1]]):
        sys.stdout.write("%s|%s|%s\\n" % (name, "" if ready is None else ready, wanted))
elif args[0] == "get" and args[1] == "pods" and "joinedcontext.com/app=true" in line:
    # AP-108: the running pod-backed Apps' addresses; `appPods: []` is an instance without one.
    sys.stdout.write("".join("%s " % ip for ip in spec.get("appPods", ["10.42.0.20"])))
elif args[0] == "run" and args[1].startswith("smoke-app") and args[1].endswith("-edge"):
    # The probe carrying APISIX's labels: the App policy's own positive control. Like the
    # cluster, it passes only with every label the edge's egress policy selects (T-2667).
    edge = all('"%s": "%s"' % pair in line for pair in EDGE_SELECTOR)
    # The pod spec as the cluster would get it, for the Service check (T-6594).
    overrides = next(a for a in args if a.startswith("--overrides="))[len("--overrides="):]
    open(os.environ["STUB_SPEC"] + ".app-edge-pod", "w").write(overrides)
    sys.exit(0 if spec.get("appEdgeReaches", True) and edge else 1)
elif args[0] == "run" and args[1].startswith("smoke-app"):
    # Reports by what it prints, like the edge-peer probe.
    if spec.get("appPeerRan", True):
        sys.stdout.write("PROBE-RAN\\n")
    for port in spec.get("appPeerReached", []):
        sys.stdout.write("REACHED-%s\\n" % port)
elif args[0] == "get" and args[1] == "pods" and "app.kubernetes.io/name=pipeline-runner-runner" in line:
    # T-3537: the runner's restart count, read at the start and the end of the run; each read
    # takes the next value of `runnerRestarts`, and `[]` is an instance without the runner.
    counts = spec.get("runnerRestarts", [0, 0])
    marker = os.environ["STUB_SPEC"] + ".runner-reads"
    reads = int(open(marker).read()) if os.path.exists(marker) else 0
    open(marker, "w").write(str(reads + 1))
    if counts:
        sys.stdout.write(str(counts[min(reads, len(counts) - 1)]))
elif args[0] == "top" and "pipeline-runner-runner" in line:
    sys.stdout.write("pipeline-runner-6bf86c8c76-zmh95   pipeline-runner-runner   76m   %s\\n" % spec.get("runnerMemory", "700Mi"))
elif args[0] == "get" and args[1] == "deployment" and args[2] == "pipeline-runner":
    sys.stdout.write(spec.get("runnerLimit", "1536Mi"))
elif args[0] == "get" and args[1] == "pods" and args[2:4] in (["-n", "linkerd"], ["-n", "linkerd-cni"]):
    # T-4384: `name=Ready status` per pod of the mesh's namespaces; `[]` is a cluster without it.
    default = {"linkerd": [["linkerd-destination-c5", "True"], ["linkerd-identity-6d", "True"]],
               "linkerd-cni": [["linkerd-cni-v2", "True"]]}
    pods = spec.get("meshPods", default).get(args[3], [])
    sys.stdout.write("".join("%s=%s\\n" % (name, ready) for name, ready in pods))
elif args[0] == "get" and args[1] == "pods" and "app.kubernetes.io/name=apisix" in line:
    # T-0939: the probe aims at the APISIX pod's own address; `apisixPodIp: ""` is no pod.
    sys.stdout.write(spec.get("apisixPodIp", "10.42.0.9"))
elif args[0] == "get" and args[1] == "pods" and "app.kubernetes.io/name=context-broker-broker" in line:
    # T-4875: the forged-caller probe aims at the broker pod's own address; "" is no pod.
    # T-6433: the meshed probe wears the pod's own ServiceAccount.
    sys.stdout.write("context-broker-broker" if "serviceAccountName" in line else spec.get("brokerPodIp", "10.42.0.30"))
elif args[0] == "get" and args[1] == "servers.policy.linkerd.io":
    sys.exit(0 if spec.get("brokerServer", True) else 1)
elif args[0] == "run" and args[1].startswith("smoke-forger"):
    # T-4875: the status the broker's proxy gave the forged HTTP request and WebSocket upgrade,
    # and only to a pod that wears the Portal's label and stays out of the mesh.
    if spec.get("forgerRan", True):
        sys.stdout.write("PROBE-RAN\\n")
    unmeshed = '"linkerd.io/inject": "disabled"' in line
    portal = '"app.kubernetes.io/name": "portal-portal"' in line
    for kind, code in zip(("HTTP", "WS"), spec.get("forgerCodes", [403, 403]) if unmeshed and portal else []):
        if code is not None:
            sys.stdout.write("%s-%s\\n" % (kind, code))
elif args[0] == "run" and args[1].startswith("smoke-meshforge"):
    # T-6433: remembered for `logs`: the status only a meshed pod wearing the broker's
    # ServiceAccount and the Portal's label gets.
    pod = json.loads(next(a for a in args if a.startswith("--overrides=")).split("=", 1)[1])
    fits = (pod["metadata"]["annotations"].get("linkerd.io/inject") == "enabled"
            and "mesh.joinedcontext.com/opt-out-reason" not in pod["metadata"]["annotations"]
            and pod["spec"]["serviceAccountName"] == "context-broker-broker"
            and pod["metadata"]["labels"]["app.kubernetes.io/name"] == "portal-portal")
    with open(os.environ["STUB_SPEC"] + ".meshforge", "w") as f:
        f.write("fits" if fits else "")
elif args[0] == "run" and args[1].startswith("smoke-edgepeer"):
    # T-0939: like the egress probe, it reports by what it prints. `edgePeerRan: False` is a
    # probe that never ran, which must not read as "the policy refused it".
    if spec.get("edgePeerRan", True):
        sys.stdout.write("PROBE-RAN\\n")
    for port in spec.get("edgePeerReached", []):
        sys.stdout.write("REACHED-%s\\n" % port)
elif args[0] == "run":
    sys.exit(0 if spec.get("netpolReachable") else 1)
else:
    sys.exit(1)
'''

# mosquitto_pub without a token: the gateway refuses the CONNECT (T-4857), unless a test says not.
MOSQUITTO_STUB = '''#!/usr/bin/env -S python3 -S
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
if spec.get("mqttAnonymous"):
    sys.exit(0)
sys.stderr.write("Connection error: Connection Refused: not authorised.\\n")
sys.exit(5)
'''

OPENSSL_STUB = '''#!/usr/bin/env -S python3 -S
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
if "-verify_hostname" in sys.argv:
    # An MQTT broker's own certificate for its host (T-4857), or the edge's default one.
    sys.exit(0 if spec.get("mqttTls", True) else 1)
sys.stdout.write("New, (NONE), Cipher is (NONE)\\n" if spec.get("cbcRefused", True)
                 else "New, TLSv1.2, Cipher is ECDHE-RSA-AES128-SHA256\\n")
'''

ROLLOUTS_STUB = "#!/bin/sh\nexit 0\n"

# scripts/smoke-ws.py has its own tests (test_smoke_ws.py); here it answers what the spec says.
WS_STUB = '''#!/usr/bin/env -S python3 -S
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
answer = spec.get("ws", "101 201")
print(answer)
sys.exit(0 if answer == "101 201" else 1)
'''


def sandbox(tmp_path: Path, spec: dict) -> dict:
    """A copy of the script with stubbed neighbours; returns the env to run it with."""
    scripts = tmp_path / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)
    (scripts / "smoke.sh").write_text(SMOKE.read_text())
    (scripts / "smoke.sh").chmod(0o755)
    # The events calendars' Endpoints, whose slugs the smoke reads from the seed beside it (T-4895).
    for name in ("bbsk/bbsk-endpoint-podujatia.yaml", "slovensko/slovensko-endpoint-podujatia.yaml"):
        seed = tmp_path / "components/context-gateway/seed" / name
        seed.parent.mkdir(parents=True, exist_ok=True)
        seed.write_text((PROJECT_ROOT / "components/context-gateway/seed" / name).read_text())
    (scripts / "wait-rollouts.sh").write_text(ROLLOUTS_STUB)
    (scripts / "wait-rollouts.sh").chmod(0o755)
    (scripts / "smoke-ws.py").write_text(WS_STUB)
    (scripts / "smoke-ws.py").chmod(0o755)

    stub_bin = tmp_path / "bin"
    stub_bin.mkdir(parents=True, exist_ok=True)
    for name, body in (("curl", CURL_STUB), ("kubectl", KUBECTL_STUB), ("openssl", OPENSSL_STUB), ("mosquitto_pub", MOSQUITTO_STUB)):
        (stub_bin / name).write_text(body)
        (stub_bin / name).chmod(0o755)

    spec_file = tmp_path / "spec.json"
    spec_file.write_text(json.dumps(spec))

    env = dict(os.environ)
    env["PATH"] = f"{stub_bin}:{env['PATH']}"
    env["STUB_SPEC"] = str(spec_file)
    env.setdefault("JC_SMOKE_RUNNER_WINDOW", "1")
    return env


def run(tmp_path: Path, spec: dict, *args: str, extra_env: dict | None = None) -> subprocess.CompletedProcess:
    env = sandbox(tmp_path, spec)
    env.update(extra_env or {})
    result = subprocess.run(
        ["scripts/smoke.sh", *args],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    # pytest cuts the repr of a CompletedProcess where the reason stands (T-2660), but shows a
    # failing case's captured stderr whole: every FAIL line, the summary and the script's own
    # stderr go there, so the next red case names its line.
    named = [line for line in result.stdout.splitlines() if "FAIL" in line or " passed, " in line]
    print("smoke.sh", *args, "->", result.returncode, *named, result.stderr, sep="\n", file=sys.stderr)
    return result


# What the Portal's /api/v1/auth/login answers: the Keycloak authorization URL carrying the
# callback the portal-api client lists (T-0270), on the Portal host (ADR-N-019).
LOGIN_REDIRECT = ("https://idm.example.test/realms/dev/protocol/openid-connect/auth?client_id=portal-api"
                  "&redirect_uri=https%3A%2F%2Fportal.example.test%2Fapi%2Fv1%2Fauth%2Fcallback&state=s")
# What the edge login answers an anonymous visitor of the Portal host: lua-resty-openidc's
# authorization URL with the `edge` client and the callback that client lists (AP-27).
EDGE_REDIRECT = ("https://idm.example.test/realms/dev/protocol/openid-connect/auth?response_type=code"
                 "&client_id=edge&state=s&redirect_uri=https%3A%2F%2Fportal.example.test%2Fcallback"
                 "&nonce=n&scope=openid&code_challenge=c&code_challenge_method=S256")

# First match wins, so the narrower keys come first.
HEALTHY_STATUSES = [
    # The apex moves an anonymous call of the hub to the data host (T-4630, EP-128).
    ["example.test/api/mcp/.well-known/oauth-protected-resource?x=1", 308],
    # The catalogue's old host moves to opendata.{domain} with path and query (T-4679).
    ["https://data.example.test/dataset/x?q=1", 301],
    # The events calendars refuse an anonymous write (T-4895).
    ["smoke-anonymous-write", 403],
    # Event keeps no history, so no Policy grants its temporal read (T-4372).
    [["hsevents", "/temporal/entities"], 404],
    ["/invoke", 401],
    ["pipelines/test", 200],
    ["/entities?", 200],
    ["/ckan", 302],
    # /docs/* has no public route: portal-ui's login answers a visitor (T-6200).
    ["/docs/User-Guide/", 302],
    ["-X DELETE", 204],
    ["smoke-not-a-urn", 400],
    ["entities/urn", 200],
    ["/entities", 201],
    [["Bearer", ".forged"], 401],
    ["eyJleHAiOjF9", 401],
    [["Bearer", "/api/v1/auth/me"], 200],
    ["/api/v1/auth/me", 401],
    ["/api/v1/health", 200],
    # The configuration repository is private: without a session the forge serves nothing (PF-79).
    ["configuration/raw/branch", 404],
]

# What a branded CKAN front page looks like from outside: the instance's own name and its
# own primary colour, both from the block above and neither of them in any image (OPS-47).
CATALOGUE_PAGE = (
    "<html><head><title>Example Context</title>"
    "<style>:root { --jc-primary: #0000bf; }</style>"
    '<link rel="stylesheet" href="/webassets/base/1a2b3c4d_main.css">'
    "</head><body>Example Context</body></html>"
)

# The Helsinki seed's slugs (T-0478), one per endpoint name, and the answers a healthy space
# gives: the policy slice is empty on the bikes endpoint, every type flows on `all`. The flow
# checks return the moment an entity answers, so a healthy instance never waits.
HELSINKI_SEED = {"events": "hsevents", "bikes": "hsbikes", "transport": "hstransport", "all": "hsall"}

# What jc-functions answers the smoke's function when the gateway answered it (T-0686).
FUNCTION_ANSWER = '{"status":200,"body":{"gateway":200},"logs":["smoke"]}'

# What the Portal answers for a sample application the lane has built (T-2599).
BUILT_APP = '{"kind":"App","status":{"build":{"commit":"%s","digest":"sha256:%s"}}}' % ("a" * 40, "b" * 64)

# The published Apps of Helsinki (T-2668): one public, one behind its own login, and a draft that
# has no route yet and is not probed.
PUBLISHED_APPS = json.dumps({"items": [
    {"kind": "App", "metadata": {"name": "hsl-transport"}, "spec": {"lifecycle": "published", "visibility": "public"}},
    {"kind": "App", "metadata": {"name": "air-quality"}, "spec": {"lifecycle": "published", "visibility": "internal"}},
    {"kind": "App", "metadata": {"name": "sketch"}, "spec": {"lifecycle": "draft", "visibility": "public"}},
]})
# What the edge sends with a public App's page when the Portal serves a basemap (AP-12, AP-67).
BASEMAP = "https://portal.example.test/api/v1/projects/helsinki/basemap/"
APP_POLICY = ("content-security-policy: default-src 'self'; script-src 'self'; "
              f"img-src 'self' data: blob: {BASEMAP}; connect-src 'self' {BASEMAP}; "
              "frame-ancestors https://portal.example.test\r\n")
# Where the edge sends an anonymous visitor of a non-public App: its own client's login.
APP_LOGIN = ("https://idm.example.test/realms/dev/protocol/openid-connect/auth?response_type=code"
             "&client_id=app-air-quality&redirect_uri=https%3A%2F%2Fair-quality.apps.example.test%2Fcallback")

# The demo people the seed creates (components/keycloak/demo-users.yaml), as People lists them.
DEMO_PEOPLE = json.dumps({"items": [{"email": "demo.%s@hel.fi" % who} for who in ("steward", "viewer", "approver", "editor")]})

# T-3957: the OGC API Features answers of a healthy dev, one Endpoint per kind. The internal
# one is seeded beside the Helsinki endpoints the residue count lists; the star one is the
# ovzdusie seed (`seedEndpoint`).
OGC_SEED = {"bikes-ops": "hsbikesops"}
OGC_COLLECTIONS = '{"collections":[{"id":"BikeHireDockingStation"},{"id":"Event"}]}'
OGC_ITEMS = ('{"type":"FeatureCollection","features":[{"type":"Feature","id":"urn:ngsi-ld:x",'
             '"geometry":null,"properties":{"name":"Kamppi"}}]}')
OGC_HEADERS = "HTTP/2 200\r\ncontent-type: application/geo+json\r\n"

HUB_METADATA = '{"resource":"https://api.example.test/api/mcp","authorization_servers":["https://api.example.test"]}'

# GET /api/v1/version of a Portal whose UI bundle was built against its API (T-2485, T-5297).
VERSION_MATCH = json.dumps({"portal": {"commit": "a9ff173", "openapi": "befc4155"},
                            "ui": {"commit": "fca645d", "openapi": "befc4155"}, "contract": "match"})

HEALTHY = {
    "ogcSeed": OGC_SEED,
    "routes": ["portal-ui", "portal-api", "portal-public-assets", "context-space", "context-endpoint", "context-mcp-hub",
               "gitea-forge", "ckan"],
    "locations": [["oauth-protected-resource?x=1", "https://api.example.test/api/mcp/.well-known/oauth-protected-resource?x=1"],
                  ["https://data.example.test/dataset/x?q=1", "https://opendata.example.test/dataset/x?q=1"]],
    # The Portal's content-hashed chunks and CKAN's fingerprinted webassets are kept a year (T-4201).
    "headers": [["/assets/", "HTTP/2 200\r\ncache-control: public, max-age=31536000, immutable\r\n"],
                ["/webassets/", "HTTP/2 200\r\ncache-control: public, max-age=31536000, immutable\r\n"]],
    # The old path of each published App moves to its own host with no cookie (AP-133).
    "dumpedHeaders": [
        ["/ogc/features/collections/", OGC_HEADERS],
        ["/apps/hsl-transport/", "HTTP/2 308\r\nlocation: https://hsl-transport.apps.example.test/\r\n"],
        ["/apps/air-quality/", "HTTP/2 308\r\nlocation: https://air-quality.apps.example.test/\r\n"],
        # A public App's own page, with the AP-12 policy that lets the basemap tiles in (T-3014).
        ["hsl-transport.apps.example.test/", "HTTP/2 200\r\n" + APP_POLICY],
    ],
    "helsinkiSeed": HELSINKI_SEED,
    "bodies": [["https://api.example.test/api/mcp/.well-known/oauth-protected-resource", HUB_METADATA],
               ["/api/v1/version", VERSION_MATCH],
               ["/ogc/features/collections/", OGC_ITEMS],
               ["/ogc/features/collections", OGC_COLLECTIONS],
               ["/api/v1/projects/helsinki/apps/helsinki-", BUILT_APP],
               ["/api/v1/projects/helsinki/apps", PUBLISHED_APPS],
               [["%{redirect_url}", "https://air-quality.apps.example.test/"], APP_LOGIN],[["Bearer", "/invoke"], FUNCTION_ANSWER],
               # The organization's Actions runner, online (T-2608, ADR-N-028).
               ["actions/runners", '{"runners":[{"name":"gitea-runner-1","status":"online"}]}'],
               ["clients?clientId=edge", '[{"protocolMappers":[{"config":{"included.client.audience": "portal-api"}}]}]'],
               # Both of the proxy's audiences: the gateway, which it reads endpoints through, and
               # the Portal's internal listener, which answers its run lookups. A healthy instance
               # has both mappers, and smoke.sh checks each on its own (T-0666, T-2420).
               ["clients?clientId=helsinki-agent-proxy",
                '[{"protocolMappers":[{"config":{"included.custom.audience": "context-gateway"}},'
                '{"config":{"included.custom.audience": "portal-internal"}}]}]'],
               # The access document each endpoint answers, which is where the probe reads the
               # type it may ask for (T-1211, EP-55): `retrieveOps` grants entity reads and not
               # the type list, so a probe that asked for the type list would be a 403.
               ["hsevents/access", '{"permissions":[{"resource":{"type":"Event"}}]}'],
               ["hsbikes/access", '{"permissions":[{"resource":{"type":"BikeHireDockingStation"}}]}'],
               ["hstransport/access", '{"permissions":[{"resource":{"type":"Vehicle"}}]}'],
               ["hsall/access", '{"permissions":[{"resource":{"type":"Event"}}]}'],
               ["hsbikes/ngsi-ld/v1/entities?type=Event", "[]"],
               # What the lifetime check reads (T-4823): a bus five minutes from its position, a
               # Fintraffic Alert ending at its validTo, and an HSL one, which keeps no lifetime.
               ["hsall/ngsi-ld/v1/entities?type=Vehicle,Alert", LIVING],
               ["hsall/ngsi-ld/v1/entities?type=", '[{"id":"urn:ngsi-ld:x"}]'],
               # What the endpoint's space lists, so the smoke can retrieve each of them by id
               # (T-0945). A healthy space serves everything it lists.
               ["entities?type=AirQualityObserved",
                '[{"id":"urn:ngsi-ld:AirQualityObserved:example.test:ovzdusie:1"},'
                '{"id":"urn:ngsi-ld:AirQualityObserved:example.test:ovzdusie:2"}]'],
               ["openid-configuration", '{"jwks_uri":"https://idm/certs"}'],
               ["/token", '{"access_token":"a.b.c"}'],
               # Organization → People, as the steward reads it (T-2688).
               ["/api/v1/organization/people", DEMO_PEOPLE],
               ["/api/v1/auth/login", LOGIN_REDIRECT],
               ["/api/v1/projects/helsinki/spaces", json.dumps({"items": [{"kind": "ContextSpace", "metadata": {"name": n}} for n in ("helsinki", "helsinki-kpi")]})],
               ["/api/v1/projects/helsinki/pipelines", json.dumps({"items": [{"kind": "Pipeline", "metadata": {"name": n}} for n in ("hel-news", "citybikes-gbfs")]})],
               ["/api/v1/projects/helsinki/endpoints", json.dumps({"items": [{"kind": "Endpoint", "metadata": {"name": "helsinki-%s" % n}} for n in HELSINKI_SEED]})],
               [["%{redirect_url}", "https://portal.example.test/"], EDGE_REDIRECT],
               [["%{redirect_url}", "https://example.test/"], "https://portal.example.test/"],
               ["datasources?dryRun=All", '{"valid": true, "probe": {"skipped": "the feed could not be reached: connection refused"}}'],
               ["api_token_list", '{"help": "", "success": true, "result": []}'],
               # The forge's teams (PF-79, PF-80): the administrators' own team and the one a
               # person signed in with Keycloak lands in. Gitea answers "none" as the summary
               # permission of any unit map it did not mint itself, so the map is what the
               # smoke reads.
               ["/api/v1/orgs/joinedcontext/teams", json.dumps([
                   {"name": "Owners", "permission": "owner",
                    "units_map": {"repo.code": "owner", "repo.issues": "owner"}},
                   {"name": "readers", "permission": "none",
                    "units_map": {"repo.code": "read", "repo.issues": "read", "repo.pulls": "read"}},
               ], separators=(",", ":"))],
               ["portal.example.test/catalogue", '<script type="module" src="/assets/index-1a2b3c4d.js"></script>'],
               # The events calendars (T-4895): an RFC 5545 feed, CRLF line ends.
               ["/ical/v1/events.ics", "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Antares//antares-ical//EN\r\nEND:VCALENDAR\r\n"],
               ["data.example.test/", CATALOGUE_PAGE]],
    "statuses": HEALTHY_STATUSES,
}


def test_a_datasource_check_that_reaches_inside_the_cluster_fails(tmp_path):
    """T-0752: a Check of an in-cluster address or the metadata service must answer unreachable;
    a status from the address, or an answer without a refusal, is the runner's egress letting a
    typed URL in."""
    spec = dict(HEALTHY)
    spec["bodies"] = [
        [["datasources?dryRun=All", "kubernetes.default.svc"], '{"valid": true, "probe": {"skipped": "the feed answered 200 OK"}}'],
        [["datasources?dryRun=All", "169.254.169.254"], '{"valid": true, "probe": {"records": 1, "bytes": 90}}'],
        *HEALTHY["bodies"],
    ]
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  a probe of https://kubernetes.default.svc/version reached it: the feed answered 200 OK" in result.stdout
    assert "FAIL  a probe of https://169.254.169.254/hetzner/v1/metadata answered no refusal" in result.stdout
    assert "ok    a probe of http://context-broker.dev.svc:8080/ngsi-ld/v1/types stays outside the cluster" in result.stdout
    assert "2 failed, 2 skipped" in result.stdout


def test_missing_arguments_are_refused(tmp_path):
    """Both URLs are mandatory: a one-argument call must not silently smoke half a platform."""
    result = run(tmp_path, HEALTHY, "https://example.test")
    assert result.returncode != 0
    assert "usage: smoke.sh" in result.stderr


def test_full_platform_passes(tmp_path):
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    # The only skips are the OGC kinds no seeded Endpoint has (T-3957): filtered view and cube.
    assert "0 failed, 2 skipped" in result.stdout
    assert [line for line in result.stdout.splitlines() if line.startswith("  skip")] == [
        "  skip  ogc filtered view (no seeded Endpoint narrows its view by a q)",
        "  skip  ogc cube (no seeded Endpoint serves a LinkML cube, T-3805)"]
    assert "ok    portal host sends an anonymous visitor to the edge login" in result.stdout
    assert "ok    apex redirects to the portal host" in result.stdout
    assert "ok    portal UI bundle was built against the binary's API (contract match)" in result.stdout
    assert "warnings" not in result.stdout
    assert "ok    portal API answers a bearer call through the edge (200)" in result.stdout
    assert "client_credentials token" in result.stdout
    assert "ok    demo user demo.viewer@hel.fi logs in with a password grant" in result.stdout
    assert "entity create through the endpoint (201)" in result.stdout
    assert "write with an id that is no NGSI-LD URN is refused (400)" in result.stdout
    assert "ok    an id naming another organization lands in this space (ADR-N-041) (201)" in result.stdout
    assert "ok    and is deleted again (204)" in result.stdout
    assert "git forge answers under the /git prefix (200)" in result.stdout
    assert "catalogue front page carries the instance name" in result.stdout
    assert "ok    data.{domain} answers 301 to the same path on opendata.{domain}" in result.stdout
    assert "ok    pipeline test runs a candidate on the runner and captures the output (200)" in result.stdout
    assert "ok    helsinki lists 4 endpoints, none of them residue of takes or e2e" in result.stdout
    assert "ok    helsinki lists 2 pipelines, none of them residue of takes or e2e" in result.stdout
    assert "ok    helsinki lists 2 spaces, none of them residue of takes or e2e" in result.stdout
    assert "ok    agent proxy client mints tokens with audience context-gateway" in result.stdout
    assert "ok    helsinki-bikes does not serve events" in result.stdout
    assert "ok    bbsk-podujatia serves its events as an iCalendar feed" in result.stdout
    assert "ok    slovensko-podujatia refuses an anonymous write (403)" in result.stdout
    assert "ok    the broker mounts the ical surface" in result.stdout
    assert "ok    helsinki-transport opens its WebSocket anonymously and subscribes to Vehicle (101 201)" in result.stdout
    assert "ok    city bike stations are flowing into the space" in result.stdout
    assert "ok    buses are flowing into the space" in result.stdout
    assert "ok    every vehicle and Fintraffic Alert carries its expiresAt (2)" in result.stdout
    assert "catalogue front page carries the branded primary colour" in result.stdout
    assert "ok    the publisher's CKAN API token authenticates as the site administrator" in result.stdout
    assert "postgres refuses a pod outside the allowed selectors" in result.stdout
    assert "ok    jc-functions refuses an invocation without the Portal's token (401)" in result.stdout
    assert "ok    jc-functions runs a function for the Portal's token and the function reads the gateway" in result.stdout
    assert "ok    a pod with the Portal's label reaches jc-functions:8080" in result.stdout
    assert "ok    jc-functions refuses a pod that is not the Portal" in result.stdout
    assert "security response headers present" in result.stdout
    assert "ok    People lists the demo people" in result.stdout
    assert "the edge refuses TLS 1.1" in result.stdout
    assert "the edge refuses CBC and 3DES suites" in result.stdout


def test_a_linkerd_control_plane_pod_that_is_not_ready_fails(tmp_path):
    """T-4384: after a reboot the control plane sat not Ready and every meshed pod with it."""
    mesh = {"linkerd": [["linkerd-destination-c5", "False"], ["linkerd-identity-6d", "True"],
                        ["linkerd-proxy-injector-55", ""]],
            "linkerd-cni": [["linkerd-cni-v2", "True"]]}
    result = run(tmp_path, dict(HEALTHY, meshPods=mesh), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  linkerd pods not Ready: linkerd-destination-c5 linkerd-proxy-injector-55\n" in result.stdout
    assert "ok    every pod in linkerd-cni is Ready" in result.stdout


def test_a_cluster_without_the_mesh_skips_its_check(tmp_path):
    result = run(tmp_path, dict(HEALTHY, meshPods={}), "https://example.test", "https://idm.example.test")
    assert "skip  linkerd (no pod in it on this cluster)" in result.stdout
    assert "skip  linkerd-cni (no pod in it on this cluster)" in result.stdout
    assert "linkerd pods not Ready" not in result.stdout


def test_an_endpoint_moved_to_a_later_seed_part_is_still_found(tmp_path):
    """A grown seed moves files between gitea-seed-<project> and its --N parts (dev 2026-11-02)."""
    moved = dict(HEALTHY, seedInLaterPart=["events", "bikes", "transport"])
    result = run(tmp_path, moved, "https://example.test", "https://idm.example.test")
    assert "names no type in its access document" not in result.stdout


def test_an_instance_without_the_helsinki_seed_skips_its_checks(tmp_path):
    """No seed, no 210 s wait for data that will never come: the section is a skip."""
    result = run(tmp_path, dict(HEALTHY, helsinkiSeed={}), "https://example.test", "https://idm.example.test")
    assert "skip  helsinki endpoints (no Helsinki seed on the gateway in this instance)" in result.stdout


def bodies_with_version(version: str) -> list:
    return [["/api/v1/version", version]] + [b for b in HEALTHY["bodies"] if b[0] != "/api/v1/version"]


def test_a_ui_bundle_built_against_another_api_warns_without_failing(tmp_path):
    """T-5297: a mismatched bundle still serves pages, so the run stays green and says why."""
    mismatch = json.dumps({"portal": {"commit": "b", "openapi": "1"}, "ui": {"commit": "a", "openapi": "2"},
                           "contract": "mismatch"})
    result = run(tmp_path, dict(HEALTHY, bodies=bodies_with_version(mismatch)),
                 "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "warn  portal UI bundle contract is mismatch, not match: " in result.stdout
    assert "\n1 warnings\n" in result.stdout


def test_a_version_that_does_not_answer_warns_as_unreadable(tmp_path):
    """No answer (an older Portal, a 404 page) is no contract: a warning naming it, not a pass."""
    result = run(tmp_path, dict(HEALTHY, bodies=bodies_with_version("<html>not found</html>")),
                 "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "warn  portal UI bundle contract is unreadable, not match: <html>not found</html>" in result.stdout


def test_a_transport_websocket_that_is_not_public_fails_with_its_answer(tmp_path):
    """T-4035: the 404 the supervisor measured on dev (no `websocket` representation) is a FAIL
    line that says what the gateway answered."""
    result = run(tmp_path, dict(HEALTHY, ws="404 -"), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  helsinki-transport WebSocket answered '404 -', expected '101 201'" in result.stdout


def test_a_vehicle_or_alert_without_its_lifetime_fails_the_run(tmp_path):
    """T-4823: with the reapers gone, a bus or a Fintraffic Alert without `expiresAt` stays in the
    space after its feed goes quiet; the run names one."""
    immortal = json.loads(LIVING)
    del immortal[0]["expiresAt"]
    late = json.loads(LIVING)
    late[1]["expiresAt"] = "2026-10-20T12:00:00Z"
    for living, culprit in ((immortal, "Vehicle:hel.fi:helsinki:22-7"), (late, "Alert:hel.fi:helsinki:GUID1")):
        bodies = [["hsall/ngsi-ld/v1/entities?type=Vehicle,Alert", json.dumps(living)]] + HEALTHY["bodies"]
        result = run(tmp_path, dict(HEALTHY, bodies=bodies), "https://example.test", "https://idm.example.test")
        assert result.returncode != 0
        assert f"FAIL  vehicles and Alerts: 1 of 2 carry no lifetime, e.g. urn:ngsi-ld:{culprit}\n" in result.stdout


def test_a_schema_artifact_is_revalidated_and_answers_304(tmp_path):
    """EP-51, T-2262: the gateway's `private, no-cache` and ETag reach the reader, and a second
    read with the ETag is a 304 — the edge no longer replaces them with `no-store`."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    # Once on main (run 35738512945, T-2653) the whole Helsinki section was skipped and pytest
    # cut the output where the reason stood: every skip line and the section are named here.
    helsinki = [line for line in result.stdout.splitlines() if "skip" in line or "helsinki" in line]
    assert "ok    a schema artifact may be kept and revalidated by its reader (private, no-cache)" in result.stdout, helsinki
    assert "ok    a second read of a schema artifact with its ETag is a 304 (304)" in result.stdout, helsinki


def test_an_edge_that_forbids_keeping_a_schema_artifact_fails_the_run(tmp_path):
    """The regression T-2262 fixed: the edge's `no-store` over the gateway's header, and no 304."""
    spec = dict(HEALTHY, schemaHeaders="HTTP/2 200\r\ncache-control: no-store, no-cache, must-revalidate\r\n")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  a schema artifact says Cache-Control 'no-store, no-cache, must-revalidate', expected 'private, no-cache'" in result.stdout
    assert "FAIL  a schema artifact carries no ETag to revalidate with" in result.stdout


def listing(kind: str, names) -> str:
    return json.dumps({"items": [{"kind": kind, "metadata": {"name": n}} for n in names]})


def test_pipeline_residue_beyond_one_take_fails_the_run(tmp_path):
    """Pipelines and spaces are judged the way endpoints are (T-0750, T-2817)."""
    listed = listing("Pipeline", ["hel-news", "citybikes-gbfs", "t1597-feed", "citybikes-0915"])
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/pipelines", listed]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  helsinki lists residue of takes or e2e among its pipelines: t1597-feed citybikes-0915 (T-0667)" in result.stdout


def test_endpoint_residue_beyond_one_take_fails_the_run(tmp_path):
    """Two journey-named endpoints are residue of takes or e2e (T-0667, T-2817)."""
    names = ["helsinki-%s" % n for n in HELSINKI_SEED] + ["t1588-a", "t1589r-b"]
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/endpoints", listing("Endpoint", names)]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  helsinki lists residue of takes or e2e among its endpoints: t1588-a t1589r-b (T-0667)" in result.stdout


def test_one_take_in_flight_is_named_and_not_residue(tmp_path):
    """A take that is running holds one resource of a kind until it cleans up (T-0750)."""
    names = ["helsinki-%s" % n for n in HELSINKI_SEED] + ["bikes-1942"]
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/endpoints", listing("Endpoint", names)]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    helsinki lists 5 endpoints, one a take in flight: bikes-1942" in result.stdout


def test_what_a_person_made_on_dev_is_not_residue(tmp_path):
    """T-2817: a generated app's endpoint and an upload are people's work, whatever their count."""
    names = ["helsinki-%s" % n for n in HELSINKI_SEED] + ["app-alerts-desk", "helsinki-bikes-mobility", "app-air-quality"]
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/endpoints", listing("Endpoint", names)]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    helsinki lists 7 endpoints, none of them residue of takes or e2e" in result.stdout


def test_the_smoke_names_residue_the_way_the_janitor_does():
    """The smoke's pattern is the janitor role's (T-2627, T-2817): what the sweep may remove is
    exactly what the smoke calls residue, so neither drifts from the other."""
    root = Path(__file__).resolve().parent.parent
    role = yaml.safe_load((root / "components/context-gateway/seed/helsinki/helsinki-role-janitor.yaml").read_text())
    (constraint,) = role["spec"]["rules"][0]["constraints"]
    smoke = (root / "scripts/smoke.sh").read_text()
    assert f"RESIDUE_NAMES='{constraint['pattern']}'" in smoke


def test_an_endpoint_list_that_does_not_answer_is_not_called_clean(tmp_path):
    """No endpoints at all for a seeded instance is a list that failed, not a clean one (T-0667)."""
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/endpoints", "{}"]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  helsinki lists no endpoints: the list did not answer" in result.stdout


def test_an_agent_proxy_without_the_gateway_audience_fails_the_run(tmp_path):
    """Seeded slugs alone never name an endpoint approved later: the kit pass reads 401 (T-0666)."""
    slugs_only = '[{"protocolMappers":[{"config":{"included.custom.audience": "si6epqkx364lprho5uaigutk274r5grb"}}]}]'
    spec = dict(HEALTHY, bodies=[["clients?clientId=helsinki-agent-proxy", slugs_only]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  agent proxy client has no context-gateway audience" in result.stdout
    spec = dict(HEALTHY, bodies=[["clients?clientId=helsinki-agent-proxy", "[]"]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "skip  agent proxy audience (no helsinki-agent-proxy client in realm dev)" in result.stdout


def test_the_functions_check_proves_portal_api_with_the_portal_pods_token_and_no_client_id(tmp_path):
    """PF-47, T-2868: portal-api holds no secret. The smoke mints a token of the Portal pod's
    ServiceAccount and presents it as the client assertion; a request naming the client id is
    one Keycloak refuses beside an assertion, and without a token nothing is invoked."""
    federated = [
        ["client_id=portal-api", '{"error":"invalid_client"}'],
        [["client_assertion=eyJ.serviceaccount.token", "jwt-bearer"], '{"access_token":"fn.token.sig"}'],
    ]
    first = tmp_path / "minted"
    first.mkdir()
    result = run(
        first, dict(HEALTHY, bodies=federated + HEALTHY["bodies"]), "https://example.test", "https://idm.example.test"
    )
    assert "ok    jc-functions runs a function for the Portal's token and the function reads the gateway" in result.stdout

    second = tmp_path / "no-token"
    second.mkdir()
    result = run(
        second,
        dict(HEALTHY, serviceAccountToken="", bodies=federated + HEALTHY["bodies"]),
        "https://example.test",
        "https://idm.example.test",
    )
    assert "FAIL  no client_credentials token for portal-api, so no function was invoked" in result.stdout


def test_a_function_that_cannot_read_the_gateway_fails_the_run(tmp_path):
    """Status 0 from the host request is the runtime's egress to the gateway refused (T-0686)."""
    cut_off = '{"status":200,"body":{"gateway":0},"logs":["smoke"]}'
    spec = dict(HEALTHY, bodies=[[["Bearer", "/invoke"], cut_off]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  jc-functions did not answer the smoke function with the gateway's 200: " + cut_off in result.stdout


def test_jc_functions_open_to_any_pod_or_closed_to_the_portal_fails_the_run(tmp_path):
    """The unlabelled probe must be refused, and the Portal-labelled one must get through, or the
    refusal only measured a pod that could not dial (T-0686)."""
    result = run(tmp_path, dict(HEALTHY, functionsReachable=True), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  an unlabelled pod reached jc-functions:8080" in result.stdout
    result = run(tmp_path, dict(HEALTHY, functionsPortalReaches=False), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  a pod with the Portal's label cannot reach jc-functions:8080" in result.stdout


def test_an_events_endpoint_without_its_calendar_fails_the_run(tmp_path):
    """T-4895: the calendar a person pasted answers a problem document, not an iCalendar feed,
    when the broker lacks `ical` or the Endpoint does not list it."""
    problem = [["/ical/v1/events.ics", '{"status":404,"title":"Not Found"}']]
    result = run(tmp_path, dict(HEALTHY, bodies=problem + HEALTHY["bodies"]), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  bbsk-podujatia serves no iCalendar feed at /ical/v1/events.ics" in result.stdout


def test_a_surface_the_broker_does_not_mount_fails_the_run(tmp_path):
    """T-3905, T-3922: `/q/health` lists each surface the deployment selected; one missing from
    it is a broker built without it or a list that lost it."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    for surface in ("views", "mcp", "access", "ws", "ogc"):
        assert f"ok    the broker mounts the {surface} surface" in result.stdout
    result = run(tmp_path, dict(HEALTHY, brokerSurfaces=["views", "mcp", "access", "ws"]),
                 "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  the broker's /q/health lists no ogc surface" in result.stdout


def test_one_ogc_row_per_endpoint_kind_names_its_endpoint(tmp_path):
    """T-3957: each kind of T-3831's OGC matrix is a row naming the seeded Endpoint it read, or a
    skip naming the kind dev has none of; the internal row reads with the demo user's token."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    for kind, name, ep in (("public", "helsinki-bikes", "hsbikes"),
                           ("internal", "helsinki-bikes-ops", "hsbikesops"),
                           ("star", "public-air", "mluyob4nz52lok3ssk7pgn5vwt")):
        assert (f"ok    ogc {kind} ({name} {ep}): conformance, 1 feature of BikeHireDockingStation "
                "as geo+json\n") in result.stdout
    assert ("ok    ogc projection (helsinki-all hsall): conformance, 1 feature of "
            "BikeHireDockingStation as geo+json, no contactPoint") in result.stdout
    assert "skip  ogc filtered view (no seeded Endpoint narrows its view by a q)" in result.stdout
    assert "skip  ogc cube (no seeded Endpoint serves a LinkML cube, T-3805)" in result.stdout
    assert "a.b.c" not in result.stdout, "a token reached the output"


@pytest.mark.parametrize("change, reason", [
    ({"statuses": [["hsbikes/ogc/features/conformance", 404]] + HEALTHY_STATUSES},
     "ogc public (helsinki-bikes hsbikes): /conformance answered 404"),
    ({"bodies": [["hsbikes/ogc/features/collections", '{"collections":[]}']] + HEALTHY["bodies"]},
     "ogc public (helsinki-bikes hsbikes): /collections lists none"),
    ({"dumpedHeaders": [["hsbikes/ogc/features/collections/", "HTTP/2 502\r\n"]] + HEALTHY["dumpedHeaders"]},
     "ogc public (helsinki-bikes hsbikes): items of BikeHireDockingStation answered HTTP/2 502"),
    ({"dumpedHeaders": [["hsbikes/ogc/features/collections/", "HTTP/2 200\r\ncontent-type: application/json\r\n"]]
      + HEALTHY["dumpedHeaders"]},
     "ogc public (helsinki-bikes hsbikes): items of BikeHireDockingStation are 'application/json', not application/geo+json"),
    ({"bodies": [["hsbikes/ogc/features/collections/", '{"type":"FeatureCollection","features":[]}']] + HEALTHY["bodies"]},
     "ogc public (helsinki-bikes hsbikes): items of BikeHireDockingStation: no feature"),
    ({"bodies": [["hsbikes/ogc/features/collections/", "<html>502</html>"]] + HEALTHY["bodies"]},
     "ogc public (helsinki-bikes hsbikes): items of BikeHireDockingStation: no FeatureCollection"),
    ({"bodies": [["hsall/ogc/features/collections/", OGC_ITEMS.replace('"name"', '"contactPoint"')]] + HEALTHY["bodies"]},
     "ogc projection (helsinki-all hsall): items of BikeHireDockingStation: the hidden contactPoint is served"),
])
def test_an_ogc_row_that_is_not_a_feature_page_of_the_view_fails_the_run(tmp_path, change, reason):
    result = run(tmp_path, dict(HEALTHY, **change), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert f"FAIL  {reason}" in result.stdout


def test_an_ogc_row_reads_the_first_collection_that_has_a_feature(tmp_path):
    """The collections are the view's LinkML classes (T-3752): a class with no entity in the space
    comes first on dev (AirQualityObserved on helsinki-all), and the row reads the next one."""
    bodies = [["hsbikes/ogc/features/collections/AirQualityObserved/", '{"type":"FeatureCollection","features":[]}'],
              ["hsbikes/ogc/features/collections/", OGC_ITEMS],
              ["hsbikes/ogc/features/collections",
               '{"collections":[{"id":"AirQualityObserved"},{"id":"BikeHireDockingStation"}]}']]
    result = run(tmp_path, dict(HEALTHY, bodies=bodies + HEALTHY["bodies"]),
                 "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert ("ok    ogc public (helsinki-bikes hsbikes): conformance, 1 feature of BikeHireDockingStation "
            "as geo+json\n") in result.stdout


def test_an_ogc_kind_dev_has_not_seeded_is_a_skip_naming_it(tmp_path):
    result = run(tmp_path, dict(HEALTHY, ogcSeed={}, demoPassword=""),
                 "https://example.test", "https://idm.example.test")
    assert "skip  ogc internal (no demo user token to read helsinki-bikes-ops with)" in result.stdout
    result = run(tmp_path, dict(HEALTHY, ogcSeed={}), "https://example.test", "https://idm.example.test")
    assert "skip  ogc internal (helsinki-bikes-ops is not seeded in this instance)" in result.stdout
    assert "ok    ogc internal" not in result.stdout


def test_an_instance_without_the_broker_skips_the_surface_checks(tmp_path):
    result = run(tmp_path, dict(HEALTHY, broker=False), "https://example.test", "https://idm.example.test")
    assert "skip  broker surfaces (the context broker is not deployed in this instance)" in result.stdout
    assert "mounts the" not in result.stdout


def test_an_instance_without_jc_functions_skips_the_functions_checks(tmp_path):
    result = run(tmp_path, dict(HEALTHY, functions=False), "https://example.test", "https://idm.example.test")
    assert "skip  functions (jc-functions is not deployed in this instance)" in result.stdout
    assert "jc-functions refuses" not in result.stdout


def test_a_ckan_token_that_no_longer_authenticates_fails_the_run(tmp_path):
    """A token signed with a key the catalogue lost reads as anonymous and is refused (T-0493)."""
    refused = '{"success": false, "error": {"__type": "Authorization Error"}}'
    spec = dict(HEALTHY, bodies=[["api_token_list", refused]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  the CKAN API token in ckan-api-token no longer authenticates (T-0493)" in result.stdout


def test_wrong_status_fails_the_run(tmp_path):
    """An authenticated route that answers an anonymous call must turn the run red."""
    spec = dict(HEALTHY, statuses=[[["Bearer", "/api/v1/auth/me"], 200], ["/api/v1/auth/me", 200]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  portal API rejects an unauthenticated call (expected 401, got 200)" in result.stdout


def test_accepted_tampered_token_fails_the_run(tmp_path):
    """APISIX no longer verifies tokens, so the Portal accepting a forged signature is the
    regression this suite exists to catch."""
    statuses = [[["Bearer", ".forged"], 200]] + HEALTHY_STATUSES
    spec = dict(HEALTHY, statuses=statuses)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  portal API refuses a tampered token (expected 401, got 200)" in result.stdout


def test_login_redirect_with_another_callback_fails_the_run(tmp_path):
    """The redirect_uri the Portal sends must be the one the client lists, exactly: the
    old /auth/callback is the defect T-0270 fixed, and Keycloak refuses it with a 400 page
    no status check ever sees."""
    bodies = [["/api/v1/auth/login", LOGIN_REDIRECT.replace("%2Fapi%2Fv1%2Fauth%2Fcallback", "%2Fauth%2Fcallback")]]
    spec = dict(HEALTHY, bodies=bodies + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  portal login redirect is not keycloak with redirect_uri=" in result.stdout


# The edge checks need no endpoint: without the context-endpoint route the run skips the
# endpoint blocks and their polling deadlines, and the edge is judged in a second.
EDGE_ONLY = dict(HEALTHY, routes=[r for r in HEALTHY["routes"] if r != "context-endpoint"])


def test_the_edge_login_is_asserted_on_a_healthy_platform(tmp_path):
    """ADR-N-019: the Portal host sends an anonymous visitor to Keycloak with the `edge`
    client, the apex redirects to the Portal host, and a bearer call reaches the Portal
    through the edge on the new host."""
    result = run(tmp_path, EDGE_ONLY, "https://example.test", "https://idm.example.test")
    assert "ok    portal host sends an anonymous visitor to the edge login" in result.stdout
    assert "ok    apex redirects to the portal host" in result.stdout
    assert "ok    portal API answers a bearer call through the edge (200)" in result.stdout
    assert "ok    portal login redirects to keycloak with the callback the client lists" in result.stdout
    assert "0 failed" in result.stdout, result.stdout


def test_a_portal_host_without_the_edge_login_fails_the_run(tmp_path):
    """ADR-N-019: an anonymous visitor of the Portal host is sent to Keycloak by the edge. A
    Portal that answers the page itself (no plugin in front), or a redirect to Keycloak with
    another client or callback (which Keycloak refuses with a 400 nobody sees), is red."""
    for body in ("", EDGE_REDIRECT.replace("client_id=edge", "client_id=portal-ui"),
                 EDGE_REDIRECT.replace("%2Fcallback", "%2Fother")):
        bodies = [[["%{redirect_url}", "https://portal.example.test/"], body]] + HEALTHY["bodies"]
        result = run(tmp_path, dict(EDGE_ONLY, bodies=bodies), "https://example.test", "https://idm.example.test")
        assert result.returncode != 0
        assert "FAIL  portal host does not redirect to the edge login with client_id=edge" in result.stdout


def test_an_apex_that_does_not_redirect_to_the_portal_fails_the_run(tmp_path):
    bodies = [[["%{redirect_url}", "https://example.test/"], ""]] + HEALTHY["bodies"]
    result = run(tmp_path, dict(EDGE_ONLY, bodies=bodies), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  apex does not redirect to https://portal.example.test/ (got: none)" in result.stdout


def test_endpoint_round_trip_uses_the_seeded_slug_and_the_conformance_token(tmp_path):
    """T-0282: the slug is the seed's, not a literal, and the writes carry the token of the
    account the policy names — the Portal's token would be refused."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    client_credentials token for banskabystrica-conformance" in result.stdout
    assert "ok    entity create through the endpoint (201)" in result.stdout
    assert "0 failed, 2 skipped" in result.stdout


def test_every_listed_entity_is_retrieved_by_id(tmp_path):
    """T-0945: a healthy space serves what it lists, and the smoke says how many it read."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    every listed entity retrieves by id (2 of them)" in result.stdout


def test_the_round_trip_writes_in_the_conformance_space_and_never_in_public_air(tmp_path):
    """F-BNT1-002: ovzdusie holds the published EEA readings, so the smoke's creates and deletes
    go to the conformance space's own Endpoint; a write to public-air would answer 403 here."""
    air = "mluyob4nz52lok3ssk7pgn5vwt"
    spec = dict(HEALTHY, statuses=[[["-X POST", air], 403], [["-X DELETE", air], 403]] + HEALTHY_STATUSES)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    entity create through the endpoint (201)" in result.stdout
    assert "ok    entity delete (204)" in result.stdout
    assert "0 failed" in result.stdout, result.stdout


def test_an_entity_that_lists_but_never_retrieves_fails_the_run(tmp_path):
    """The drift T-0945 is about: the broker holds a row the gateway refuses on retrieve, so
    nobody can read or correct it. The smoke names the id, and fails."""
    spec = dict(HEALTHY, statuses=[["entities/urn:ngsi-ld:AirQualityObserved", 400]] + HEALTHY_STATUSES)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  listed but not retrievable" in result.stdout
    assert "urn:ngsi-ld:AirQualityObserved:example.test:ovzdusie:1(400)" in result.stdout


def test_the_public_reads_history_and_a_type_without_one_answers_404(tmp_path):
    """T-4372: an anonymous temporal query on public-air answers from the space's history, and
    the events endpoint, whose type keeps none, grants no history read."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    an anonymous visitor reads the air history of public-air (200)" in result.stdout
    assert "ok    helsinki-events serves no history (Event keeps none) (404)" in result.stdout


@pytest.mark.parametrize("statuses, failure", [
    ([[["ngsi-ld/v1/temporal/entities", "AirQualityObserved"], 404]],
     "FAIL  an anonymous visitor reads the air history of public-air (expected 200, got 404)"),
    ([[["hsevents", "/temporal/entities"], 200]],
     "FAIL  helsinki-events serves no history (Event keeps none) (expected 404, got 200)"),
])
def test_a_public_history_that_is_refused_or_one_served_without_history_fails(tmp_path, statuses, failure):
    """The 404 Claude.ai met on every public history read (T-4372), and its opposite."""
    result = run(tmp_path, dict(HEALTHY, statuses=statuses + HEALTHY_STATUSES), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert failure in result.stdout


def test_a_space_holding_nothing_skips_the_retrieval(tmp_path):
    """An empty space is not a broken one: skip, never a silent pass."""
    spec = dict(HEALTHY, bodies=[["entities?type=AirQualityObserved", ""]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "skip  every listed entity retrieves by id (the space holds none)" in result.stdout


def test_unseeded_gateway_skips_the_round_trip(tmp_path):
    """An instance whose gateway seeds no Endpoint has nothing to round-trip: skip, never pass."""
    spec = dict(HEALTHY, seedEndpoint="")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "skip  endpoint round trip (no Endpoint seeded" in result.stdout
    assert "skip  ogc star (public-air is not seeded in this instance)" in result.stdout
    assert "0 failed, 4 skipped" in result.stdout


def test_endpoint_not_found_fails_the_run(tmp_path):
    """The 404 that hid behind a slug the gateway could never serve is a failure, not a skip."""
    spec = dict(HEALTHY, statuses=[["/entities", 404]] + HEALTHY_STATUSES)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  entity create through the endpoint (expected 201, got 404)" in result.stdout


def test_refused_demo_login_fails_the_run(tmp_path):
    """T-0248: a seeded demo user Keycloak refuses (wrong password, required action, policy)
    is a red run, because DEMO.md step 1 starts with exactly this login."""
    # The script reads the token out of the body, so a refusal is a body without one.
    spec = dict(HEALTHY, bodies=[["grant_type=password", '{"error":"invalid_grant"}']] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  demo user demo.viewer@hel.fi cannot log in with a password grant" in result.stdout


def test_instance_without_demo_users_skips_the_login(tmp_path):
    """Production seeds no demo users: the login is skipped, never passed."""
    spec = dict(HEALTHY, demoPassword="")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "skip  demo user login (no demo users seeded" in result.stdout
    assert "skip  portal space list (no demo user token)" in result.stdout
    assert "skip  sample apps (no demo user token to read them with)" in result.stdout
    assert "skip  App routes (no demo user token to list the published Apps with)" in result.stdout
    assert "skip  ogc internal (no demo user token to read helsinki-bikes-ops with)" in result.stdout
    assert "0 failed, 7 skipped" in result.stdout


def test_absent_routes_skip_instead_of_passing(tmp_path):
    """A component this instance does not deploy must be reported as skipped, never as a pass."""
    spec = dict(HEALTHY, routes=["keycloak"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "skip  portal API (route portal-api not configured" in result.stdout
    assert "skip  endpoint round trip (route context-endpoint not configured" in result.stdout
    assert "skip  portal host (route portal-ui not configured" in result.stdout
    assert "skip  helsinki endpoints (route context-endpoint not configured" in result.stdout
    assert "skip  git forge (route gitea-forge not configured" in result.stdout
    assert "skip  catalogue (route ckan not configured" in result.stdout
    assert "skip  chunked body (route portal-api not configured" in result.stdout
    assert "skip  App routes (route portal-ui not configured" in result.stdout
    assert "skip  ogc api features (route context-endpoint not configured" in result.stdout
    assert "skip  api host (route context-mcp-hub not configured" in result.stdout
    assert "11 skipped" in result.stdout


def test_an_app_old_path_that_serves_or_sets_a_cookie_fails_the_run(tmp_path):
    """AP-133, AP-135 (T-2839): the apex answers an App's old path with a 308 to the App's own
    host and nothing else; serving it there, or setting a cookie on the way, is red."""
    for moved, said in [
        ("HTTP/2 200\r\ncontent-type: text/html\r\n", "HTTP/2 200"),
        ("HTTP/2 308\r\nlocation: https://example.test/apps/air-quality/\r\n", "HTTP/2 308"),
        ("HTTP/2 308\r\nlocation: https://air-quality.apps.example.test/\r\nset-cookie: jc_app=1\r\n", "HTTP/2 308"),
    ]:
        spec = dict(HEALTHY, dumpedHeaders=[["/apps/air-quality/", moved]] + HEALTHY["dumpedHeaders"])
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert result.returncode == 1, moved
        assert f"FAIL  App air-quality's old path does not move to its host without a cookie (got: {said})" in result.stdout
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    App hsl-transport's old path answers 308 to https://hsl-transport.apps.example.test/ with no cookie" in result.stdout


def test_a_public_app_whose_policy_shuts_out_the_basemap_fails_the_run(tmp_path):
    """AP-12, AP-67 (T-3014): a public App's map draws on the Portal's basemap route, so its page
    policy lists that route in img-src and connect-src; a page with no policy, or one that lists
    the route in one directive only, is a blank map and red."""
    only_img = APP_POLICY.replace(f"connect-src 'self' {BASEMAP}", "connect-src 'self'")
    for headers, said in [
        ("HTTP/2 200\r\ncontent-security-policy: frame-ancestors 'self' portal.example.test\r\n",
         "frame-ancestors 'self' portal.example.test"),
        ("HTTP/2 200\r\n" + only_img, only_img.split(": ", 1)[1].strip()),
        ("HTTP/2 200\r\n", "none"),
    ]:
        spec = dict(HEALTHY, dumpedHeaders=[["hsl-transport.apps.example.test/", headers]] + HEALTHY["dumpedHeaders"])
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert result.returncode == 1, headers
        assert ("FAIL  public App hsl-transport's Content-Security-Policy does not list "
                f"{BASEMAP} in img-src and connect-src, so its map draws no tiles (got: {said})") in result.stdout
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert f"ok    public App hsl-transport's policy lets the basemap tiles in ({BASEMAP})" in result.stdout


def test_a_public_app_host_that_answers_once_the_lane_republished_is_green(tmp_path):
    """T-3424: an apply that re-vendored an App leaves its host 404 until the lane republishes the
    build; the host check waits for it within the sample apps' bound and then passes."""
    spec = dict(HEALTHY, statusSequences=[["hsl-transport.apps.example.test/", [404, 404, 200]]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test", extra_env={"JC_SMOKE_POLL": "0"})
    assert "ok    public App hsl-transport answers an anonymous visitor on its own host (200)" in result.stdout
    assert "FAIL  public App hsl-transport" not in result.stdout


def test_a_429_from_the_shared_edge_budget_is_waited_out_and_asked_again(tmp_path):
    """T-4188: a 429 is the per-IP budget every agent on the address spends, not a check's answer;
    the check waits out the window the edge names and keeps the answer it asks for."""
    spec = dict(HEALTHY, rateLimitReset=0,
                statusSequences=[["/.well-known/openid-configuration", [429, 200]]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    keycloak OIDC discovery over valid TLS (200)" in result.stdout
    assert "FAIL  keycloak OIDC discovery" not in result.stdout


def test_a_429_past_three_windows_fails_the_check(tmp_path):
    """T-4188: the wait is bounded; an edge still refusing after three windows is red."""
    spec = dict(HEALTHY, rateLimitReset=0,
                statusSequences=[["/.well-known/openid-configuration", [429]]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "FAIL  keycloak OIDC discovery over valid TLS (expected 200, got 429)" in result.stdout


def test_a_public_app_host_still_missing_past_the_bound_fails_the_run(tmp_path):
    """T-3424: the wait is bounded; a host still 404 past it is red, as before."""
    spec = dict(HEALTHY, statusSequences=[["hsl-transport.apps.example.test/", [404]]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test",
                 extra_env={"JC_SMOKE_POLL": "0", "JC_SMOKE_APP_WAIT": "1"})
    assert result.returncode == 1
    assert "FAIL  public App hsl-transport answers an anonymous visitor on its own host (expected 200, got 404)" in result.stdout


def test_an_instance_without_a_basemap_does_not_ask_for_it_in_a_policy(tmp_path):
    """AP-67: the basemap is optional; where the Portal proxies none, no App policy names it."""
    spec = dict(HEALTHY, statuses=[["basemap/default/style.json", 404]] + HEALTHY["statuses"],
                dumpedHeaders=[["hsl-transport.apps.example.test/", "HTTP/2 200\r\n"]] + HEALTHY["dumpedHeaders"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "basemap" not in "\n".join(line for line in result.stdout.splitlines() if "hsl-transport" in line)
    assert "FAIL  public App hsl-transport" not in result.stdout


def test_an_unbranded_catalogue_fails_the_run(tmp_path):
    """The whole claim of OPS-46/OPS-47 is that the catalogue a visitor sees is the
    instance's own. A CKAN serving the stock theme answers 200 all the same, so only the
    page content can catch a theme that did not load."""
    spec = dict(HEALTHY, bodies=[["data.example.test/", "<html><body>CKAN</body></html>"]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  catalogue front page does not carry the instance name" in result.stdout
    assert "FAIL  catalogue front page does not carry the branded primary colour" in result.stdout


def test_reachable_database_fails_the_run(tmp_path):
    """The NetworkPolicy check is inverted: the probe pod connecting is the failure."""
    spec = dict(HEALTHY, netpolReachable=True)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  an unlabelled pod reached postgres-cluster-rw:5432" in result.stdout


def test_reachable_coredns_fails_the_run(tmp_path):
    """T-0454: default-deny is Ingress and Egress, and a controller enforcing only ingress
    passes every other check in this suite. CoreDNS is the leg that discriminates: it is up
    and it is reachable the moment the egress half stops being enforced."""
    spec = dict(HEALTHY, egressReached=["DNS"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  an unlabelled pod reached CoreDNS" in result.stdout


def test_reachable_internet_fails_the_run(tmp_path):
    """The other leg, and the one a break-in actually uses."""
    spec = dict(HEALTHY, egressReached=["NET"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  an unlabelled pod reached 1.1.1.1:443" in result.stdout


def test_both_egress_legs_are_named_when_both_reach(tmp_path):
    spec = dict(HEALTHY, egressReached=["DNS", "NET"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  an unlabelled pod reached CoreDNS 1.1.1.1:443" in result.stdout


def test_an_egress_probe_that_never_ran_fails_the_run(tmp_path):
    """A pod that never started produces exactly the same silence as a pod the policy
    stopped. Without this the check would go green the day the image or the namespace
    changes, which is the failure mode this whole task exists to prevent."""
    spec = dict(HEALTHY, egressProbeRan=False)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the egress probe never ran" in result.stdout


def test_a_pod_that_reaches_the_edge_data_plane_fails_the_run(tmp_path):
    """EP-20, T-0939: APISIX trusts X-Real-IP from the pod network, so a pod other than the
    ingress controller that reaches 9080 can name itself any caller and spend their quota."""
    spec = dict(HEALTHY, edgePeerReached=["9080"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  a pod outside the ingress controller reached APISIX on port 9080 —" in result.stdout


def test_a_pod_that_reaches_the_mesh_port_of_the_edge_fails_the_run(tmp_path):
    """EP-20, T-0939: 4143 is where meshed traffic lands; narrowing 9080 alone leaves it open."""
    spec = dict(HEALTHY, edgePeerReached=["9080", "4143"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "reached APISIX on port 9080 4143 —" in result.stdout


def test_an_edge_peer_probe_that_never_ran_fails_the_run(tmp_path):
    """EP-20: a probe that never ran is silent exactly like a refused one."""
    spec = dict(HEALTHY, edgePeerRan=False)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the edge-peer probe never ran" in result.stdout


def test_no_apisix_pod_to_probe_fails_the_run(tmp_path):
    """EP-20: with no APISIX pod address the probe has nothing to aim at, which is no verdict."""
    spec = dict(HEALTHY, apisixPodIp="")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  no APISIX pod in" in result.stdout


def test_an_edge_that_admits_only_the_ingress_controller_passes(tmp_path):
    """EP-20: the green line names both ports it measured."""
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "ok    APISIX refuses a pod that is not the ingress controller on 9080 and 4143" in result.stdout


def test_an_enforced_egress_policy_passes(tmp_path):
    spec = dict(HEALTHY)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "ok    an unlabelled pod reaches neither CoreDNS nor the internet" in result.stdout


def test_the_pass_line_reports_how_long_the_controller_took(tmp_path):
    """T-0459: the controller programs a NEW pod's egress chains a few seconds after the pod
    is running, so a probe that calls out at container start measures that gap and not the
    policy. The probe settles first and prints how long it waited; the number is on the pass
    line so a window that grows is visible instead of silent."""
    spec = dict(HEALTHY, egressSettled=8)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "(settled after 8s, ceiling 30s)" in result.stdout


def test_a_pod_filtered_from_its_first_packet_settles_at_zero(tmp_path):
    """What a plugin that programs policy before the pod joins the network would produce, and
    the reading that says the window has been closed rather than merely waited out."""
    spec = dict(HEALTHY, egressSettled=0)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "(settled after 0s, ceiling 30s)" in result.stdout


def test_a_window_that_never_closes_still_fails_the_run(tmp_path):
    """The settle loop is bounded, so a pod that is never filtered runs the measurement anyway
    and fails on it. Waiting forever for a refusal that is not coming would turn the check into
    a hang, which reads as a broken suite rather than as an open control."""
    spec = dict(HEALTHY, egressSettled=72, egressReached=["NET"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  an unlabelled pod reached 1.1.1.1:443" in result.stdout


def test_a_settle_window_inside_the_ceiling_passes_and_prints_both_numbers(tmp_path):
    """SEC-GAP-05 (docs/Deployment/08, "The startup window"): the ceiling is what makes the gap an
    accepted one rather than an open one, so
    the pass line carries the measured window and the ceiling it was held to. A reader of the
    smoke output can tell a window that is closing from one that is creeping towards the bound."""
    spec = dict(HEALTHY, egressSettled=24)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0
    assert "ok    an unlabelled pod reaches neither CoreDNS nor the internet (settled after 24s, ceiling 30s)" in result.stdout


def test_a_settle_window_over_the_ceiling_fails_the_run(tmp_path):
    """SEC-GAP-05: the gap is bounded by this check and by nothing else. A controller that has got
    slower must fail the run, naming the measured seconds and the allowed seconds, instead of
    printing a larger number on a green line."""
    spec = dict(HEALTHY, egressSettled=44)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the egress settle window was 44s, over the 30s ceiling (JC_NETPOL_SETTLE)" in result.stdout


def test_the_ceiling_defaults_to_thirty_seconds_and_the_variable_moves_it(tmp_path):
    """The default is the documented one (docs/Deployment/08) and it is read from the
    environment, so a cluster with a slower controller is a deliberate decision with a value
    beside it rather than an edit to the script."""
    spec = dict(HEALTHY, egressSettled=44)
    default = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert default.returncode == 1
    assert "over the 30s ceiling" in default.stdout
    raised = run(tmp_path, spec, "https://example.test", "https://idm.example.test",
                 extra_env={"JC_NETPOL_SETTLE": "60"})
    assert raised.returncode == 0
    assert "(settled after 44s, ceiling 60s)" in raised.stdout
    # An empty value is an unset one, the way every other knob of this script reads its
    # environment: a pipeline that passes JC_NETPOL_SETTLE="$SOMETHING" with nothing behind it
    # gets the default and not a refusal.
    empty = run(tmp_path, spec, "https://example.test", "https://idm.example.test",
                extra_env={"JC_NETPOL_SETTLE": ""})
    assert empty.returncode == 1
    assert "over the 30s ceiling" in empty.stdout


@pytest.mark.parametrize("value", ["thirty", "30s", "-5", "2.5", " 30"])
def test_a_ceiling_that_is_not_a_whole_number_of_seconds_is_refused(tmp_path, value):
    """An unparsable value must not read as zero, as infinity, or as the default: either of the
    first two silently inverts the check, and the third hides a typo in a deployment pipeline."""
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test",
                 extra_env={"JC_NETPOL_SETTLE": value})
    assert result.returncode == 2
    assert "JC_NETPOL_SETTLE must be a whole number of seconds" in result.stderr
    assert "settled after" not in result.stdout


def test_a_ceiling_the_probe_could_never_exceed_is_refused(tmp_path):
    """The probe waits at most 18 rounds of 4 seconds, so a ceiling at or above 72 seconds can
    never be exceeded: the check would report a pass for every window there is. Refusing the
    value keeps "raise the number until it is green" from being a way to switch the bound off."""
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test",
                 extra_env={"JC_NETPOL_SETTLE": "72"})
    assert result.returncode == 2
    assert "is not below the probe own bound of 72s" in result.stderr


def test_the_probe_bound_in_the_script_matches_the_one_the_ceiling_is_checked_against(tmp_path):
    """The probe's bound lives in the pod's own shell command and the ceiling check compares
    against a constant, so the two can drift apart. Then a ceiling between the real bound and
    the constant would pass validation and never bite."""
    script = SMOKE.read_text()
    rounds = int(re.search(r"i=0; while \[ \$i -lt (\d+) \]", script).group(1))
    sleep = int(re.search(r"i=\$\(\(i\+1\)\); sleep (\d+);", script).group(1))
    constant = int(re.search(r"^settle_probe_seconds=(\d+)$", script, re.M).group(1))
    assert rounds * sleep == constant


def test_a_probe_that_reports_no_window_fails_the_run(tmp_path):
    """The window is the measurement this check exists for. A probe that reached nothing and
    printed no number proved the policy and not the bound, and an empty measurement compared
    with the ceiling would pass by accident."""
    spec = dict(HEALTHY, egressSettled=None)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the egress probe printed no settle window" in result.stdout


def test_no_password_or_client_secret_rides_on_a_command_line(tmp_path):
    # F-R2DEP3-003: the Keycloak master admin password and the client secrets went in curl argv,
    # readable in /proc by any process on the smoke host (prod-smoke runs this too).
    argv = tmp_path / "argv.log"
    secrets = {"JC_SMOKE_ADMIN_PASSWORD": "adm1n-pass-value", "JC_SMOKE_CLIENT_SECRET": "client-secret-value",
               "JC_SMOKE_DEMO_PASSWORD": "demo-pass-value"}
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test",
                 extra_env={**secrets, "STUB_ARGV": str(argv)})
    logged = argv.read_text()
    assert "grant_type=password" in logged, "the run reached the password grants"
    for value in secrets.values():
        assert value not in logged, f"{value} rode on a curl command line"
    assert "ok    client_credentials token" in result.stdout


def test_missing_client_secret_fails_the_run(tmp_path):
    """No token means the suite proved nothing about authentication; it must not pass."""
    spec = dict(HEALTHY, clientSecret="")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  no client secret for apisix-gateway" in result.stdout


def test_missing_security_header_fails_the_run(tmp_path):
    """A route that stops emitting HSTS is a silent regression; the suite must catch it."""
    spec = dict(HEALTHY, defaultHeaders="HTTP/2 200\r\nserver: APISIX\r\nx-frame-options: DENY\r\n")
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  response headers missing: strict-transport-security" in result.stdout


def test_legacy_tls_and_ciphers_fail_the_run(tmp_path):
    """BSI TR-02102 is asserted against the live listener, so both halves must be able to fail."""
    spec = dict(HEALTHY, tls11=0, cbcRefused=False)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the edge negotiated TLS 1.1" in result.stdout
    assert "FAIL  the edge negotiated a CBC or 3DES suite" in result.stdout


def test_an_instance_without_the_store_skips_the_section(tmp_path):
    """A component this instance does not deploy is skipped, never passed (T-0925)."""
    result = run(tmp_path, {**HEALTHY, "artifactStore": False}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "skip  artifact store (the component is not deployed in this instance)" in result.stdout


def test_a_store_no_organization_can_read_fails(tmp_path):
    """The reconciler minting nothing, or handing nothing over, is what T-0933 looked like from
    the outside: the store runs and no serving namespace can read it."""
    result = run(tmp_path, {**HEALTHY, "artifactReaders": []}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  no organization has a reader credential" in result.stdout


def test_a_reader_missing_a_key_fails(tmp_path):
    """Half a credential is a pod that starts and cannot authenticate."""
    spec = {**HEALTHY, "artifactReaderKeys": {"artifact-store-reader-helsinki": ["ACCESS_KEY_ID"]}}
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  a reader Secret is missing a key: artifact-store-reader-helsinki/ACCESS_SECRET_KEY" in result.stdout


def test_a_writer_credential_in_the_cluster_fails(tmp_path):
    """The writer publishes. A Secret carrying it hands a serving pod the power to replace an
    artifact, which is the one thing the two roles exist to keep apart (PF-32)."""
    spec = {**HEALTHY, "artifactWriters": ["artifact-store-writer-helsinki"]}
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  a writer credential was written into the cluster:artifact-store-writer-helsinki" in result.stdout


def test_an_edge_that_logs_a_pod_address_fails(tmp_path):
    """Every caller arriving as the ingress pod is one rate-limit bucket for the internet."""
    result = run(tmp_path, {**HEALTHY, "edgeCaller": "10.42.0.125"}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  the edge logged the caller as 10.42.0.125, a pod address" in result.stdout


def test_an_instance_without_the_edge_skips_the_caller_check(tmp_path):
    result = run(tmp_path, {**HEALTHY, "edge": False}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "skip  edge caller (no APISIX in this instance)" in result.stdout


def test_a_request_that_never_reached_the_access_log_skips(tmp_path):
    """A log the smoke cannot read is not a passing check."""
    result = run(tmp_path, {**HEALTHY, "edgeCaller": ""}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "skip  edge caller (the request did not reach the edge's access log)" in result.stdout


def test_an_instance_where_no_pipeline_asks_for_a_credential_skips(tmp_path):
    """No `pipeline-secrets` Secret is the installation whose pipelines name no credential,
    not a broken resolver (T-0935)."""
    result = run(tmp_path, {**HEALTHY, "pipelineSecrets": None}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "skip  pipeline credentials (no pipeline on this instance declares a secretRef)" in result.stdout


def test_a_credential_the_runner_never_receives_fails_the_run(tmp_path):
    """The Secret written and the process not carrying it is a stream that reads an empty
    password and a broker that refuses it — with every manifest looking correct."""
    spec = {**HEALTHY, "runnerEnvMissing": ["DEMO_FEED_PASSWORD"]}
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  the runner's environment is missing a resolved credential: DEMO_FEED_PASSWORD" in result.stdout


def test_a_credential_copied_into_a_configmap_fails_the_run(tmp_path):
    """A credential in a ConfigMap is a credential in `kubectl get -o yaml`, which is the one
    thing the whole resolution path exists to avoid (CC-06)."""
    spec = {**HEALTHY, "leakedCredential": "a-credential"}
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  a resolved credential is written in plaintext into a ConfigMap: DEMO_FEED_PASSWORD" in result.stdout


def test_a_resolver_that_refused_everything_fails_the_run(tmp_path):
    """An empty Secret is every reference refused: the runner has nothing, and the pipelines
    that named a credential are not running."""
    result = run(tmp_path, {**HEALTHY, "pipelineSecrets": {}}, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1, result.stdout
    assert "FAIL  pipeline-secrets is empty" in result.stdout


def test_a_runner_that_reaches_the_internet_fails_the_run(tmp_path):
    """AP-81: the Actions runner runs untrusted build code with egress to the forge, the Portal
    API and DNS only (ADR-N-028)."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    a runner pod resolves names and reaches no internet address" in result.stdout
    result = run(tmp_path, dict(HEALTHY, runnerReached=["DNS", "NET"]), "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  a pod with the runner's label reached 1.1.1.1:443" in result.stdout


def test_a_runner_probe_that_measured_nothing_fails_the_run(tmp_path):
    """AP-81: a probe that never ran, or could not even resolve a name, proves no refusal."""
    for spec, line in [
        (dict(HEALTHY, runnerProbeRan=False), "FAIL  the runner probe never ran"),
        (dict(HEALTHY, runnerReached=[]), "FAIL  a pod with the runner's label cannot resolve names"),
    ]:
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert result.returncode != 0
        assert line in result.stdout


def test_an_instance_without_the_runner_skips_its_checks(tmp_path):
    result = run(tmp_path, dict(HEALTHY, runnerNamespace=""), "https://example.test", "https://idm.example.test")
    assert "skip  actions runner egress (no gitea-runner in this instance)" in result.stdout
    assert "skip  actions runner (no gitea-runner in this instance)" in result.stdout


def test_a_runner_the_forge_does_not_show_online_fails_the_run(tmp_path):
    """ADR-N-028 §3.3: a runner Deployment whose runner never came online builds nothing."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    an Actions runner is registered in joinedcontext and online (waited 0s)" in result.stdout
    offline = dict(HEALTHY, bodies=[["actions/runners", '{"runners":[{"name":"r-1","status":"offline"}]}']]
                   + HEALTHY["bodies"])
    result = run(tmp_path, offline, "https://example.test", "https://idm.example.test",
                 extra_env={"JC_SMOKE_RUNNER_WAIT": "0"})
    assert result.returncode != 0
    assert "FAIL  no Actions runner of joinedcontext is online after 0s" in result.stdout


def test_the_runner_is_looked_for_in_the_organization_that_holds_the_apps(tmp_path):
    """T-2969: with `appsOrganization` the runners register in that organization, which the
    bootstrap records beside their token; the configuration's organization is the fallback."""
    result = run(tmp_path, dict(HEALTHY, runnerOwner="joinedcontext-apps"), "https://example.test", "https://idm.example.test")
    assert "ok    an Actions runner is registered in joinedcontext-apps and online (waited 0s)" in result.stdout


def test_an_app_pod_reached_only_from_apisix_passes(tmp_path):
    """AP-26, AP-108: the positive control and the refusal, with how many App pods ran."""
    result = run(tmp_path, dict(HEALTHY, appPods=["10.42.0.20", "10.42.0.21"]), "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "ok    a pod with APISIX's label reaches an App pod on 8080 (first of 2)" in result.stdout
    assert "ok    an App pod refuses a pod that is not APISIX on 8080 and 4143" in result.stdout


def test_an_app_pod_open_to_any_pod_fails_the_run(tmp_path):
    """AP-26: a pod that reaches the App past the edge skips its login front."""
    result = run(tmp_path, dict(HEALTHY, appPeerReached=["8080", "4143"]), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  a pod outside APISIX reached the App pod at 10.42.0.20 on port 8080 4143 —" in result.stdout


def test_an_app_pod_apisix_cannot_reach_fails_the_run(tmp_path):
    """AP-108: an App the edge cannot reach serves nobody, and makes the refusal meaningless."""
    result = run(tmp_path, dict(HEALTHY, appEdgeReaches=False), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  a pod with APISIX's label cannot reach the App pod at 10.42.0.20:8080" in result.stdout


def test_the_probe_with_apisix_labels_never_serves_apisix_traffic(tmp_path):
    """T-6594: APISIX's labels are Service apisix-gateway's selector, and a Ready pod carrying them is
    one of its endpoints: Traefik sent visitors to the busybox probe (502 on / at 08:06Z). Only a pod
    that never turns Ready stays out of the Service while the NetworkPolicies still select it."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    a pod with APISIX's label reaches an App pod on 8080 (first of 1)" in result.stdout
    pods = list(tmp_path.glob("*.app-edge-pod"))
    assert len(pods) == 1, pods
    container = json.loads(pods[0].read_text())["spec"]["containers"][0]
    assert container.get("readinessProbe", {}).get("exec", {}).get("command") == ["false"], container


def test_an_app_peer_probe_that_never_ran_fails_the_run(tmp_path):
    result = run(tmp_path, dict(HEALTHY, appPeerRan=False), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the App-peer probe never ran" in result.stdout


def test_an_instance_without_app_pods_skips_with_the_count(tmp_path):
    result = run(tmp_path, dict(HEALTHY, appPods=[]), "https://example.test", "https://idm.example.test")
    assert "skip  App pods (0 pod-backed Apps run in this instance)" in result.stdout
    assert "reaches an App pod" not in result.stdout


def test_every_seeded_sample_app_is_built_and_the_wait_is_said(tmp_path):
    """AP-75, AP-80 (T-2599): each seeded app reads as built at its commit, with the seconds waited."""
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    for app in ("helsinki-bikes", "helsinki-events", "helsinki-alerts"):
        assert re.search(rf"ok    sample app {app} is built at aaaaaaaaaaaa after [0-9]+ s", result.stdout)


def test_a_sample_app_the_lane_never_built_fails_the_run(tmp_path):
    """AP-80: no status.build within the wait is a failure naming the app, not a skip."""
    spec = dict(HEALTHY, bodies=[b for b in HEALTHY["bodies"] if b[1] != BUILT_APP])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test", extra_env={"JC_SMOKE_APP_WAIT": "0"})
    assert result.returncode == 1
    assert "FAIL  sample app helsinki-alerts has no status.build after 0 s" in result.stdout


def test_an_instance_without_sample_apps_skips_them(tmp_path):
    result = run(tmp_path, dict(HEALTHY, sampleApps=[]), "https://example.test", "https://idm.example.test")
    assert "skip  sample apps (none is seeded in this instance)" in result.stdout


def test_a_published_app_whose_pod_is_ready_passes(tmp_path):
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test")
    assert "ok    App deployment dev/app-air-quality has 1 of 1 pods ready" in result.stdout


def test_a_published_app_with_no_ready_pod_fails_the_run(tmp_path):
    """T-2665: an image the node cannot pull left two Apps in ImagePullBackOff while smoke was green."""
    spec = dict(HEALTHY, appDeployments=[["dev/app-air-quality", 1, 1], ["dev/app-hsl-transport", None, 1]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  App deployment dev/app-hsl-transport has 0 of 1 pods ready" in result.stdout


def test_an_instance_without_pod_backed_apps_skips_the_readiness(tmp_path):
    result = run(tmp_path, dict(HEALTHY, appDeployments=[]), "https://example.test", "https://idm.example.test")
    assert "skip  App deployments (no pod-backed App is published in this instance)" in result.stdout


def test_the_stub_admits_the_edge_probe_by_the_selector_the_edge_policy_really_has():
    """T-2667: the smoke's APISIX-labelled probe was refused on dev because it carried the name
    label alone while the edge's egress policy selects name and instance; the stub holds the same
    selector as the chart, so the probe's labels are checked against what the cluster applies."""
    import yaml

    policies = yaml.safe_load((PROJECT_ROOT / "components/apisix/networkpolicies.yaml").read_text())
    for name, value in policies["apisix"]["podSelector"].items():
        assert f'("{name}", "{value}")' in KUBECTL_STUB, name


def test_the_runners_walls_are_read_from_the_running_pod(tmp_path):
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test")
    assert "ok    the runner runs as uid 1000 and holds no Kubernetes token" in result.stdout


@pytest.mark.parametrize(
    ("walls", "said"),
    [
        ({"runnerUid": 0}, "FAIL  the runner runs as uid 0: an application's build runs as root"),
        ({"runnerSaToken": True}, "FAIL  the runner holds a Kubernetes ServiceAccount token"),
        ({"runnerWallsRead": False}, "FAIL  the runner could not be asked for its uid and token"),
    ],
)
def test_a_runner_as_root_or_with_a_token_fails_the_run(tmp_path, walls, said):
    """T-1707 steps 2 and 4 (AP-81): the untrusted build runs as the runner's uid and next to its
    filesystem; root, a mounted token, or a runner nobody could ask is a failure, never a skip."""
    result = run(tmp_path, dict(HEALTHY, **walls), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert said in result.stdout


def test_every_published_app_answers_on_its_own_route_at_the_edge(tmp_path):
    """T-2668, AP-28, AP-29: a public App serves an anonymous visitor, any other sends the
    visitor to the login of its own client; a draft App is not probed."""
    result = run(tmp_path, dict(HEALTHY), "https://example.test", "https://idm.example.test")
    assert result.returncode == 0, result.stdout
    assert "ok    public App hsl-transport answers an anonymous visitor on its own host (200)" in result.stdout
    assert "ok    internal App air-quality sends an anonymous visitor to its own login (app-air-quality)" in result.stdout
    assert "sketch" not in result.stdout


def test_an_app_that_falls_through_to_the_shared_surface_fails_the_run(tmp_path):
    """The shared `edge` client's login means the Portal wrote no route for the App (T-2668)."""
    fallback = APP_LOGIN.replace("client_id=app-air-quality", "client_id=edge")
    spec = dict(HEALTHY, bodies=[[["%{redirect_url}", "https://air-quality.apps.example.test/"], fallback], *HEALTHY["bodies"]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  internal App air-quality has no route of its own at the edge" in result.stdout


def test_a_client_id_that_only_starts_with_the_app_name_fails_the_run(tmp_path):
    """`app-air-quality-2` is another App's client, not this one's."""
    other = APP_LOGIN.replace("client_id=app-air-quality&", "client_id=app-air-quality-2&")
    spec = dict(HEALTHY, bodies=[[["%{redirect_url}", "https://air-quality.apps.example.test/"], other], *HEALTHY["bodies"]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  internal App air-quality has no route of its own at the edge" in result.stdout


def test_a_public_app_that_asks_for_a_login_fails_the_run(tmp_path):
    """AP-28: the measured defect of T-2668, a public App answering 302 to Keycloak."""
    spec = dict(HEALTHY, statuses=[["https://hsl-transport.apps.example.test/", 302], *HEALTHY["statuses"]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  public App hsl-transport answers an anonymous visitor on its own host (expected 200, got 302)" in result.stdout


def test_no_published_app_skips_the_app_routes(tmp_path):
    spec = dict(HEALTHY, bodies=[["/api/v1/projects/helsinki/apps", '{"items": []}'] if b[0] == "/api/v1/projects/helsinki/apps" else b for b in HEALTHY["bodies"]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "skip  App routes (no App is published in helsinki)" in result.stdout


def test_the_steward_grant_answers_totp_from_its_secret(tmp_path):
    """T-5120: demo.steward holds mfa-required, so Keycloak gives its token only with a TOTP code;
    the smoke computes one from the seed in Secret keycloak-user-demo-steward and sends it."""
    bodies = [[["username=demo.steward", "totp="], '{"access_token":"steward.otp.sig"}'],
              [["/api/v1/organization/people", "Bearer steward.otp.sig"], DEMO_PEOPLE],
              ["/api/v1/organization/people", '{"items":[]}']]
    spec = dict(HEALTHY, demoTotp="JourneySeed-0123456789abcdefghij", bodies=bodies + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    People lists the demo people" in result.stdout, result.stdout
    assert "JourneySeed" not in result.stdout + result.stderr
    # Without the seed the grant carries no code, Keycloak refuses it, and People goes red.
    spec = dict(HEALTHY, bodies=bodies + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "FAIL  People does not list:" in result.stdout


def test_a_demo_person_missing_from_people_fails_the_run(tmp_path):
    """T-2688: DEMO.md walks Organization → People; a demo person it does not list is red."""
    listed = json.dumps({"items": [{"email": "demo.steward@hel.fi"}, {"email": "demo.viewer@hel.fi"}]})
    spec = dict(HEALTHY, bodies=[["/api/v1/organization/people", listed]] + HEALTHY["bodies"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  People does not list: demo.approver@hel.fi" in result.stdout


def _teams(*extra):
    """The forge's teams as HEALTHY has them, plus `extra` (PF-80, PF-87)."""
    return json.dumps([
        {"id": 1, "name": "Owners", "permission": "owner", "includes_all_repositories": True,
         "units_map": {"repo.code": "owner", "repo.issues": "owner"}},
        {"id": 2, "name": "readers", "permission": "none", "includes_all_repositories": True,
         "units_map": {"repo.code": "read", "repo.issues": "read", "repo.pulls": "read"}},
        *extra,
    ], separators=(",", ":"))


WRITERS = {"id": 12, "name": "helsinki-writers", "permission": "none", "includes_all_repositories": False,
           "units_map": {"repo.code": "write", "repo.pulls": "write"}}


def _with_teams(teams, *bodies):
    spec = dict(HEALTHY)
    spec["bodies"] = [["/api/v1/orgs/joinedcontext/teams", teams], *bodies, *HEALTHY["bodies"]]
    return spec


def test_a_project_writers_team_on_its_own_repository_keeps_merging_the_portals(tmp_path):
    """Layout 2 (PF-87): `{slug}-writers` writes its project repository; that is not PF-80's."""
    spec = _with_teams(_teams(WRITERS), ["/api/v1/teams/12/repos", '[{"id":5,"name":"helsinki"}]'])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    no forge team writes the configuration repository" in result.stdout, result.stdout
    assert "PF-80)" not in "".join(l for l in result.stdout.splitlines() if "FAIL" in l)


def test_a_team_that_writes_the_configuration_repository_fails(tmp_path):
    spec = _with_teams(_teams(WRITERS), ["/api/v1/teams/12/repos", '[{"id":5,"name":"helsinki"},{"id":1,"name":"configuration"}]'])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  forge team(s) helsinki-writers write the configuration repository" in result.stdout


def test_a_team_that_writes_every_repository_fails(tmp_path):
    spec = _with_teams(_teams(dict(WRITERS, name="everyone", includes_all_repositories=True)))
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  forge team(s) everyone write the configuration repository" in result.stdout


REFUSED_BEFORE_RUN = json.dumps({
    "valid": False, "lane": "red", "plan": {"summary": {"create": 0, "update": 0, "delete": 0}, "fields": []},
    "verdict": {"ok": False, "findings": [{"level": "error", "path": "spec.http.url", "message":
        "a pipeline does not connect to the runner's own pod or a private address: the URL names one (PL-07)"}]},
})


def test_a_probe_refused_by_validation_before_it_runs_stays_outside(tmp_path):
    """T-3162 refuses a private or link-local host in jc-core, so the dry run never reaches the
    runner; that red answer is a refusal, not 'no refusal' (T-3197)."""
    spec = dict(HEALTHY)
    spec["bodies"] = [[["datasources?dryRun=All", "169.254.169.254"], REFUSED_BEFORE_RUN], *HEALTHY["bodies"]]
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "ok    a probe of https://169.254.169.254/hetzner/v1/metadata stays outside the cluster: refused before it runs" in result.stdout, result.stdout


def test_a_red_dry_run_for_another_reason_is_still_no_refusal(tmp_path):
    spec = dict(HEALTHY)
    other = REFUSED_BEFORE_RUN.replace("a pipeline does not connect to the runner's own pod or a private address", "spec.http.url is not a URL")
    spec["bodies"] = [[["datasources?dryRun=All", "169.254.169.254"], other], *HEALTHY["bodies"]]
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  a probe of https://169.254.169.254/hetzner/v1/metadata answered no refusal" in result.stdout


def test_a_runner_that_restarted_during_the_run_fails_it(tmp_path):
    """T-3537: an OOMKill stops every project's streams; the run that saw one is not green."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    the pipeline runner did not restart during the smoke run (0 restarts)" in result.stdout
    result = run(tmp_path / "restarted", dict(HEALTHY, runnerRestarts=[3, 4]), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the pipeline runner restarted during the smoke run (3 -> 4)" in result.stdout


def test_a_runner_at_80_percent_of_its_limit_fails_the_run(tmp_path):
    """T-3537: under 80 % of the limit passes; at it, in GiB or MiB, fails; unreadable skips."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    the pipeline runner uses 700Mi of its 1536Mi limit, under 80 %" in result.stdout
    for name, spec, line in [
        ("full", dict(HEALTHY, runnerMemory="1229Mi"), "FAIL  the pipeline runner uses 1229Mi of its 1536Mi limit, 80 % or more"),
        ("gib", dict(HEALTHY, runnerMemory="1700Mi", runnerLimit="2Gi"), "FAIL  the pipeline runner uses 1700Mi of its 2048Mi limit, 80 % or more"),
        ("odd", dict(HEALTHY, runnerLimit="1.5G"), 'skip  pipeline runner memory (working set "700" or limit "1.5G" not readable in MiB)'),
        ("none", dict(HEALTHY, runnerRestarts=[]), "skip  pipeline runner memory (no pipeline-runner in this instance)"),
    ]:
        result = run(tmp_path / name, spec, "https://example.test", "https://idm.example.test")
        assert line in result.stdout, name



def test_the_smoke_client_mints_the_internal_ogc_rows_audience_from_the_seed():
    """T-3957: dev-smoke's "ogc internal" row reads helsinki-bikes-ops with the demo user's token
    from the smoke client; the gateway admits it only when `aud` names that Endpoint's slug
    (PF-45), so the client's mapper carries exactly the seeded slug."""
    seed = yaml.safe_load(
        (PROJECT_ROOT / "components/context-gateway/seed/helsinki/helsinki-endpoint-bikes-ops.yaml").read_text()
    )
    clients = yaml.safe_load((PROJECT_ROOT / "components/apisix/keycloak-clients.yaml").read_text())
    audiences = {
        mapper["config"].get("included.custom.audience")
        for mapper in clients["apisix-gateway"]["rawValues"]["protocolMappers"]
    }
    assert seed["spec"]["slug"] in audiences
    assert "ogc_row internal helsinki-bikes-ops" in SMOKE.read_text()


def test_the_internal_ogc_rows_reader_is_granted_the_stations_on_helsinki_bikes_ops():
    """T-3957: on an `organization` Endpoint the gateway gives a signed-in person no `public`
    role (GW22), so the demo viewer the row reads as needs a grant of their own on the space.
    The Endpoint names no policyRef, so every Policy of its space applies; one of them must
    let a group of the viewer's token read the stations, or /collections lists none."""
    seed_dir = PROJECT_ROOT / "components/context-gateway/seed/helsinki"
    endpoint = yaml.safe_load((seed_dir / "helsinki-endpoint-bikes-ops.yaml").read_text())
    assert endpoint["spec"]["audience"] == "organization"
    assert "policyRef" not in endpoint["spec"]
    space = endpoint["spec"]["contextSpaceRef"]
    users = yaml.safe_load((PROJECT_ROOT / "components/keycloak/demo-users.yaml").read_text())
    groups = set(users["demo.viewer"]["groups"])
    # A person a helsinki RoleBinding names also carries the derived `helsinki-readers` in
    # jc_groups, the group a grant names since T-6470 (the plain group no longer matches then).
    viewer = users["demo.viewer"]["id"]
    if any(
        subject.get("user") == viewer
        for path in seed_dir.glob("helsinki-rolebinding-*.yaml")
        for subject in yaml.safe_load(path.read_text())["spec"].get("subjects") or []
    ):
        groups.add("helsinki-readers")
    assert 'demo_user="${JC_SMOKE_DEMO_USER:-demo.viewer}"' in SMOKE.read_text()

    def grants(policy):
        spec = policy["spec"]
        ref = spec["contextSpaceRef"]
        return (
            (ref.get("name") if isinstance(ref, dict) else ref) == space
            and spec["assignee"] == {"kind": "group", "id": spec["assignee"]["id"]}
            and spec["assignee"]["id"] in groups
            and "retrieveOps" in spec["operations"]
            and any(e.get("type") == "BikeHireDockingStation" for i in spec["information"] for e in i["entities"])
        )

    policies = [
        doc
        for path in sorted(seed_dir.glob("helsinki-policy-*.yaml"))
        for doc in yaml.safe_load_all(path.read_text())
        if doc and doc.get("kind") == "Policy"
    ]
    assert any(grants(policy) for policy in policies), "no group of demo.viewer reads the stations"


def test_only_a_runner_refusal_that_still_recurs_fails_the_run(tmp_path):
    """T-4055: a refusal from the apply, older than the window, passes; one inside it fails."""
    defined = "Streams CRUD Error: failed to init processor: symbol transit_realtime.FeedMessage already defined"
    result = run(tmp_path, dict(HEALTHY, runnerRefused=[[600, defined]]), "https://example.test", "https://idm.example.test")
    assert "ok    the pipeline runner refused no stream in the last 1 s" in result.stdout
    result = run(tmp_path / "standing", dict(HEALTHY, runnerRefused=[[600, defined], [0, defined]]),
                 "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the pipeline runner refused streams: Streams CRUD Error: failed to init processor: symbol" in result.stdout


def test_the_data_host_is_checked_and_the_old_address_must_move(tmp_path):
    """T-4630, EP-128: the hub answers on `api.{domain}` from the gateway, and the apex address
    people copied is a 308 to the same path and query there."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "  ok    the api host serves the hub from the gateway" in result.stdout
    assert "  ok    the apex sends the old hub address to the api host (308)" in result.stdout
    # An apex that still serves the hub itself, and a data host that is not the gateway, both fail.
    spec = dict(HEALTHY, statuses=HEALTHY_STATUSES[1:], locations=[],
                bodies=[["https://api.example.test/api/mcp/.well-known", "<html>404</html>"], *HEALTHY["bodies"][1:]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "  FAIL  the api host does not serve the hub's metadata naming https://api.example.test/api/mcp" in result.stdout
    assert "  FAIL  the apex answers the old hub address 200 without a Location, not 308 to https://api.example.test" in result.stdout


@pytest.mark.parametrize("status, location", [
    (200, ""),  # the old host still serves the catalogue itself
    (301, "https://opendata.example.test/"),  # moved, but path and query lost
    (301, "https://evil.example/dataset/x?q=1"),  # moved off our domain
])
def test_the_old_catalogue_host_must_redirect_with_path_and_query(tmp_path, status, location):
    """T-4679: data.{domain} answers only a 301 to the same path and query on opendata.{domain}."""
    old = "https://data.example.test/dataset/x?q=1"
    spec = dict(HEALTHY, statuses=[[old, status], *HEALTHY["statuses"]],
                locations=[[old, location], *HEALTHY["locations"]])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode != 0
    assert "FAIL  data.{domain} does not redirect to opendata.{domain} with path and query" in result.stdout


def test_the_broker_refusing_a_forged_caller_passes(tmp_path):
    """T-4875: the probe wears the Portal's label outside the mesh; the broker's proxy says 403."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    the broker's proxy refuses a forged x-jc-subject" in result.stdout


def test_a_forged_caller_the_broker_answers_fails_the_run(tmp_path):
    """T-4875: a forged x-jc-subject that reaches the broker reads as anyone."""
    spec = dict(HEALTHY, forgerCodes=[200, 101])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "reached the broker with a forged subject (HTTP 200, WebSocket 101)" in result.stdout


def test_a_forged_caller_probe_without_an_answer_fails_the_run(tmp_path):
    """T-4875: no status at all is a probe that never reached the proxy, not a refusal."""
    for spec in (dict(HEALTHY, forgerCodes=[403, None]), dict(HEALTHY, forgerRan=False)):
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert result.returncode == 1
        assert "so the refusal was not measured" in result.stdout or "the forged-caller probe never ran" in result.stdout


def test_an_instance_without_a_meshed_broker_skips_the_forged_caller_probe(tmp_path):
    for spec in (dict(HEALTHY, brokerServer=False), dict(HEALTHY, brokerPodIp="")):
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert "broker callers (no meshed context broker with its Server in this instance)" in result.stdout


def test_the_broker_dropping_a_meshed_forged_subject_passes(tmp_path):
    """T-6433: past the proxy the broker's own identity forges a subject; it reads as nobody."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    assert "ok    the broker drops a forged x-jc-subject from a meshed identity" in result.stdout


@pytest.mark.parametrize("code", [200, 500])
def test_a_meshed_forged_subject_the_broker_believes_fails_the_run(tmp_path, code):
    spec = dict(HEALTHY, meshForgerCode=code)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert f"the broker believed a forged x-jc-subject from a meshed identity outside its list (HTTP {code})" in result.stdout


@pytest.mark.parametrize("code", [403, None])
def test_a_meshed_forger_the_proxy_stopped_measures_nothing(tmp_path, code):
    """T-6433: a 403 is the proxy or a believed subject's policy, never the proof of a drop."""
    spec = dict(HEALTHY, meshForgerCode=code)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "so who the broker believes was not measured" in result.stdout


def test_a_meshed_forger_that_never_finished_fails_the_run(tmp_path):
    spec = dict(HEALTHY, meshForgerRan=False)
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test",
                 extra_env={"JC_SMOKE_MESHED_WAIT": "0"})
    assert result.returncode == 1
    assert "the meshed forged-caller probe never finished" in result.stdout


def test_the_mqtt_brokers_serve_their_certificate_and_refuse_a_connect_without_a_token(tmp_path):
    """T-4857: each stamped broker runs, its host on 8883 or its dedicated port verifies, a tokenless
    CONNECT is refused, and plain 1883 is closed; a broker that admits one fails the run."""
    result = run(tmp_path, HEALTHY, "https://example.test", "https://idm.example.test")
    for line in ("mqtt mqtt-helsinki-sensors runs",
                 "mqtt mqtt-helsinki-vehicles runs",
                 "mqtt sensors.helsinki.mqtt.example.test:8883 serves a valid certificate for sensors.helsinki.mqtt.example.test",
                 "mqtt vehicles.helsinki.mqtt.example.test:18830 serves a valid certificate for vehicles.helsinki.mqtt.example.test",
                 "mqtt vehicles.helsinki.mqtt.example.test:18830 refuses a CONNECT without a token",
                 "mqtt sensors.helsinki.mqtt.example.test:1883 (plain MQTT) is closed from outside"):
        assert f"  ok    {line}" in result.stdout, result.stdout
    assert result.returncode == 0 and "FAIL  mqtt" not in result.stdout, result.stdout
    assert "cert.sensors" not in result.stdout, result.stdout
    for spec, failure in ((dict(HEALTHY, mqttAnonymous=True), "a CONNECT without a token was not refused"),
                          (dict(HEALTHY, mqttTls=False), "no verified TLS for"),
                          (dict(HEALTHY, mqttBrokers=["mqtt-helsinki-sensors 0", "mqtt-helsinki-vehicles 1"]),
                           "mqtt-helsinki-sensors has no ready replica"),
                          (dict(HEALTHY, mqttCerts=[]), "mqtt-helsinki-vehicles: neither a Certificate nor a HostSNI route"),
                          (dict(HEALTHY, mqttRoutes=["mqtt-helsinki-sensors mqtts HostSNI(`sensors.helsinki.mqtt.example.test`)"]), "mqtt-helsinki-vehicles: no route on 8883 or a dedicated port")):
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert result.returncode == 1 and failure in result.stdout, result.stdout


def test_a_custom_hostname_broker_takes_its_host_from_its_route(tmp_path):
    """T-4857, T-5874: a broker on the admin's own certificate has no Certificate object; its 8883
    route names the host."""
    spec = dict(HEALTHY, mqttBrokers=["mqtt-helsinki-sensors 1"], mqttCerts=[], mqttSeeded=1,
                mqttRoutes=["mqtt-helsinki-sensors mqtts HostSNI(`mqtt.city.example`)"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert "  ok    mqtt mqtt.city.example:8883 serves a valid certificate for mqtt.city.example" in result.stdout, result.stdout


def test_an_instance_without_mqtt_brokers_skips_their_checks(tmp_path):
    for spec in (dict(HEALTHY, mqttBrokers=[], mqttSeeded=0), dict(HEALTHY, mqttBrokers=[], mqttSeeded=0, mqttNamespace="")):
        result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
        assert "  skip  mqtt edge brokers (no MqttBroker seeded or stamped in this instance)" in result.stdout, result.stdout


def test_seeded_mqtt_brokers_that_do_not_run_fail_the_run(tmp_path):
    """T-4857 (deployer 2026-10-28): the check looked in dev-mqtt and skipped while the brokers ran
    in dev; it reads their namespace from the broker-template ConfigMap, and a seed whose brokers
    do not all run fails instead of skipping. Only gitea-seed-* ConfigMaps count as the seed."""
    result = run(tmp_path, dict(HEALTHY, mqttSeeded=3), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1 and "the seed declares 3 MqttBroker(s), 2 run in dev" in result.stdout, result.stdout
    result = run(tmp_path, dict(HEALTHY, mqttBrokers=[], mqttNamespace=""), "https://example.test", "https://idm.example.test")
    assert result.returncode == 1 and "the seed declares 2 MqttBroker(s), 0 run in no namespace (no broker-template" in result.stdout, result.stdout
    assert "skip  mqtt edge brokers" not in result.stdout


def test_a_public_guide_page_fails_the_run(tmp_path):
    """T-6200: the documentation is for signed-in people; a guide the edge serves to a visitor
    is a red run, by page body and by status."""
    bodies = [["/docs/API/07-websocket", "<title>Live Updates over WebSocket</title>"]]
    statuses = [["/docs/User-Guide/", 200]]
    spec = dict(EDGE_ONLY, bodies=bodies + HEALTHY["bodies"], statuses=statuses + HEALTHY["statuses"])
    result = run(tmp_path, spec, "https://example.test", "https://idm.example.test")
    assert result.returncode == 1
    assert "FAIL  the WebSocket guide reads without signing in" in result.stdout
    assert "FAIL  a visitor asking for a guide page is sent to sign in (expected 302, got 200)" in result.stdout
