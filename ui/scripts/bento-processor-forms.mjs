#!/usr/bin/env node
// The form of every processor step, read from the pinned runner and never from a manual (PL-56):
//   docker run --rm --entrypoint /bento ghcr.io/warpstreamlabs/bento:1.21.1 list --format json-full \
//     | node scripts/bento-processor-forms.mjs src/schemas/bento-processors.json src/schemas/bento-processor-forms.json
// For each processor the palette offers (the names in bento-processors.json, the list jc-core admits)
// writes a JSON Schema and an rjsf uiSchema built from the runner's own field tree: required is what
// the runner requires, a closed option list is a select, a Bloblang or interpolated field a monospace
// text box, a list of processors or a free map a YAML box (`ui:options.yaml`), a secret a `${VAR}`.
import { readFileSync, writeFileSync } from "node:fs";
import { alsoSecret } from "./also-secret.mjs";
import { helpText } from "./help-text.mjs";

const SCALAR = { string: "string", int: "integer", float: "number", bool: "boolean" };

/** The runner's description in whole sentences, its Markdown links reduced to their text. */
function sentence(text) {
  return helpText((text ?? "").split("\n")[0].replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"));
}

/** One field of the runner as a schema and its uiSchema; `undefined` for a deprecated field. */
function field(node, path = "") {
  if (node.is_deprecated) return undefined;
  const ui = {};
  let schema;
  const options = node.options ?? node.annotated_options?.map(([value]) => value);
  if (node.type === "object" && node.kind === "scalar" && node.children?.length) {
    schema = object(node.children, ui, path);
  } else if (node.type === "object" && node.kind === "array" && node.children?.length) {
    const items = {};
    schema = { type: "array", items: object(node.children, items, path) };
    ui.items = items;
  } else if (SCALAR[node.type] && node.kind === "scalar") {
    schema = { type: SCALAR[node.type] };
    if (options?.length && node.type === "string") schema.enum = options;
  } else if (SCALAR[node.type] && node.kind === "array") {
    schema = { type: "array", items: { type: SCALAR[node.type] } };
  } else {
    // A list of processors, a map, a scanner, a list of lists: YAML the form parses on the way out.
    schema = { type: "string" };
    ui["ui:widget"] = "textarea";
    ui["ui:options"] = { yaml: true };
  }
  if (node.is_secret || (path && alsoSecret(path))) {
    // A credential never sits in a manifest: the field takes an environment variable (MF-35).
    schema = { type: "string", pattern: "^\\$\\{[A-Z0-9_]+\\}$" };
    ui["ui:options"] = { secret: true };
  }
  if (node.bloblang || (node.interpolated && schema.type === "string" && !schema.enum)) {
    ui["ui:widget"] = "textarea";
    ui["ui:options"] = { ...ui["ui:options"], code: true };
  }
  const description = sentence(node.description);
  if (description) schema.description = description;
  const scalarDefault = node.default !== undefined && node.default !== null && typeof node.default !== "object";
  if (scalarDefault && !ui["ui:options"]?.yaml && !ui["ui:options"]?.secret) schema.default = node.default;
  // A secret takes only `${VAR}`: its examples are credentials (Bento's docs carry real emulator keys).
  const example = !ui["ui:options"]?.secret && node.examples?.length ? node.examples[0] : undefined;
  if (typeof example === "string" && schema.type === "string") {
    ui["ui:placeholder"] = example;
  }
  if (node.is_advanced) ui["ui:options"] = { ...ui["ui:options"], advanced: true };
  return { schema, ui, required: node.default === undefined && !node.is_advanced && !node.is_optional };
}

/** An object of the runner's children; advanced fields go last. */
function object(children, ui, prefix = "") {
  const schema = { type: "object", properties: {}, required: [] };
  const plain = [];
  const advanced = [];
  for (const child of children) {
    const built = field(child, prefix ? `${prefix}.${child.name}` : child.name);
    if (!built) continue;
    schema.properties[child.name] = built.schema;
    if (Object.keys(built.ui).length) ui[child.name] = built.ui;
    if (built.required) schema.required.push(child.name);
    (child.is_advanced ? advanced : plain).push(child.name);
  }
  if (!schema.required.length) delete schema.required;
  ui["ui:order"] = [...plain, ...advanced, "*"];
  return schema;
}

const [palettePath, outPath] = process.argv.slice(2);
if (!palettePath || !outPath) {
  console.error("usage: bento list --format json-full | node scripts/bento-processor-forms.mjs <bento-processors.json> <out.json>");
  process.exit(1);
}
const palette = new Set(JSON.parse(readFileSync(palettePath, "utf8")).map(({ name }) => name));
const listing = JSON.parse(readFileSync(0, "utf8"));
const forms = {};
for (const processor of listing.processors.filter(({ name }) => palette.has(name))) {
  const config = processor.config;
  const ui = {};
  const schema =
    config.type === "object" && config.kind === "scalar"
      ? object(config.children ?? [], ui)
      : (() => {
          const built = field({ ...config, description: undefined });
          Object.assign(ui, built.ui);
          return built.schema;
        })();
  forms[processor.name] = { schema, uiSchema: ui };
}
const missing = [...palette].filter((name) => !forms[name]);
if (missing.length) {
  console.error(`the runner does not ship: ${missing.join(", ")}`);
  process.exit(1);
}
const sorted = Object.fromEntries(Object.entries(forms).sort(([a], [b]) => a.localeCompare(b)));
writeFileSync(outPath, `${JSON.stringify({ version: listing.version, processors: sorted })}\n`);
console.log(`${Object.keys(sorted).length} processor forms → ${outPath}`);
