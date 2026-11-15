"""The pinned Portal reads every field the seed's spaces carry (T-4054).

The Portal drops a manifest whole when its jc-core does not know a field: on 2026-10-12 the
Helsinki space gained `quality` and `retention` (T-3904) while dev still ran a Portal built on a
jc-core without them (T-3899), and the Portal removed every Helsinki view; all its Endpoints
answered 404. The portal pin names the platform commit its jc-core is built from (`platform` in
components/portal/images.yaml); a space field the seed carries needs that commit to contain the
one that taught jc-core the field.

Whether one commit contains another is asked of a platform clone (`JC_PLATFORM_DIR`, or the
sibling joinedcontext-platform folder) or else of GitHub's compare API; without either the
ancestry case skips, and the format case still holds.
"""

import functools
import json
import os
import re
import subprocess
import urllib.request
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
PLATFORM_REPO = "marek-mraz-jc/joinedcontext-platform"
# One worker reads the whole seed once per platform commit (refusals() caches it).
pytestmark = pytest.mark.xdist_group("portal-pin-seed")

# A ContextSpace spec field, and the platform commit whose jc-core first reads it.
SINCE = {
    "quality": "2883bd06a17ebe8568f37579d0578025fc9eb963",  # T-3899
    "retention": "2883bd06a17ebe8568f37579d0578025fc9eb963",  # T-3899
}

# A retention `history` word, and the platform commit whose jc-core first accepts it: an older
# one refuses the value and drops the space as it drops an unknown field (T-4275).
HISTORY_SINCE = {
    "off": "1e5b0ec9938bd530c6e810e86d872d4dae1442fb",  # T-4275
    "forever": "1e5b0ec9938bd530c6e810e86d872d4dae1442fb",  # T-4275
}


def pinned_platform() -> str:
    return yaml.safe_load((ROOT / "components/portal/images.yaml").read_text())["portal"]["portal"].get("platform", "")


def space_fields() -> dict[str, list[str]]:
    """Each field of SINCE a seed space carries, with the files that carry it."""
    found: dict[str, list[str]] = {}
    for path in sorted(ROOT.glob("components/*/seed/**/*.yaml")):
        for document in yaml.safe_load_all(path.read_text()):
            if isinstance(document, dict) and document.get("kind") == "ContextSpace":
                for field in SINCE.keys() & (document.get("spec") or {}).keys():
                    found.setdefault(field, []).append(str(path.relative_to(ROOT)))
    return found


def contains(pin: str, since: str) -> bool | None:
    """Whether platform commit `pin` contains `since`; None when nothing can answer."""
    clone = Path(os.environ.get("JC_PLATFORM_DIR", ROOT.parent / "joinedcontext-platform"))
    if (clone / ".git").exists():
        result = subprocess.run(["git", "-C", str(clone), "merge-base", "--is-ancestor", since, pin],
                                capture_output=True, text=True)
        if result.returncode in (0, 1):
            return result.returncode == 0
    request = urllib.request.Request(f"https://api.github.com/repos/{PLATFORM_REPO}/compare/{since}...{pin}")
    if os.environ.get("GITHUB_TOKEN"):
        request.add_header("Authorization", f"Bearer {os.environ['GITHUB_TOKEN']}")
    try:
        with urllib.request.urlopen(request, timeout=20) as answer:
            return json.load(answer)["status"] in ("ahead", "identical")
    except (OSError, KeyError, ValueError):
        return None


def test_the_portal_pin_names_its_platform_commit():
    assert re.fullmatch(r"[0-9a-f]{40}", pinned_platform()), "portal images.yaml: platform is the full commit"


def test_a_space_field_in_the_seed_is_one_the_pinned_portal_reads():
    fields = space_fields()
    assert set(fields) <= set(SINCE)
    pin = pinned_platform()
    for field, files in fields.items():
        known = contains(pin, SINCE[field])
        if known is None:
            pytest.skip("no platform clone and no answer from GitHub")
        assert known, (
            f"{files} carry `{field}`, which jc-core reads from platform {SINCE[field][:7]} on; "
            f"the pinned Portal is built on {pin[:7]}: pin a Portal that has it in the same commit"
        )


# An AssistantDeployment spec field, and the platform commit whose jc-core first reads it: an older
# Portal or assistant refuses it and drops the deployment whole (T-4343, the T-4296 lesson).
ASSISTANT_SINCE = {
    "widget": "9fdbd9bddd8e1ee677963614df3d25daf8c0c40a",  # T-4343
}


def assistant_fields() -> dict[str, list[str]]:
    """Each field of ASSISTANT_SINCE a seed AssistantDeployment carries, with the files that carry it."""
    found: dict[str, list[str]] = {}
    for path in sorted(ROOT.glob("components/*/seed/**/*.yaml")):
        for document in yaml.safe_load_all(path.read_text()):
            if isinstance(document, dict) and document.get("kind") == "AssistantDeployment":
                for field in ASSISTANT_SINCE.keys() & (document.get("spec") or {}).keys():
                    found.setdefault(field, []).append(str(path.relative_to(ROOT)))
    return found


def test_an_assistant_field_in_the_seed_is_one_the_pinned_portal_reads():
    pin = pinned_platform()
    for field, files in assistant_fields().items():
        known = contains(pin, ASSISTANT_SINCE[field])
        if known is None:
            pytest.skip("no platform clone and no answer from GitHub")
        assert known, (
            f"{files} carry `{field}`, which jc-core reads from platform {ASSISTANT_SINCE[field][:7]} on; "
            f"the pinned Portal is built on {pin[:7]}: pin a Portal (and an assistant) that has it first"
        )


def test_the_widget_field_is_caught_on_todays_pin():
    """platform 77d9a82, the Portal pin before T-4343, has no `widget`."""
    known = contains("77d9a825a46beb3cbf45bf512d922b2a1c6e7b75", ASSISTANT_SINCE["widget"])
    if known is None:
        pytest.skip("no platform clone and no answer from GitHub")
    assert known is False


def history_words() -> dict[str, list[str]]:
    """Each word of HISTORY_SINCE a seed space's retention says, with the files that say it."""
    found: dict[str, list[str]] = {}
    for path in sorted(ROOT.glob("components/*/seed/**/*.yaml")):
        for document in yaml.safe_load_all(path.read_text()):
            if isinstance(document, dict) and document.get("kind") == "ContextSpace":
                for rule in (document.get("spec") or {}).get("retention") or []:
                    if rule.get("history") in HISTORY_SINCE:
                        found.setdefault(rule["history"], []).append(str(path.relative_to(ROOT)))
    return found


def test_a_history_word_in_the_seed_is_one_the_pinned_portal_accepts():
    pin = pinned_platform()
    for word, files in history_words().items():
        known = contains(pin, HISTORY_SINCE[word])
        if known is None:
            pytest.skip("no platform clone and no answer from GitHub")
        assert known, (
            f"{sorted(set(files))} say `history: {word}`, which jc-core accepts from platform "
            f"{HISTORY_SINCE[word][:7]} on; the pinned Portal is built on {pin[:7]}: pin a Portal that has it"
        )


def test_an_older_pin_is_caught():
    """The pin dev ran when the Helsinki views went (portal 2c47e68, platform 5a2c609)."""
    known = contains("5a2c6091c1ccb05da2708d3d7f6f78194c92722d", SINCE["quality"])
    if known is None:
        pytest.skip("no platform clone and no answer from GitHub")
    assert known is False


# A Pipeline step kind, and the platform commit whose jc-core first renders it: an older Portal refuses
# an unknown compute kind and the pipeline stops (T-5382 converted 45 ŠÚ SR pipelines to `datacube`).
STEP_SINCE = {
    "datacube": "12d5f50ba5a469d742a62ebff027f17e6bffcdb8",  # T-5295
}


def step_kinds() -> dict[str, list[str]]:
    """Each kind of STEP_SINCE a seed Pipeline's steps use, with the files that use it."""
    found: dict[str, list[str]] = {}
    for path in sorted(ROOT.glob("components/*/seed/**/*.yaml")):
        for document in yaml.safe_load_all(path.read_text()):
            if isinstance(document, dict) and document.get("kind") == "Pipeline":
                for step in (document.get("spec") or {}).get("steps") or []:
                    if step.get("kind") in STEP_SINCE:
                        found.setdefault(step["kind"], []).append(str(path.relative_to(ROOT)))
    return found


def test_a_step_kind_in_the_seed_is_one_the_pinned_portal_renders():
    kinds = step_kinds()
    assert kinds, "the susr-* pipelines carry datacube steps"
    pin = pinned_platform()
    for kind, files in kinds.items():
        known = contains(pin, STEP_SINCE[kind])
        if known is None:
            pytest.skip("no platform clone and no answer from GitHub")
        assert known, (
            f"{len(files)} pipelines ({files[0]}, ...) use `kind: {kind}`, which jc-core renders from platform "
            f"{STEP_SINCE[kind][:7]} on; the pinned Portal is built on {pin[:7]}: pin a Portal that has it first"
        )


def test_the_datacube_step_is_caught_on_the_pin_before_it():
    """platform 0069530, the pin this file named before T-5382, has no datacube step."""
    known = contains("0069530d691bd35438d6eac0c3367e8adb2c0a8c", STEP_SINCE["datacube"])
    if known is None:
        pytest.skip("no platform clone and no answer from GitHub")
    assert known is False


# The whole seed against each pinned jc-core (T-5357). The SINCE maps above hold only the fields someone
# listed: deployment 653fd563 carried `spec.catalog.harvest` (T-4744) under portal 299564d, whose jc-core
# (platform 5a24c42) predates it, and prod's Portal refused 9 Endpoints and withdrew 81 CKAN datasets.
# jc-core's committed schemas/kinds/*.json are what its serde types accept, with two known gaps: a
# templated `{orgDomain}` URN and a few custom string forms fail every commit's schema. So a seed document
# fails only where the pinned commit's schema refuses something platform main's schema accepts.
def platform_clone() -> Path:
    return Path(os.environ.get("JC_PLATFORM_DIR", ROOT.parent / "joinedcontext-platform"))


def git_show(clone: Path, commit: str, path: str) -> str | None:
    result = subprocess.run(["git", "-C", str(clone), "show", f"{commit}:{path}"], capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else None


def unavailable(why: str):
    """CI fetches every commit this gate reads (ci.yml), so there a missing one is a red, not a skip."""
    if os.environ.get("CI"):
        pytest.fail(why)
    pytest.skip(why)


def portal_jc_core_rev(cargo_toml: str) -> str:
    match = re.search(r'^jc-core = \{[^}]*\brev = "([0-9a-f]{40})"', cargo_toml, re.MULTILINE)
    assert match, "the portal's Cargo.toml pins jc-core by a full `rev`"
    return match.group(1)


def pinned_commits() -> dict[str, str]:
    images = yaml.safe_load((ROOT / "components/portal/images.yaml").read_text())["portal"]["portal"]
    gateway = yaml.safe_load((ROOT / "components/context-gateway/images.yaml").read_text())["context-gateway"]["gateway"]
    return {"portal": images["commit"], "gateway": gateway["commit"]}


def portal_platform() -> str:
    """The platform commit the pinned Portal's jc-core is built from, read from its Cargo.toml."""
    portal = pinned_commits()["portal"]
    clone = Path(os.environ.get("JC_PORTAL_DIR", ROOT.parent / "joinedcontext-portal"))
    cargo = git_show(clone, portal, "Cargo.toml")
    if cargo is None:
        unavailable(f"portal {portal[:7]} is not in {clone}")
    return portal_jc_core_rev(cargo)


def test_the_platform_field_is_the_pinned_portals_jc_core():
    """The hand-kept `platform` field said 0069530 at 653fd563 while portal 299564d ran jc-core 5a24c42."""
    assert pinned_platform() == portal_platform(), (
        f"components/portal/images.yaml: platform is {pinned_platform()[:7]}, but portal "
        f"{pinned_commits()['portal'][:7]}'s Cargo.toml builds jc-core from {portal_platform()[:7]}: write that commit"
    )


@functools.cache
def seed_documents() -> tuple[tuple[str, dict], ...]:
    return tuple(
        (f"{path.relative_to(ROOT)}#{index}", document)
        for path in sorted(ROOT.glob("components/*/seed/**/*.yaml"))
        for index, document in enumerate(yaml.safe_load_all(path.read_text()))
        if isinstance(document, dict) and isinstance(document.get("kind"), str)
    )


def reason(error) -> str:
    """The innermost cause: an anyOf/oneOf refusal says which branch failed and why, not the whole value."""
    while error.context:
        error = max(error.context, key=lambda inner: (len(inner.absolute_path), inner.validator != "type"))
    return error.message[:160]


@functools.cache
def refusals(commit: str) -> dict[str, dict[tuple, str]]:
    """What platform `commit`'s schemas refuse in each seed document: (where, which keyword) -> the message."""
    jsonschema = pytest.importorskip("jsonschema")
    validators: dict[str, object] = {}
    found: dict[str, dict[tuple, str]] = {}
    for name, document in seed_documents():
        kind = document["kind"]
        if kind not in validators:
            text = git_show(platform_clone(), commit, f"schemas/kinds/{kind}.json")
            validators[kind] = text and jsonschema.Draft7Validator(json.loads(text))
        validator = validators[kind]
        if not validator:
            found[name] = {(("kind",), "kind"): f"jc-core has no {kind}"}
            continue
        errors = {(tuple(map(str, error.absolute_path)), error.validator): reason(error)
                  for error in validator.iter_errors(document)}
        if errors:
            found[name] = errors
    return found


def newly_refused(pin: str, reference: str) -> list[str]:
    """Each refusal of `pin` that `reference` does not share, as `file#document (path: message)`."""
    known = refusals(reference)
    return [
        f"{name} ({'.'.join(where) or 'document'}: {message})"
        for name, errors in refusals(pin).items()
        for (where, keyword), message in sorted(errors.items())
        if (where, keyword) not in known.get(name, {})
    ]


def platform_main() -> str:
    for ref in ("origin/main", "main"):
        result = subprocess.run(["git", "-C", str(platform_clone()), "rev-parse", "--verify", "-q", f"{ref}^{{commit}}"],
                                capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.strip()
    unavailable(f"no platform main in {platform_clone()}")


@pytest.mark.parametrize("image", ["portal", "gateway"])
def test_every_seed_manifest_is_one_the_pinned_jc_core_reads(image):
    pin = portal_platform() if image == "portal" else pinned_commits()["gateway"]
    if git_show(platform_clone(), pin, "schemas/kinds/ContextSpace.json") is None:
        unavailable(f"platform {pin[:7]} is not in {platform_clone()}")
    refused = newly_refused(pin, platform_main())
    assert not refused, (
        f"the {image} pin's jc-core (platform {pin[:7]}) refuses what platform main reads, and drops these "
        f"manifests whole: {refused[:10]}{' ...' if len(refused) > 10 else ''}; pin a {image} built on a platform "
        f"commit that has them in the same commit"
    )


def test_the_harvest_field_of_653fd563_is_caught():
    """The prod incident: Endpoint spec.catalog.harvest under platform 5a24c42 (portal 299564d)."""
    if git_show(platform_clone(), "5a24c4298689bd21e842511bda8fbf21157b1580", "schemas/kinds/Endpoint.json") is None:
        unavailable("platform 5a24c42 is not in the clone")
    harvest = {name for name, document in seed_documents()
               if document["kind"] == "Endpoint" and "harvest" in ((document.get("spec") or {}).get("catalog") or {})}
    assert harvest, "seed Endpoints carry spec.catalog.harvest"
    refused = {line.split(" ")[0] for line in newly_refused("5a24c4298689bd21e842511bda8fbf21157b1580", platform_main())
               if "(spec.catalog: " in line}
    assert harvest <= refused, sorted(harvest - refused)
