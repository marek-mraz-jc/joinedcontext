/**
 * The publish flow's form (EP-83): the drafted `spec.catalog` as fields a person edits, and back.
 * Pure, so every rule of the form is tested without a dialog.
 */
export const LICENCES = ["CC_BY_4_0", "CC0", "CC_BYSA_4_0", "ODC_BY", "ODC_ODBL", "ODC_PDDL"] as const;
export const THEMES = [
  "AGRI",
  "ECON",
  "EDUC",
  "ENER",
  "ENVI",
  "GOVE",
  "HEAL",
  "INTR",
  "JUST",
  "REGI",
  "SOCI",
  "TECH",
  "TRAN",
] as const;
export const FREQUENCIES = [
  "CONT",
  "HOURLY",
  "DAILY",
  "WEEKLY",
  "MONTHLY",
  "QUARTERLY",
  "ANNUAL",
  "IRREG",
  "NEVER",
  "UNKNOWN",
] as const;

export type CatalogBlock = Record<string, unknown>;

export interface PublishForm {
  /** The language the texts below are in: the draft's keyword language, else the reader's. */
  language: string;
  publisher: string;
  contactName: string;
  contactEmail: string;
  license: string;
  themes: string[];
  /** Comma-separated, as a person types them. */
  keywords: string;
  spatial: string;
  temporalStart: string;
  temporalEnd: string;
  frequency: string;
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function list(text: string): string[] {
  return [...new Set(text.split(",").map((item) => item.trim()).filter((item) => item !== ""))];
}

/** The draft's catalogue block as the form's fields. */
export function formOf(catalog: CatalogBlock, fallbackLanguage: string): PublishForm {
  const keywords = record(catalog.keywords);
  const publisher = record(record(catalog.publisher).name);
  const language = Object.keys(keywords)[0] ?? Object.keys(publisher)[0] ?? fallbackLanguage;
  const contact = record(catalog.contactPoint);
  const temporal = record(catalog.temporal);
  const words = keywords[language];
  return {
    language,
    publisher: str(publisher[language]) || str(Object.values(publisher)[0]),
    contactName: str(contact.name),
    contactEmail: str(contact.email),
    license: str(catalog.license),
    themes: Array.isArray(catalog.themes) ? catalog.themes.filter((t): t is string => typeof t === "string") : [],
    keywords: Array.isArray(words) ? words.filter((w): w is string => typeof w === "string").join(", ") : "",
    spatial: Array.isArray(catalog.spatial)
      ? catalog.spatial.filter((s): s is string => typeof s === "string").join(", ")
      : "",
    temporalStart: str(temporal.start),
    temporalEnd: str(temporal.end),
    frequency: str(catalog.frequency),
  };
}

/** What is wrong with the form, as locale keys per field; empty when it can be proposed. */
export function problems(form: PublishForm): Partial<Record<keyof PublishForm, string>> {
  const out: Partial<Record<keyof PublishForm, string>> = {};
  if (!form.license) out.license = "catalogue.publish.error.licence";
  const hasName = form.contactName.trim() !== "";
  const hasEmail = form.contactEmail.trim() !== "";
  if (hasName && !hasEmail) out.contactEmail = "catalogue.publish.error.contactBoth";
  if (hasEmail && !hasName) out.contactName = "catalogue.publish.error.contactBoth";
  if (hasEmail && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(form.contactEmail.trim())) {
    out.contactEmail = "catalogue.publish.error.email";
  }
  if (form.temporalStart && form.temporalEnd && form.temporalEnd < form.temporalStart) {
    out.temporalEnd = "catalogue.publish.error.period";
  }
  return out;
}

/**
 * The catalogue block the Change writes: the draft with the form's fields over it, so a member
 * the form does not show (attribution, sources, legislation) is carried as the draft had it.
 */
export function catalogOf(draft: CatalogBlock, form: PublishForm): CatalogBlock {
  const out: CatalogBlock = { ...draft };
  const set = (key: string, value: unknown, empty: boolean) => {
    if (empty) {
      delete out[key];
    } else {
      out[key] = value;
    }
  };
  // A text per language: the form edits one language and leaves the others as they were.
  const inLanguage = (map: unknown, value: unknown, empty: boolean) => {
    const next = { ...record(map) };
    if (empty) {
      delete next[form.language];
    } else {
      next[form.language] = value;
    }
    return next;
  };
  const publisher = record(draft.publisher);
  const names = inLanguage(publisher.name, form.publisher.trim(), form.publisher.trim() === "");
  set("publisher", { ...publisher, name: names }, Object.keys(names).length === 0);
  set(
    "contactPoint",
    { name: form.contactName.trim(), email: form.contactEmail.trim() },
    form.contactName.trim() === "" || form.contactEmail.trim() === "",
  );
  set("license", form.license, form.license === "");
  set("themes", form.themes, form.themes.length === 0);
  const keywords = list(form.keywords);
  const words = inLanguage(draft.keywords, keywords, keywords.length === 0);
  set("keywords", words, Object.keys(words).length === 0);
  const spatial = list(form.spatial);
  set("spatial", spatial, spatial.length === 0);
  set(
    "temporal",
    {
      ...(form.temporalStart ? { start: form.temporalStart } : {}),
      ...(form.temporalEnd ? { end: form.temporalEnd } : {}),
    },
    !form.temporalStart && !form.temporalEnd,
  );
  set("frequency", form.frequency, form.frequency === "");
  return out;
}

/** One attribute of the draft (`CatalogueDraft.attributes`): offered unless `withheld` says why. */
export interface DraftAttribute {
  type: string;
  name: string;
  withheld?: "personal" | "hidden";
}

/**
 * The Endpoint's `projection.hiddenAttributes` after the form (DM-122, EP-61): what it hid
 * already, every personal attribute (a public projection of one is refused, so it is hidden
 * rather than left to fail), and each offered attribute the person left out of the dataset.
 */
export function hiddenOf(current: unknown, attributes: DraftAttribute[], leftOut: string[]): string[] {
  const kept = Array.isArray(current) ? current.filter((h): h is string => typeof h === "string") : [];
  const personal = attributes.filter((a) => a.withheld === "personal").map((a) => a.name);
  return [...new Set([...kept, ...personal, ...leftOut])].sort();
}

/** The columns the public dataset carries: the offered attributes the person kept. */
export function columnsOf(attributes: DraftAttribute[], leftOut: string[]): string[] {
  return [...new Set(attributes.filter((a) => !a.withheld && !leftOut.includes(a.name)).map((a) => a.name))];
}

/** A text per language with `language`'s entry set to `value`, or removed when it is empty. */
export function textsWith(current: unknown, language: string, value: string): Record<string, string> | undefined {
  const next: Record<string, string> =
    typeof current === "string" && current.trim() !== ""
      ? { [language]: current }
      : Object.fromEntries(Object.entries(record(current)).filter((e): e is [string, string] => typeof e[1] === "string"));
  if (value.trim() === "") delete next[language];
  else next[language] = value.trim();
  return Object.keys(next).length > 0 ? next : undefined;
}

/** Path members that reach an object's prototype rather than a member of it (F-UI2-002). */
const PROTOTYPE_KEYS = new Set(["__proto__", "constructor", "prototype"]);

/**
 * A manifest with a change's field diff applied (`FieldChange`: dotted paths with `[i]` indices,
 * `to` absent for a removed leaf), so the approval page previews what the Change makes. Only the
 * members the preview reads are needed; a key holding a dot (a label) is outside them.
 */
export function applyFields(
  base: Record<string, unknown>,
  fields: Array<{ path: string; to?: unknown }>,
): Record<string, unknown> {
  const out = structuredClone(base);
  for (const field of fields) {
    const keys = field.path
      .split(".")
      .flatMap((part) => part.split(/\[(\d+)\]/).filter((k) => k !== ""))
      .map((k) => (/^\d+$/.test(k) ? Number(k) : k));
    // A path is data: one naming the prototype would write into every object of the page.
    if (keys.some((key) => typeof key === "string" && PROTOTYPE_KEYS.has(key))) continue;
    let node: Record<string | number, unknown> = out;
    keys.forEach((key, i) => {
      if (i === keys.length - 1) {
        if (field.to === undefined) {
          if (Array.isArray(node)) node[key as number] = undefined;
          else delete node[key];
        } else {
          node[key] = field.to;
        }
        return;
      }
      if (node[key] === null || typeof node[key] !== "object") {
        node[key] = typeof keys[i + 1] === "number" ? [] : {};
      }
      node = node[key] as Record<string | number, unknown>;
    });
  }
  // A removed array element leaves a hole; the manifest has none.
  const compact = (value: unknown): unknown =>
    Array.isArray(value)
      ? value.filter((v) => v !== undefined).map(compact)
      : value && typeof value === "object"
        ? Object.fromEntries(Object.entries(value).map(([k, v]) => [k, compact(v)]))
        : value;
  return compact(out) as Record<string, unknown>;
}
