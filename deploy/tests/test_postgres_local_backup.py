"""T-5839 (CC-06): the nightly pg_dumpall is age-encrypted before it touches the node volume.

The dump job pipes `pg_dumpall | gzip | age.py encrypt` into the file, sealed to the operator's
key (`localBackup.ageRecipients`) and to the generated drill key, which the monthly drill opens
it with. tests/test_dr_drill.py runs both scripts with the age CLI standing in for age.py; here
age.py itself runs in the image the cluster runs it in, against that CLI, since the CI venv has
no python3-cryptography.
"""

import os
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
CHART = ROOT / "components/postgres/charts/databases"
# The operator recipient the default environments name: the dev age key of .sops.yaml (public).
OPERATOR_RECIPIENT = yaml.safe_load((ROOT / ".sops.yaml").read_text())["creation_rules"][0]["age"].split(",")[0]


def one(docs: list[dict], kind: str, name: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


@pytest.fixture(scope="module")
def smartcity(rendered):
    return rendered("smartcity")


@pytest.mark.parametrize("cluster", ["postgres-cluster", "apps-db"])
def test_the_dump_is_encrypted_before_it_touches_the_volume(smartcity, cluster):
    pod = one(smartcity, "CronJob", f"{cluster}-local-backup")["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    container, = pod["containers"]
    script = container["command"][2]
    pipe = script[script.index("{ pg_dumpall"):script.index('> "$file.part"')]
    assert pipe.index("gzip") < pipe.index("python3 /opt/age/age.py encrypt --identity-file /key/key $AGE_RECIPIENTS") \
        < pipe.index("head -c"), "sealed in the pipe, before head writes the file"
    assert "file=/backup/$(date -u +%Y-%m-%dT%H%MZ).sql.gz.age" in script
    assert "-name '*.sql.gz.age' -mmin -1200" in script, "the apply's bind check looks for sealed dumps"
    env = {e["name"]: e.get("value") for e in container["env"]}
    assert env["AGE_RECIPIENTS"] == OPERATOR_RECIPIENT
    volumes = {v["name"]: v for v in pod["volumes"]}
    assert volumes["key"]["secret"] == {"secretName": f"{cluster}-local-backup-key", "defaultMode": 0o440}
    assert volumes["age"]["configMap"]["name"] == f"{cluster}-local-backup-age"
    mounts = {m["name"]: m for m in container["volumeMounts"]}
    assert mounts["key"] == {"name": "key", "mountPath": "/key", "readOnly": True}
    age_py = one(smartcity, "ConfigMap", f"{cluster}-local-backup-age")["data"]["age.py"]
    assert age_py == (CHART / "files/age.py").read_text()


def test_the_drill_key_is_generated_where_the_dump_and_the_drill_run(smartcity):
    for cluster in ("postgres-cluster", "apps-db"):
        secret = one(smartcity, "Secret", f"{cluster}-local-backup-key")
        assert secret["metadata"]["namespace"] == one(smartcity, "CronJob", f"{cluster}-local-backup")["metadata"]["namespace"]
        assert secret["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"
    drill = one(smartcity, "CronJob", "postgres-cluster-dr-drill")["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    init, = drill["initContainers"]
    assert {m["name"] for m in init["volumeMounts"]} >= {"key", "age"}
    drill_container, = drill["containers"]
    assert "key" not in {m["name"] for m in drill_container["volumeMounts"]}, "the API-token container holds no key"


def helm_template(tmp_path: Path, *sets: str) -> subprocess.CompletedProcess:
    if shutil.which("helm") is None:
        pytest.skip("helm not installed")
    values = tmp_path / "values.yaml"
    values.write_text("securityDefaults:\n  podSecurityContext: {runAsNonRoot: true}\n"
                      "  securityContext: {readOnlyRootFilesystem: true}\n")
    args = ["helm", "template", "x", str(CHART), "-f", str(values), "--set", "localBackup.enabled=true"]
    for s in sets:
        args += ["--set", s]
    return subprocess.run(args, capture_output=True, text=True, check=False)


def test_jc_prod_dumps_every_database_nightly_and_the_drill_restores_the_dump(rendered):
    # F-1 (owner 2026-11-11): jc-prod serves the public smartcity hosts and declared no backup.
    docs = rendered("jc-prod")
    cron = one(docs, "CronJob", "postgres-cluster-local-backup")
    assert cron["metadata"]["namespace"] == "jc-prod" and cron["spec"]["schedule"] == "30 2 * * *"
    env = {e["name"]: e.get("value") for e in cron["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["EXCLUDE_DATABASES"] == "", "jc_antares is the data; the dump leaves nothing out"
    # Its own volume, never the database's PVC: a lost data volume keeps the dumps.
    pvc = one(docs, "PersistentVolumeClaim", "postgres-cluster-local-backup")
    assert pvc["metadata"]["namespace"] == "jc-prod"
    drill = one(docs, "CronJob", "postgres-cluster-dr-drill")["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    claims = {v["persistentVolumeClaim"]["claimName"] for v in drill["volumes"] if "persistentVolumeClaim" in v}
    assert claims == {"postgres-cluster-local-backup"}
    assert [c["name"] for c in drill["initContainers"]] == ["dump"], "the drill restores the newest dump first"


def test_the_render_refuses_a_dump_sealed_to_no_operator_key(tmp_path):
    refused = helm_template(tmp_path)
    assert refused.returncode != 0 and "localBackup.ageRecipients is empty" in refused.stderr
    refused = helm_template(tmp_path, "localBackup.ageRecipients={age1short}")
    assert refused.returncode != 0 and "ageRecipients" in refused.stderr, "the schema holds each to an age1 key"
    rendered = helm_template(tmp_path, f"localBackup.ageRecipients={{{OPERATOR_RECIPIENT}}}")
    assert rendered.returncode == 0, rendered.stderr
    kinds = [d["kind"] for d in yaml.safe_load_all(rendered.stdout) if d]
    assert "CronJob" in kinds and "ConfigMap" in kinds


IMAGE_FILE = ROOT / "components/postgres/images.yaml"


def in_image(tmp_path: Path, args: list[str], data: bytes) -> subprocess.CompletedProcess:
    """files/age.py run with the cluster's python3 in the roleJob image, as uid 1000."""
    if shutil.which("docker") is None:
        pytest.skip("docker not installed")
    image = yaml.safe_load(IMAGE_FILE.read_text())["postgres"]["roleJob"]
    ran = subprocess.run(
        ["docker", "run", "--rm", "-i", "--network", "none", "-u", "1000", "--entrypoint", "python3",
         "-v", f"{CHART / 'files/age.py'}:/age.py:ro", "-v", f"{tmp_path}:/w:ro",
         f"{image['repository']}@{image['digest']}", "/age.py", *args],
        input=data, capture_output=True, check=False, timeout=600,
    )
    if ran.returncode == 125:
        pytest.skip(f"the pinned image does not run here: {ran.stderr.decode().strip()}")
    return ran


def test_age_py_in_the_cluster_image_writes_what_age_opens_and_opens_what_age_writes(tmp_path):
    if not shutil.which("age-keygen"):
        pytest.skip("age not installed (ci.yml installs it)")
    identity = tmp_path / "identity.txt"
    subprocess.run(["age-keygen", "-o", str(identity)], check=True, capture_output=True)
    operator = subprocess.run(["age-keygen", "-y", str(identity)], check=True, capture_output=True, text=True).stdout.strip()
    drill = os.urandom(32).hex()
    (tmp_path / "key").write_text(drill)
    tmp_path.chmod(0o755)
    (tmp_path / "key").chmod(0o644)
    # Empty, short, and an exact multiple of the 64 KiB chunk: the edge cases of the STREAM.
    for size in (0, 200_000, 2 * 65536):
        plain = os.urandom(size)
        sealed = in_image(tmp_path, ["encrypt", "--identity-file", "/w/key", operator], plain)
        assert sealed.returncode == 0, sealed.stderr
        assert sealed.stdout.startswith(b"age-encryption.org/v1\n")
        assert subprocess.run(["age", "-d", "-i", str(identity)], input=sealed.stdout,
                              capture_output=True, check=True).stdout == plain, "the operator opens it with age"
        opened = in_image(tmp_path, ["decrypt", "--identity-file", "/w/key"], sealed.stdout)
        assert opened.returncode == 0 and opened.stdout == plain, "the drill opens it with its key"
    by_cli = subprocess.run(["age", "-r", operator], input=b"y", capture_output=True, check=True).stdout
    refused = in_image(tmp_path, ["decrypt", "--identity-file", "/w/key"], by_cli)
    assert refused.returncode == 1 and b"no recipient of this file is the key given" in refused.stderr
    truncated = in_image(tmp_path, ["decrypt", "--identity-file", "/w/key"], sealed.stdout[:70_000])
    assert truncated.returncode == 1 and b"is damaged or the file is truncated" in truncated.stderr
    nobody = in_image(tmp_path, ["encrypt"], b"x")
    assert nobody.returncode != 0 and nobody.stdout == b""
