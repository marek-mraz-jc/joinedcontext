/**
 * An Endpoint's `spec.catalog` block: what a DCAT-AP catalogue needs beyond the distributions
 * (T-2789, EP-78, EP-79, EP-80). The manifest keeps text per language; the form shows one plain
 * string in the author's language and writes it back over the entry it showed, so another
 * language already in the manifest is kept (UI-50).
 */

/** The EU licence table codes whose duties are known (EP-79), in the order the picker lists them. */
export const CATALOG_LICENCES = ["CC_BY_4_0", "CC_BYSA_4_0", "CC0", "ODC_BY", "ODC_ODBL", "ODC_PDDL"] as const;

/** The EU data-theme table. */
export const DATA_THEMES = [
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

/** The EU frequency table, fastest first. */
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

/** What a licence asks of a user, as the offer states it (EP-79). */
export function dutyOf(licence: string | undefined): "none" | "attribute" | "shareAlike" | undefined {
  switch (licence) {
    case "CC0":
    case "ODC_PDDL":
      return "none";
    case "CC_BY_4_0":
    case "ODC_BY":
      return "attribute";
    case "CC_BYSA_4_0":
    case "ODC_ODBL":
      return "shareAlike";
    default:
      return undefined;
  }
}

/** The EU licence table IRI of a code, which the record names. */
export function licenceIri(licence: string): string {
  return `http://publications.europa.eu/resource/authority/licence/${licence}`;
}

/** A NUTS code (`SK032`) or an absolute http(s) IRI, as jc-core accepts a `spatial` entry. */
export const SPATIAL_PATTERN = "^([A-Z]{2}[0-9A-Z]{0,3}|https?://[^\\s<>\"{}|^`\\\\]+)$";

/** An absolute http(s) IRI with nothing that would close it in Turtle. */
export const IRI_PATTERN = "^https?://[^\\s<>\"{}|^`\\\\]+$";

/** An address a `mailto:` IRI can carry, as jc-core checks it. */
export const EMAIL_PATTERN = "^[A-Za-z0-9._+-]{1,64}@[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$";

type Texts = Record<string, string>;

/** The manifest's `spec.catalog`, as far as the form reads and writes it. */
export interface CatalogManifest {
  publisher?: { name?: Texts; uri?: string };
  contactPoint?: { name?: string; email?: string };
  license?: string;
  attribution?: Texts;
  themes?: string[];
  keywords?: Record<string, string[]>;
  spatial?: string[];
  temporal?: { start?: string; end?: string };
  frequency?: string;
  source?: Array<{ url?: string; title?: Texts; description?: Texts }>;
  pipelineRef?: { kind?: string; name?: string };
  applicableLegislation?: string[];
}

/** The same block as the form holds it: one string per text, the reference by its name. */
export interface CatalogForm {
  publisher?: { name?: string; uri?: string };
  contactPoint?: { name?: string; email?: string };
  license?: string;
  attribution?: string;
  themes?: string[];
  keywords?: string[];
  spatial?: string[];
  temporal?: { start?: string; end?: string };
  frequency?: string;
  source?: Array<{ url?: string; title?: string; description?: string }>;
  pipelineRef?: string;
  applicableLegislation?: string[];
}

/**
 * The language whose entry a plain string stands for: the reader's when the map has it, else
 * English, else the first the map holds, else the reader's for a map that holds nothing yet.
 */
export function shownLanguage(map: Record<string, unknown> | undefined, language: string): string {
  const keys = Object.keys(map ?? {});
  if (keys.includes(language)) {
    return language;
  }
  if (keys.includes("en")) {
    return "en";
  }
  return keys[0] ?? language;
}

function shown(map: Texts | undefined, language: string): string | undefined {
  const text = map?.[shownLanguage(map, language)];
  return text && text.trim() !== "" ? text : undefined;
}

/**
 * The entry a plain string is written to: the one the form showed (English, else the first), or
 * the author's language for a map that holds nothing yet.
 */
function entryOf(map: Record<string, unknown> | undefined, language: string): string {
  return Object.keys(map ?? {}).length > 0 ? shownLanguage(map, SHOWN) : language;
}

/** The language the form shows a stored text in, as the title shows it: English, else the first. */
const SHOWN = "en";

/** A plain string written back over the entry it showed; a cleared one removes that entry. */
function written(map: Texts | undefined, text: string | undefined, language: string): Texts | undefined {
  const next: Texts = { ...(map ?? {}) };
  const key = entryOf(map, language);
  if (text && text.trim() !== "") {
    next[key] = text.trim();
  } else {
    delete next[key];
  }
  return Object.keys(next).length > 0 ? next : undefined;
}

const filled = (items: string[] | undefined): string[] =>
  (items ?? []).map((item) => item.trim()).filter((item) => item !== "");

/**
 * The stored block as the form shows it: each text the entry the title rule shows, English else
 * the first, and written back over that same entry (UI-50).
 */
export function catalogForm(catalog: CatalogManifest | undefined): CatalogForm | undefined {
  const language = SHOWN;
  if (!catalog) {
    return undefined;
  }
  const keywords = catalog.keywords ?? {};
  return {
    ...(catalog.publisher
      ? { publisher: { name: shown(catalog.publisher.name, language), uri: catalog.publisher.uri } }
      : {}),
    ...(catalog.contactPoint ? { contactPoint: { ...catalog.contactPoint } } : {}),
    ...(catalog.license ? { license: catalog.license } : {}),
    ...(catalog.attribution ? { attribution: shown(catalog.attribution, language) } : {}),
    ...(catalog.themes ? { themes: [...catalog.themes] } : {}),
    ...(Object.keys(keywords).length > 0
      ? { keywords: [...(keywords[shownLanguage(keywords, language)] ?? [])] }
      : {}),
    ...(catalog.spatial ? { spatial: [...catalog.spatial] } : {}),
    ...(catalog.temporal ? { temporal: { ...catalog.temporal } } : {}),
    ...(catalog.frequency ? { frequency: catalog.frequency } : {}),
    ...(catalog.source
      ? {
          source: catalog.source.map((source) => ({
            url: source.url,
            title: shown(source.title, language),
            description: shown(source.description, language),
          })),
        }
      : {}),
    ...(catalog.pipelineRef?.name ? { pipelineRef: catalog.pipelineRef.name } : {}),
    ...(catalog.applicableLegislation ? { applicableLegislation: [...catalog.applicableLegislation] } : {}),
  };
}

/**
 * The block the manifest carries for what the form holds: each text written over the entry it
 * showed in `stored`, every other language kept; nothing when the form holds nothing.
 */
export function catalogOf(
  form: CatalogForm | undefined,
  stored: CatalogManifest | undefined,
  language: string,
): CatalogManifest | undefined {
  if (!form) {
    return undefined;
  }
  const out: CatalogManifest = {};
  const publisherName = written(stored?.publisher?.name, form.publisher?.name, language);
  const publisherUri = form.publisher?.uri?.trim();
  if (publisherName || publisherUri) {
    out.publisher = {
      name: publisherName ?? {},
      ...(publisherUri ? { uri: publisherUri } : {}),
    };
  }
  const contactName = form.contactPoint?.name?.trim() ?? "";
  const contactEmail = form.contactPoint?.email?.trim() ?? "";
  if (contactName || contactEmail) {
    out.contactPoint = { name: contactName, email: contactEmail };
  }
  if (form.license) {
    out.license = form.license;
  }
  const attribution = written(stored?.attribution, form.attribution, language);
  if (attribution) {
    out.attribution = attribution;
  }
  const themes = filled(form.themes);
  if (themes.length > 0) {
    out.themes = [...new Set(themes)];
  }
  const keywords: Record<string, string[]> = { ...(stored?.keywords ?? {}) };
  const keywordLanguage = entryOf(stored?.keywords, language);
  const words = filled(form.keywords);
  if (words.length > 0) {
    keywords[keywordLanguage] = words;
  } else {
    delete keywords[keywordLanguage];
  }
  if (Object.keys(keywords).length > 0) {
    out.keywords = keywords;
  }
  const spatial = filled(form.spatial);
  if (spatial.length > 0) {
    out.spatial = spatial;
  }
  const start = form.temporal?.start?.trim();
  const end = form.temporal?.end?.trim();
  if (start || end) {
    out.temporal = { ...(start ? { start } : {}), ...(end ? { end } : {}) };
  }
  if (form.frequency) {
    out.frequency = form.frequency;
  }
  const sources = (form.source ?? [])
    .filter((source) => source.url?.trim())
    .map((source) => {
      const url = (source.url ?? "").trim();
      const before = stored?.source?.find((item) => item.url === url);
      return {
        url,
        title: written(before?.title, source.title, language) ?? {},
        description: written(before?.description, source.description, language) ?? {},
      };
    });
  if (sources.length > 0) {
    out.source = sources;
  }
  const pipeline = form.pipelineRef?.trim();
  if (pipeline) {
    out.pipelineRef = { kind: "Pipeline", name: pipeline };
  }
  const laws = filled(form.applicableLegislation);
  if (laws.length > 0) {
    out.applicableLegislation = laws;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

/** One contact of the Organization manifest. */
interface OrganizationContact {
  role?: string;
  name?: string;
  email?: string;
}

/**
 * What a new endpoint's catalogue starts with, from the Organization (EP-80): its title as the
 * publisher, its verified domain as the publisher's address, and its `open-data` contact as the
 * contact point. A person's contact is never offered; an organization without an open-data desk
 * prefills no contact.
 */
export function prefillFromOrganization(
  organization: { metadata?: { title?: unknown }; spec?: unknown } | undefined,
): CatalogForm | undefined {
  if (!organization) {
    return undefined;
  }
  const spec = (organization.spec ?? {}) as { domain?: string; contacts?: OrganizationContact[] };
  const title = organization.metadata?.title;
  const name =
    typeof title === "string" ? title : shown(title as Texts | undefined, SHOWN);
  const desk = (spec.contacts ?? []).find((contact) => contact.role === "open-data");
  const form: CatalogForm = {
    ...(name || spec.domain
      ? { publisher: { name, ...(spec.domain ? { uri: `https://${spec.domain}/` } : {}) } }
      : {}),
    ...(desk?.email ? { contactPoint: { name: desk.name ?? "", email: desk.email } } : {}),
  };
  return Object.keys(form).length > 0 ? form : undefined;
}

/**
 * The offer the licence makes, as sentences a person reads (EP-79): who may use the data, and
 * what they owe for it. Built from the manifest, so it says what the record will say.
 */
export function offerSentences(
  t: (key: string, values?: Record<string, string>) => string,
  catalog: CatalogManifest | undefined,
  audience: string,
  language: string,
): string[] {
  const licence = catalog?.license;
  const duty = dutyOf(licence);
  if (!licence || !duty) {
    return [t("endpoints.page.catalog.noLicence")];
  }
  const publisher =
    shown(catalog?.publisher?.name, language) ?? t("endpoints.page.catalog.thePublisher");
  const who = t(`endpoints.page.catalog.who.${audience === "public" ? "public" : audience}`);
  const sentences = [t(`endpoints.page.catalog.use.${duty}`, { who, publisher })];
  if (duty === "shareAlike") {
    sentences.push(t("endpoints.page.catalog.shareAlike"));
  }
  return sentences;
}
