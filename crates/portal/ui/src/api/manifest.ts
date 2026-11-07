import type { components } from "./schema";

/**
 * A manifest as the views read it. `spec` is free-form JSON, which the OpenAPI document
 * declares as a bare object and openapi-typescript therefore renders `Record<string, never>`;
 * this is the one place that says what it really is (T-0249 re-derives it from jc-core).
 */
export type Manifest = Omit<components["schemas"]["ResourceEnvelope"], "spec"> & {
  spec: Record<string, unknown>;
};

export type Change = components["schemas"]["Change"];
/** What a proposal of one resource sends: the manifest, and beside it a draft or its files (T-1488). */
export type ResourceProposal = components["schemas"]["ResourceProposal"];

/** The project segment of the organization's own manifests: roles, bindings, agent profiles, projects (PF-49). */
export const ORG_NAMESPACE = "org";

/** A write answers `202` with a Change, or `200` with a dry-run result. */
export function isChange(result: unknown): result is Change {
  return typeof result === "object" && result !== null && "metadata" in result;
}

export function asManifests(items: components["schemas"]["ResourceEnvelope"][]): Manifest[] {
  return items as Manifest[];
}

/**
 * Reads `metadata.title` for the active locale, falling back to the name. A title is one plain
 * string (UI-50); the legacy language map still reads.
 */
export function localized(
  title: string | Record<string, string> | null | undefined,
  locale: string,
  fallback: string,
): string {
  if (typeof title === "string") {
    return title.trim() === "" ? fallback : title;
  }
  if (!title) {
    return fallback;
  }
  return title[locale] ?? title[locale.split("-")[0]] ?? title.en ?? title.sk ?? fallback;
}

/**
 * The language of a legacy map a form's one box shows for `locale`, the order `localized` reads
 * it in: the locale, its base, `en`, `sk`, then the map's first entry.
 */
function shownLanguage(map: Record<string, unknown>, locale: string): string | undefined {
  return [locale, locale.split("-")[0], "en", "sk"].find((key) => key in map) ?? Object.keys(map)[0];
}

/**
 * What one box writes over a stored title or description (UI-50, T-2764): one string over a legacy
 * language map replaces the entry the box showed and keeps every other language; over anything
 * else it is the string written.
 */
export function rewritten(stored: unknown, written: string, locale: string): string | Record<string, unknown> {
  if (!stored || typeof stored !== "object") {
    return written;
  }
  const map = stored as Record<string, unknown>;
  const language = shownLanguage(map, locale);
  return language === undefined ? written : { ...map, [language]: written };
}

/**
 * A title as the one string a form edits and a manifest now writes (UI-50): a plain string as
 * written, a legacy language map as `en`, then its first non-empty value. Saving the form writes
 * the string, so a manifest converts itself on its next change.
 */
export function plainTitle(title: unknown): string | undefined {
  if (typeof title === "string") {
    return title.trim() === "" ? undefined : title;
  }
  if (!title || typeof title !== "object") {
    return undefined;
  }
  const texts = Object.entries(title as Record<string, unknown>).filter(
    (entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].trim() !== "",
  );
  return (texts.find(([locale]) => locale === "en") ?? texts[0])?.[1];
}

/**
 * The manifest a form proposes for one it edits (AG-77, PF-57): `next`, what the form wrote,
 * over `base`, what is in Git. The spec keys the form owns come from `next` alone (a cleared
 * field is removed); every other key of the base travels unchanged, so an edit of one field
 * never proposes the loss of a `policyRef` or a `publish` block the form has no control for.
 * A localized title keeps its map, the form's plain text replacing the entry it showed.
 * Without a base (a new resource) `next` is the manifest.
 */
export function overlay(base: Manifest | undefined, next: Manifest, owned: string[]): Manifest {
  if (!base) {
    return next;
  }
  const spec: Record<string, unknown> = { ...base.spec };
  for (const key of owned) {
    delete spec[key];
  }
  Object.assign(spec, next.spec);
  const metadata: Record<string, unknown> = { ...base.metadata, ...next.metadata };
  const title = next.metadata.title;
  const baseTitle = base.metadata.title;
  if (typeof title === "string" && baseTitle && typeof baseTitle === "object") {
    const shown = Object.entries(baseTitle as Record<string, unknown>).find(
      ([, text]) => typeof text === "string" && text === plainTitle(baseTitle),
    );
    metadata.title = { ...(baseTitle as Record<string, unknown>), [shown?.[0] ?? "en"]: title };
  } else if (title === undefined) {
    delete metadata.title;
  }
  // The space label is the form's convenience, not a change to propose on a manifest without it.
  if (base.metadata.labels) {
    metadata.labels = { ...base.metadata.labels, ...next.metadata.labels };
  } else {
    delete metadata.labels;
  }
  return { ...next, metadata: metadata as Manifest["metadata"], spec };
}

/**
 * The value without the empty leaves a form leaves behind: rjsf keeps an empty object for
 * every group the user opened and left alone, and an empty string for every field they
 * cleared, none of which a manifest should carry.
 */
export function prune<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.filter((item) => item !== undefined && item !== "") as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, member] of Object.entries(value)) {
      const cleaned = prune(member);
      const empty =
        cleaned === undefined ||
        cleaned === "" ||
        (cleaned !== null && typeof cleaned === "object" && Object.keys(cleaned).length === 0);
      if (!empty) {
        out[key] = cleaned;
      }
    }
    return out as T;
  }
  return value;
}

/**
 * The name a `Ref` carries. jc-core writes a reference either as a bare name or as
 * `{ kind, name, namespace? }` (MF-07), and a manifest in the mirror may hold either, so
 * reading `spec.contextSpaceRef` as a string gives `[object Object]` half the time, or worse:
 * a manifest whose own reference is an object nested inside a reference.
 */
export function refName(reference: unknown): string {
  if (typeof reference === "string") {
    return reference;
  }
  if (typeof reference === "object" && reference !== null && "name" in reference) {
    const name = (reference as { name?: unknown }).name;
    return typeof name === "string" ? name : "";
  }
  return "";
}

/**
 * The metadata of the manifest an edit started from, kept whole: a kind's form edits the spec and
 * has no field for a title, a description or a label, so an edit that rebuilt metadata from the
 * name alone would propose deleting them.
 */
export function storedMetadata(stored: unknown): Record<string, unknown> {
  const metadata = (stored as { metadata?: unknown } | null | undefined)?.metadata;
  return metadata && typeof metadata === "object" && !Array.isArray(metadata)
    ? (metadata as Record<string, unknown>)
    : {};
}

/**
 * The secret names this project already references, so the picker offers what exists instead
 * of inviting a typo (CC-06).
 *
 * The Portal never lists the secret store itself: a name is not a value, and a name nobody uses
 * yet is typed in full and read by an approver with the merge request.
 */
export function knownSecretNames(manifests: Manifest[]): string[] {
  const names = new Set<string>();
  const walk = (value: unknown): void => {
    if (Array.isArray(value)) {
      value.forEach(walk);
      return;
    }
    if (!value || typeof value !== "object") {
      return;
    }
    for (const [key, member] of Object.entries(value)) {
      if (key.endsWith("Ref") || key === "secretRefs" || key === "secrets") {
        for (const entry of Array.isArray(member) ? member : [member]) {
          const named = (entry ?? {}) as { name?: unknown };
          if (typeof named.name === "string") {
            names.add(named.name);
          }
        }
      }
      walk(member);
    }
  };
  manifests.forEach((manifest) => {
    walk(manifest.spec);
  });
  return [...names].sort();
}
