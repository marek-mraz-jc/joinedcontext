/**
 * The catalog of organization policies and limits as the settings page and its form use it
 * (T-2715, PF-96…PF-102, ADR-N-035; API/01 §28). The route answers each entry with the operator's
 * bound and the value in force; this file turns an entry into words and into a form field whose
 * `minimum`/`maximum` are that bound, so a value outside it is refused in the form before the
 * Portal refuses it on the server.
 */
import type { components } from "../../api/schema";
import type { JsonSchema } from "../../components/forms/types";

export type LimitEntry = components["schemas"]["LimitEntry"];
export type OrganizationLimits = components["schemas"]["OrganizationLimits"];

/** The settings page's sections, in the catalog's order (PF-102). */
export const SECTIONS = ["projects", "applications", "edge", "signIn", "people", "agents", "pipelinesAndData", "mqtt"] as const;

type T = (key: string, options?: Record<string, unknown>) => string;

/** An entry's key in the locales: its path without `spec.`. */
export function entryKey(path: string): string {
  return path.replace(/^spec\./, "");
}

/** What a person reads for an entry's range: `10 – 6000`, or `at least 0` without a ceiling. */
export function boundText(t: T, entry: Pick<LimitEntry, "min" | "max">): string {
  return entry.max === null || entry.max === undefined
    ? t("organization.limits.atLeast", { min: entry.min })
    : t("organization.limits.between", { min: entry.min, max: entry.max });
}

/** The value in force: the organization's, else the default, else no limit. */
export function valueText(t: T, entry: Pick<LimitEntry, "value" | "default">): string {
  if (entry.value !== null && entry.value !== undefined) return String(entry.value);
  if (entry.default !== null && entry.default !== undefined) {
    return t("organization.limits.byDefault", { value: entry.default });
  }
  return t("organization.limits.noLimit");
}

/** The field's help: the hint, the default and the bound, the three things PF-102 asks for. */
export function fieldHelp(t: T, entry: LimitEntry): string {
  const byDefault =
    entry.default === null || entry.default === undefined
      ? t("organization.limits.defaultNone")
      : t("organization.limits.defaultIs", { value: entry.default });
  return `${t(`organization.limit.${entryKey(entry.path)}.hint`)} ${byDefault} ${t("organization.limits.allowed", {
    bound: boundText(t, entry),
  })}`;
}

type Node = { type?: string; title?: string; description?: string; properties?: Record<string, Node> } & Record<string, unknown>;

/**
 * The form schema with every catalog entry at its manifest path: an integer held to the entry's
 * range, titled and explained in the person's language. An object on the way is created with its
 * group's title, and a field already there (the quota, the cooldown) keeps its own and gains the
 * range and the help.
 */
export function withCatalog(schema: JsonSchema, entries: readonly LimitEntry[], t: T): JsonSchema {
  const root = structuredClone(schema) as Node;
  for (const entry of entries) {
    const segments = entryKey(entry.path).split(".");
    let node = root;
    for (const segment of segments.slice(0, -1)) {
      node.properties ??= {};
      node.properties[segment] ??= {
        type: "object",
        title: t(`organization.limitGroup.${segment}`),
        description: t(`organization.limitGroup.${segment}Hint`),
      };
      node = node.properties[segment];
    }
    const name = segments[segments.length - 1];
    node.properties ??= {};
    node.properties[name] = {
      ...node.properties[name],
      type: "integer",
      title: t(`organization.limit.${entryKey(entry.path)}.label`),
      description: fieldHelp(t, entry),
      minimum: entry.min,
      ...(entry.max === null || entry.max === undefined ? {} : { maximum: entry.max }),
    };
  }
  return root as JsonSchema;
}
