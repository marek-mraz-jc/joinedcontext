"""T-2873: no demo work and no city dataset is refused by a quota (owner, 2026-09-25, PF-73).

The praha project on dev refused its pipelines at a resident quota of one. Every Project and
Organization the seed ships allows at least 100 context spaces, pipelines, public endpoints and
apps, and runtime ceilings a city dataset does not reach. The quota stays as the mechanism:
each dimension is still set, so a project the Portal opens later still has one to raise.
"""

from pathlib import Path

from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"

FLOORS = {
    "contextSpaces": 100,
    "residentPipelines": 100,
    "publicEndpoints": 100,
    "apps": 100,
    "ingestEventsPerSecond": 500,
    "agentRunsPerDay": 2000,
    "entitiesPerSpace": 1_000_000,
    "requestsPerMinute": 6000,
}


def seeded_quotas() -> list[tuple[str, dict]]:
    found = []
    for path, doc in seed_documents():
        if not isinstance(doc, dict):
            continue
        # `mqtt` is the project's MQTT allowance, no PF-73 dimension: a `spec.quotas` holding only
        # it leaves every dimension to the organization (portal src/quotas.rs effective()).
        own = {k: v for k, v in doc.get("spec", {}).get("quotas", {}).items() if k != "mqtt"}
        if doc.get("kind") == "Project" and own:
            found.append((path.relative_to(SEED).as_posix(), own))
        elif doc.get("kind") == "Organization" and "quota" in doc.get("spec", {}).get("projects", {}):
            found.append((path.relative_to(SEED).as_posix(), doc["spec"]["projects"]["quota"]))
    return found


def short_of_the_floor(quota: dict) -> list[str]:
    return [
        f"{dimension} is {quota.get(dimension)!r}, below {floor}"
        for dimension, floor in FLOORS.items()
        if not isinstance(quota.get(dimension), int) or quota[dimension] < floor
    ]


def test_every_seeded_quota_leaves_room_for_the_demo():
    quotas = seeded_quotas()
    # praha, the two Bystrica projects, helsinki-mobility and the helsinki organization.
    assert len(quotas) >= 5, quotas
    problems = [f"{path}: {p}" for path, quota in quotas for p in short_of_the_floor(quota)]
    assert not problems, "\n".join(problems)


def test_a_quota_as_small_as_the_one_that_refused_praha_is_named():
    refused_on_dev = dict(FLOORS, residentPipelines=1)
    assert short_of_the_floor(refused_on_dev) == ["residentPipelines is 1, below 100"]
    assert short_of_the_floor({k: v for k, v in FLOORS.items() if k != "apps"}) == [
        "apps is None, below 100"
    ]


# --- zip downloads (T-5701, F-BNT1-010) ------------------------------------------------------------

ROOT = SEED.parent.parent.parent


def zip_unpacks(processors: list, inside_try: bool = False):
    """Every `unarchive: zip` of a processor list, with whether a size gate guards it: a mapping
    that throws on `content().length()` earlier in the same `try:` list, so a refused download
    skips the unpack (outside a `try:`, Bento runs the unpack on the errored message anyway)."""
    for i, step in enumerate(processors or []):
        if not isinstance(step, dict):
            continue
        unarchive = step.get("unarchive")
        if isinstance(unarchive, dict) and unarchive.get("format") == "zip":
            before = [s.get("mapping", "") for s in processors[:i] if isinstance(s, dict)]
            yield inside_try and any("content().length() >" in m and "throw(" in m for m in before)
        for key, child in step.items():
            if key in ("try", "catch") or (isinstance(child, dict) and "processors" in child):
                yield from zip_unpacks(child if isinstance(child, list) else child["processors"], key == "try")
            elif key == "switch":
                for case in child:
                    yield from zip_unpacks(case.get("processors", []))


def bento_files() -> list[Path]:
    return sorted([*SEED.rglob("*-bento.yaml"), *(ROOT / "seeds").rglob("*.yaml")])


def test_every_zip_a_mapping_unpacks_is_size_capped_first():
    import yaml

    seen, ungated = 0, []
    for path in bento_files():
        doc = yaml.safe_load(path.read_text())
        if not isinstance(doc, dict) or not isinstance(doc.get("pipeline"), dict):
            continue
        for gated in zip_unpacks(doc["pipeline"].get("processors", [])):
            seen += 1
            if not gated:
                ungated.append(path.relative_to(ROOT).as_posix())
    assert seen >= 3, "the zip pipelines of the seed are found"
    assert not ungated, f"unarchive zip without a content().length() gate before it in the same try: {ungated}"


def test_every_zip_a_datasource_reads_is_bounded_on_its_inflated_size():
    """The runner's own archive reader inflates a member only up to maxBytes (T-3951)."""
    unbounded = [path.relative_to(SEED).as_posix() for path, doc in seed_documents()
                 if isinstance(doc, dict) and doc.get("kind") == "DataSource"
                 and (doc.get("spec", {}).get("http") or {}).get("archive", {}).get("format") == "zip"
                 and not doc["spec"]["http"]["archive"].get("maxBytes")]
    assert not unbounded, unbounded
