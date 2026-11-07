import type { JsonSchema, UiSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123, words } from "./kinds";

/**
 * The Blueprint form (T-1537, CC-23…CC-27, CC-59): a template the organization offers in the Flows
 * gallery, written by an administrator on the Organization page.
 *
 * The parameter schema is a JSON Schema the gallery renders as the run's form, so it is written as
 * JSON in a text area and stored as the object it parses to. Text that does not parse travels as
 * written, and jc-core refuses it naming `spec.parameterSchema`, beside the field.
 */

/** A version as jc-core's `SemVer` reads it (DM-22, CC-26). */
export const SEMVER_PATTERN = "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$";

export const RISK_CLASSES = ["green", "yellow", "red"] as const;

export interface BlueprintForm {
  name?: string;
  version?: string;
  category?: string;
  riskClass?: string;
  allowedRoles?: string[];
  parameterSchema?: string;
  templates?: { name?: string; template?: string }[];
}

export function blueprintSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string): JsonSchema => ({ type: "string", title: t(`blueprints.field.${key}`) });
  return {
    type: "object",
    required: ["name", "version", "riskClass", "allowedRoles", "parameterSchema", "templates"],
    properties: {
      name: { ...text("name"), pattern: DNS1123, maxLength: 63 },
      version: { ...text("version"), pattern: SEMVER_PATTERN },
      category: { ...text("category"), pattern: DNS1123, maxLength: 63 },
      riskClass: { ...text("riskClass"), ...words(t, "choice.riskClass", RISK_CLASSES), default: "yellow" },
      // CC-59: a blueprint nobody may run never reaches the gallery.
      allowedRoles: {
        type: "array",
        title: t("blueprints.field.allowedRoles"),
        minItems: 1,
        items: { ...text("role"), pattern: DNS1123, maxLength: 63 },
      },
      parameterSchema: { ...text("parameterSchema"), pattern: "\\S" },
      // CC-23: a blueprint that expands to nothing is not a blueprint.
      templates: {
        type: "array",
        title: t("blueprints.field.templates"),
        minItems: 1,
        items: {
          type: "object",
          required: ["name", "template"],
          properties: {
            name: { ...text("templateName"), pattern: DNS1123, maxLength: 63 },
            template: { ...text("template"), pattern: "\\S" },
          },
        },
      },
    },
  };
}

/** Code in text areas; no autocomplete, so a browser does not offer one template's text in another. */
export const blueprintUiSchema: UiSchema = {
  parameterSchema: { "ui:widget": "textarea", "ui:options": { rows: 10 }, "ui:autocomplete": "off" },
  templates: {
    items: {
      template: { "ui:widget": "textarea", "ui:options": { rows: 10 }, "ui:autocomplete": "off" },
    },
  },
};

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** The parameter schema as the object it parses to, or the text as written when it does not parse. */
function parsed(text: string | undefined): unknown {
  if (text === undefined) return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toBlueprintManifest(form: BlueprintForm, stored?: unknown): unknown {
  const category = filled(form.category);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Blueprint",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: "org" },
    spec: {
      version: filled(form.version),
      ...(category ? { category } : {}),
      riskClass: form.riskClass,
      allowedRoles: (form.allowedRoles ?? []).flatMap((role) => filled(role) ?? []),
      parameterSchema: parsed(form.parameterSchema),
      templates: (form.templates ?? []).map((one) => ({ name: filled(one.name), template: one.template })),
    },
  };
}

/** The stored manifest back as the form, the parameter schema as indented JSON. */
export function fromBlueprintManifest(manifest: unknown): BlueprintForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof spec.version === "string" ? { version: spec.version } : {}),
    ...(typeof spec.category === "string" ? { category: spec.category } : {}),
    ...(typeof spec.riskClass === "string" ? { riskClass: spec.riskClass } : {}),
    allowedRoles: Array.isArray(spec.allowedRoles) ? (spec.allowedRoles as string[]) : [],
    ...(spec.parameterSchema !== undefined
      ? {
          parameterSchema:
            typeof spec.parameterSchema === "string"
              ? spec.parameterSchema
              : JSON.stringify(spec.parameterSchema, null, 2),
        }
      : {}),
    templates: Array.isArray(spec.templates) ? (spec.templates as BlueprintForm["templates"]) : [],
  };
}
