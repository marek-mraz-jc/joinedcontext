"""T-0018: bootstrap secret generation and SOPS-encrypted environment secrets."""

import base64
import json
import os
import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml
from conftest import load_yaml_all

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHECK = PROJECT_ROOT / "scripts" / "check-secrets.py"
GENERATOR_CHART = PROJECT_ROOT / "components" / "secrets" / "charts" / "secrets-generator"

requires_sops = pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen") and shutil.which("helmfile")),
    reason="sops, age-keygen and helmfile are needed for the SOPS round trip",
)


def check(rendered: str, components: dict[str, str], tmp_path: Path) -> subprocess.CompletedProcess:
    """Run the verification script against a throwaway render + component tree."""
    manifest = tmp_path / "rendered.yaml"
    manifest.write_text(rendered)
    comp_dir = tmp_path / "components"
    for component, body in components.items():
        (comp_dir / component).mkdir(parents=True, exist_ok=True)
        (comp_dir / component / "secrets.yaml").write_text(body)
    return subprocess.run(
        ["python3", str(CHECK), str(manifest), str(comp_dir)],
        capture_output=True,
        text=True,
    )


def secret_manifest(name: str, keys: dict[str, str], keep: bool = True) -> str:
    data = {k: base64.b64encode(v.encode()).decode() for k, v in keys.items()}
    doc = {
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": name,
            "namespace": "local",
            "annotations": {"helm.sh/resource-policy": "keep"} if keep else {},
        },
        "data": data,
    }
    return yaml.safe_dump(doc)


DECLARATION = textwrap.dedent(
    """\
    ---
    demo:
      part:
        db-demo:
          username: 'demo'
          password:
            length: 32
            generate: true
          componentNamespaces:
            - demo
    """
)


def test_declared_and_rendered_secret_passes(tmp_path):
    result = check(secret_manifest("db-demo", {"password": "a" * 32}), {"demo": DECLARATION}, tmp_path)
    assert result.returncode == 0, result.stdout


def test_short_declaration_is_refused(tmp_path):
    """A component may not weaken a credential by declaring fewer than 32 characters."""
    weak = DECLARATION.replace("length: 32", "length: 16")
    result = check(secret_manifest("db-demo", {"password": "a" * 16}), {"demo": weak}, tmp_path)
    assert result.returncode == 1
    assert "below the 32 minimum" in result.stdout


def test_rendered_value_shorter_than_declared_is_refused(tmp_path):
    result = check(secret_manifest("db-demo", {"password": "a" * 8}), {"demo": DECLARATION}, tmp_path)
    assert result.returncode == 1
    assert "decodes to 8 characters, declared 32" in result.stdout


def test_undeclared_generated_secret_is_refused(tmp_path):
    """A Secret nobody declared is a credential nobody reviews."""
    result = check(secret_manifest("mystery", {"password": "a" * 32}), {"demo": DECLARATION}, tmp_path)
    assert result.returncode == 1
    assert "no declaration" in result.stdout


def demo_users(tmp_path: Path, *names: str) -> None:
    """components/keycloak/demo-users.yaml, the file the password generator reads."""
    keycloak = tmp_path / "components" / "keycloak"
    keycloak.mkdir(parents=True, exist_ok=True)
    (keycloak / "demo-users.yaml").write_text(yaml.safe_dump({n: {"firstName": "Demo"} for n in names}))


def test_demo_user_password_is_declared_by_the_demo_users_file(tmp_path):
    """The generated demo logins are declared in demo-users.yaml, not in a component secrets.yaml."""
    demo_users(tmp_path, "demo.steward")
    result = check(
        secret_manifest("keycloak-user-demo-steward", {"password": "a" * 32}),
        {"demo": DECLARATION},
        tmp_path,
    )
    assert result.returncode == 0, result.stdout


def test_demo_user_password_shorter_than_32_is_refused(tmp_path):
    demo_users(tmp_path, "demo.steward")
    result = check(
        secret_manifest("keycloak-user-demo-steward", {"password": "a" * 8}),
        {"demo": DECLARATION},
        tmp_path,
    )
    assert result.returncode == 1
    assert "decodes to 8 characters, declared 32" in result.stdout


def test_an_approvers_totp_seed_is_declared_and_held_to_32_characters(tmp_path):
    """T-5120: a demo person holding portal-approver signs in with TOTP, so its Secret carries
    the seed beside the password, generated as long as every other credential."""
    keycloak = tmp_path / "components" / "keycloak"
    keycloak.mkdir(parents=True, exist_ok=True)
    (keycloak / "demo-users.yaml").write_text(yaml.safe_dump({"demo.steward": {"realmRoles": ["portal-approver"]}}))
    good = check(secret_manifest("keycloak-user-demo-steward", {"password": "a" * 32, "totp": "b" * 32}),
                 {"demo": DECLARATION}, tmp_path)
    assert good.returncode == 0, good.stdout
    missing = check(secret_manifest("keycloak-user-demo-steward", {"password": "a" * 32}), {"demo": DECLARATION}, tmp_path)
    assert missing.returncode == 1
    assert "declared key 'totp' is missing" in missing.stdout


def realm_secret(approver_mfa: bool) -> str:
    """The keycloak-config-cli realm Secret, its portal-approver with or without `mfa-required`."""
    approver = {"name": "portal-approver", "composite": approver_mfa}
    if approver_mfa:
        approver["composites"] = {"realm": ["mfa-required"]}
    realm = {"realm": "jc", "roles": {"realm": [approver, {"name": "mfa-required"}]}}
    data = {"jc.json": base64.b64encode(json.dumps(realm).encode()).decode()}
    return yaml.safe_dump({"apiVersion": "v1", "kind": "Secret",
                           "metadata": {"name": "keycloak-config-cli-config-realms"}, "data": data})


def test_an_approver_without_mfa_in_the_realm_needs_no_totp_seed(tmp_path):
    """T-5776: dev's realm drops `mfa-required` from portal-approver, so its demo approvers get no
    seed; the declaration follows the rendered realm, and a realm keeping it still asks for one."""
    keycloak = tmp_path / "components" / "keycloak"
    keycloak.mkdir(parents=True, exist_ok=True)
    (keycloak / "demo-users.yaml").write_text(yaml.safe_dump({"demo.steward": {"realmRoles": ["portal-approver"]}}))
    password_only = secret_manifest("keycloak-user-demo-steward", {"password": "a" * 32})
    off = check(realm_secret(False) + "---\n" + password_only, {"demo": DECLARATION}, tmp_path)
    assert off.returncode == 0, off.stdout
    on = check(realm_secret(True) + "---\n" + password_only, {"demo": DECLARATION}, tmp_path)
    assert on.returncode == 1 and "declared key 'totp' is missing" in on.stdout


def test_demo_user_secret_for_an_unnamed_user_is_refused(tmp_path):
    """A `keycloak-user-*` Secret nobody named in demo-users.yaml is still undeclared."""
    demo_users(tmp_path, "demo.steward")
    result = check(
        secret_manifest("keycloak-user-someone-else", {"password": "a" * 32}),
        {"demo": DECLARATION},
        tmp_path,
    )
    assert result.returncode == 1
    assert "no declaration" in result.stdout


def test_secret_without_keep_policy_is_not_counted(tmp_path):
    """Without resource-policy: keep a helm uninstall would take the credentials with it."""
    result = check(secret_manifest("db-demo", {"password": "a" * 32}, keep=False), {"demo": DECLARATION}, tmp_path)
    assert result.returncode == 1
    assert "the secrets component did not render" in result.stdout


@requires_sops
def test_sops_encrypted_value_reaches_the_generated_secret(tmp_path):
    """An operator-supplied credential travels `ref+sops://` -> helmfile -> Secret.

    helmfile resolves `ref+sops://` itself (vals), so no helm plugin is involved and the
    plaintext exists only in memory: what the environment file holds is a named reference,
    which is exactly what ADR-N-012 asks manifests to store.
    """
    key = tmp_path / "age.key"
    subprocess.run(["age-keygen", "-o", str(key)], check=True, capture_output=True)
    recipient = subprocess.run(
        ["age-keygen", "-y", str(key)], check=True, capture_output=True, text=True
    ).stdout.strip()

    plain = tmp_path / "plain.yaml"
    plain.write_text("keycloak:\n  adminPassword: sops-supplied-password-0123456789\n")
    encrypted = tmp_path / "secrets.enc.yaml"
    encrypted.write_text(
        subprocess.run(
            ["sops", "--encrypt", "--age", recipient, str(plain)],
            # Outside the repository: its .sops.yaml (T-1712) names the recipients of the
            # repository's own files, and this throwaway key is none of them.
            check=True, capture_output=True, text=True, cwd=tmp_path,
        ).stdout
    )
    assert "sops-supplied-password" not in encrypted.read_text(), "sops left the value in plaintext"

    helmfile = tmp_path / "helmfile.yaml"
    helmfile.write_text(
        textwrap.dedent(
            f"""\
            releases:
              - name: secrets-keycloak-admin-user
                chart: {GENERATOR_CHART}
                namespace: local
                values:
                  - secrets:
                      - name: keycloak-admin-user
                        namespaces: [local]
                        keys:
                          password: ref+sops://{encrypted}#/keycloak/adminPassword
            """
        )
    )
    env = dict(os.environ, SOPS_AGE_KEY_FILE=str(key))
    out = subprocess.run(
        ["helmfile", "-f", str(helmfile), "template", "--skip-deps"],
        capture_output=True, text=True, env=env, cwd=tmp_path,
    )
    assert out.returncode == 0, out.stderr
    docs = [d for d in load_yaml_all(out.stdout) if isinstance(d, dict)]
    secrets = [d for d in docs if d.get("kind") == "Secret"]
    assert len(secrets) == 1, out.stdout
    assert base64.b64decode(secrets[0]["data"]["password"]).decode() == "sops-supplied-password-0123456789"
    assert secrets[0]["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"


def generated_secrets(docs: list[dict]) -> list[dict]:
    return [d for d in docs if d.get("kind") == "Secret"
            and d["metadata"].get("labels", {}).get("helm.sh/chart", "").startswith("secrets-0.")]


def test_a_disabled_generator_renders_no_secret(rendered, rendered_variant):
    """T-5702 (F-DEP2-014): `enabled: false | default true` is true in sprig, so switching the
    generator off still installed every generated Secret."""
    assert generated_secrets(rendered("minimal")), "the generator renders Secrets when enabled"

    def disable(tree: Path):
        path = tree / "components/secrets/default-environment.yaml.gotmpl"
        text = path.read_text()
        assert "  generator:\n    enabled: true\n" in text
        path.write_text(text.replace("  generator:\n    enabled: true\n", "  generator:\n    enabled: false\n", 1))

    assert generated_secrets(rendered_variant("minimal", disable)) == []


# jc-core SecretRef::validate (envelope.rs env_var_refusal, T-5702): the names a pipeline's or a
# catalogue's secret may be injected under. Mirrored here so a seed that would be refused at the
# platform's next pin turns this repository red first.
ENV_VAR = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
PROCESS_ENV_VARS = {"PATH", "HOME", "TMPDIR", "SHELL", "GODEBUG", "GOMAXPROCS", "GOGC", "GOMEMLIMIT",
                    "GOTRACEBACK", "GOFLAGS", "SSLKEYLOGFILE"}


def env_var_refused(name: str) -> bool:
    return (not ENV_VAR.match(name) or name in PROCESS_ENV_VARS or name.endswith("_PROXY")
            or name.startswith(("JC_", "LD_", "SSL_CERT_")))


def test_every_seeded_secret_ref_env_var_passes_the_platform_rule():
    """Supervisor 2026-10-25T18:55Z (T-5702): the refused envVar list leaves every seeded
    secretRef valid."""
    seen = {}
    for path in sorted(PROJECT_ROOT.glob("components/**/*.yaml*")) + sorted(PROJECT_ROOT.glob("demos/**/*.yaml*")):
        for match in re.finditer(r"envVar:\s*[\"']?([^\s\"'#]+)", path.read_text(errors="replace")):
            seen.setdefault(match.group(1), path.relative_to(PROJECT_ROOT))
    assert seen, "no seeded envVar found: the scan is broken"
    refused = {name: str(path) for name, path in seen.items() if env_var_refused(name)}
    assert refused == {}
    assert not env_var_refused("CKAN_API_TOKEN") and env_var_refused("HTTPS_PROXY")
