#!/usr/bin/env python3
"""A `docker` stand-in that records Bento runs for the parity suite.

The deployment tests run Bento as `docker run ... <bento image> ...`. On PATH ahead of the real
docker, this script runs the same command with the local reference binary (.bento/bento, fetched
by scripts/reference-bento.sh) and writes one case per run into $PARITY_CAPTURE: the Bento argv,
the -e environment, stdin, every file the argv names, stdout and the exit code. Any other docker
call passes through to $REAL_DOCKER."""
import base64, hashlib, json, os, re, subprocess, sys, tempfile, time
from pathlib import Path

REAL = os.environ.get("REAL_DOCKER", "/usr/bin/docker")
BENTO = os.environ["PARITY_BENTO"]
OUT = Path(os.environ["PARITY_CAPTURE"])
VALUE_FLAGS = {"-e", "--env", "-v", "--volume", "-w", "--workdir", "--entrypoint", "--network", "--user", "-u",
               "--memory", "--name", "--cpus", "--add-host"}


def passthrough():
    os.execv(REAL, [REAL] + sys.argv[1:])


def main():
    argv = sys.argv[1:]
    if not argv or argv[0] != "run":
        passthrough()
    env, mounts, entrypoint, i = {}, [], None, 1
    while i < len(argv) and argv[i].startswith("-"):
        flag, val = argv[i], None
        if "=" in flag and flag.startswith("--"):
            flag, val = flag.split("=", 1)
        elif flag in VALUE_FLAGS:
            i += 1
            val = argv[i]
        if flag in ("-e", "--env"):
            k, _, v = val.partition("=")
            env[k] = v if _ else os.environ.get(k, "")
        elif flag in ("-v", "--volume"):
            host, cont = val.split(":")[:2]
            mounts.append((cont.rstrip("/"), host.rstrip("/")))
        elif flag == "--entrypoint":
            entrypoint = val
        i += 1
    image, rest = argv[i], argv[i + 1:]
    if "warpstreamlabs/bento" not in image:
        passthrough()
    tmp = tempfile.mkdtemp(prefix="bento-shim-")
    mounts.sort(key=lambda m: -len(m[0]))

    def host_path(s):
        s = s.replace("/tmp/c.json", f"{tmp}/c.json")
        for cont, host in mounts:
            s = re.sub(rf"(?<![\w/]){re.escape(cont)}(?=/|\b)", host, s)
        return s

    if entrypoint == "sh":
        script = host_path(rest[rest.index("-c") + 1])
        script = re.sub(r"(?<![\w/])/bento\b", BENTO, script)
        cmd = ["sh", "-c", script]
    else:
        cmd = [BENTO] + [host_path(a) for a in rest]
    stdin = sys.stdin.buffer.read() if not sys.stdin.isatty() else b""
    run_env = {"PATH": os.environ["PATH"], "HOME": tmp, **env}
    # The second this run started at: a mapping reading now() is replayed on it (ekiben run --clock).
    clock = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    p = subprocess.run(cmd, input=stdin, capture_output=True, env=run_env)
    sys.stdout.buffer.write(p.stdout)
    sys.stderr.buffer.write(p.stderr)
    files = {}
    for token in re.findall(r"[\w./-]+", " ".join(cmd[1:])):
        f = Path(token)
        if f.is_absolute() and f.is_file() and f.resolve() != Path(BENTO).resolve() and f.stat().st_size < 50_000_000:
            files[token] = base64.b64encode(f.read_bytes()).decode()
    case = {"test": os.environ.get("PYTEST_CURRENT_TEST", ""), "cmd": cmd, "env": env, "tmp": tmp,
            "stdin": base64.b64encode(stdin).decode(), "files": files,
            "stdout": base64.b64encode(p.stdout).decode(), "stderr": p.stderr.decode(errors="replace")[-4000:],
            "code": p.returncode, "clock": clock}
    blob = json.dumps(case, sort_keys=True).encode()
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / (hashlib.sha256(blob).hexdigest()[:16] + ".json")).write_bytes(blob)
    sys.exit(p.returncode)


main()
