import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123 } from "./kinds";

/**
 * The ScopeDefinition form (T-2955, R19, ADR 005): one node of the scope tree a Policy grants by and
 * a guarded entity is labelled with.
 *
 * A person writes the path only. Whether the node is a root and which node it hangs under follow
 * from the path, so the form derives `isRoot` and `isChildOf` instead of asking for three fields
 * that must agree; jc-core refuses a manifest where they do not.
 */

/** The three taxonomy roots of ADR 004 and segments of `[A-Za-z0-9._-]`, as jc-core checks them. */
export const SCOPE_PATH = "^/(geo|domain|admin)(/[A-Za-z0-9._-]+)*$";

export interface ScopeDefinitionForm {
  name?: string;
  scopeString?: string;
}

export function scopeDefinitionSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string): JsonSchema => ({ type: "string", title: t(`scopedefinitions.field.${key}`) });
  return {
    type: "object",
    required: ["name", "scopeString"],
    properties: {
      name: { ...text("name"), pattern: DNS1123, maxLength: 63 },
      scopeString: { ...text("scopeString"), pattern: SCOPE_PATH },
    },
  };
}

/** The node the path hangs under: `/geo/SK/BB` for `/geo/SK/BB/Radvan`, none for a root. */
export function parentOf(scopeString: string): string | undefined {
  const cut = scopeString.lastIndexOf("/");
  return cut > 0 ? scopeString.slice(0, cut) : undefined;
}

/** The form as the manifest the API stores; the metadata the form has no field for travels on. */
export function toScopeDefinitionManifest(project: string, form: ScopeDefinitionForm, stored?: unknown): unknown {
  const scopeString = (form.scopeString ?? "").trim();
  const parent = parentOf(scopeString);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ScopeDefinition",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: project },
    spec: {
      scopeString,
      isRoot: parent === undefined,
      ...(parent === undefined ? {} : { isChildOf: parent }),
    },
  };
}

/** The stored manifest back as the form. */
export function fromScopeDefinitionManifest(manifest: unknown): ScopeDefinitionForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: { scopeString?: unknown } };
  const scopeString = envelope.spec?.scopeString;
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof scopeString === "string" ? { scopeString } : {}),
  };
}
