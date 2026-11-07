import type { JsonSchema } from "../components/forms/types";
import { DNS1123 } from "./kinds";

/**
 * The DataModel form (T-2357, DM-01, DM-22, DM-26, DM-28): what a person decides about a model
 * version by hand.
 *
 * A model is written on the Models page's LinkML editor, and saving the source is what produces
 * the manifest's version, classes and generated artifacts; typing them into a form would put the
 * manifest out of step with the source it was generated from. So the form shows those as they are
 * and decides two things: where the version is in its life (DM-26) and whether its entities may
 * carry attributes the model does not declare (DM-28). Everything else is written back as it was
 * read, whatever the form's model says about it.
 */

/** DM-26: the four states of a model version, as jc-core spells them. */
export const DATA_MODEL_LIFECYCLES = ["draft", "published", "deprecated", "retired"] as const;

/** DM-22: `major.minor.patch`, as jc-core's `SemVer` accepts it. */
export const SEMVER_PATTERN = "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$";

export interface DataModelForm {
  name?: string;
  contextSpaceRef?: string;
  version?: string;
  linkml?: string;
  lifecycle?: string;
  openWorld?: boolean;
}

export function dataModelSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name", "lifecycle"],
    properties: {
      name: { type: "string", title: t("models.field.name"), pattern: DNS1123, maxLength: 63 },
      // With the namespace it is the model's path (MF-06): moving it is a new model, not an edit.
      contextSpaceRef: { type: "string", title: t("models.field.space"), pattern: DNS1123, readOnly: true },
      version: { type: "string", title: t("models.field.version"), pattern: SEMVER_PATTERN, readOnly: true },
      linkml: { type: "string", title: t("models.field.linkml"), readOnly: true },
      lifecycle: {
        type: "string",
        title: t("models.field.lifecycle"),
        oneOf: DATA_MODEL_LIFECYCLES.map((state) => ({
          const: state,
          title: t(`models.lifecycleOption.${state}`),
        })),
      },
      openWorld: { type: "boolean", title: t("models.field.openWorld"), default: false },
    },
  };
}

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

export function fromDataModelManifest(manifest: unknown): DataModelForm {
  const m = record(manifest) as Manifest;
  const spec = record(m.spec);
  return {
    name: text(record(m.metadata).name),
    contextSpaceRef: text(spec.contextSpaceRef),
    version: text(spec.version),
    linkml: text(spec.linkml),
    lifecycle: text(spec.lifecycle),
    openWorld: spec.openWorld === true,
  };
}

/**
 * Only the two decisions travel: the lifecycle and `openWorld` are written onto the manifest the
 * dialog read, and the read-only fields are taken from it rather than from the form, so a value
 * edited past the read-only control (a crafted request, the YAML of another tab) cannot move the
 * model or its version. The status is never proposed.
 */
export function toDataModelManifest(stored: unknown, form: DataModelForm): Manifest {
  const m = record(stored) as Manifest;
  const spec = record(m.spec);
  return {
    apiVersion: m.apiVersion ?? "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { ...record(m.metadata), name: form.name ?? text(record(m.metadata).name) ?? "" },
    spec: {
      ...spec,
      lifecycle: form.lifecycle ?? spec.lifecycle,
      openWorld: form.openWorld === true,
    },
  };
}
