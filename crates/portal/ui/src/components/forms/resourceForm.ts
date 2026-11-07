import type { TFunction } from "i18next";
import type { ErrorSchema } from "@rjsf/utils";
import { forPeople } from "../../api/client";
import type { JsonSchema, ManifestSource } from "./types";

/** The name a form names its resource by: `name` at the top, else `metadata.name`. */
export function extractName(form: unknown): string | undefined {
  if (!form || typeof form !== "object") return undefined;
  const f = form as Record<string, unknown>;
  if (typeof f.name === "string" && f.name.trim()) return f.name.trim();
  if (f.metadata && typeof f.metadata === "object") {
    const m = f.metadata as Record<string, unknown>;
    if (typeof m.name === "string" && m.name.trim()) return m.name.trim();
  }
  return undefined;
}

/** The manifest a form writes: through `source` when the dialog has one, else the form itself. */
export function manifestOf<T>(form: T, source: ManifestSource<T> | undefined): unknown {
  return source ? source.toManifest(form) : form;
}

/**
 * The node of an rjsf `ErrorSchema` that marks one field, created along the way: `.spec.url`
 * becomes `{ spec: { url: { __errors: [...] } } }`, which is what puts `aria-invalid` on the input
 * and its sentence in `aria-describedby`.
 */
export function atPath(root: ErrorSchema, path: string[]): string[] {
  let node = root as Record<string, unknown>;
  for (const segment of path.filter(Boolean)) {
    node[segment] = (node[segment] as Record<string, unknown> | undefined) ?? {};
    node = node[segment] as Record<string, unknown>;
  }
  const list = (node.__errors as string[] | undefined) ?? [];
  node.__errors = list;
  return list;
}

/**
 * The control that edits a field: a workbench control standing in for a hidden form field
 * carries `data-field`, a form field has rjsf's `root_…` id. A hidden input is no control.
 */
export function controlOf(field: string): HTMLElement | null {
  const target =
    document.querySelector<HTMLElement>(`[data-field="${CSS.escape(field)}"]`) ??
    document.getElementById(`root_${field.split(".").join("_")}`);
  return target && !(target instanceof HTMLInputElement && target.type === "hidden") ? target : null;
}

/** The name a field shows, its schema `title`, or the last segment of its path without one. */
export function titleAt(schema: JsonSchema, path: string[]): string {
  let node: JsonSchema | undefined = schema;
  for (const segment of path) {
    const properties: Record<string, JsonSchema> | undefined = (
      node as { properties?: Record<string, JsonSchema> } | undefined
    )?.properties;
    node = properties?.[segment];
  }
  const title = (node as { title?: unknown } | undefined)?.title;
  return typeof title === "string" && title.trim() !== "" ? title : (path[path.length - 1] ?? "");
}

/**
 * The form's field a finding's manifest path names: `metadata.name` is the form's `name`, and a
 * form that holds the spec's fields at its top level (most do, through `source`) has
 * `spec.contextSpaceRef` as `contextSpaceRef` (T-2731, UI-45).
 */
export function fieldPath(path: string, schema: JsonSchema): string[] {
  // An element is `roles[0]` in the server's path and `roles.0` in rjsf's errors (T-5590).
  const segments = path.replace(/^metadata\./, "").replace(/\[(\d+)\]/g, ".$1").split(".");
  const flat = segments[0] === "spec" && !(schema.properties && "spec" in schema.properties);
  return flat ? segments.slice(1) : segments;
}

/**
 * A finding's sentence. The server refuses a name with the regular expression it failed (jc-core
 * `validate_dns1123_label`); a person gets the field's own sentence in their language instead
 * (T-4209, UI-44).
 */
export function findingSentence(message: string, t: TFunction): string {
  return /DNS-1123 label regex/.test(message) ? t("form.dns1123") : forPeople(message);
}
