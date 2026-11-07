import { useEffect, useState } from "react";
import type { ErrorSchema } from "@rjsf/utils";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import type { JsonSchema, UiSchema } from "../../components/forms/types";
import { YamlFieldError } from "../../schemas/kinds";

/** One processor's form, generated from the pinned runner by scripts/bento-processor-forms.mjs (PL-56). */
export interface ProcessorForm {
  schema: JsonSchema;
  uiSchema: UiSchema;
}

export interface ProcessorForms {
  version: string;
  processors: Record<string, ProcessorForm>;
}

let cached: ProcessorForms | undefined;

/** The processor forms (~150 KB), imported on first use so the Studio's bundle stays small. */
export function useProcessorForms(): ProcessorForms | undefined {
  const [forms, setForms] = useState<ProcessorForms | undefined>(cached);
  useEffect(() => {
    if (cached) return;
    let active = true;
    void import("../../schemas/bento-processor-forms.json").then((mod) => {
      cached = mod.default as unknown as ProcessorForms;
      if (active) setForms(cached);
    });
    return () => {
      active = false;
    };
  }, []);
  return forms;
}

type Ui = Record<string, unknown> | undefined;

const optionsOf = (ui: Ui) =>
  (ui?.["ui:options"] ?? {}) as { yaml?: boolean; secret?: boolean };
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * A processor's config as the form holds it: every field the runner documents as a list of
 * processors, a map or another document becomes the YAML text its box edits.
 */
export function toFormData(
  value: unknown,
  schema: JsonSchema,
  ui: Ui,
): unknown {
  if (optionsOf(ui).yaml) {
    return value === undefined || value === null || typeof value === "string"
      ? value
      : stringifyYaml(value).trimEnd();
  }
  if (schema.type === "object" && isRecord(value) && schema.properties) {
    const out: Record<string, unknown> = { ...value };
    for (const [key, child] of Object.entries(schema.properties)) {
      if (key in value)
        out[key] = toFormData(value[key], child as JsonSchema, ui?.[key] as Ui);
    }
    return out;
  }
  if (
    schema.type === "array" &&
    Array.isArray(value) &&
    isRecord(schema.items)
  ) {
    return value.map((item) =>
      toFormData(item, schema.items as JsonSchema, ui?.items as Ui),
    );
  }
  return value;
}

/**
 * The form's data back as the runner's config: YAML text parsed into what the manifest carries
 * and an emptied YAML box left out. A box that does not parse throws `YamlFieldError` naming
 * its dotted path (`""` for the whole step, `0.processors` for a case), so a broken value never
 * reaches the manifest as a string and the sentence lands at its box. A field the form filled
 * with the runner's own default, which the block it started from (`original`) did not write,
 * stays out: opening a step changes nothing in the manifest.
 */
export function fromFormData(
  value: unknown,
  schema: JsonSchema,
  ui: Ui,
  path = "",
  original?: unknown,
): unknown {
  if (optionsOf(ui).yaml) {
    if (typeof value !== "string") return value;
    if (value.trim() === "") return undefined;
    try {
      return parseYaml(value) as unknown;
    } catch (error) {
      throw new YamlFieldError(path, (error as Error).message.split("\n")[0]);
    }
  }
  if (schema.type === "object" && isRecord(value) && schema.properties) {
    const before = isRecord(original) ? original : {};
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
      const child = schema.properties[key] as JsonSchema | undefined;
      const next = child
        ? fromFormData(item, child, ui?.[key] as Ui, path ? `${path}.${key}` : key, before[key])
        : item;
      if (next === undefined) continue;
      const filled =
        !(key in before) &&
        ((child?.default !== undefined && next === child.default) ||
          (isRecord(next) && Object.keys(next).length === 0));
      if (!filled) out[key] = next;
    }
    return out;
  }
  if (schema.type === "array" && Array.isArray(value) && isRecord(schema.items)) {
    const before = Array.isArray(original) ? original : [];
    return value.map((item, index) =>
      fromFormData(
        item,
        schema.items as JsonSchema,
        ui?.items as Ui,
        path ? `${path}.${index}` : `${index}`,
        before[index],
      ),
    );
  }
  return value;
}

/** The uiSchema with the words the generator cannot know: a secret field says what it takes. */
export function withHelp(ui: UiSchema, secretHelp: string): UiSchema {
  const walk = (node: Ui): Ui => {
    if (!isRecord(node)) return node;
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(node)) {
      out[key] = key.startsWith("ui:") ? child : walk(child as Ui);
    }
    if (optionsOf(node).secret) out["ui:help"] = secretHelp;
    return out;
  };
  return walk(ui as Ui) as UiSchema;
}

/**
 * The schema with the pipeline's own variables offered on every secret field, as `${VAR}`
 * (T-3972, PL-15): a credential is named, never typed, and these are the names the runner
 * injects for this pipeline.
 */
export function withVariables(schema: JsonSchema, ui: UiSchema, variables: string[]): JsonSchema {
  if (variables.length === 0) return schema;
  const offered = variables.map((name) => `\${${name}}`);
  const walk = (node: unknown, uiNode: Ui): unknown => {
    if (!isRecord(node)) return node;
    if (optionsOf(uiNode).secret && node.type === "string") return { ...node, examples: offered };
    const out: Record<string, unknown> = { ...node };
    if (isRecord(node.properties)) {
      out.properties = Object.fromEntries(
        Object.entries(node.properties).map(([key, child]) => [key, walk(child, isRecord(uiNode?.[key]) ? (uiNode?.[key] as Ui) : undefined)]),
      );
    }
    if (isRecord(node.items)) out.items = walk(node.items, isRecord(uiNode?.items) ? (uiNode?.items as Ui) : undefined);
    return out;
  };
  return walk(schema, ui as Ui) as JsonSchema;
}

/** An rjsf error schema carrying `message` at the dotted `path` that `fromFormData` names. */
export function errorAt(path: string, message: string): ErrorSchema {
  const root: Record<string, unknown> = {};
  let node = root;
  for (const key of path === "" ? [] : path.split(".")) {
    const next: Record<string, unknown> = {};
    node[key] = next;
    node = next;
  }
  node.__errors = [message];
  return root as ErrorSchema;
}
