#!/usr/bin/env python3
"""nats_jetstream input/output and nats_kv: ekiben against Bento 1.22 on one nats-server -js.

Each case starts a fresh `nats:2.12-alpine -js` container, seeds it with the nats CLI
(natsio/nats-box), runs the same config through Bento and through ekiben (each on its own
fresh server with the same seed), and compares what each wrote: stdout lines (payload and
metadata), the JetStream consumer each created, the stream or KV bucket afterwards.

Usage: scripts/nats-parity/parity.py --bento .bento/bento --ekiben target/release/ekiben
Prints one line per case (`identical`, `DIFF` with the difference, or `KNOWN` with the
documented reason) and exits 1 on any DIFF. No docker: prints SKIP and exits 0.
"""
import argparse
import base64
import json
import os
import subprocess
import sys
import tempfile
import time

NATS = "nats:2.12-alpine"
BOX = "natsio/nats-box:0.18.1"
VOLATILE_META = {"nats_timestamp_unix_nano", "nats_kv_created"}


def sh(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


class Server:
    """A fresh nats-server -js; with `nkey=True` it admits only one generated user nkey."""

    def __init__(self, nkey=False):
        self.auth_dir = None
        if nkey:
            self.auth_dir = tempfile.mkdtemp(prefix="nats-parity-auth-")
            seed, pub = sh(["docker", "run", "--rm", BOX, "nsc", "generate", "nkey", "--user"]).stdout.split()[:2]
            with open(os.path.join(self.auth_dir, "user.nk"), "w") as f:
                f.write(seed + "\n")
            with open(os.path.join(self.auth_dir, "server.conf"), "w") as f:
                f.write("jetstream {}\nauthorization { users = [ { nkey: %s } ] }\n" % pub)
            os.chmod(self.auth_dir, 0o755)
            for name in ("user.nk", "server.conf"):
                os.chmod(os.path.join(self.auth_dir, name), 0o644)
        self.nkey_file = os.path.join(self.auth_dir, "user.nk") if nkey else ""

    def __enter__(self):
        if self.auth_dir:
            args = ["-v", f"{self.auth_dir}:/auth:ro", NATS, "-c", "/auth/server.conf"]
        else:
            args = [NATS, "-js"]
        out = sh(["docker", "run", "-d", "--rm", "-p", "127.0.0.1::4222", *args])
        if out.returncode:
            raise RuntimeError(out.stderr)
        self.id = out.stdout.strip()
        port = sh(["docker", "port", self.id, "4222"]).stdout.splitlines()[0].rsplit(":", 1)[1]
        self.url = f"nats://127.0.0.1:{port}"
        for _ in range(50):
            if "ok" in self.nats("server", "check", "jetstream").stdout.lower() or \
               self.nats("account", "info").returncode == 0:
                break
            time.sleep(0.1)
        return self

    def __exit__(self, *a):
        sh(["docker", "rm", "-f", self.id])

    def nats(self, *args, stdin=None):
        auth = ["-v", f"{self.auth_dir}:/auth:ro"] if self.auth_dir else []
        cred = ["--nkey", "/auth/user.nk"] if self.auth_dir else []
        return sh(["docker", "run", "--rm", "-i", "--network", "host", *auth, BOX, "nats",
                   "--server", self.url, *cred, *args], input=stdin)

    def req(self, subject, body=""):
        out = self.nats("req", subject, body, "--raw")
        return json.loads(out.stdout) if out.stdout.strip() else {"error": out.stderr}


def run_tool(cmd, conf, url, stdin_text, seconds, during=None):
    """Runs `cmd` on `conf` for at most `seconds`; `during(t)` runs once after 2 s."""
    with tempfile.NamedTemporaryFile("w", suffix=".yaml", delete=False) as f:
        f.write(conf.replace("NATS_URL", url))
        path = f.name
    args = [cmd, "-c", path] if cmd.endswith("bento") else [cmd, "run", path]
    stdin_file = tempfile.TemporaryFile("w+")
    stdin_file.write(stdin_text or "")
    stdin_file.seek(0)
    p = subprocess.Popen(args, stdin=stdin_file, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True)
    seen = None
    if during:
        time.sleep(2)
        seen = during()
    try:
        out, err = p.communicate(timeout=seconds)
    except subprocess.TimeoutExpired:
        p.terminate()
        out, err = p.communicate()
    os.unlink(path)
    return out, err, seen


def lines_of(out):
    rows = []
    for line in out.splitlines():
        if line.startswith("level="):
            continue  # Bento's log lines (stdout when the stream has no stdout output)
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            rows.append(line)
            continue
        if isinstance(row, dict) and isinstance(row.get("payload"), str) and '"created"' in row["payload"]:
            items = json.loads(row["payload"])
            for it in items:
                created = it.get("created", "")
                # The time differs per run; its format (Go RFC3339Nano) must not.
                ok = created.endswith("Z") and "+" not in created
                it["created"] = "<rfc3339nano>" if ok else created
            row["payload"] = json.dumps(items, sort_keys=True)
        if isinstance(row, dict) and isinstance(row.get("meta"), dict):
            for k in VOLATILE_META & row["meta"].keys():
                row["meta"][k] = "<set>" if row["meta"][k] else ""
        rows.append(json.dumps(row, sort_keys=True))
    return sorted(rows)


CONSUMER_FIELDS = ["durable_name", "deliver_policy", "ack_policy", "ack_wait",
                   "max_ack_pending", "filter_subject", "deliver_group", "max_deliver",
                   "replay_policy"]


def consumers(srv, stream):
    info = srv.req(f"$JS.API.CONSUMER.LIST.{stream}", "{}")
    rows = []
    for c in info.get("consumers") or []:
        cfg = c["config"]
        row = {k: cfg.get(k) for k in CONSUMER_FIELDS}
        row["push"] = bool(cfg.get("deliver_subject"))
        rows.append(row)
    return sorted(rows, key=json.dumps)


def stream_msgs(srv, stream):
    out = []
    for seq in range(1, 100):
        m = srv.req(f"$JS.API.STREAM.MSG.GET.{stream}", json.dumps({"seq": seq}))
        if "message" not in m:
            break
        msg = m["message"]
        hdrs = base64.b64decode(msg.get("hdrs", "")).decode() if msg.get("hdrs") else ""
        hdr_lines = sorted(l for l in hdrs.split("\r\n")[1:] if l)
        out.append({"subject": msg["subject"], "data": base64.b64decode(msg.get("data", "")).decode(),
                    "headers": hdr_lines})
    return out


def kv_dump(srv, bucket):
    keys = sorted(srv.nats("kv", "ls", bucket).stdout.split())
    return {k: srv.nats("kv", "get", bucket, k, "--raw").stdout for k in keys}


ECHO = """
pipeline:
  processors:
    - mapping: |
        root.payload = content().string()
        root.meta = @
"""

SEED_STREAM = [("stream", "add", "S", "--subjects", "p.>", "--defaults")]
SEED_MSGS = [("pub", "p.a", "one", "-H", "X-Trace:t1"), ("pub", "p.b", "two"),
             ("pub", "p.a", "three", "-H", "X-Trace:t3")]


def input_case(name, fields, seed=(), during_pub=None, known=None):
    body = "\n".join(f"    {k}: {v}" for k, v in fields.items())
    conf = f"input:\n  nats_jetstream:\n    urls: [ NATS_URL ]\n{body}\n{ECHO}\noutput:\n  stdout: {{}}\n"
    return {"name": name, "kind": "input", "conf": conf, "seed": SEED_STREAM + list(seed) + SEED_MSGS,
            "during_pub": during_pub, "known": known}


CASES = [
    input_case("input subject, ephemeral, deliver all", {"subject": "p.>"}),
    input_case("input deliver last", {"subject": "p.>", "deliver": "last"}),
    input_case("input deliver last_per_subject", {"subject": "p.>", "deliver": "last_per_subject"}),
    input_case("input deliver new", {"subject": "p.>", "deliver": "new"},
               during_pub=("pub", "p.c", "four", "-H", "X-Trace:t4")),
    input_case("input stream named", {"subject": "p.a", "stream": "S"}),
    input_case("input queue (durable = queue)", {"subject": "p.>", "queue": "workers"}),
    input_case("input queue + durable", {"subject": "p.>", "queue": "workers", "durable": "dur"}),
    input_case("input ack_wait and max_ack_pending", {"subject": "p.>", "queue": "q",
                                                      "ack_wait": "7s", "max_ack_pending": 3}),
    input_case("input durable without queue", {"subject": "p.>", "durable": "dur"},
               known="ekiben makes a new durable without queue a pull consumer so several instances "
                     "share it; Bento makes a push consumer, which a second Bento instance cannot bind "
                     "(nats: consumer is already bound). Messages and metadata compare equal."),
    dict(input_case("input auth.nkey_file on a server that requires it",
                    {"subject": "p.>", "auth": "{ nkey_file: NKEY_FILE }"}), nkey=True),
    input_case("input bind to an existing pull consumer", {"bind": "true", "stream": "S", "durable": "pre"},
               seed=[("consumer", "add", "S", "pre", "--pull", "--filter", "p.a", "--deliver", "all",
                      "--ack", "explicit", "--defaults")]),
    {"name": "output subject interpolation, headers, metadata", "kind": "output",
     "conf": """
input:
  stdin: {}
pipeline:
  processors:
    - mapping: |
        meta trace_id = this.id.string()
        meta other = "x"
        root = this
output:
  nats_jetstream:
    urls: [ NATS_URL ]
    subject: p.${! json("kind") }
    headers:
      X-Source: ekiben-parity
      X-Id: ${! json("id") }
    metadata:
      include_prefixes: [ trace_ ]
""", "stdin": '{"id":1,"kind":"a"}\n{"id":2,"kind":"b"}\n{"id":3,"kind":"a"}\n', "seed": SEED_STREAM},
    {"name": "output nats_kv", "kind": "kv",
     "conf": """
input:
  stdin: {}
output:
  nats_kv:
    urls: [ NATS_URL ]
    bucket: B
    key: k.${! json("id") }
""", "stdin": '{"id":1}\n{"id":2}\n{"id":1,"v":2}\n', "seed": [("kv", "add", "B", "--history", "5")]},
]

KV_SEED = [("kv", "add", "B", "--history", "5"), ("kv", "put", "B", "k.1", "v1"),
           ("kv", "put", "B", "k.1", "v1b"), ("kv", "put", "B", "k.2", "v2")]
for op, key, extra, stdin in [
    ("get", "k.1", "", "x\n"), ("get", "missing", "", "x\n"),
    ("get_revision", "k.1", '        revision: "2"\n', "x\n"),
    ("create", "k.3", "", "new\n"), ("create", "k.1", "", "dup\n"),
    ("put", "k.1", "", "v1c\n"),
    ("update", "k.2", '        revision: "4"\n', "upd\n"), ("update", "k.2", '        revision: "1"\n', "stale\n"),
    ("delete", "k.2", "", "x\n"), ("purge", "k.1", "", "x\n"),
    ("history", "k.1", "", "x\n"), ("keys", "k.*", "", "x\n"),
]:
    CASES.append({"name": f"processor nats_kv {op} {key}" + (" (" + extra.split(":")[1].strip() + ")" if extra else ""),
                  "kind": "kv", "seed": KV_SEED, "stdin": stdin, "conf": f"""
input:
  stdin: {{}}
pipeline:
  processors:
    - nats_kv:
        urls: [ NATS_URL ]
        bucket: B
        operation: {op}
        key: {key}
{extra}    - mapping: |
        root.payload = content().string()
        root.meta = @
        root.error = error()
output:
  stdout: {{}}
"""})

CASES.append({"name": "cache nats_kv set/get/add/delete", "kind": "kv", "seed": KV_SEED,
              "stdin": '{"k":"c.1"}\n', "conf": """
input:
  stdin: {}
cache_resources:
  - label: kv
    nats_kv:
      urls: [ NATS_URL ]
      bucket: B
pipeline:
  processors:
    - cache: { resource: kv, operator: set, key: '${! json("k") }', value: first }
    - cache: { resource: kv, operator: add, key: '${! json("k") }', value: second }
    - mapping: 'root.add_error = error()'
    - cache: { resource: kv, operator: get, key: c.1 }
    - mapping: 'root = {"got": content().string(), "err": error()}'
output:
  stdout: {}
"""})


def run_case(case, tool, cmd):
    with Server(nkey=case.get("nkey", False)) as srv:
        case = dict(case, conf=case["conf"].replace("NKEY_FILE", srv.nkey_file))
        for s in case["seed"]:
            r = srv.nats(*s)
            if r.returncode:
                raise RuntimeError(f"seed {s}: {r.stderr}")
        during = None
        if case["kind"] == "input":
            def during():
                if case.get("during_pub"):
                    srv.nats(*case["during_pub"])
                    time.sleep(0.5)
                return consumers(srv, "S")
        out, err, seen = run_tool(cmd, case["conf"], srv.url, case.get("stdin", ""),
                                  4 if case["kind"] == "input" else 15, during)
        result = {"stdout": lines_of(out)}
        if seen is not None:
            result["consumers"] = seen
        if case["kind"] == "output":
            # Bento publishes a batch concurrently (max_in_flight), so stream order is not part
            # of the contract: compare the stored messages as a set.
            result["stream"] = sorted(stream_msgs(srv, "S"), key=json.dumps)
        if case["kind"] == "kv":
            result["bucket"] = kv_dump(srv, "B")
        if not out.strip() and case["kind"] != "output" and not result.get("bucket"):
            result["stderr"] = err.strip().splitlines()[-3:]
        return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bento", default=".bento/bento")
    ap.add_argument("--ekiben", required=True)
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    if sh(["docker", "info"]).returncode:
        print("SKIP nats parity: docker is not available")
        return 0
    diffs = 0
    for case in CASES:
        if a.only and a.only not in case["name"]:
            continue
        b = run_case(case, "bento", os.path.abspath(a.bento))
        e = run_case(case, "ekiben", os.path.abspath(a.ekiben))
        if b == e and not b["stdout"] and case["kind"] == "input":
            diffs += 1
            print(f"DIFF       {case['name']}: neither tool read a message: {b.get('stderr')}")
            continue
        if b == e:
            print(f"identical  {case['name']}")
            continue
        keys = [k for k in sorted(set(b) | set(e)) if b.get(k) != e.get(k)]
        if case.get("known") and keys == ["consumers"]:
            print(f"KNOWN      {case['name']}: {case['known']}")
            continue
        diffs += 1
        print(f"DIFF       {case['name']}")
        for k in keys:
            print(f"    {k}\n      bento:  {json.dumps(b.get(k))}\n      ekiben: {json.dumps(e.get(k))}")
    print(f"{len([c for c in CASES if a.only in c['name']]) - diffs} of "
          f"{len([c for c in CASES if a.only in c['name']])} cases without a DIFF")
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())
