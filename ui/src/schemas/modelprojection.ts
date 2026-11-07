import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123 } from "./kinds";

/**
 * The ModelProjection form (T-1548, MP-01…MP-03): the named subset of a space's model an Endpoint
 * exposes, written on its own list so a second Endpoint can reference it.
 *
 * A class left out is not exposed; a class listed with no slots is exposed with identity only. The
 * form carries the space as the label the repository path is derived from, as the endpoint form does.
 */

export const SPACE_LABEL = "joinedcontext.com/space";

/** An NGSI-LD entity type short name, as jc-core's `validate_entity_type` reads one. */
const TYPE_PATTERN = "^[A-Za-z][A-Za-z0-9_]*$";
/** A LinkML slot name: letters, digits, underscore. */
const SLOT_PATTERN = "^[A-Za-z0-9_]+$";

const FILTERS = ["q", "scopeQ", "geoQ", "temporalQ"] as const;

export interface ModelProjectionForm {
  name?: string;
  contextSpaceRef?: string;
  dataModelRef?: { name?: string; version?: string };
  classes?: { name?: string; slots?: string[] }[];
  filter?: { q?: string; scopeQ?: string; geoQ?: string; temporalQ?: string };
}

export function modelProjectionSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string, pattern = "\\S"): JsonSchema => ({
    type: "string",
    title: t(`projections.field.${key}`),
    pattern,
  });
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "dataModelRef", "classes"],
    properties: {
      name: { ...text("name", DNS1123), maxLength: 63 },
      contextSpaceRef: { ...text("contextSpaceRef", DNS1123), maxLength: 63 },
      dataModelRef: {
        type: "object",
        title: t("projections.field.dataModelRef"),
        required: ["name", "version"],
        properties: {
          name: { ...text("dataModel", DNS1123), maxLength: 63 },
          // DM-22: a served major, "1", "2" …
          version: { ...text("version", "^[1-9][0-9]*$"), default: "1" },
        },
      },
      // MP-01: a projection exposes at least one class.
      classes: {
        type: "array",
        title: t("projections.field.classes"),
        minItems: 1,
        items: {
          type: "object",
          required: ["name"],
          properties: {
            name: text("class", TYPE_PATTERN),
            slots: { type: "array", title: t("projections.field.slots"), uniqueItems: true, items: text("slot", SLOT_PATTERN) },
          },
        },
      },
      filter: {
        type: "object",
        title: t("projections.field.filter"),
        properties: Object.fromEntries(FILTERS.map((key) => [key, text(key)])),
      },
    },
  };
}

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toModelProjectionManifest(project: string, form: ModelProjectionForm, stored?: unknown): unknown {
  const space = filled(form.contextSpaceRef);
  const metadata = storedMetadata(stored) as { labels?: Record<string, string> };
  const filter = Object.fromEntries(
    FILTERS.flatMap((key) => {
      const value = filled(form.filter?.[key]);
      return value ? [[key, value]] : [];
    }),
  );
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ModelProjection",
    metadata: {
      ...metadata,
      name: form.name,
      namespace: project,
      // On create only: a label written on an edit would move the file and leave the old one behind.
      ...(space && stored === undefined ? { labels: { ...metadata.labels, [SPACE_LABEL]: space } } : {}),
    },
    spec: {
      contextSpaceRef: space,
      dataModelRef: { kind: "DataModel", name: filled(form.dataModelRef?.name), version: filled(form.dataModelRef?.version) },
      // An empty slot list is identity only (MP-01), so a class row keeps `slots: []`.
      classes: (form.classes ?? []).flatMap((row) => {
        const name = filled(row.name);
        return name ? [{ name, slots: (row.slots ?? []).flatMap((slot) => filled(slot) ?? []) }] : [];
      }),
      ...(Object.keys(filter).length > 0 ? { filter } : {}),
    },
  };
}

/** The stored manifest back as the form. */
export function fromModelProjectionManifest(manifest: unknown): ModelProjectionForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const model = (spec.dataModelRef ?? {}) as { name?: string; version?: string };
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof spec.contextSpaceRef === "string" ? { contextSpaceRef: spec.contextSpaceRef } : {}),
    dataModelRef: {
      ...(model.name ? { name: model.name } : {}),
      ...(model.version !== undefined ? { version: String(model.version) } : {}),
    },
    classes: Array.isArray(spec.classes)
      ? (spec.classes as { name?: string; slots?: string[] }[]).map((row) => ({ name: row.name, slots: row.slots ?? [] }))
      : [],
    ...(spec.filter ? { filter: spec.filter as ModelProjectionForm["filter"] } : {}),
  };
}
