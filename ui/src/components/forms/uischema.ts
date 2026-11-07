/**
 * The `kind: UiSchema` manifest as an RJSF `uiSchema` (T-0202, UI-02, CC-29, CC-31).
 *
 * A UiSchema manifest arranges a form; it never declares a field. Everything renderable comes
 * from the JSON Schema, so this consumer only ever reorders, labels, hides and picks widgets
 * for fields the schema already has (Architecture/09 section 2).
 *
 * Two things it refuses to pass through. A widget name the Portal has not registered is
 * dropped with its reason, because RJSF throws on an unknown widget and a form that does not
 * render is a worse answer than a form with a default input. And a field marked `advanced` is
 * hidden unless the caller asks for the advanced mode: Git mechanics are fields like any
 * other, and CC-29 is about which of them a steward has to look at, not about removing them.
 */

import type { UiSchema } from "./types";

/** RJSF's own widgets, by the name a manifest writes. */
const BUILT_IN_WIDGETS = [
  "text",
  "textarea",
  "select",
  "checkboxes",
  "checkbox",
  "radio",
  "range",
  "color",
  "date",
  "date-time",
  "password",
  "updown",
  "email",
  "uri",
  "hidden",
] as const;

/**
 * A human string: one plain string in the author's language (UI-50), or the legacy map of
 * every locale the manifest carries (Architecture/09 section 3), still read through v0.9.
 */
export type LanguageMap = string | Record<string, string>;

/** One field's arrangement. */
export interface FieldArrangement {
  widget?: string;
  help?: LanguageMap;
  /**
   * An example the field accepts. A list is an example of the field's *items*: the control a
   * person types into is the item, so a list written at the array itself reaches a control that
   * holds a list and reads as "Invalid type" (T-2257).
   */
  placeholder?: string | (string | number)[];
  /** A twelfth of the row, so `6` is half. */
  columns?: number;
  readOnly?: boolean;
  /** Hidden outside the advanced mode (CC-29). */
  advanced?: boolean;
}

/** One visual grouping of a flat schema. */
export interface Group {
  title?: LanguageMap;
  description?: LanguageMap;
  /**
   * Folded shut when the form opens, with its title and its one-line description as the handle
   * (Architecture/09 section 2). A long form otherwise puts the three fields that matter and the
   * twenty-seven that do not on one screen, in schema order (T-1607). A group holding a required
   * field is never folded: the form would ask for something the person cannot see.
   */
  folded?: boolean;
  fields: string[];
}

/** The `spec` of a `kind: UiSchema` manifest. */
export interface UiSchemaSpec {
  /** The kind whose form this arranges. */
  for: string;
  /**
   * What the kind is for, in two sentences, shown at the top of the form before it asks anything
   * (Architecture/09 section 2): what it does for the person, and what it needs from them.
   */
  about?: LanguageMap;
  /**
   * The kind's page in the User Guide, as a path inside it: `User-Guide/05-endpoints-and-sharing`
   * (Architecture/09 section 2). Never an address — the installation's `documentationBaseUrl`
   * says where the guide is served, and a form shows the link only when both halves are there.
   * A value with a scheme, an authority, a leading slash or a `..` segment is dropped, so a
   * committed arrangement cannot point the link at a site of its own.
   */
  guide?: string;
  /**
   * Top-level fields of which the form renders one at a time: a data source's `mqtt`, `http`,
   * `webSocket`, `gtfsRt`, a sync source's `git`, `bundle`, `platformApi`. One manifest arranges
   * every branch, so the ones the person did not pick are absent from the schema in front of them
   * and are passed over in silence. Everything not named here is still reported when the schema
   * does not have it, because that is a typo and a manifest silently ignored is worse than none.
   */
  variants?: string[];
  order?: string[];
  groups?: Group[];
  fields?: Record<string, FieldArrangement>;
}

/** A `kind: UiSchema` manifest as it is committed. */
export interface UiSchemaManifest {
  apiVersion?: string;
  kind?: string;
  metadata?: { name?: string };
  spec: UiSchemaSpec;
}

export interface ArrangeOptions {
  /** The locale to resolve language maps in; falls back to Slovak, then to any. */
  locale?: string;
  /** The names the JSON Schema actually declares. Arrangement of anything else is dropped. */
  properties?: string[];
  /** Whether the advanced fields are shown (CC-29). */
  advanced?: boolean;
  /** Widget names the Portal has registered beyond RJSF's own. */
  widgets?: string[];
  /** The fields the person has to fill: one of them in a group keeps that group unfolded (T-1607). */
  required?: string[];
  /**
   * What `{project}` and `{orgDomain}` in an example stand for: the project the form writes into
   * and the organization's own domain, so an example reads as this organization's and "Use
   * example" fills a value that exists here (T-2752). An example naming a value that is not
   * given is left out rather than shown with its braces.
   */
  examples?: { project?: string; orgDomain?: string };
}

/** An example with its `{project}` and `{orgDomain}` filled in, or nothing when one is not known. */
export function exampleOf(
  written: string,
  values: { project?: string; orgDomain?: string } = {},
): string | undefined {
  let unknown = false;
  const filled = written.replace(/\{(project|orgDomain)\}/g, (_, name: "project" | "orgDomain") => {
    const value = values[name];
    if (value === undefined || value === "") {
      unknown = true;
      return "";
    }
    return value;
  });
  return unknown ? undefined : filled;
}

/** What one arrangement produced, and what it could not use. */
export interface Arranged {
  uiSchema: UiSchema;
  /** What the kind is for, in the caller's locale, when the manifest says. */
  about?: string;
  /** The guide page this kind's arrangement names, when it names a usable one. */
  guide?: string;
  /** Everything the manifest asked for that this form cannot do, each in one clause. */
  problems: string[];
}

/** The default locale of the platform, and the fallback of every language map. */
const DEFAULT_LOCALE = "sk";

/**
 * A `guide` the form may build a link from: a relative path inside the documentation site.
 *
 * The link is the installation's base URL joined with this, so anything that could leave that
 * site is refused rather than trimmed — a scheme, an authority, a leading slash, a `..` segment,
 * a backslash or whitespace. A manifest names a page; it never names a place (Architecture/09
 * section 2).
 */
function guidePath(value: string | undefined, problems: string[]): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  const path = value.trim().replace(/^\/+|\/+$/g, "");
  const leaves =
    path === "" ||
    path.includes("://") ||
    path.startsWith("//") ||
    path.split("/").includes("..") ||
    /[\\\s]/.test(path) ||
    /^[a-z][a-z0-9+.-]*:/i.test(path);
  if (leaves) {
    problems.push(`guide ${JSON.stringify(value)} is not a path inside the documentation site`);
    return undefined;
  }
  return path;
}

/** A plain string as written; a legacy map in the caller's locale, then Slovak, then anything. */
/**
 * Whether the form fills a field by itself: it has a default, or every field it requires has
 * one. Such a required field may sit behind a fold, since the person need not type it (T-4824).
 */
export function prefilled(schema: unknown): boolean {
  const definition = schema as
    | { default?: unknown; required?: string[]; properties?: Record<string, unknown> }
    | undefined;
  if (definition?.default !== undefined) return true;
  const required = definition?.required ?? [];
  return (
    definition?.properties !== undefined &&
    required.length > 0 &&
    required.every((field) => prefilled(definition.properties?.[field]))
  );
}

export function localized(map: LanguageMap | undefined, locale?: string): string | undefined {
  if (!map) {
    return undefined;
  }
  if (typeof map === "string") {
    return map;
  }
  const wanted = locale ? map[locale] : undefined;
  return wanted ?? map[DEFAULT_LOCALE] ?? Object.values(map)[0];
}

/**
 * Every field a manifest may arrange, by the path a manifest writes: a top-level name, a dot into
 * an object, `[]` into an array's items (Architecture/09 section 2).
 *
 * The fields a person understands least are the nested ones — `http.url`,
 * `caching.maxAgeSeconds`, `pages[].widgets[].widgetType` — so a path set of top-level names alone
 * left the help of a whole form unwritable (T-1612).
 */
export function paths(schema: { properties?: Record<string, unknown> } | undefined): string[] {
  const found: string[] = [];
  const walk = (node: unknown, prefix: string): void => {
    const properties = (node as { properties?: Record<string, unknown> } | undefined)?.properties;
    if (!properties) {
      return;
    }
    for (const [name, definition] of Object.entries(properties)) {
      if (!definition || typeof definition !== "object") {
        continue;
      }
      const path = prefix ? `${prefix}.${name}` : name;
      found.push(path);
      const field = definition as { properties?: unknown; items?: unknown };
      if (field.properties) {
        walk(field, path);
      } else if (field.items) {
        walk(field.items, `${path}[]`);
      }
    }
  };
  walk(schema, "");
  return found;
}

type Requiring = {
  properties?: Record<string, unknown>;
  required?: string[];
  if?: unknown;
  then?: Requiring;
  allOf?: Requiring[];
};

/**
 * A node with the `then` of every `if` its value meets folded in, so a field a branch asks for
 * (a public channel's origins) is counted as the form marks it (T-4282). `holds` is the form's
 * validator; without it the branches are not read.
 */
function withBranches(node: Requiring | undefined, value: unknown, holds?: (condition: unknown, value: unknown) => boolean): Requiring | undefined {
  if (!node || !holds) return node;
  const active = [node, ...(node.allOf ?? [])].filter((one) => one.if !== undefined && one.then && holds(one.if, value)).map((one) => one.then as Requiring);
  if (active.length === 0) return node;
  const properties: Record<string, unknown> = { ...(node.properties ?? {}) };
  for (const branch of active) {
    for (const [name, extra] of Object.entries(branch.properties ?? {})) {
      const own = (properties[name] ?? {}) as Requiring;
      const more = (extra ?? {}) as Requiring;
      properties[name] = more.required ? { ...own, required: [...new Set([...(own.required ?? []), ...more.required])] } : own;
    }
  }
  return { ...node, properties, required: [...new Set([...(node.required ?? []), ...active.flatMap((one) => one.required ?? [])])] };
}

/**
 * How much of what the form must have is in it: the required fields it still wants, counted from the
 * schema and the data in hand (T-1607).
 *
 * What counts is what the form asks for on screen: the required fields of the object itself and of
 * every required object inside it, whose inputs the form renders straight away. An optional object
 * is not walked at all — a data source's TLS block requires a secret name only once somebody opens
 * it. A person who reads "2 of 7" and cannot find the seventh field has been lied to.
 */
export function requiredProgress(
  schema: { properties?: Record<string, unknown>; required?: string[] } | undefined,
  data: unknown,
  holds?: (condition: unknown, value: unknown) => boolean,
): { filled: number; total: number } {
  let filled = 0;
  let total = 0;
  const walk = (given: Requiring | undefined, value: unknown): void => {
    const node = withBranches(given, value, holds);
    const properties = node?.properties ?? {};
    for (const name of node?.required ?? []) {
      const definition = properties[name] as (Requiring & { type?: string }) | undefined;
      const held = (value as Record<string, unknown> | undefined)?.[name];
      if (definition?.properties) {
        // A required object: the form renders its fields at once, so they are being asked for.
        walk(definition, held);
        continue;
      }
      total += 1;
      const empty =
        held === undefined ||
        held === null ||
        held === "" ||
        (Array.isArray(held) && held.length === 0);
      if (!empty) {
        filled += 1;
      }
    }
  };
  walk(schema as Requiring | undefined, data);
  return { filled, total };
}

/**
 * Writes one field's arrangement into the RJSF `uiSchema` at the path the manifest named: a dot
 * enters an object, `[]` enters an array's `items`, which is where RJSF looks for the arrangement
 * of every element of a list.
 */
function at(uiSchema: UiSchema, path: string, entry: Record<string, unknown>): void {
  const steps = path.split(".").flatMap((step) =>
    step.endsWith("[]") ? [step.slice(0, -2), "items"] : [step],
  );
  const leaf = steps.pop() as string;
  let node = uiSchema as Record<string, unknown>;
  for (const step of steps) {
    const next = node[step];
    node[step] = next && typeof next === "object" ? next : {};
    node = node[step] as Record<string, unknown>;
  }
  node[leaf] = { ...((node[leaf] as Record<string, unknown> | undefined) ?? {}), ...entry };
}

/**
 * The RJSF `uiSchema` one manifest describes.
 *
 * Order: the manifest's names first, in its order, then `"*"` so RJSF renders everything the
 * manifest did not mention in schema order after them. A form that dropped an unmentioned
 * field would hide a value the user has to fill in.
 */
export function arrange(manifest: UiSchemaManifest, options: ArrangeOptions = {}): Arranged {
  const problems: string[] = [];
  const spec = manifest.spec;
  // An empty path set is a schema that has not arrived, not a schema without fields: a dialog that
  // opens while its lists are still loading would otherwise show a complaint about every field the
  // manifest arranges, and take it back a moment later. Nothing is checked, as with no set at all.
  const known = options.properties?.length ? options.properties : undefined;
  const widgets = new Set<string>([...BUILT_IN_WIDGETS, ...(options.widgets ?? [])]);
  const required = new Set(options.required ?? []);

  const declared = (field: string): boolean => {
    if (!known || known.includes(field)) {
      return true;
    }
    // A branch the person did not pick (`spec.variants`) is absent from the schema in front of
    // them and is not a mistake: it is passed over without a word. A field the manifest names
    // anywhere else is a typo and still says so.
    // A variant is a top-level branch or one nested field (`rateLimits.burst`, T-3142).
    const root = field.split(/[.[]/)[0];
    const variants = spec.variants ?? [];
    if ((variants.includes(root) && !known.includes(root)) || variants.includes(field)) {
      return false;
    }
    problems.push(`the schema has no field \`${field}\`, so nothing was arranged for it`);
    return false;
  };

  /**
   * `ui:order` and a group hold top-level names: RJSF orders and groups the fields of one object,
   * and a nested path there would silently order nothing (Architecture/09 section 2).
   */
  const topLevel = (field: string, where: string): boolean => {
    if (field.includes(".") || field.includes("[")) {
      problems.push(`\`${field}\` is a nested field, so ${where} cannot hold it`);
      return false;
    }
    return declared(field);
  };

  const uiSchema: UiSchema = {};

  for (const [field, arrangement] of Object.entries(spec.fields ?? {})) {
    if (!declared(field)) {
      continue;
    }
    const entry: Record<string, unknown> = {};

    if (arrangement.widget !== undefined) {
      if (widgets.has(arrangement.widget)) {
        entry["ui:widget"] = arrangement.widget;
      } else {
        problems.push(
          `\`${arrangement.widget}\` is not a widget this Portal registers, so \`${field}\` keeps its default input`,
        );
      }
    }

    const help = localized(arrangement.help, options.locale);
    if (help !== undefined) {
      entry["ui:help"] = help;
    }
    if (Array.isArray(arrangement.placeholder)) {
      // The example of one item, on the item: filling the first empty one is what the person
      // wants, and an array whose items each carry the example needs no example of its own.
      const [first] = arrangement.placeholder;
      const example = first === undefined ? undefined : exampleOf(String(first), options.examples);
      if (example !== undefined) {
        at(uiSchema, `${field}[]`, { "ui:placeholder": example });
      }
    } else if (arrangement.placeholder !== undefined) {
      const example = exampleOf(String(arrangement.placeholder), options.examples);
      if (example !== undefined) {
        entry["ui:placeholder"] = example;
      }
    }
    if (arrangement.readOnly !== undefined) {
      entry["ui:readonly"] = arrangement.readOnly;
    }
    if (arrangement.columns !== undefined) {
      if (Number.isInteger(arrangement.columns) && arrangement.columns >= 1 && arrangement.columns <= 12) {
        entry["ui:options"] = { columns: arrangement.columns };
      } else {
        problems.push(`\`${field}\` asks for ${arrangement.columns} of twelve columns, which is not a column count`);
      }
    }
    // CC-29: hidden by arrangement, never removed. RJSF keeps the value and submits it, so
    // the commit message a steward never sees is still the one the Change carries.
    if (arrangement.advanced && options.advanced !== true) {
      entry["ui:widget"] = "hidden";
    }

    if (Object.keys(entry).length > 0) {
      at(uiSchema, field, entry);
    }
  }

  const order = (spec.order ?? []).filter((field) => topLevel(field, "the reading order"));
  if (order.length > 0) {
    uiSchema["ui:order"] = [...order, "*"];
  }

  const groups = (spec.groups ?? [])
    .map((group) => ({
      title: localized(group.title, options.locale),
      description: localized(group.description, options.locale),
      // A group with a required field in it stays open whatever the manifest says: a form that
      // asks for something behind a fold asks for something nobody can see.
      folded: group.folded === true && !(group.fields ?? []).some((field) => required.has(field)),
      fields: (group.fields ?? []).filter((field) => topLevel(field, "a group")),
    }))
    .filter((group) => group.fields.length > 0);
  if (groups.length > 0) {
    uiSchema["ui:options"] = { ...(uiSchema["ui:options"] as object | undefined), groups };
  }

  const guide = guidePath(spec.guide, problems);

  return { uiSchema, about: localized(spec.about, options.locale), guide, problems };
}

/**
 * What a later manifest for the same kind changes about an earlier one: a field at a time, and
 * `order` or `groups` whole (Architecture/09 section 2).
 *
 * This is how the arrangement the UI ships meets the one an organization commits to
 * `portal/forms/`: writing help for one field keeps the shipped help on all the others, which a
 * whole-file override would throw away. An order or a grouping is replaced entire, because half of
 * someone else's reading order is not a reading order.
 */
function merged(base: UiSchemaManifest, over: UiSchemaManifest): UiSchemaManifest {
  const fields: Record<string, FieldArrangement> = { ...(base.spec.fields ?? {}) };
  for (const [name, arrangement] of Object.entries(over.spec.fields ?? {})) {
    fields[name] = { ...fields[name], ...arrangement };
  }
  return {
    ...over,
    spec: {
      ...base.spec,
      ...over.spec,
      order: over.spec.order ?? base.spec.order,
      groups: over.spec.groups ?? base.spec.groups,
      fields,
    },
  };
}

/**
 * The manifests of a directory, indexed by the kind each one arranges.
 *
 * A document that is not a `UiSchema` manifest, or that names no kind, is left out rather
 * than indexed under a guess: `portal/forms/` is read as a whole and one malformed file must
 * not take the other forms down with it.
 *
 * Two manifests for one kind are merged in the order given, the later winning field by field:
 * the caller passes the arrangements the UI ships first and the configuration repository's after
 * them, so an organization changes what it wrote about and nothing else.
 */
export function index(
  documents: unknown[],
): { forms: Record<string, UiSchemaManifest>; problems: string[] } {
  const forms: Record<string, UiSchemaManifest> = {};
  const problems: string[] = [];

  for (const document of documents) {
    const manifest = document as UiSchemaManifest | null;
    if (!manifest || typeof manifest !== "object" || manifest.kind !== "UiSchema") {
      problems.push("a document in portal/forms is not a UiSchema manifest and was left out");
      continue;
    }
    const target = manifest.spec?.for;
    if (typeof target !== "string" || target.length === 0) {
      problems.push(
        `\`${manifest.metadata?.name ?? "unnamed"}\` names no kind in spec.for, so no form knows to use it`,
      );
      continue;
    }
    forms[target] = forms[target] ? merged(forms[target], manifest) : manifest;
  }

  return { forms, problems };
}

const isPlain = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The caller's uiSchema over the manifest's, field by field at every depth: a caller that hides
 * `source.query.type` keeps the help the manifest wrote for `source.query.geoQ` (T-2754).
 */
export function mergeUi(arranged: unknown, own: unknown): unknown {
  if (!isPlain(arranged) || !isPlain(own)) {
    return own === undefined ? arranged : own;
  }
  const merged: Record<string, unknown> = { ...arranged };
  for (const [key, value] of Object.entries(own)) {
    merged[key] = mergeUi(arranged[key], value);
  }
  return merged;
}
