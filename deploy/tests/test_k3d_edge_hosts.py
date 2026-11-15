"""T-4790: the upgrade job's smoke reaches every host it checks, over a verified chain.

ci-full 39347776680 ran scripts/smoke.sh after `just upgrade` with only the apex, idm and portal
in /etc/hosts and the harness's self-signed CA untrusted: 39 checks answered 000 and the green
upgrade rolled back. scripts/k3d-edge-hosts.sh lists the hosts the edge serves; the upgrade script
resolves them to the k3d edge and trusts cert-manager/ca-secret beside the system roots.
"""

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "k3d-edge-hosts.sh"


def hosts(domain: str) -> subprocess.CompletedProcess:
    return subprocess.run([str(SCRIPT), domain], capture_output=True, text=True)


def test_every_host_smoke_checks_is_listed():
    r = hosts("joinedcontext.test")
    assert r.returncode == 0, r.stderr
    listed = set(r.stdout.split())
    smoke = (ROOT / "scripts" / "smoke.sh").read_text()
    wanted = {f"{sub}.joinedcontext.test" for sub in re.findall(r"https://([a-z0-9-]+)\.\$\{base#https://\}", smoke)}
    assert wanted, "smoke.sh addresses no subdomain any more; the pattern above needs updating"
    assert wanted | {"joinedcontext.test", "idm.joinedcontext.test"} <= listed


def test_a_domain_with_shell_or_hosts_syntax_is_refused():
    for bad in ("x.test 10.0.0.1 evil", "x.test\n1.2.3.4 y", "$(id).test", ".test", "x.test."):
        r = hosts(bad)
        assert r.returncode == 2, bad
        assert r.stdout == ""


def test_the_upgrade_script_resolves_those_hosts_and_trusts_the_edge_ca():
    text = (ROOT / "scripts" / "ci-upgrade-e2e.sh").read_text()
    assert './scripts/k3d-edge-hosts.sh "$DOMAIN"' in text
    assert "cert-manager get secret ca-secret" in text
    assert "export CURL_CA_BUNDLE=" in text and "SSL_CERT_FILE=" in text
    # verification stays on: nothing in the job turns it off
    assert not re.search(r"curl[^\n]*\s-k\b|--insecure|CURL_INSECURE", text)
    # the hosts are in place before the green upgrade smokes them
    assert text.index("k3d-edge-hosts.sh") < text.index('just upgrade "$ENV" "$NEXT" --domain "$DOMAIN"')
