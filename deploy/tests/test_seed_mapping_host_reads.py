"""Every seeded mapping reads only what the Portal lets a mapping read (PL-16, PL-50): `env()` of
JC_ORG_DOMAIN, JC_SOURCE_SPACE, JC_SPACE or JC_SPACE_<n>, and no file or host. The Portal refuses
the whole pipeline otherwise (jc-core `validate_author_bloblang`); banskabystrica/kataster sat in
Error on prod for reading JC_KATASTER_THIRD (T-4753)."""

import re
from pathlib import Path

SEED = Path(__file__).resolve().parents[1] / "components"
INJECTED = re.compile(r"JC_ORG_DOMAIN|JC_SOURCE_SPACE|JC_SPACE(_[0-9]+)?")
HOST = re.compile(r"\b(env|file|file_json|file_rel|file_rel_json|hostname)\(\s*(\"([^\"]*)\")?")


def test_every_seeded_mapping_reads_only_the_injected_variables():
    reads = [
        (path.relative_to(SEED), call.group(0))
        for path in SEED.rglob("*bento.yaml")
        for call in HOST.finditer(path.read_text())
        if call.group(1) != "env" or not INJECTED.fullmatch(call.group(3) or "")
    ]
    assert reads == []


def test_the_check_catches_a_variable_of_its_own():
    assert HOST.search('env("JC_KATASTER_THIRD")').group(3) == "JC_KATASTER_THIRD"
    assert not INJECTED.fullmatch("JC_KATASTER_THIRD")
    assert INJECTED.fullmatch("JC_SPACE_2")
