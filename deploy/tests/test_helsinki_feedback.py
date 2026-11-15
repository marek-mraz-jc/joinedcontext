"""The City of Helsinki's published feedback becomes SDM service_requests, typed ServiceRequest (T-5257).

Bento runs the committed mapping over six reports recorded from the Open311 Issue Reporting API on
2026-10-22 (`fixtures/helsinki/open311/requests.json`, with the services list beside it) and six
rows the test made from the first: a point in Tampere, a swapped latitude and longitude, a report
filed in 2099, a status Open311 does not name, a sector code the services list does not hold, a report
listed twice and one whose text and HTML answer carry an email, a phone number and a signed name. The request per date window is the cut: the test answers it with the
recorded list. Without a container runtime the Bento cases skip.
"""

import json
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-feedback-bento.yaml"
FIXTURES = Path(__file__).resolve().parent / "fixtures/helsinki/open311"
URN = "urn:ngsi-ld:ServiceRequest:hel.fi:helsinki:open311-{}"


def run(services: bytes, requests: list) -> tuple[list[dict], str]:
    steps = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    for step in steps:
        if "branch" in step:
            http = step["branch"]["processors"][-1]["http"]
            assert http["url"].startswith("https://palautteet.hel.fi/public-api/open311-public-service/v1/requests.json?")
            step["branch"]["processors"] = [{"mapping": f"root = {json.dumps(requests)}"}]
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps},
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "info", "format": "logfmt"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=services, capture_output=True, timeout=120)
    assert result.returncode == 0, result.stderr.decode()
    windows = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]
    return windows, result.stderr.decode()


@requires_docker
def test_recorded_reports_map_to_valid_service_requests_and_bad_rows_are_counted():
    requests = json.loads((FIXTURES / "requests.json").read_text())
    windows, log = run((FIXTURES / "services.json").read_bytes(), requests)
    # Outside 02 UTC two six-day windows are read; each answers the same list here.
    assert len(windows) in (2, 15)
    reports = {e["id"]: e for e in windows[0]}
    assert sorted(reports) == sorted(URN.format(n) for n in (
        "3578865", "3578445", "3579320", "3577836", "3582455", "3576064", "9000001", "9000002", "9000005", "9000006"))
    counted = ('by reason {\\"duplicate\\":1,\\"filed in the future\\":1,\\"no position (kept without a point)\\":1,'
               '\\"outside Helsinki (kept without a point)\\":2,\\"unknown status\\":1}')
    assert counted in log, log
    for entity in reports.values():
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
        # SDM names the type service_requests; the space types it ServiceRequest (helsinki.linkml.yaml).
        errors = [e for e in open_data.sdm_errors("dataModel.IssueTracking/service_requests", entity)
                  if not e.startswith("type:")]
        assert not errors, (entity["id"], errors)
        assert not {"media_url", "email", "first_name", "last_name", "phone", "device_id"} & set(entity)
    open_ = reports[URN.format("3578865")]
    assert open_["status"]["value"] == "open" and open_["location"]["value"]["type"] == "Point"
    assert "@none" in open_["description"]["languageMap"]
    assert reports[URN.format("3579320")]["status"]["value"] == "closed"
    assert reports[URN.format("3576064")]["service_name"]["value"] == "General and other feedback"
    assert "location" not in reports[URN.format("3582455")], "a report filed with no point keeps none"
    assert "location" not in reports[URN.format("9000001")] and "location" not in reports[URN.format("9000002")]
    answer = reports[URN.format("3579320")]["status_notes"]["value"]
    assert "<" not in answer and "&nbsp;" not in answer and answer.startswith("Hei!\nKyseinen kohta")
    private = reports[URN.format("9000006")]
    text, notes = private["description"]["languageMap"]["@none"], private["status_notes"]["value"]
    assert text == "Katuvalo rikki.\nSoittakaa [phone removed], [phone removed] tai [phone removed], [email removed]\nHavaittu 2026-10-19. Auto [plate removed] ja [plate removed] esteenä, ei ABCD-1234.", text
    assert notes == "Hei!\nKorjattu 22.10.2026. Lisätietoja: [email removed], puh. [phone removed].", notes
    unknown = reports[URN.format("9000005")]
    assert unknown["service_code"]["value"] == "9999" and "service_name" not in unknown


@requires_docker
def test_an_empty_services_list_fails_the_run():
    steps = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][:1]
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps},
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "error"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=b"[]", capture_output=True, timeout=120)
    assert b"the feedback system lists no service" in result.stderr + result.stdout


def test_the_endpoint_publishes_only_service_requests_with_its_sources_named():
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-feedback.yaml").read_text())
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-feedback.yaml").read_text())
    assert endpoint["spec"]["policyRef"].endswith(":" + policy["metadata"]["name"])
    assert policy["spec"]["information"] == [{"entities": [{"type": "ServiceRequest"}]}]
    assert endpoint["spec"]["catalog"]["pipelineRef"]["name"] == "feedback-requests"
    assert "CC BY 4.0" in endpoint["spec"]["catalog"]["attribution"]["en"]
