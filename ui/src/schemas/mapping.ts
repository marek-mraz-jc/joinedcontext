import type { JsonSchema, UiSchema } from "../components/forms/types";
import { DNS1123, words } from "./kinds";

/**
 * The Mapping form (T-2354, DM-33..DM-39): the parts of a Mapping a person edits field by field.
 *
 * The transformation itself — a LinkML-Map `TransformationSpecification` — is not a field. It is
 * written slot by slot on the Models page's Mappings tab, which is the editor that can show what a
 * derivation does; a form would hand it over as a text box. The edit form carries the stored
 * transformation over untouched, together with anything else the form does not show
 * (`vocabularyAlignment`), so saving the form never drops what the tab wrote.
 */

/** The served major of a model, as jc-core's `MAJOR_RE` accepts it (DM-22, DM-33). */
export const MAJOR_VERSION_PATTERN = "^(0|[1-9][0-9]*)$";

/** A native block attaches to a slot by its LinkML identifier (DM-38). */
export const TARGET_SLOT_PATTERN = "^[A-Za-z_][A-Za-z0-9_]*$";

/** A path relative to the manifest: not absolute, no `..` segment (jc-core `validate_relative_path`). */
export const RELATIVE_PATH_PATTERN = "^(?!/)(?!(?:.*/)?\\.\\.(?:/|$)).+$";

/** The languages a native block may be written in (DM-37). */
export const NATIVE_LANGUAGES = ["bloblang"] as const;

export interface MappingForm {
  name?: string;
  contextSpaceRef?: string;
  source?: { name?: string; version?: string };
  target?: { name?: string; version?: string };
  tests?: { input?: string; expect?: string }[];
  native?: { targetSlot?: string; language?: string; source?: string }[];
  artifacts?: { bloblang?: string; gatewayIr?: string };
}

function modelRef(t: (key: string) => string, title: string): JsonSchema {
  return {
    type: "object",
    title,
    required: ["name", "version"],
    properties: {
      name: { type: "string", title: t("mappings.field.model"), pattern: DNS1123, maxLength: 63 },
      version: { type: "string", title: t("mappings.field.version"), pattern: MAJOR_VERSION_PATTERN },
    },
  };
}

function path(title: string): JsonSchema {
  return { type: "string", title, pattern: RELATIVE_PATH_PATTERN, maxLength: 253 };
}

export function mappingSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "source", "target", "tests"],
    properties: {
      name: { type: "string", title: t("mappings.field.name"), pattern: DNS1123, maxLength: 63 },
      contextSpaceRef: { type: "string", title: t("mappings.field.space"), pattern: DNS1123, maxLength: 63 },
      source: modelRef(t, t("mappings.source")),
      target: modelRef(t, t("mappings.target")),
      // DM-39: at least one golden test, and the admission check refuses a Mapping without one.
      tests: {
        type: "array",
        title: t("mappings.field.tests"),
        minItems: 1,
        items: {
          type: "object",
          required: ["input", "expect"],
          properties: {
            input: path(t("mappings.field.testInput")),
            expect: path(t("mappings.field.testExpect")),
          },
        },
      },
      native: {
        type: "array",
        title: t("mappings.field.native"),
        items: {
          type: "object",
          required: ["targetSlot", "language", "source"],
          properties: {
            targetSlot: { type: "string", title: t("mappings.targetSlot"), pattern: TARGET_SLOT_PATTERN },
            language: {
              type: "string",
              title: t("mappings.field.language"),
              ...words(t, "choice.mappingLanguage", NATIVE_LANGUAGES),
              default: "bloblang",
            },
            // Non-empty after trimming, as jc-core refuses it (DM-38).
            source: { type: "string", title: t("mappings.field.nativeSource"), pattern: "\\S" },
          },
        },
      },
      artifacts: {
        type: "object",
        title: t("mappings.field.artifacts"),
        properties: {
          bloblang: path(t("mappings.field.bloblang")),
          gatewayIr: path(t("mappings.field.gatewayIr")),
        },
      },
    },
  };
}

/** Code in a text area; no autocomplete, so a browser does not offer one city's code on another's. */
export const mappingUiSchema: UiSchema = {
  native: {
    items: {
      source: { "ui:widget": "textarea", "ui:options": { rows: 6 }, "ui:autocomplete": "off" },
    },
  },
};

type Manifest = {
  apiVersion?: string;
  kind?: string;
  metadata?: Record<string, unknown>;
  spec?: Record<string, unknown>;
};

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

/** The stored manifest as the form's model; what the form does not show is not in it. */
export function fromMappingManifest(manifest: unknown): MappingForm {
  const m = record(manifest) as Manifest;
  const spec = record(m.spec);
  const ref = (value: unknown) => {
    const r = record(value);
    return { name: text(r.name), version: text(r.version) };
  };
  const artifacts = record(spec.artifacts);
  return {
    name: text(record(m.metadata).name),
    contextSpaceRef: text(spec.contextSpaceRef),
    source: ref(spec.source),
    target: ref(spec.target),
    tests: (Array.isArray(spec.tests) ? spec.tests : []).map((test) => {
      const r = record(test);
      return { input: text(r.input), expect: text(r.expect) };
    }),
    native: (Array.isArray(spec.native) ? spec.native : []).map((block) => {
      const r = record(block);
      return { targetSlot: text(r.targetSlot), language: text(r.language), source: text(r.source) };
    }),
    artifacts: { bloblang: text(artifacts.bloblang), gatewayIr: text(artifacts.gatewayIr) },
  };
}

/**
 * The form written back onto the stored manifest: every field the form shows replaces its value,
 * everything else — the transformation, an alignment, the refs' `kind` — is kept as it was. The
 * status is the reconciler's and is never proposed.
 */
export function toMappingManifest(stored: unknown, form: MappingForm): Manifest {
  const m = record(stored) as Manifest;
  const spec = record(m.spec);
  const ref = (was: unknown, now: MappingForm["source"]) => ({
    ...record(was),
    name: now?.name ?? "",
    version: now?.version ?? "",
  });
  const artifacts = Object.fromEntries(
    Object.entries({ ...record(spec.artifacts), ...(form.artifacts ?? {}) }).filter(
      ([, value]) => value !== undefined && value !== "",
    ),
  );
  const native = (form.native ?? []).map((block) => ({
    targetSlot: block.targetSlot ?? "",
    language: block.language ?? "bloblang",
    source: block.source ?? "",
  }));
  const nextSpec: Record<string, unknown> = {
    ...spec,
    contextSpaceRef: form.contextSpaceRef ?? "",
    source: ref(spec.source, form.source),
    target: ref(spec.target, form.target),
    tests: (form.tests ?? []).map((test) => ({ input: test.input ?? "", expect: test.expect ?? "" })),
    native,
    artifacts,
  };
  if (native.length === 0) delete nextSpec.native;
  if (Object.keys(artifacts).length === 0) delete nextSpec.artifacts;
  return {
    apiVersion: m.apiVersion ?? "joinedcontext.com/v1alpha1",
    kind: "Mapping",
    metadata: { ...record(m.metadata), name: form.name ?? "" },
    spec: nextSpec,
  };
}
