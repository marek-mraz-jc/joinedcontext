"""What the open-data mapping tests share (T-2787, T-2785): run a committed mapping by Bento over a
recorded answer, reduce an entity to `keyValues`, and check it against its space's generated JSON
Schema, which is what the gateway enforces."""

import json
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

# The digest the pipeline runner pins (test_bystrica_pipelines.py, test_demo_feeds.py).
BENTO = "ghcr.io/warpstreamlabs/bento:1.22.0@sha256:ef674bebae45793c982c350d8f37c2c7661133c1dc65a5517fb977b36dc09841"

requires_docker = pytest.mark.skipif(
    shutil.which("docker") is None, reason="no container runtime, so Bento cannot run the mapping"
)

# The runner's shared caches, which a mapping may name (T-4052).
RESOURCES = Path(__file__).resolve().parent.parent / "components/pipeline-runner/streams/resources.yaml"


def _bento(mapping: Path, document: bytes, space: str, domain: str, codec: str, metadata: dict | None = None,
           then: tuple[dict, ...] = ()) -> bytes:
    pipeline = yaml.safe_load(mapping.read_text())["pipeline"]
    # The steps the reconciler renders after the mapping, such as a spec.lifetime (PL-80).
    pipeline["processors"].extend(then)
    if metadata:
        # What a connection's own steps leave on the message, such as the member name an archive
        # read sets (PL-72), set the way those steps set it.
        pipeline["processors"].insert(0, {"mapping": "\n".join(f"meta {k} = {json.dumps(v)}" for k, v in metadata.items())})
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": pipeline,
        # The runner's caches under the names a mapping writes: the Portal renders a step's `gtfs`
        # as its project's `helsinki__gtfs` (T-5337), and the mapping runs here as written.
        "cache_resources": [{**cache, "label": cache["label"].split("__", 1)[1]}
                            for cache in yaml.safe_load(RESOURCES.read_text())["cache_resources"]],
        # The runner's shared egress limit, which a mapping that fetches further pages names.
        "rate_limit_resources": yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"],
        "output": {"stdout": {"codec": codec}},
        "logger": {"level": "error"},
    }
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", f"JC_ORG_DOMAIN={domain}", "-e", f"JC_SPACE={space}",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=120,
    )
    assert result.returncode == 0, result.stderr.decode() or result.stdout.decode()
    return result.stdout


def run(mapping: Path, document: bytes, space: str, domain: str = "hel.fi") -> list[dict]:
    """The mapping's processors over one fetched document, as the runner feeds it."""
    return json.loads(_bento(mapping, document, space, domain, "all-bytes"))


def run_all(mapping: Path, document: bytes, space: str, domain: str = "hel.fi", metadata: dict | None = None,
            then: tuple[dict, ...] = ()) -> list[dict]:
    """Like `run`, for a mapping that answers many messages (one entity each, or arrays), as one
    that splits and regroups its rows does: the runner batches whichever it gets (T-3356)."""
    entities: list[dict] = []
    for line in _bento(mapping, document, space, domain, "lines", metadata, then).decode().splitlines():
        if line.strip():
            message = json.loads(line)
            entities.extend(message if isinstance(message, list) else [message])
    return entities


SYSTEM_MEMBERS = {"expiresAt", "createdAt", "modifiedAt", "deletedAt"}


def key_values(entity: dict) -> dict:
    """The entity as `keyValues`, the shape the generated JSON Schema describes. The NGSI-LD
    system members (`expiresAt`, NGSI-LD 4.22, and the broker's timestamps) are no attribute of
    a model, so the schema does not describe them and they are left out."""
    out = {}
    for name, attribute in entity.items():
        if name in SYSTEM_MEMBERS:
            continue
        if isinstance(attribute, list) and all(isinstance(i, dict) and "type" in i for i in attribute):
            # A multi-instance attribute (one instance per datasetId): the schema describes one
            # instance, so the last one stands for the slot here; schema_errors checks each.
            out[name] = key_values({name: attribute[-1]})[name] if attribute else attribute
        elif not isinstance(attribute, dict) or "type" not in attribute:
            out[name] = attribute
        elif attribute["type"] == "Relationship":
            out[name] = attribute["object"]
        elif attribute["type"] == "LanguageProperty":
            out[name] = attribute["languageMap"]
        elif attribute["type"] == "JsonProperty":
            out[name] = attribute["json"]
        else:
            out[name] = attribute["value"]
    return out


def schema_errors(schema_file: Path, entity: dict) -> list[str]:
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads(schema_file.read_text())
    # The class definition refers to the enums beside it, so it is checked inside the document.
    validator = jsonschema.Draft7Validator(
        {**schema["definitions"][entity["type"]], "definitions": schema["definitions"]},
        format_checker=jsonschema.Draft7Validator.FORMAT_CHECKER,
    )
    errors = [e.message for e in validator.iter_errors(key_values(entity))]
    for name, attribute in entity.items():
        if isinstance(attribute, list) and attribute and all(isinstance(i, dict) and "type" in i for i in attribute):
            for instance in attribute[:-1]:
                errors += [e.message for e in validator.iter_errors(key_values({**entity, name: instance}))]
    return errors


# The Smart Data Models the seeds validate against, vendored at the commits pins.json names (T-4319).
SDM = Path(__file__).resolve().parent.parent / "scripts/open-data/sdm"


def sdm_errors(model: str, entity: dict) -> list[str]:
    """The entity, as `keyValues`, against the vendored schema.json of `model`
    (`dataModel.Subject/Model`). Its `$ref`s to common-schema.json resolve to the vendored copy,
    never the network."""
    jsonschema = pytest.importorskip("jsonschema")
    from referencing import Registry, Resource
    from referencing.jsonschema import DRAFT7

    common = Resource.from_contents(json.loads((SDM / "common-schema.json").read_text()), DRAFT7)
    registry = Registry().with_resources((u, common) for u in (
        "https://smart-data-models.github.io/data-models/common-schema.json",
        "https://raw.githubusercontent.com/smart-data-models/data-models/master/common-schema.json"))
    # A subject's own shared definitions (dataModel.Weather/weather-schema.json), at the same pin.
    registry = registry.with_resources(
        (f"https://smart-data-models.github.io/dataModel.{p.parent.name}/{p.name}",
         Resource.from_contents(json.loads(p.read_text()), DRAFT7)) for p in SDM.glob("*/*-schema.json"))
    schema = json.loads((SDM / model.removeprefix("dataModel.") / "schema.json").read_text())
    validator = jsonschema.Draft7Validator(schema, registry=registry, format_checker=jsonschema.Draft7Validator.FORMAT_CHECKER)
    # A LanguageProperty is read the way NGSI-LD `options=keyValues&lang=` answers it: one string,
    # here its first language, which is the shape the models describe.
    entity = {k: {"type": "Property", "value": next(iter(v["languageMap"].values()), "")}
              if isinstance(v, dict) and v.get("type") == "LanguageProperty" else v for k, v in entity.items()}
    return [f"{'/'.join(map(str, e.absolute_path))}: {e.message}" for e in validator.iter_errors(key_values(entity))]
