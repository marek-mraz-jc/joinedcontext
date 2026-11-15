"""T-6444: one kube-hetzner module, one tfvars per cluster, state outside Git, guarded recipes."""

import ipaddress
import os
import re
import stat
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
HETZNER = ROOT / "cluster/hetzner"
MODULE = HETZNER / "module"
SCRIPT = HETZNER / "cluster.sh"
TFVARS = sorted((HETZNER / "clusters").glob("*.tfvars"))
TOKEN = "t" * 64

# Stands in for terraform: logs its arguments and environment, saves a plan, answers `show -json`.
FAKE_TERRAFORM = r"""#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_DIR/argv"
printf '%s\n' "$TF_VAR_hcloud_token" >"$FAKE_DIR/token"
for a in "$@"; do
  case $a in
    -out=*) touch "${a#-out=}" ;;
    -json) cat "$FAKE_DIR/plan.json"; exit 0 ;;
  esac
done
case " $* " in *" output "*) echo fake-kubeconfig ;; esac
"""
NO_LOSS = '{"resource_changes": [{"type": "hcloud_server", "address": "a.b", "change": {"actions": ["update"]}}]}'
LOSS = ('{"resource_changes": [{"type": "hcloud_server", "address": "module.kube-hetzner.x",'
        ' "change": {"actions": ["delete", "create"]}}]}')


def test_three_clusters_are_declared():
    assert [p.stem for p in TFVARS] == ["dev", "smartcity", "stage"]


@pytest.mark.parametrize("path", TFVARS, ids=lambda p: p.stem)
def test_tfvars_names_only_declared_variables(path):
    declared = set(re.findall(r'^variable "(\w+)"', (MODULE / "variables.tf").read_text(), re.M))
    used = set(re.findall(r"^(\w+)\s*=", path.read_text(), re.M))
    assert used and used <= declared, used - declared


def test_every_variable_without_a_default_is_set_or_set_by_the_script():
    text = (MODULE / "variables.tf").read_text()
    required = {name for name, body in re.findall(r'^variable "(\w+)" \{\n(.*?)^\}', text, re.M | re.S)
                if "default" not in body}
    for path in TFVARS:
        used = set(re.findall(r"^(\w+)\s*=", path.read_text(), re.M))
        assert required - used == {"hcloud_token", "ssh_key_dir"}, path.stem


@pytest.mark.parametrize("path", TFVARS, ids=lambda p: p.stem)
def test_tfvars_holds_no_token_like_string(path):
    text = path.read_text()
    assert not re.search(r"\b[A-Za-z0-9_+/-]{40,}\b", text)
    names = set(re.findall(r"^(\w+)\s*=", text, re.M)) - {"enable_secrets_encryption"}
    assert not [n for n in names if re.search(r"(?i)token|secret|password", n)]


@pytest.mark.parametrize("path", TFVARS, ids=lambda p: p.stem)
def test_admin_cidrs_are_never_open(path):
    line = re.search(r"^firewall_allowed_ips\s*=\s*\[(.*?)\]", path.read_text(), re.M).group(1)
    cidrs = re.findall(r'"([^"]+)"', line)
    # One host each (the agents' server; its address is private, the private environments/tests, T-6984);
    # a person's address goes in private.tfvars.
    assert cidrs and all(ipaddress.ip_network(c).prefixlen == 32 for c in cidrs), cidrs


def test_module_refuses_an_open_cidr():
    text = (MODULE / "variables.tf").read_text()
    for name in ("firewall_allowed_ips", "firewall_extra_ips"):
        assert f'!endswith(c, "/0")' in text.split(f'variable "{name}"')[1].split("\nvariable ")[0]
    main = (MODULE / "main.tf").read_text()
    assert "firewall_kube_api_source = local.admin_cidrs" in main
    assert "firewall_ssh_source      = local.admin_cidrs" in main


def test_module_pins_kube_hetzner_and_providers_exactly():
    main = (MODULE / "main.tf").read_text()
    assert re.search(r'source\s+= "kube-hetzner/kube-hetzner/hcloud"\n\s+version = "\d+\.\d+\.\d+"', main)
    block = main.split("required_providers {")[1]
    pinned = dict(re.findall(r'(\w+) = \{\n\s+source\s+= "[^"]+"\n\s+version = "(\d+\.\d+\.\d+)"', block))
    assert set(pinned) == {"hcloud", "tls"}
    lock = (MODULE / ".terraform.lock.hcl").read_text()
    for name, version in pinned.items():
        assert re.search(rf'/{name}" \{{\n\s+version\s+= "{re.escape(version)}"\n', lock), name


@pytest.mark.parametrize("path", ["terraform.tfstate", "terraform.tfstate.backup", "x.tfstate.1.backup",
                                  "cluster/hetzner/module/terraform.tfstate", "cluster/hetzner/stage.tfplan",
                                  "cluster/hetzner/module/.terraform/x", "cluster/hetzner/module/k3s_kubeconfig.yaml"])
def test_gitignore_covers_state_and_plans(path):
    assert subprocess.run(["git", "check-ignore", "-q", "--no-index", path], cwd=ROOT).returncode == 0, path


def test_lock_file_is_not_ignored():
    assert subprocess.run(["git", "check-ignore", "-q", "--no-index", "cluster/hetzner/module/.terraform.lock.hcl"],
                          cwd=ROOT).returncode == 1


def test_a_single_node_cluster_keeps_the_controller_manager_flag_with_leader_election_off():
    for name in ("dev", "stage"):
        text = (HETZNER / f"clusters/{name}.tfvars").read_text()
        assert "flex-volume-plugin-dir=/var/lib/kubelet/volumeplugins" in text
        assert text.count('"leader-elect=false"') == 3


def test_mqtt_ports_render_only_when_the_cluster_opens_them():
    text = (MODULE / "extra-manifests/40-traefik-linkerd.yaml.tpl").read_text()
    assert text.count('%{ if mqtt_ports_open == "true"') == 2
    assert "mqtt_ports_open             = var.mqtt_ports_open" in (MODULE / "main.tf").read_text()


@pytest.fixture
def run(tmp_path):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    fake = bin_dir / "terraform"
    fake.write_text(FAKE_TERRAFORM)
    fake.chmod(0o755)
    secrets = tmp_path / "secrets"
    secrets.mkdir()
    for project in ("stage", "prod", "dev"):
        (secrets / f"hcloud-token-{project}").write_text(TOKEN)
    (tmp_path / "plan.json").write_text(NO_LOSS)

    def call(action, name, stdin="", **env):
        full = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "FAKE_DIR": str(tmp_path),
                "CLUSTER_STATE_DIR": str(tmp_path / "state"), "SECRETS_DIR": str(secrets), **env}
        full.pop("ALLOW_REPLACE", None) if "ALLOW_REPLACE" not in env else None
        if "AGENT_NAME" not in env:
            full.pop("AGENT_NAME", None)
        return subprocess.run([str(SCRIPT), action, name], input=stdin, env=full, capture_output=True, text=True)

    call.dir = tmp_path
    return call


def argv(run):
    log = run.dir / "argv"
    return log.read_text() if log.exists() else ""


def test_plan_saves_a_plan_in_a_private_state_directory(run):
    done = run("plan", "stage")
    assert done.returncode == 0, done.stderr
    state = run.dir / "state/stage"
    assert (state / "stage.tfplan").exists()
    assert stat.S_IMODE(state.stat().st_mode) == 0o700
    assert stat.S_IMODE((state / "ssh").stat().st_mode) == 0o700
    assert stat.S_IMODE((state / "ssh/ssh-stage").stat().st_mode) == 0o600
    assert f"path={state}/terraform.tfstate" in argv(run)


def test_the_token_reaches_terraform_only_through_the_environment(run):
    assert run("plan", "stage").returncode == 0
    assert (run.dir / "token").read_text().strip() == TOKEN
    assert TOKEN not in argv(run)


def test_plan_stops_on_a_lost_server_and_drops_the_plan(run):
    (run.dir / "plan.json").write_text(LOSS)
    done = run("plan", "stage")
    assert done.returncode == 1
    assert "ALLOW_REPLACE=stage" in done.stderr and "module.kube-hetzner.x" in done.stderr
    assert not (run.dir / "state/stage/stage.tfplan").exists()


def test_allow_replace_must_name_this_cluster(run):
    (run.dir / "plan.json").write_text(LOSS)
    assert run("plan", "stage", ALLOW_REPLACE="dev").returncode == 1
    assert run("plan", "stage", ALLOW_REPLACE="stage").returncode == 0


def test_apply_checks_the_saved_plan_again(run):
    (run.dir / "plan.json").write_text(LOSS)
    assert run("plan", "stage", ALLOW_REPLACE="stage").returncode == 0
    assert run("apply", "stage").returncode == 1
    assert " apply " not in f" {argv(run)} ".replace("\n", " ")


def test_apply_refuses_without_a_saved_plan(run):
    done = run("apply", "stage")
    assert done.returncode == 1 and "no saved plan" in done.stderr


def test_apply_takes_the_saved_plan_and_consumes_it(run):
    assert run("plan", "stage").returncode == 0
    done = run("apply", "stage")
    assert done.returncode == 0, done.stderr
    state = run.dir / "state/stage"
    assert f"apply -input=false {state}/stage.tfplan" in argv(run)
    assert not (state / "stage.tfplan").exists()
    assert stat.S_IMODE((state / "kubeconfig.yaml").stat().st_mode) == 0o600


def test_destroy_refuses_smartcity_even_for_prod_deployer(run):
    done = run("destroy", "smartcity", stdin="smartcity\nsmartcity\n", AGENT_NAME="prod-deployer")
    assert done.returncode == 1 and "never destroys" in done.stderr
    assert argv(run) == ""


@pytest.mark.parametrize("typed", ["", "stage\n", "stage\nstaeg\n", "dev\ndev\n"])
def test_destroy_needs_the_name_typed_twice(run, typed):
    assert run("destroy", "stage", stdin=typed).returncode == 1
    assert "destroy -input" not in argv(run)


def test_destroy_runs_after_the_name_typed_twice(run):
    assert run("destroy", "stage", stdin="stage\nstage\n").returncode == 0
    assert "destroy -input=false -auto-approve" in argv(run)


def test_smartcity_belongs_to_prod_deployer(run):
    assert run("plan", "smartcity").returncode == 1
    assert run("plan", "smartcity", AGENT_NAME="worker-1").returncode == 1
    assert argv(run) == ""
    assert run("plan", "smartcity", AGENT_NAME="prod-deployer").returncode == 0


def test_a_person_s_address_comes_from_the_state_directory(run):
    assert run("plan", "stage").returncode == 0
    assert "private.tfvars" not in argv(run)
    (run.dir / "state/stage/private.tfvars").write_text('firewall_extra_ips = ["192.0.2.1/32"]\n')
    assert run("plan", "stage").returncode == 0
    assert "private.tfvars" in argv(run)


def test_state_with_no_key_is_refused(run):
    state = run.dir / "state/stage"
    (state / "ssh").mkdir(parents=True)
    (state / "terraform.tfstate").write_text("{}")
    done = run("plan", "stage")
    assert done.returncode == 1 and "copy the cluster's key" in done.stderr


@pytest.mark.parametrize("name", ["nope", "../dev", ""])
def test_unknown_cluster_is_refused(run, name):
    assert run("plan", name).returncode == 1
    assert argv(run) == ""


def test_missing_token_is_refused(run):
    (run.dir / "secrets/hcloud-token-stage").unlink()
    done = run("plan", "stage")
    assert done.returncode == 1 and "no token" in done.stderr
