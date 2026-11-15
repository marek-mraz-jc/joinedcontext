"""deploy/ci/public-check.py: one planted file per rule is refused by file, line and rule, never
by value; a clean tree passes (docs/Deployment/01-repository-layout.md §4)."""
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "ci" / "public-check.py"
# Built from parts, so this file itself holds no value of a refused class.
AGE = "age1" + "q" * 58
ADDR = ".".join(["5", "6", "7", "8"])  # no documentation range: a global address
PLANTED = {
    "address.txt": (f"server: {ADDR}\n", "a global IPv4 address"),
    "recipient.yaml": (f"recipient: {AGE}\n", "an age recipient"),
    "secret.yaml": ("password: ENC[AES256_GCM" + ",data:abc]\n", "a sops value or mapping"),
    "mapping.yaml": ("a: 1\nsops:\n  version: 3\n", "a sops value or mapping"),
    "prod.tfvars": ("name = \"x\"\n", "a .tfvars file"),
    ".gitleaksignore": ("f" * 40 + ":a.rs:generic-api-key:1\n", "a .gitleaksignore sha that is no commit"),
}
CLEAN = (
    "dns: 1.1.1.1 9.9.9.9\n"
    "test: 203.0.113.10 198.51.100.7 192.0.2.1 10.0.0.5\n"
    "version: 1.2.3.4.5\n"
    "see clause 4.5.6.7\n"
    "pages: 185.199.109.133\n"
)


def git(tmp, *args):
    subprocess.run(["git", "-C", str(tmp), *args], check=True, capture_output=True)


def run(tmp):
    return subprocess.run([sys.executable, "-I", str(SCRIPT), "."], cwd=tmp, capture_output=True, text=True)


@pytest.fixture
def repo(tmp_path):
    git(tmp_path, "init", "-q")
    (tmp_path / "clean.txt").write_text(CLEAN)
    git(tmp_path, "add", "-A")
    return tmp_path


def test_a_clean_tree_passes(repo):
    r = run(repo)
    assert r.returncode == 0, r.stderr
    assert "clean" in r.stderr


@pytest.mark.parametrize("name", sorted(PLANTED))
def test_each_rule_refuses_without_printing_the_value(repo, name):
    text, rule = PLANTED[name]
    (repo / name).write_text(text)
    git(repo, "add", "-A")
    r = run(repo)
    assert r.returncode == 1, r.stderr
    assert f"{name}" in r.stderr and rule in r.stderr, r.stderr
    for value in (ADDR, AGE, "ENC[", "f" * 40):
        assert value not in r.stderr


def test_an_untracked_file_is_not_read(repo):
    (repo / "address.txt").write_text(f"server: {ADDR}\n")
    assert run(repo).returncode == 0


def test_the_deploy_tree_is_clean():
    root = SCRIPT.parent.parent
    r = subprocess.run([sys.executable, "-I", str(SCRIPT), "."], cwd=root, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
