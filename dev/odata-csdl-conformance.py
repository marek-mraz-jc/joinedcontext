#!/usr/bin/env python3
"""`$metadata` of odata-csdl against the pinned OASIS artefacts (ADR-0024 decision 6, T-3502).

    dev/odata-csdl-conformance.py        run every check (CI step of the same name)

The OASIS files are fetched at the pinned commits and checked against their SHA-256 before use;
they are never vendored (their licence is the OASIS IPR policy). The checks, each named as the
test plan names it:

  odata_csdl_metadata_validates_against_oasis_schemas
      the example's CSDL XML against schemas/edmx.xsd (with catalog.xml, xmllint) and its CSDL
      JSON against schemas/csdl.schema.json (the ajv library, the validator the OASIS
      repository uses; ajv-cli is not used, its dependency tree carries open advisories);
  odata_csdl_xml_and_json_agree
      the OASIS converter odata-csdl-xml2json turns our XML into exactly our JSON;
  odata_csdl_annotation_terms_exist
      every annotation term (and every record member under it) is read from the pinned
      vocabulary files or from the document's own schemas, so a misspelled term fails; a
      misspelling is planted first to prove the check can fail.

Needs: cargo, xmllint (libxml2-utils), node with npm.
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
RAW = "https://raw.githubusercontent.com/oasis-tcs"
SCHEMAS = "odata-csdl-schemas/e6b66a96ec5f74edb0c0e192978a39f146f70d76"
VOCABULARIES = "odata-vocabularies/3e36ad2a4930a47feaf7a7cd0b78ed4f2bafcfb2"

PINNED = {
    f"{SCHEMAS}/schemas/edmx.xsd": "bb8dd3e090955cc7653e361eec8f48b02c095979992839ebe878a3e49dcbd316",
    f"{SCHEMAS}/schemas/edm.xsd": "a2e01a415137b5e84d442f8f35cb5d2e054b9fe5e6d18dab18c76bab8fea8d86",
    f"{SCHEMAS}/schemas/csdl.schema.json": "c211eff8a91be39f594b1eb84760306b730cda9a82ec204b2f8186e5018ffb80",
    f"{SCHEMAS}/catalog.xml": "30d8a483f0031925475e11686b3becf034ad3782369f3e314bfa919e19badb62",
    f"{VOCABULARIES}/vocabularies/Org.OData.Core.V1.json": "5688bb6cdb1463951d405750101561c550482594ce226d71d6bb4458ab80779d",
    f"{VOCABULARIES}/vocabularies/Org.OData.Capabilities.V1.json": "7e86f9cc5d7224e091d078322c8acd623ef14ffd0e4ae4a2690baeec70c35d6c",
    f"{VOCABULARIES}/vocabularies/Org.OData.Measures.V1.json": "05d4719d5c9ac62e27e835ad8475e5a1facc18ccea887c1e97224428158381f8",
    f"{VOCABULARIES}/vocabularies/Org.OData.Validation.V1.json": "1dd95ce867f672b4fc9ff9e7e419036929523342d3b0c4443c02258e0687a981",
}

# Exact versions; `--ignore-scripts` keeps install scripts of the tree from running.
NPM = {"ajv": "8.20.0", "odata-csdl": "0.11.2"}

# ajv as the OASIS repository runs it: draft-07, unicode patterns, unknown keywords tolerated.
AJV = """
const Ajv = require("ajv");
const fs = require("node:fs");
const [schema, data] = process.argv.slice(1).map((f) => JSON.parse(fs.readFileSync(f)));
const validate = new Ajv({ strict: false, allErrors: true }).compile(schema);
if (!validate(data)) {
  console.error(JSON.stringify(validate.errors, null, 2));
  process.exit(1);
}
"""


def fetch(work: pathlib.Path) -> pathlib.Path:
    """The pinned files under `work/oasis/<path after the commit>`, each checksum-verified."""
    for path, digest in PINNED.items():
        _, _, inner = path.split("/", 2)
        target = work / "oasis" / inner
        target.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(f"{RAW}/{path}", timeout=60) as response:
            data = response.read()
        got = hashlib.sha256(data).hexdigest()
        if got != digest:
            sys.exit(f"{path}: sha256 {got}, pinned {digest}")
        target.write_bytes(data)
    return work / "oasis"


def run(*argv: str, cwd: pathlib.Path | None = None, env: dict | None = None) -> str:
    done = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{' '.join(argv)} failed:\n{done.stdout}{done.stderr}")
    return done.stdout + done.stderr


def schemas_accept(oasis: pathlib.Path, out: pathlib.Path, tools: pathlib.Path) -> None:
    env = {**os.environ, "XML_CATALOG_FILES": str(oasis / "catalog.xml")}
    run("xmllint", "--noout", "--catalogs", "--schema", str(oasis / "schemas/edmx.xsd"),
        str(out / "metadata.xml"), env=env)
    run("node", "-e", AJV, str(oasis / "schemas/csdl.schema.json"), str(out / "metadata.json"),
        cwd=tools)
    print("odata_csdl_metadata_validates_against_oasis_schemas: ok")


def converter_agrees(out: pathlib.Path, tools: pathlib.Path) -> None:
    converted = out / "converted.json"
    said = run(str(tools / "node_modules/.bin/odata-csdl-xml2json"), "-t", str(converted),
               str(out / "metadata.xml"))
    if said.strip() != str(converted):
        sys.exit(f"odata_csdl_xml_and_json_agree: the converter warned:\n{said}")
    ours = json.loads((out / "metadata.json").read_text())
    theirs = json.loads(converted.read_text())
    # The converter writes a numeric SRID as a number; CSDL JSON 7.2.5 says "The value of $SRID
    # is a string containing a number or the symbolic value variable", and csdl.schema.json
    # types it `string`. Ours follows the specification; theirs is read the same way.
    def srid_as_string(node):
        if isinstance(node, dict):
            return {k: str(v) if k == "$SRID" else srid_as_string(v) for k, v in node.items()}
        if isinstance(node, list):
            return [srid_as_string(v) for v in node]
        return node
    theirs = srid_as_string(theirs)
    if ours != theirs:
        diff = [k for k in sorted(set(ours) | set(theirs)) if ours.get(k) != theirs.get(k)]
        sys.exit("odata_csdl_xml_and_json_agree: differs at top-level members "
                 f"{diff}\nours:   {json.dumps({k: ours.get(k) for k in diff})[:4000]}\n"
                 f"theirs: {json.dumps({k: theirs.get(k) for k in diff})[:4000]}")
    print("odata_csdl_xml_and_json_agree: ok")


def unknown_terms(document: dict, vocabularies: dict[str, dict]) -> list[str]:
    """Every annotation of `document` whose term, or a record member under it, no schema has."""
    schemas = {**vocabularies, **{k: v for k, v in document.items() if not k.startswith("$")}}
    alias = {ns: ns for ns in schemas}
    for ns, schema in schemas.items():
        if isinstance(schema, dict) and "$Alias" in schema:
            alias[schema["$Alias"]] = ns
    for reference in document.get("$Reference", {}).values():
        for include in reference.get("$Include", []):
            alias[include.get("$Alias", include["$Namespace"])] = include["$Namespace"]

    def element(qualified: str) -> dict | None:
        qualifier, _, name = qualified.rpartition(".")
        schema = schemas.get(alias.get(qualifier, ""))
        found = schema.get(name) if isinstance(schema, dict) else None
        return found if isinstance(found, dict) else None

    def members(type_name: str) -> set[str]:
        names: set[str] = set()
        while (ty := element(type_name)) is not None:
            names |= {k for k in ty if not k.startswith(("$", "@"))}
            type_name = ty.get("$BaseType", "")
        return names

    bad: list[str] = []

    def walk(node, at: str) -> None:
        if isinstance(node, list):
            for item in node:
                walk(item, at)
        if not isinstance(node, dict):
            return
        for key, value in node.items():
            if key.startswith("@"):
                term_name = key[1:].split("#")[0]
                term = element(term_name)
                if term is None or term.get("$Kind") != "Term":
                    bad.append(f"{at}{key}")
                    continue
                allowed = members(term.get("$Type", "Edm.String"))
                records = value if isinstance(value, list) else [value]
                for record in records:
                    if isinstance(record, dict) and not any(k.startswith("$") for k in record):
                        bad.extend(f"{at}{key}/{m}" for m in record if not m.startswith("@") and m not in allowed)
            walk(value, f"{at}{key}/")

    walk(document, "")
    return bad


def terms_exist(out: pathlib.Path, oasis: pathlib.Path) -> None:
    vocabularies = {}
    for file in (oasis / "vocabularies").glob("*.json"):
        vocabularies.update({k: v for k, v in json.loads(file.read_text()).items() if not k.startswith("$")})
    document = json.loads((out / "metadata.json").read_text())
    for right, wrong in (('"@Core.Computed"', '"@Core.Computd"'), ('"Insertable"', '"Insertabel"')):
        planted = json.loads(json.dumps(document).replace(right, wrong, 1))
        if not any(wrong.strip('"@') in b for b in unknown_terms(planted, vocabularies)):
            sys.exit(f"odata_csdl_annotation_terms_exist: the planted {wrong} was not found")
    bad = unknown_terms(document, vocabularies)
    if bad:
        sys.exit("odata_csdl_annotation_terms_exist: unknown terms or members:\n  " + "\n  ".join(bad))
    print("odata_csdl_annotation_terms_exist: ok")


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        oasis = fetch(work)
        out = work / "out"
        out.mkdir()
        run("cargo", "run", "--locked", "-q", "-p", "odata-csdl", "--example", "metadata", "--", str(out), cwd=ROOT)
        tools = work / "tools"
        tools.mkdir()
        (tools / "package.json").write_text(json.dumps({"private": True, "dependencies": NPM}))
        run("npm", "install", "--ignore-scripts", "--no-audit", "--no-fund", "--silent", cwd=tools)
        schemas_accept(oasis, out, tools)
        converter_agrees(out, tools)
        terms_exist(out, oasis)


if __name__ == "__main__":
    main()
